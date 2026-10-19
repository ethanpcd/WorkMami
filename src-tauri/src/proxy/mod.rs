//! 本地 OpenAI 兼容反向代理。
//! - 监听 127.0.0.1:<port>，校验 Bearer key
//! - /v1/chat/completions：多账号调度 + 会话粘性 + failover + 流式透传
//! - /v1/models、/v1/workmami/quota、/v1/workmami/health
//! 上游为 {domain}/v2/chat/completions（强制流式，非流式客户端自动组装）。

use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Local, Utc};
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::accounts::{self, Account};
use crate::api::{api_base, should_refresh};
use crate::state::{AppState, Strategy};
use crate::usage::RequestLog;

pub struct ProxyHandle;

/// 启动反代（若 settings.proxy.enabled）。返回是否已启动。
pub async fn maybe_start(state: Arc<AppState>) -> bool {
    let ps = state.proxy_settings();
    if !ps.enabled {
        return false;
    }
    start(state).await
}

pub async fn start(state: Arc<AppState>) -> bool {
    if *state.proxy_running.lock().unwrap() {
        return true;
    }
    let ps = state.proxy_settings();
    let bind = format!("{}:{}", ps.host, ps.port);

    // 先真正绑定端口再置 running，避免"乐观置位"导致状态与实际不符。
    // 带少量重试：重启时上一个实例的优雅关闭可能尚未释放端口。
    let mut listener = None;
    for attempt in 0..20 {
        match tokio::net::TcpListener::bind(&bind).await {
            Ok(l) => {
                listener = Some(l);
                break;
            }
            Err(e) => {
                if attempt == 19 {
                    tracing::error!("反代绑定 {bind} 失败: {e}");
                    return false;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    let listener = match listener {
        Some(l) => l,
        None => return false,
    };

    let notify = Arc::new(Notify::new());
    let my_gen = {
        let mut g = state.proxy_gen.lock().unwrap();
        *g += 1;
        *g
    };
    {
        *state.proxy_shutdown.lock().unwrap() = Some(notify.clone());
        *state.proxy_running.lock().unwrap() = true;
    }

    let app = build_router(state.clone());
    let st = state.clone();
    tokio::spawn(async move {
        let serve = axum::serve(listener, app)
            .with_graceful_shutdown(async move { notify.notified().await });
        if let Err(e) = serve.await {
            tracing::error!("反代退出: {e}");
        }
        // 仅当自己仍是当前活动代次时才清除 running，避免旧实例退出把新实例状态覆盖为 false。
        if *st.proxy_gen.lock().unwrap() == my_gen {
            *st.proxy_running.lock().unwrap() = false;
            st.proxy_shutdown.lock().unwrap().take();
        }
    });
    true
}

pub fn stop(state: &Arc<AppState>) {
    // 用 notify_one（会保留一个许可）而非 notify_waiters，避免"尚无等待者"时关闭信号丢失、端口不释放。
    if let Some(n) = state.proxy_shutdown.lock().unwrap().take() {
        n.notify_one();
    }
    *state.proxy_running.lock().unwrap() = false;
}

fn build_router(state: Arc<AppState>) -> Router {
    let authed = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/models/:id", get(get_model))
        .route("/v1/workmami/quota", get(quota_endpoint))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_key));

    Router::new()
        .route("/v1/workmami/health", get(health))
        .merge(authed)
        .with_state(state)
        // 放宽请求体上限：axum 默认只有 2 MiB，长会话 / 大附件会被 413 拒掉。
        // handler 用 Bytes 全量缓冲，所以这里调大而不是关掉，64 MiB 足够又不会无上限吃内存。
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
}

// ---------- 鉴权中间件 ----------
async fn require_key(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let ps = state.proxy_settings();
    if !ps.api_key.is_empty() {
        let provided = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().strip_prefix("Bearer ").unwrap_or(s.trim()).to_string())
            .unwrap_or_default();
        if provided != ps.api_key {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"message": "无效的 API Key", "type": "auth_error"}})),
            )
                .into_response();
        }
    }
    next.run(req).await
}

// ---------- 端点 ----------
async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    let running = *state.proxy_running.lock().unwrap();
    Json(json!({
        "ok": true,
        "running": running,
        "platform": crate::platform::platform_name(),
        "accounts": state.accounts.all().len(),
    }))
}

async fn list_models(State(state): State<Arc<AppState>>) -> Json<Value> {
    // 优先返回已从 WorkBuddy 实时拉取并缓存的内置模型列表；
    // 尚未拉取时回落到本地配置的默认模型 + 别名映射，保证端点始终可用。
    let cached: Vec<Value> = state
        .models_cache
        .lock()
        .unwrap()
        .iter()
        .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(|id| {
            json!({
                "id": id,
                "object": "model",
                "created": m.get("contextWindow").and_then(|v| v.as_i64()).unwrap_or(0),
                "owned_by": "workbuddy",
            })
        }))
        .collect();
    if !cached.is_empty() {
        return Json(json!({"object": "list", "data": cached}));
    }
    // 尚未拉取模型列表时，至少回落到默认模型，保证端点始终可用。
    let ps = state.proxy_settings();
    let data = vec![json!({"id": ps.default_model, "object": "model", "owned_by": "workbuddy"})];
    Json(json!({"object": "list", "data": data}))
}

async fn get_model(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Json<Value> {
    let _ = state.proxy_settings();
    Json(json!({"id": id, "object": "model", "owned_by": "workbuddy"}))
}

async fn quota_endpoint(State(state): State<Arc<AppState>>) -> Json<Value> {
    let list: Vec<Value> = state
        .accounts
        .all()
        .into_iter()
        .map(|a| {
            json!({
                "uid": a.uid,
                "nickname": a.nickname,
                "quota": a.quota,
                "enabled": a.enabled,
                "cooling": state.is_cooling(&a.uid),
            })
        })
        .collect();
    Json(json!({"accounts": list, "ts": Utc::now().to_rfc3339()}))
}

async fn chat_completions(State(state): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Response {
    let ua = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, &format!("请求体解析失败: {e}")),
    };

    let ps = state.proxy_settings();
    // 模型解析：客户端传什么就用什么，未传则用默认模型。
    // 若请求的模型不在已知列表中（例如用户误填 API Key 作为模型名），自动回落到默认模型。
    let requested_raw = req
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("")
        .to_string();
    let requested = if requested_raw.is_empty() || !is_known_model(&state, &requested_raw) {
        if !requested_raw.is_empty() && requested_raw != ps.default_model {
            tracing::warn!(
                "请求模型 {:?} 不在已知列表中，回落到默认模型 {:?}",
                requested_raw,
                ps.default_model
            );
        }
        ps.default_model.clone()
    } else {
        requested_raw
    };
    if let Some(obj) = req.as_object_mut() {
        obj.insert("model".into(), json!(requested));
    }
    apply_reasoning_default(&mut req, &ps);

    let is_stream = req.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let session_hash = session_key(&req, &headers);

    // 候选账号（按策略 + 粘性 + 冷却过滤）
    let candidates = select_candidates(&state, &session_hash);
    if candidates.is_empty() {
        return err_resp(
            StatusCode::SERVICE_UNAVAILABLE,
            "没有可用账号：请在“账号管理”导入或授权账号，或检查积分是否耗尽",
        );
    }

    let mut last_err = String::from("无可用上游");
    for (idx, acc) in candidates.iter().enumerate() {
        match forward_once(&state, acc, req.clone(), is_stream, &ua, &session_hash, &requested).await {
            Ok(resp) => return resp,
            Err(e) => {
                last_err = e;
                // 决定是否需要换下一个账号
                if idx + 1 >= ps.max_retries as usize {
                    break;
                }
            }
        }
    }
    err_resp(StatusCode::BAD_GATEWAY, &format!("所有账号转发失败：{last_err}"))
}

// ---------- 模型验证 ----------
/// 检查请求的模型名是否在已知列表中（缓存的模型列表 + 默认模型）。
/// 若缓存为空（尚未拉取过模型列表），视为"全部放行"避免误拦。
fn is_known_model(state: &Arc<AppState>, model: &str) -> bool {
    let ps = state.proxy_settings();
    if model == ps.default_model {
        return true;
    }
    let cache = state.models_cache.lock().unwrap();
    // 缓存为空时不做校验，放行（避免启动初期误拦）
    if cache.is_empty() {
        return true;
    }
    cache.iter().any(|m| {
        m.get("id").and_then(|i| i.as_str()) == Some(model)
    })
}

// ---------- 调度 ----------
fn select_candidates(state: &Arc<AppState>, session_hash: &str) -> Vec<Account> {
    let ps = state.proxy_settings();
    let mut enabled: Vec<Account> = state
        .accounts
        .enabled_accounts()
        .into_iter()
        .filter(|a| accounts::load_token(&a.uid).is_some())
        .collect();
    if enabled.is_empty() {
        return Vec::new();
    }
    enabled.retain(|a| !state.is_cooling(&a.uid));
    if enabled.is_empty() {
        // 全在冷却则放行全部，给一次机会
        enabled = state.accounts.enabled_accounts();
    }

    // 会话粘性优先
    if let Some(uid) = state.sticky_account(session_hash, ps.sticky_ttl_min) {
        if let Some(pos) = enabled.iter().position(|a| a.uid == uid) {
            let acc = enabled.remove(pos);
            enabled.insert(0, acc);
        }
    }

    match ps.strategy {
        Strategy::RoundRobin => {
            let mut c = state.rr_cursor.lock().unwrap();
            let n = enabled.len().max(1);
            let start = *c % n;
            *c = c.wrapping_add(1);
            let mut v = enabled.clone();
            v.rotate_left(start);
            v
        }
        Strategy::Drain => {
            // 集中消耗余额最少的
            enabled.sort_by(|a, b| remaining(a).partial_cmp(&remaining(b)).unwrap_or(std::cmp::Ordering::Equal));
            enabled
        }
        Strategy::Smart => {
            // 余额多的优先，粘性账号已置顶
            enabled.sort_by(|a, b| remaining(b).partial_cmp(&remaining(a)).unwrap_or(std::cmp::Ordering::Equal));
            enabled
        }
    }
}

fn remaining(a: &Account) -> f64 {
    a.quota.as_ref().map(|q| q.remaining).unwrap_or(f64::MAX)
}

fn session_key(req: &Value, headers: &HeaderMap) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    if let Some(sid) = headers.get("x-session-id").and_then(|v| v.to_str().ok()) {
        h.update(sid.as_bytes());
    } else if let Some(msgs) = req.get("messages").and_then(|m| m.as_array()) {
        // 首条 user 内容
        for m in msgs {
            if m.get("role").and_then(|r| r.as_str()) == Some("user") {
                if let Some(c) = m.get("content") {
                    h.update(c.to_string().as_bytes());
                }
                break;
            }
        }
    }
    let digest = h.finalize();
    hex::encode(&digest[..12])
}

fn apply_reasoning_default(req: &mut Value, ps: &crate::state::ProxySettings) {
    if req.get("reasoning_effort").is_some() || req.get("thinking").is_some() {
        return; // 客户端已指定
    }
    if !ps.reasoning_effort.is_empty() && ps.reasoning_effort != "none" {
        if let Some(obj) = req.as_object_mut() {
            obj.insert("reasoning_effort".into(), json!(ps.reasoning_effort));
        }
    }
}

// ---------- 转发 ----------
async fn forward_once(
    state: &Arc<AppState>,
    acc: &Account,
    req: Value,
    is_stream: bool,
    ua: &str,
    session_hash: &str,
    model: &str,
) -> Result<Response, String> {
    let mut tokens = accounts::load_token(&acc.uid).ok_or_else(|| "缺少 token".to_string())?;

    // 过期则刷新
    if should_refresh(&tokens) && !tokens.refresh_token.is_empty() {
        match state.api.refresh_token(&tokens).await {
            Ok(nt) => {
                let _ = accounts::store_token(&acc.uid, &nt);
                tokens = nt;
            }
            Err(e) => tracing::warn!("刷新 token 失败({}): {e}", acc.uid),
        }
    }

    // 正确上游路径：{domain}/v2/chat/completions（不支持非流式，必须强制 stream）
    let url = format!("{}/v2/chat/completions", api_base(&tokens));
    let mut upstream_req = req.clone();
    if let Some(obj) = upstream_req.as_object_mut() {
        obj.insert("stream".into(), json!(true));
        // 确保上游在流式结束时返回 usage（含真实 credit 积分）
        let so = obj.entry("stream_options").or_insert_with(|| json!({}));
        if let Some(so_obj) = so.as_object_mut() {
            so_obj.insert("include_usage".into(), json!(true));
        }
    }

    let http = &state.api;
    let started = Utc::now().timestamp_millis();
    let resp = http
        .post_chat_async(&url, &tokens, &upstream_req)
        .await
        .map_err(|e| format!("网络错误: {e}"))?;

    let status = resp.status();
    let code = status.as_u16();

    // 401：刷新一次重试；429/积分耗尽：冷却并换号
    if code == 401 {
        state.cool_down(&acc.uid, 15_000);
        return Err(format!("账号 {} 鉴权失效(401)", acc.uid));
    }
    if code == 429 || is_quota_exhausted(status, &resp).await {
        state.cool_down(&acc.uid, 60_000);
        return Err(format!("账号 {} 限流/积分不足({code})", acc.uid));
    }
    if !status.is_success() {
        let txt = resp.text().await.unwrap_or_default();
        dump_chat_debug(&upstream_req, code, &txt);
        let head: String = txt.chars().take(200).collect();
        return Err(format!("上游 {code}: {head}"));
    }

    state.clear_cooling(&acc.uid);
    let _ = state.accounts.touch_used(&acc.uid);
    state.set_sticky(session_hash, &acc.uid, state.proxy_settings().sticky_ttl_min);

    if is_stream {
        stream_response(state, acc, model, started, resp, upstream_req.clone()).await
    } else {
        // 上游强制流式，这里收集 SSE 并组装为完整 JSON 返回给非流式客户端
        sse_to_json_response(state, acc, model, started, resp, ua, upstream_req.clone()).await
    }
}

// 由于 reqwest 不能既判断 body 又透传，这里对 429 之外的错误码用状态判断即可。
async fn is_quota_exhausted(_status: StatusCode, _resp: &reqwest::Response) -> bool {
    false
}

/// 上游强制流式，收集 SSE chunks 后组装为 OpenAI 非流式 JSON 响应。
async fn sse_to_json_response(
    state: &Arc<AppState>,
    acc: &Account,
    model: &str,
    started: i64,
    resp: reqwest::Response,
    ua: &str,
    req: Value,
) -> Result<Response, String> {
    let mut bytes_stream = resp.bytes_stream();
    let mut content_buf = String::new();
    let mut reasoning_buf = String::new();
    let mut usage = json!({});
    let mut completion_id = String::new();
    let mut created: i64 = 0;
    let mut finish_reason = String::new();

    while let Some(chunk) = bytes_stream.next().await {
        let chunk = chunk.map_err(|e| format!("流读取错误: {e}"))?;
        if let Ok(text) = std::str::from_utf8(&chunk) {
            for line in text.lines() {
                let payload = match line.strip_prefix("data:") {
                    Some(p) => p.trim(),
                    None => continue,
                };
                if payload == "[DONE]" || payload.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(payload) {
                    if completion_id.is_empty() {
                        completion_id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        created = v.get("created").and_then(|x| x.as_i64()).unwrap_or(0);
                    }
                    if let Some(u) = v.get("usage") {
                        if !u.is_null() && u.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                            usage = u.clone();
                        }
                    }
                    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
                        if let Some(ch) = choices.first() {
                            if let Some(delta) = ch.get("delta") {
                                if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
                                    content_buf.push_str(c);
                                }
                                if let Some(rc) = delta.get("reasoning_content").and_then(|x| x.as_str()) {
                                    reasoning_buf.push_str(rc);
                                }
                            }
                            if let Some(fr) = ch.get("finish_reason").and_then(|x| x.as_str()) {
                                if !fr.is_empty() {
                                    finish_reason = fr.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let latency = Utc::now().timestamp_millis() - started;
    // 诊断：非流式组装后，若命中拒绝/空内容/错误关键字，落盘供真机定位。
    if finish_reason.contains("refusal")
        || finish_reason == "content_filter"
        || content_buf.is_empty()
    {
        dump_chat_debug(&req, 200, &format!("finish_reason={finish_reason}\ncontent={content_buf}"));
    }
    log_usage(state, acc, model, &usage, latency, 0, 200, true, ua);

    let mut message = json!({"role": "assistant", "content": content_buf});
    if !reasoning_buf.is_empty() {
        message.as_object_mut().unwrap().insert("reasoning_content".into(), json!(reasoning_buf));
    }

    let out = json!({
        "id": if completion_id.is_empty() { format!("cmpl-{}", Utc::now().timestamp_millis()) } else { completion_id },
        "object": "chat.completion",
        "created": if created > 0 { created } else { Utc::now().timestamp() },
        "model": model,
        "choices": [{"index": 0, "message": message, "finish_reason": if finish_reason.is_empty() { "stop".to_string() } else { finish_reason }}],
        "usage": usage,
    });
    Ok(Json(out).into_response())
}

async fn stream_response(
    state: &Arc<AppState>,
    acc: &Account,
    model: &str,
    started: i64,
    resp: reqwest::Response,
    req: Value,
) -> Result<Response, String> {
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::ReceiverStream;

    let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(64);
    let state2 = state.clone();
    let acc_uid = acc.uid.clone();
    let model2 = model.to_string();

    // 后台任务：逐 chunk 透传 + 收集 usage
    tokio::spawn(async move {
        let mut bytes_stream = resp.bytes_stream();
        let mut usage = json!({});
        let mut ttft: i64 = 0;
        let mut first = true;
        // 区分“上游/超时导致的中断”与“客户端主动断开”：前者应记为失败，不能谎报成成功。
        let mut upstream_err = false;
        // 诊断用：累积原始 SSE 文本（上限 200KB），命中拒绝/错误关键字时落盘。
        let mut raw: Vec<u8> = Vec::new();
        let cap = 200_000usize;

        while let Some(chunk) = bytes_stream.next().await {
            match chunk {
                Ok(c) => {
                    if first && !c.is_empty() {
                        ttft = Utc::now().timestamp_millis() - started;
                        first = false;
                    }
                    scan_usage(&c, &mut usage);
                    if raw.len() < cap {
                        let take = (cap - raw.len()).min(c.len());
                        raw.extend_from_slice(&c[..take]);
                    }
                    if tx.send(Ok(c)).await.is_err() {
                        break; // 客户端已断开（正常，不算失败）
                    }
                }
                Err(e) => {
                    upstream_err = true;
                    let _ = tx.send(Err(std::io::Error::other(e.to_string()))).await;
                    break;
                }
            }
        }
        let latency = Utc::now().timestamp_millis() - started;
        let (status, ok): (i64, bool) = if upstream_err { (500, false) } else { (200, true) };
        // 命中安全拦截/错误关键字时落盘（REFUSAL 等），便于真机定位反代与内置的差异。
        let text = String::from_utf8_lossy(&raw).to_string();
        if upstream_err
            || text.contains("REFUSAL")
            || text.contains("refusal")
            || text.contains("content_filter")
            || text.contains("\"error\"")
        {
            dump_chat_debug(&req, status as u16, &text);
        }
        log_usage_by_uid(&state2, &acc_uid, &model2, &usage, latency, ttft, status, ok, "");
    });

    let body = Body::from_stream(ReceiverStream::new(rx));
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(axum::http::header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok((headers, body).into_response())
}

fn scan_usage(chunk: &Bytes, usage: &mut Value) {
    // SSE 行：data: {json}
    if let Ok(text) = std::str::from_utf8(chunk) {
        for line in text.lines() {
            let payload = match line.strip_prefix("data:") {
                Some(p) => p.trim(),
                None => continue,
            };
            if payload == "[DONE]" || payload.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(payload) {
                if let Some(u) = v.get("usage") {
                    if !u.is_null() && u.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                        *usage = u.clone();
                    }
                }
            }
        }
    }
}

fn log_usage(
    state: &Arc<AppState>,
    acc: &Account,
    model: &str,
    usage: &Value,
    latency: i64,
    ttft: i64,
    status: i64,
    ok: bool,
    ua: &str,
) {
    log_usage_by_uid(state, &acc.uid, model, usage, latency, ttft, status, ok, ua);
}

fn log_usage_by_uid(
    state: &Arc<AppState>,
    uid: &str,
    model: &str,
    usage: &Value,
    latency: i64,
    ttft: i64,
    status: i64,
    ok: bool,
    ua: &str,
) {
    let g = |k: &str| usage.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
    let prompt = g("prompt_tokens");
    let completion = g("completion_tokens");
    let total = {
        let t = g("total_tokens");
        if t > 0 { t } else { prompt + completion }
    };
    let cached = usage
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    let reasoning = usage
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    // 优先使用上游返回的真实积分（credit 字段），仅字段不存在时回落到估算
    let credits = usage
        .get("credit")
        .and_then(|c| c.as_f64())
        .unwrap_or_else(|| total as f64 / 1000.0);
    let _ = state.usage.record(&RequestLog {
        ts: Utc::now().timestamp_millis(),
        uid: uid.to_string(),
        model: model.to_string(),
        session_hash: String::new(),
        source: "proxy".into(),
        prompt_tokens: prompt,
        completion_tokens: completion,
        reasoning_tokens: reasoning,
        cached_tokens: cached,
        total_tokens: total,
        credits,
        latency_ms: latency,
        ttft_ms: ttft,
        status,
        ok,
        ua: ua.to_string(),
    });
}

/// 诊断落盘：把反代实际发给上游的请求（已强制 stream，不含 token）+ 上游返回内容，
/// 在命中拒绝/错误时写入 WorkMami/proxy-chat-debug.json，供真机定位 REFUSAL 等问题。
fn dump_chat_debug(req: &Value, status: u16, resp_text: &str) {
    let path = crate::platform::workmami_paths().root.join("proxy-chat-debug.json");
    let head: String = resp_text.chars().take(6000).collect();
    let obj = json!({
        "fired_at_local": Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "status": status,
        "request": req,
        "response_text": head,
    });
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&obj).unwrap_or_default());
}

fn err_resp(code: StatusCode, msg: &str) -> Response {
    (
        code,
        Json(json!({"error": {"message": msg, "type": "proxy_error"}})),
    )
        .into_response()
}
