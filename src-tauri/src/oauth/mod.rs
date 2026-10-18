//! 网页授权登录（OAuth state 设备流）。
//!
//! 依据 workbuddy-switch / cockpit-tools 的真实实现：WorkBuddy 提供一组**公开**的
//! `/v2/plugin/auth/*` 端点（走 www.codebuddy.cn），基于 `state` 关联登录，**无需 Turing 设备指纹**：
//!   1. POST /v2/plugin/auth/state?platform=workbuddy → {state, authUrl}
//!   2. 打开 authUrl 让用户扫码/登录（服务端把该账号与 state 绑定）
//!   3. GET  /v2/plugin/auth/token?state= → 登录完成后返回 {accessToken, refreshToken, domain, expiresIn}
//!   4. GET  /v2/plugin/login/account?state=（带 Bearer + X-Domain）→ {uid, nickname, enterprise...}
//! 拿到的是真正的插件 Bearer accessToken，可同时驱动积分/签到接口与本地反代。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::Utc;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::accounts;
use crate::api::ApiClient;
use crate::error::{WmError, WmResult};
use crate::state::AppState;
use crate::wbauth::AuthTokens;

/// 会话有效期（秒），与官方 OAUTH_TIMEOUT 对齐。
const OAUTH_TIMEOUT_SECONDS: i64 = 600;

/// 登录窗口 label 前缀，用于识别与清理。
const OAUTH_LABEL_PREFIX: &str = "oauth-";

/// 窗口事件回调：用户关闭/销毁登录窗口时，若尚未抓到 token，
/// 立即把该会话标记为结束，避免前端弹窗永远卡在 pending 反复纠缠。
pub fn on_window_event(label: &str, destroyed: bool) {
    let _ = destroyed;
    if let Some(sid) = label.strip_prefix(OAUTH_LABEL_PREFIX) {
        let mut m = caps().lock().unwrap();
        if let Some(c) = m.get_mut(sid) {
            if c.captured.is_none() {
                c.done = true;
            }
        }
    }
}

/// 已抓到的登录结果。
struct Captured {
    access: String,
    refresh: String,
    domain: String,
    uid: String,
    nickname: String,
    etype: String,
    expires_at: Option<i64>,
}

struct Capture {
    state: String,
    created: i64,
    done: bool,
    captured: Option<Captured>,
}

static CAPTURES: OnceLock<Mutex<HashMap<String, Capture>>> = OnceLock::new();
fn caps() -> &'static Mutex<HashMap<String, Capture>> {
    CAPTURES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub enum PollOutcome {
    Pending,
    Authorized(accounts::Account),
    Expired,
}

/// 发起登录：申请 state、打开登录窗口、后台轮询 token。返回会话 id。
pub async fn begin(app: &AppHandle, api: Arc<ApiClient>) -> WmResult<String> {
    let (state_token, auth_url) = api.oauth_state().await?;
    let session_id = uuid::Uuid::new_v4().to_string();

    caps().lock().unwrap().insert(
        session_id.clone(),
        Capture {
            state: state_token.clone(),
            created: Utc::now().timestamp(),
            done: false,
            captured: None,
        },
    );

    // 先关闭可能残留的旧登录窗，避免多次点击叠加出多个窗口（看起来像"一直弹"）。
    for (lbl, w) in app.webview_windows() {
        if lbl.starts_with(OAUTH_LABEL_PREFIX) {
            let _ = w.close();
        }
    }

    // 打开登录窗口（用户在此扫码/登录，动作会把账号与 state 绑定）。
    // 需求：加第 2+ 个账号时，store 里还残留上一个账号的 codebuddy.cn 会话，直接开 authUrl 会弹“选择账号”。
    // 做法：先建窗停在 about:blank → 清一次共享 WebView 会话（去掉上一个账号的 cookie）→ 留足时间等异步清理跑完
    //       → 再 navigate 到 authUrl。关键：清理必须赶在登录页加载之前完成，否则会把本次登录会话一起抹掉→“账号不存在”。
    // 只影响 WebView 存储，不动已入库账号的 Bearer token（存于本地 vault），故不影响第一个账号的验证/刷新/签到。
    let label = format!("{OAUTH_LABEL_PREFIX}{session_id}");
    let url: tauri::Url = auth_url.parse().map_err(|e| WmError::Other(format!("登录地址无效: {e}")))?;
    let blank: tauri::Url = "about:blank".parse().map_err(|e| WmError::Other(format!("空白地址无效: {e}")))?;
    let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(blank))
        .title("WorkBuddy 登录")
        .inner_size(980.0, 760.0)
        .min_inner_size(520.0, 480.0)
        .resizable(true)
        .build()
        .map_err(|e| WmError::Other(format!("打开登录窗口失败: {e}")))?;
    // 在主窗触发整库清理（macOS/Windows 均为共享 store）；登录窗停在 about:blank 不受影响。
    match app.get_webview_window("main") {
        Some(main) => {
            let _ = main.clear_all_browsing_data();
        }
        None => {
            let _ = win.clear_all_browsing_data();
        }
    }
    // 清理是异步的且无完成回调，给足余量（上次 450ms 不够会抢跑），确保清理先于登录页加载完成。
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let _ = win.navigate(url);

    // 后台轮询 token
    let sid = session_id.clone();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        for _ in 0..(OAUTH_TIMEOUT_SECONDS / 2) {
            tokio::time::sleep(Duration::from_millis(2000)).await;
            match api.oauth_token(&state_token).await {
                Ok(Some(tok)) => {
                    // 拉账号信息
                    let acc = api.oauth_account(&state_token, &tok.access, &tok.domain).await.unwrap_or_default();
                    let uid = acc
                        .get("uid")
                        .and_then(|v| v.as_str())
                        .or_else(|| acc.get("userId").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .to_string();
                    let nickname = acc
                        .get("nickname")
                        .and_then(|v| v.as_str())
                        .or_else(|| acc.get("displayName").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .to_string();
                    let has_ent = acc.get("enterpriseId").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
                        || acc.get("enterpriseName").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
                    let etype = if has_ent { "enterprise" } else { "personal" }.to_string();

                    if uid.is_empty() {
                        // 拿到 token 但取不到 uid：视为失败，结束等待
                        caps().lock().unwrap().entry(sid.clone()).and_modify(|c| c.done = true);
                        if let Some(w) = app2.get_webview_window(&label) {
                            let _ = w.close();
                        }
                        return;
                    }

                    caps().lock().unwrap().entry(sid.clone()).and_modify(|c| {
                        c.captured = Some(Captured {
                            access: tok.access.clone(),
                            refresh: tok.refresh.clone(),
                            domain: tok.domain.clone(),
                            uid,
                            nickname,
                            etype,
                            expires_at: tok.expires_at,
                        });
                    });
                    if let Some(w) = app2.get_webview_window(&label) {
                        let _ = w.close();
                    }
                    return;
                }
                Ok(None) => continue,
                Err(_) => continue,
            }
        }
        // 超时
        caps().lock().unwrap().entry(sid).and_modify(|c| c.done = true);
    });

    Ok(session_id)
}

/// 轮询登录结果。抓到后组装 token 并入库。
pub async fn poll(state: &Arc<AppState>, session_id: &str) -> WmResult<PollOutcome> {
    let captured = caps().lock().unwrap().get(session_id).and_then(|c| c.captured.as_ref().map(|cap| {
        (
            cap.access.clone(),
            cap.refresh.clone(),
            cap.domain.clone(),
            cap.uid.clone(),
            cap.nickname.clone(),
            cap.etype.clone(),
            cap.expires_at,
        )
    }));

    if let Some((access, refresh, domain, uid, nickname, etype, expires_at)) = captured {
        let tokens = AuthTokens {
            access_token: access,
            refresh_token: refresh,
            token_type: "Bearer".into(),
            expires_at,
            refresh_expires_at: None,
            scope: String::new(),
            domain: if domain.is_empty() { "www.codebuddy.cn".into() } else { domain },
            uid: uid.clone(),
        };
        let acc = accounts::upsert_from_oauth(&state.accounts, &uid, &nickname, &etype, tokens)?;
        // 清理，避免重复入库
        let mut m = caps().lock().unwrap();
        if let Some(c) = m.get_mut(session_id) {
            c.done = true;
            c.captured = None;
        }
        return Ok(PollOutcome::Authorized(acc));
    }

    let expired = {
        let m = caps().lock().unwrap();
        m.get(session_id)
            .map(|c| c.done || Utc::now().timestamp() - c.created > OAUTH_TIMEOUT_SECONDS)
            .unwrap_or(true)
    };
    Ok(if expired { PollOutcome::Expired } else { PollOutcome::Pending })
}
