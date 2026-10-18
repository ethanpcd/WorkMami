//! Tauri IPC 命令层：前端通过 invoke 调用这些命令。

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_notification::NotificationExt;

use crate::accounts::{self, Account};
use crate::api;
use crate::data::{self, Category};
use crate::error::WmError;
use crate::oauth;
use crate::proxy;
use crate::scheduler;
use crate::state::{AppSettings, AppState};
use crate::wbauth;

type R<T> = Result<T, WmError>;

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

// ---------- 通用 / 系统 ----------
#[tauri::command]
pub fn app_info() -> Value {
    serde_json::json!({
        "platform": crate::platform::platform_name(),
        "version": env!("CARGO_PKG_VERSION"),
        "paths": {
            "workmamiRoot": crate::platform::workmami_paths().root.to_string_lossy(),
            "workbuddyHome": crate::platform::workbuddy_paths().home.to_string_lossy(),
            "desktopInfo": crate::platform::workbuddy_paths().existing_info_file().map(|p| p.to_string_lossy().to_string()),
        }
    })
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppState>>) -> R<AppSettings> {
    Ok(state.settings.read().unwrap().clone())
}

#[tauri::command]
pub async fn save_settings(state: State<'_, Arc<AppState>>, settings: AppSettings) -> R<()> {
    {
        *state.settings.write().unwrap() = settings.clone();
    }
    state.save_settings()?;
    // 依据 enabled 启停反代
    if settings.proxy.enabled {
        proxy::stop(state.inner());
        proxy::start(state.inner().clone()).await;
    } else {
        proxy::stop(state.inner());
    }
    Ok(())
}

#[tauri::command]
pub fn proxy_status(state: State<'_, Arc<AppState>>) -> R<Value> {
    let running = *state.proxy_running.lock().unwrap();
    let ps = state.proxy_settings();
    Ok(serde_json::json!({
        "running": running,
        "enabled": ps.enabled,
        "host": ps.host,
        "port": ps.port,
        "baseUrl": format!("http://{}:{}/v1", ps.host, ps.port),
    }))
}

// ---------- 账号 ----------
#[tauri::command]
pub fn list_accounts(state: State<'_, Arc<AppState>>) -> R<Vec<Account>> {
    Ok(state.accounts.all())
}

#[tauri::command]
pub fn credential_status(state: State<'_, Arc<AppState>>, uid: String) -> R<bool> {
    let _ = state;
    Ok(accounts::load_token(&uid).is_some())
}

#[tauri::command]
pub fn set_account_enabled(state: State<'_, Arc<AppState>>, uid: String, enabled: bool) -> R<()> {
    state.accounts.set_enabled(&uid, enabled)
}

#[tauri::command]
pub fn set_account_tags(state: State<'_, Arc<AppState>>, uid: String, tags: Vec<String>) -> R<()> {
    state.accounts.set_tags(&uid, tags)
}

#[tauri::command]
pub fn delete_account(state: State<'_, Arc<AppState>>, uid: String) -> R<()> {
    state.accounts.remove(&uid)
}

#[tauri::command]
pub async fn refresh_quota(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    uids: Option<Vec<String>>,
) -> R<Vec<scheduler::RefreshResult>> {
    let st = state.inner().clone();
    let results = scheduler::refresh_quota(st, uids).await;
    let ok = results.iter().filter(|r| r.ok).count();
    let _ = app.emit("quota-refreshed", &results);
    tracing::info!("刷新积分完成：{ok}/{}", results.len());
    Ok(results)
}

// ---------- 积分清单（奖励批次 + 消耗明细） ----------
#[tauri::command]
pub async fn credit_ledger(
    state: State<'_, Arc<AppState>>,
    uid: String,
    days: Option<i64>,
) -> R<Value> {
    let tokens = accounts::load_token(&uid)
        .ok_or_else(|| WmError::Credential("无本地凭证".into()))?;
    let days = days.unwrap_or(7).clamp(1, 90);
    // 奖励与消耗各自独立：任一失败不影响另一部分展示。
    let rewards = state.api.fetch_rewards(&tokens).await.unwrap_or_default();
    let usage = state.api.fetch_usage(&tokens, days).await.unwrap_or_default();
    let reward_count = rewards.iter().filter(|r| r.remaining > 0.0).count();
    let reward_remaining: f64 = rewards.iter().map(|r| r.remaining.max(0.0)).sum();
    let usage_total: f64 = usage.iter().map(|u| u.credits).sum();
    Ok(serde_json::json!({
        "rewards": rewards,
        "rewardCount": reward_count,
        "rewardRemaining": reward_remaining,
        "usage": usage,
        "usageTotal": usage_total,
        "days": days,
    }))
}

// ---------- WorkBuddy 内置模型列表（实时） ----------
/// 拉取当前 WorkBuddy 账号可见的内置模型列表（含上下文窗口等参数），并缓存供反代 /v1/models 复用。
#[tauri::command]
pub async fn list_models(state: State<'_, Arc<AppState>>, uid: Option<String>) -> R<Vec<Value>> {
    // 选一个可用凭证：优先指定 uid，否则第一个启用账号，再退化为任意账号。
    let tokens = match &uid {
        Some(u) => accounts::load_token(u),
        None => state
            .accounts
            .enabled_accounts()
            .into_iter()
            .chain(state.accounts.all())
            .find_map(|a| accounts::load_token(&a.uid)),
    }
    .ok_or_else(|| WmError::Credential("无可用账号凭证，请先添加账号".into()))?;

    let models = state.api.fetch_models(&tokens).await?;
    let vals: Vec<Value> = models
        .iter()
        .map(|m| serde_json::to_value(m).unwrap_or(Value::Null))
        .collect();
    // 更新缓存：反代对外暴露 /v1/models 时直接用真实列表。
    *state.models_cache.lock().unwrap() = vals.clone();
    Ok(vals)
}

// ---------- OAuth 网页授权（内嵌登录窗抓取） ----------
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthStart {
    pub session_id: String,
}

#[tauri::command]
pub async fn oauth_begin(app: tauri::AppHandle, state: State<'_, Arc<AppState>>) -> R<OAuthStart> {
    let session_id = oauth::begin(&app, state.api.clone()).await?;
    Ok(OAuthStart { session_id })
}

#[tauri::command]
pub async fn oauth_poll(state: State<'_, Arc<AppState>>, session_id: String) -> R<Value> {
    match oauth::poll(state.inner(), &session_id).await {
        Ok(oauth::PollOutcome::Pending) => Ok(serde_json::json!({"status": "pending"})),
        Ok(oauth::PollOutcome::Authorized(acc)) => Ok(serde_json::json!({"status": "authorized", "account": acc})),
        Ok(oauth::PollOutcome::Expired) => Ok(serde_json::json!({"status": "expired"})),
        Err(e) => Err(e.into()),
    }
}

// ---------- 签到 ----------
#[tauri::command]
pub async fn checkin_status(state: State<'_, Arc<AppState>>, uid: String) -> R<Value> {
    let tokens = accounts::load_token(&uid)
        .ok_or_else(|| WmError::Credential("无本地凭证".into()))?;
    let (checked, streak, raw) = state.api.fetch_checkin_status(&tokens).await?;
    Ok(serde_json::json!({ "checkedIn": checked, "streakDays": streak, "status": raw }))
}

#[tauri::command]
pub async fn checkin_all(app: AppHandle, state: State<'_, Arc<AppState>>) -> R<Vec<scheduler::CheckinResult>> {
    let st = state.inner().clone();
    let results = scheduler::checkin_all(st.clone()).await;
    let gained: Vec<_> = results.iter().filter(|r| r.ok && !r.already).collect();
    if !gained.is_empty() {
        let _ = scheduler::refresh_quota(st, None).await;
        let _ = app.notification().builder().title("WorkMami 批量签到").body(format!("成功签到 {} 个账号", gained.len())).show();
    }
    Ok(results)
}

#[tauri::command]
pub async fn checkin_one(app: AppHandle, state: State<'_, Arc<AppState>>, uid: String) -> R<scheduler::CheckinResult> {
    let st = state.inner().clone();
    let r = scheduler::checkin_one(st, &uid).await;
    let _ = app.emit("checkin-done", &r);
    Ok(r)
}

// ---------- 猫猫旅行 ----------
/// 旅行概览：返回所有启用账号的旅行状态 + 共享地点配置。
#[tauri::command]
pub async fn travel_overview(state: State<'_, Arc<AppState>>) -> R<Value> {
    let st = state.inner().clone();
    let enabled = st.accounts.enabled_accounts();
    let api = st.api.clone();

    // 拿一个 token 来获取共享配置（地点列表）
    let tokens_first = enabled
        .iter()
        .find_map(|a| accounts::load_token(&a.uid));

    let config: Vec<Value> = match &tokens_first {
        Some(t) => api
            .buddy_travel_config(t)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|l| serde_json::to_value(&l).unwrap_or(Value::Null))
            .collect(),
        None => Vec::new(),
    };

    // 并发查各账号状态
    let mut handles = Vec::new();
    for acc in &enabled {
        let Some(tokens) = accounts::load_token(&acc.uid) else {
            continue;
        };
        let api = api.clone();
        let uid = acc.uid.clone();
        let nickname = acc.nickname.clone();
        handles.push(tauri::async_runtime::spawn(async move {
            let status = match api.buddy_travel_status(&tokens).await {
                Ok(s) => serde_json::to_value(&s).unwrap_or(Value::Null),
                Err(e) => json!({"error": e.to_string()}),
            };
            // 一并拉取旅行台账（服务端真实记录），供前端展示“今日几次/各赚多少/是否到账”。
            let records = match api.buddy_travel_records(&tokens, 1, 20).await {
                Ok(r) => serde_json::to_value(&r).unwrap_or_else(|_| json!([])),
                Err(_) => json!([]),
            };
            json!({
                "uid": uid,
                "nickname": nickname,
                "status": status,
                "records": records,
            })
        }));
    }
    let mut accounts_out: Vec<Value> = Vec::new();
    for h in handles {
        if let Ok(v) = h.await {
            accounts_out.push(v);
        }
    }

    let settings = st.settings.read().unwrap().travel.clone();
    Ok(json!({
        "config": config,
        "accounts": accounts_out,
        "autoEnabled": settings.auto_enabled,
        "locationPolicy": settings.location_policy,
    }))
}

/// 派猫出发。
#[tauri::command]
pub async fn travel_depart(state: State<'_, Arc<AppState>>, uid: String, location_id: i64) -> R<Value> {
    let tokens = accounts::load_token(&uid)
        .ok_or_else(|| WmError::Credential("无本地凭证".into()))?;
    let result = state.api.buddy_travel_depart(&tokens, location_id).await?;
    Ok(result)
}

/// 领取旅行奖励。
#[tauri::command]
pub async fn travel_claim(state: State<'_, Arc<AppState>>, uid: String) -> R<Value> {
    let tokens = accounts::load_token(&uid)
        .ok_or_else(|| WmError::Credential("无本地凭证".into()))?;
    let result = state.api.buddy_travel_claim(&tokens).await?;
    Ok(result)
}

/// 旅行历史记录（服务端真实台账）：单独拉取，用于历史弹窗；概览已内嵌今日记录。
#[tauri::command]
pub async fn travel_records(state: State<'_, Arc<AppState>>, uid: String) -> R<Vec<api::TravelRecord>> {
    let tokens = accounts::load_token(&uid)
        .ok_or_else(|| WmError::Credential("无本地凭证".into()))?;
    let records = state.api.buddy_travel_records(&tokens, 1, 50).await?;
    Ok(records)
}

// ---------- 数据管理 ----------
#[tauri::command]
pub fn data_list(state: State<'_, Arc<AppState>>, category: String, uid: Option<String>) -> R<Vec<data::DataItem>> {
    let _ = state;
    let cat = Category::from_str(&category)?;
    data::list(cat, uid.as_deref())
}

#[tauri::command]
pub fn data_read(item: data::DataItem) -> R<String> {
    data::read_item(&item)
}

#[tauri::command]
pub fn data_delete(items: Vec<data::DataItem>) -> R<usize> {
    data::delete(&items)
}

#[tauri::command]
pub fn data_export(item: data::DataItem, dest_dir: String) -> R<String> {
    let p = data::export(&item, &PathBuf::from(dest_dir))?;
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
pub fn data_backup(items: Vec<data::DataItem>, label: String, dest_dir: Option<String>) -> R<String> {
    let p = data::backup(&items, &label, dest_dir.as_deref())?;
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
pub fn data_migrate(from_uid: String, to_uid: String, categories: Vec<String>) -> R<usize> {
    let cats: Vec<Category> = categories
        .iter()
        .map(|c| Category::from_str(c))
        .collect::<Result<_, _>>()
        ?;
    data::migrate(&from_uid, &to_uid, &cats)
}

/// 扫描本机存在数据账号的 uid 列表（供数据管理页选择）。
#[tauri::command]
pub fn data_uids(state: State<'_, Arc<AppState>>) -> R<Vec<String>> {
    let mut uids: Vec<String> = state.accounts.all().into_iter().map(|a| a.uid).collect();
    // 补充：~/.workbuddy 下的 user-*-personal 与 security/<uid>
    let p = crate::platform::workbuddy_paths();
    if let Ok(rd) = std::fs::read_dir(&p.home) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(u) = name.strip_prefix("user-").and_then(|s| s.strip_suffix("-personal")) {
                if !uids.iter().any(|x| x == u) {
                    uids.push(u.to_string());
                }
            }
        }
    }
    // security/<uid> 目录（Windows 上发现历史数据账号的主来源）
    if let Ok(rd) = std::fs::read_dir(&p.security_dir) {
        for e in rd.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if is_uid_like(&name) && !uids.iter().any(|x| x == &name) {
                uids.push(name);
            }
        }
    }
    if let Some((info, _)) = wbauth::load_and_decrypt().ok() {
        for (uid, _, _) in wbauth::list_account_meta(&info) {
            if !uids.iter().any(|x| x == &uid) {
                uids.push(uid);
            }
        }
    }
    uids.sort();
    uids.dedup();
    Ok(uids)
}

// ---------- 看板 ----------
#[tauri::command]
pub fn dashboard_summary(state: State<'_, Arc<AppState>>, from_ms: i64, to_ms: i64) -> R<Value> {
    let from = ts(from_ms);
    let to = ts(to_ms);
    let cur = state.usage.summary(from, to)?;
    let span = to - from;
    let prev = state.usage.summary(from - span, from)?;
    Ok(serde_json::json!({ "current": cur, "previous": prev }))
}

#[tauri::command]
pub fn dashboard_trend(state: State<'_, Arc<AppState>>, from_ms: i64, to_ms: i64, granularity: String) -> R<Value> {
    let pts = state.usage.trend(ts(from_ms), ts(to_ms), &granularity)?;
    Ok(Value::Array(serde_json::from_value(serde_json::to_value(&pts)?)?))
}

#[tauri::command]
pub fn dashboard_top(state: State<'_, Arc<AppState>>, from_ms: i64, to_ms: i64, kind: String, limit: Option<i64>) -> R<Value> {
    let limit = limit.unwrap_or(10);
    let v = match kind.as_str() {
        "models" => serde_json::to_value(state.usage.top_models(ts(from_ms), ts(to_ms), limit)?)?,
        "sessions" => serde_json::to_value(state.usage.top_sessions(ts(from_ms), ts(to_ms), limit)?)?,
        "accounts" => serde_json::to_value(state.usage.per_account(ts(from_ms), ts(to_ms))?)?,
        _ => Value::Array(vec![]),
    };
    Ok(v)
}

#[tauri::command]
pub fn dashboard_heatmap(state: State<'_, Arc<AppState>>, from_ms: i64, to_ms: i64) -> R<Value> {
    let cells = state.usage.heatmap(ts(from_ms), ts(to_ms))?;
    Ok(serde_json::to_value(cells)?)
}

#[tauri::command]
pub fn dashboard_balances(state: State<'_, Arc<AppState>>) -> R<Value> {
    let h = state.balance_history.lock().unwrap();
    Ok(serde_json::to_value(&*h)?)
}

/// 从 workbuddy.db 导入 App 内用量到看板（合并数据源）。
#[tauri::command]
pub fn dashboard_import_app(state: State<'_, Arc<AppState>>, uid: Option<String>) -> R<u64> {
    let p = crate::platform::workbuddy_paths();
    let u = uid.unwrap_or_else(|| "local".into());
    state.usage.import_app_usage(&p.workbuddy_db, &u)
}

/// 请求流水明细（本工具代理/导入的记录，按时间倒序）：看每一次请求走了哪个账号。
#[tauri::command]
pub fn dashboard_requests(
    state: State<'_, Arc<AppState>>,
    from_ms: i64,
    to_ms: i64,
    limit: Option<i64>,
) -> R<Value> {
    let limit = limit.unwrap_or(300).clamp(1, 2000);
    let rows = state.usage.recent_requests(ts(from_ms), ts(to_ms), limit)?;
    Ok(serde_json::to_value(rows)?)
}

/// 各账号「真实扣费」：逐账号拉官方 get-user-request-usage 汇总区间内实际消耗积分。
/// 与代理侧「估算积分」互为对照——这才是能和官方后台对账的数字。
#[tauri::command]
pub async fn dashboard_account_billing(
    state: State<'_, Arc<AppState>>,
    from_ms: i64,
    to_ms: i64,
) -> R<Value> {
    // 区间跨度换算成天数（fetch_usage 以「最近 N 天」计，1~90）。
    let days = ((to_ms - from_ms) / 86_400_000 + 1).clamp(1, 90);
    let api = state.api.clone();
    let mut handles = Vec::new();
    for acc in state.accounts.all() {
        let Some(tokens) = accounts::load_token(&acc.uid) else {
            continue;
        };
        let api = api.clone();
        let uid = acc.uid.clone();
        let nickname = acc.nickname.clone();
        handles.push(tauri::async_runtime::spawn(async move {
            let usage = api.fetch_usage(&tokens, days).await.unwrap_or_default();
            let credits: f64 = usage.iter().map(|u| u.credits).sum();
            let requests = usage.len() as i64;
            serde_json::json!({
                "uid": uid,
                "nickname": nickname,
                "credits": credits,
                "requests": requests,
            })
        }));
    }
    let mut items: Vec<Value> = Vec::new();
    for h in handles {
        if let Ok(v) = h.await {
            items.push(v);
        }
    }
    items.sort_by(|a, b| {
        b["credits"]
            .as_f64()
            .unwrap_or(0.0)
            .partial_cmp(&a["credits"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total: f64 = items.iter().map(|v| v["credits"].as_f64().unwrap_or(0.0)).sum();
    Ok(serde_json::json!({ "days": days, "total": total, "items": items }))
}

// ---------- 工具 ----------
fn ts(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_else(Utc::now)
}

/// 判断目录名是否为 WorkBuddy 的 uid（UUID 形态：36 字符、4 个连字符、其余十六进制）。
/// 用于过滤 security/ 下的非账号目录（如 `threat-database`）。
fn is_uid_like(s: &str) -> bool {
    s.len() == 36
        && s.matches('-').count() == 4
        && s.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

/// 供调度器/启动复用的默认时间范围（近 30 天）。
pub fn default_range() -> (i64, i64) {
    let to = now_ms();
    let from = (Utc::now() - Duration::days(30)).timestamp_millis();
    (from, to)
}
