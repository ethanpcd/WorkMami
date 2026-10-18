//! WorkBuddy / CodeBuddy 云端 API 客户端：积分、状态、签到、OAuth、token 刷新。
//!
//! 说明：以下 endpoint 依据对官方客户端与 Cockpit Tools 的静态分析确定，
//! 响应字段采用防御式解析（多候选键名）。真机联调阶段若个别路径/字段有出入，
//! 只需调整本文件顶部的常量与 pick_* 逻辑。

use std::collections::HashSet;

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::Serialize;
use serde_json::{json, Value};

use crate::accounts::{QuotaResource, QuotaSnapshot};
use crate::error::{WmError, WmResult};
use crate::platform::workmami_paths;
use crate::wbauth::AuthTokens;

// ---- 端点常量（依据 workbuddy-switch / cockpit-tools 的真实实现确定）----
/// 插件/业务 API 域名：OAuth 与 /v2/plugin、/v2/billing 都走这个域。
/// 账号 token 默认签发给 www.codebuddy.cn，业务请求 host 必须与 X-Domain 一致，否则网关拒绝。
pub const PLUGIN_API: &str = "https://www.codebuddy.cn";
/// 反代上游 chat 域名。
pub const UPSTREAM_CHAT: &str = "https://copilot.tencent.com";
/// 加油站/成长中心网页。
pub const GROWTH_CENTER_URL: &str = "https://www.workbuddy.cn/profile/growth-center";

/// 插件接口前缀（OAuth / 刷新）。
pub const PLUGIN_PREFIX: &str = "/v2/plugin";
/// OAuth platform 参数（国内版）。
pub const OAUTH_PLATFORM: &str = "workbuddy";

// OAuth 设备流（公开端点，无需 Turing 设备指纹）
pub const OAUTH_STATE: &str = "/auth/state";
pub const OAUTH_TOKEN: &str = "/auth/token";
pub const OAUTH_ACCOUNT: &str = "/login/account";

// 积分资源（国内版三路接口，POST，host 随账号 domain）：
// summary 给出各资源包当前周期用量；paid/free 给出付费/免费资源包明细（含到期时间）。
pub const RESOURCE_SUMMARY: &str = "/billing/meter/get-user-resource-summary";
pub const RESOURCE_PAID: &str = "/billing/meter/get-user-resource-paid-packages";
pub const RESOURCE_FREE: &str = "/billing/meter/get-user-resource-free-packages";
/// 积分消耗明细（请求级用量）：国内版固定走 workbuddy.cn（与 codebuddy.cn 网关互认 token + X-Domain）。
pub const REQUEST_USAGE_URL: &str = "https://www.workbuddy.cn/billing/meter/get-user-request-usage";
/// WorkBuddy 内置可用模型列表（GET，host 随账号 domain，返回 { models: [...] }）。
/// 依据官方客户端 main/server.js 的 listAvailableModels 确定；服务端会随上新动态变化，需实时拉取。
pub const PERSONAL_MODELS: &str = "/v2/enterprises/personal/models";
// 账号身份（GET，返回 data.accounts[]）
pub const USER_STATUS: &str = "/console/accounts";
// 签到
pub const CHECKIN_STATUS: &str = "/v2/billing/meter/checkin-activity-status";
pub const CHECKIN_DO: &str = "/v2/billing/meter/daily-checkin";
// token 刷新（header X-Refresh-Token、X-Auth-Refresh-Source: plugin）
pub const TOKEN_REFRESH: &str = "/v2/plugin/auth/token/refresh";

/// 根据账号 domain 计算业务 API base（domain 为空时回落 codebuddy.cn）。
pub fn api_base(tokens: &AuthTokens) -> String {
    let d = tokens.domain.trim();
    if d.is_empty() {
        return PLUGIN_API.to_string();
    }
    if d.starts_with("http://") || d.starts_with("https://") {
        return d.trim_end_matches('/').to_string();
    }
    format!("https://{d}")
}

// ---- 积分资源包：商品码清单与官方中文名映射（照抄官方客户端码表）----
/// 到期"临近"阈值（天）。
const EXPIRING_SOON_DAYS: i64 = 7;
/// DeductionEndTime 比 CycleEndTime 晚超过该天数时，视前者为长期占位，改用 CycleEndTime。
const EXPIRY_CYCLE_OVERRIDE_DAYS: i64 = 365;
/// 解析出的到期距 now 超过该天数视为长期有效（expireAt=None），过滤 2049 这类占位值。
const FAR_FUTURE_EXPIRY_DAYS: i64 = 730;

const PAID_PACKAGE_CODES: &[&str] = &[
    "TCACA_code_002_AkiJS3ZHF5",
    "TCACA_code_023_4xbGhMrE6q",
    "TCACA_code_026_BaESVICNoi",
    "TCACA_code_027_0FCGVA6vSa",
    "TCACA_code_009_0XmEQc2xOf",
    "TCACA_code_038_OhvqZtiPKr",
    "TCACA_code_003_FAnt7lcmRT",
    "TCACA_code_036_lupO5WgNdG",
];
const FREE_PACKAGE_CODES: &[&str] = &[
    "TCACA_code_008_cfWoLwvjU4",
    "TCACA_code_007_nzdH5h4Nl0",
    "TCACA_code_028_NtpWi0jzXs",
    "TCACA_code_029_6wCGEWquYy",
    "TCACA_code_030_BjSt89qTvr",
    "TCACA_code_001_PqouKr6QWV",
    "TCACA_code_006_DbXS0lrypC",
    "TCACA_code_035_ArVxJcGDsm",
    "TCACA_code_037_WxOD3MpI2o",
    "TCACA_code_039_KRcQj7wUat",
    "TCACA_code_040_mi9rCYg46x",
];

/// 商品码 → 官方中文名；未登记则回落后端下发的原始包名。
fn package_display_name(code: &str, raw_name: &str) -> String {
    let official = match code {
        "TCACA_code_001_PqouKr6QWV" => "CodeBuddy 个人体验版",
        "TCACA_code_002_AkiJS3ZHF5" => "版本基础用量",
        "TCACA_code_003_FAnt7lcmRT" => "CodeBuddy 个人标准版",
        "TCACA_code_005_maRGyrHhw1" => "版本基础用量",
        "TCACA_code_006_DbXS0lrypC" => "CodeBuddy 个人体验版",
        "TCACA_code_007_nzdH5h4Nl0" => "平台奖励积分",
        "TCACA_code_008_cfWoLwvjU4" => "版本基础用量",
        "TCACA_code_009_0XmEQc2xOf" => "购买积分",
        "TCACA_code_023_4xbGhMrE6q" => "版本基础用量",
        "TCACA_code_026_BaESVICNoi" => "版本基础用量",
        "TCACA_code_027_0FCGVA6vSa" => "版本基础用量",
        "TCACA_code_028_NtpWi0jzXs" => "版本赠送用量",
        "TCACA_code_029_6wCGEWquYy" => "平台奖励积分",
        "TCACA_code_030_BjSt89qTvr" => "平台奖励积分",
        "TCACA_code_035_ArVxJcGDsm" => "版本基础用量",
        "TCACA_code_036_lupO5WgNdG" => "购买积分",
        "TCACA_code_037_WxOD3MpI2o" => "版本赠送用量",
        "TCACA_code_038_OhvqZtiPKr" => "购买积分",
        "TCACA_code_039_KRcQj7wUat" => "版本基础用量",
        "TCACA_code_040_mi9rCYg46x" => "版本基础用量",
        _ => "",
    };
    if !official.is_empty() {
        return official.to_string();
    }
    if !raw_name.is_empty() {
        return raw_name.to_string();
    }
    code.to_string()
}

/// 封装 reqwest 客户端。
pub struct ApiClient {
    http: reqwest::Client,
    /// 反代流式（chat completions SSE）专用客户端：只设连接超时，不设总超时。
    /// 与 `http` 分开的关键原因：reqwest 的 `.timeout()` 是涵盖“读完整个响应体”的总超时，
    /// 而长任务（写文档/多轮工具调用/推理模型思考）的 SSE 可持续数分钟到数十分钟，
    /// 若共用 30 秒总超时会在中途掉掉连接，导致下游“网络错误重试→用户已取消”。
    http_stream: reqwest::Client,
}

impl Default for ApiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent("WorkMami/0.1")
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .expect("reqwest client");
        // 流式专用：不设 `.timeout()`（无总超时上限），仅保留连接超时，保证长 SSE 不被掉断。
        let http_stream = reqwest::Client::builder()
            .user_agent("WorkMami/0.1")
            .connect_timeout(std::time::Duration::from_secs(15))
            .tcp_nodelay(true)
            .build()
            .expect("reqwest stream client");
        Self { http, http_stream }
    }

    fn authed(&self, method: reqwest::Method, url: String, tokens: &AuthTokens) -> reqwest::RequestBuilder {
        self.authed_on(&self.http, method, url, tokens)
    }

    /// 用指定客户端构建带鉴权头的请求（供普通 `http` 与流式 `http_stream` 共用）。
    fn authed_on(&self, client: &reqwest::Client, method: reqwest::Method, url: String, tokens: &AuthTokens) -> reqwest::RequestBuilder {
        let ty = if tokens.token_type.is_empty() { "Bearer" } else { tokens.token_type.as_str() };
        let mut req = client
            .request(method, url)
            .header("Authorization", format!("{ty} {}", tokens.access_token))
            .header("Accept", "application/json, text/plain, */*")
            .header("X-Client-Platform", "web");
        if !tokens.domain.is_empty() {
            req = req.header("X-Domain", &tokens.domain);
        }
        if !tokens.uid.is_empty() {
            req = req.header("X-User-Id", &tokens.uid);
        }
        req
    }

    /// 反代（异步）：向上游 chat completions 发起请求，返回原始响应。
    /// 使用 http_stream（无总超时），避免长任务 SSE 被 30 秒掉断。
    pub async fn post_chat_async(
        &self,
        url: &str,
        tokens: &AuthTokens,
        body: &Value,
    ) -> WmResult<reqwest::Response> {
        let req = self
            .authed_on(&self.http_stream, reqwest::Method::POST, url.to_string(), tokens)
            .header("Content-Type", "application/json")
            .header("Accept", match body.get("stream").and_then(|s| s.as_bool()) {
                Some(true) => "text/event-stream",
                _ => "application/json",
            })
            .json(body);
        let resp = req.send().await?;
        Ok(resp)
    }

    async fn get_json(&self, path: &str, tokens: &AuthTokens) -> WmResult<Value> {
        let url = format!("{}{path}", api_base(tokens));
        let resp = self
            .authed(reqwest::Method::GET, url, tokens)
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        if !status.is_success() {
            return Err(WmError::Upstream(format!("{path} -> {status}: {}", summarize(&body))));
        }
        Ok(body)
    }

    async fn post_json(&self, path: &str, tokens: &AuthTokens, body: &Value) -> WmResult<Value> {
        let url = format!("{}{path}", api_base(tokens));
        let resp = self
            .authed(reqwest::Method::POST, url, tokens)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let out: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        if !status.is_success() {
            return Err(WmError::Upstream(format!("{path} -> {status}: {}", summarize(&out))));
        }
        Ok(out)
    }

    /// 拉取 WorkBuddy 内置可用模型列表（实时，服务端上新即反映）。过滤文生图模型。
    /// 字段以官方客户端为准：id/name/maxInputTokens(上下文窗口)/maxOutputTokens/supports*。
    pub async fn fetch_models(&self, tokens: &AuthTokens) -> WmResult<Vec<ModelInfo>> {
        let resp = self.get_json(PERSONAL_MODELS, tokens).await?;
        // 诊断落盘：保存原始响应，便于真机核对字段（无 token）。
        let _ = std::fs::write(
            workmami_paths().root.join("models-debug.json"),
            serde_json::to_vec_pretty(&resp).unwrap_or_default(),
        );
        let mut out: Vec<ModelInfo> = Vec::new();
        for obj in find_objects(&resp, &["models", "Models"]) {
            let id = match pick_str(&obj, &["id", "Id", "modelId", "model_id"]) {
                Some(s) if !s.trim().is_empty() => s,
                _ => continue,
            };
            // 与官方 listAvailableModels 一致：跳过 tags 含 text-to-image 的文生图模型。
            let is_image = obj
                .get("tags")
                .or_else(|| obj.get("Tags"))
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|x| x.as_str() == Some("text-to-image")))
                .unwrap_or(false);
            if is_image {
                continue;
            }
            let name = pick_str(&obj, &["name", "displayName", "title", "label"])
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| id.clone());
            let num = |keys: &[&str]| pick_f64(&obj, keys).map(|x| x.round() as i64).unwrap_or(0);
            let default_effort = obj
                .get("reasoning")
                .or_else(|| obj.get("Reasoning"))
                .and_then(|r| r.get("effort"))
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string();
            out.push(ModelInfo {
                context_window: num(&["maxInputTokens", "contextWindow", "maxAllowedSize", "contextLength"]),
                max_output_tokens: num(&["maxOutputTokens", "maxCompletionTokens"]),
                supports_tool_call: pick_bool(&obj, &["supportsToolCall", "supports_tools", "toolCall"]).unwrap_or(false),
                supports_images: pick_bool(&obj, &["supportsImages", "supports_vision", "vision"]).unwrap_or(false),
                supports_reasoning: pick_bool(&obj, &["supportsReasoning", "supports_reasoning"]).unwrap_or(false),
                description: pick_str(&obj, &["description", "descriptionCn", "descriptionEn", "desc"]).unwrap_or_default(),
                id,
                name,
                default_effort,
            });
        }
        Ok(out)
    }

    // ---------- 猫猫旅行（成长中心）----------
    /// GET 活动接口：先试原路径，404 再回落 `/v2` 前缀版本（网关两种前缀都收）。
    /// 连接层瞬时失败（WmError::Http 且 reqwest is_request/is_connect/is_timeout）自动重试，
    /// 最多 3 次、间隔 1.2s；业务错误（401/403 等）不重试，避免掩盖真问题。
    /// depart/claim 等写接口故意不重试，防止重复提交。
    async fn get_activity(&self, path: &str, tokens: &AuthTokens) -> WmResult<Value> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.get_activity_once(path, tokens).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    let transient = matches!(&e, WmError::Http(re) if re.is_request() || re.is_connect() || re.is_timeout());
                    if attempt >= 3 || !transient {
                        return Err(e);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                }
            }
        }
    }

    async fn get_activity_once(&self, path: &str, tokens: &AuthTokens) -> WmResult<Value> {
        match self.get_json(path, tokens).await {
            Ok(v) => Ok(v),
            Err(e) if e.to_string().contains("404") => {
                self.get_json(&format!("/v2{path}"), tokens).await
            }
            Err(e) => Err(e),
        }
    }

    /// POST 活动接口：同样带 `/v2` 前缀回落。
    /// 对 4xx 响应，若 body 是有效 JSON（含 code 字段），视为业务错误正常返回给 activity_data 处理。
    async fn post_activity(&self, path: &str, tokens: &AuthTokens, body: &Value) -> WmResult<Value> {
        match self.post_activity_raw(path, tokens, body).await {
            Ok(v) => Ok(v),
            Err(e) if e.to_string().contains("404") => {
                self.post_activity_raw(&format!("/v2{path}"), tokens, body).await
            }
            Err(e) => Err(e),
        }
    }

    async fn post_activity_raw(&self, path: &str, tokens: &AuthTokens, body: &Value) -> WmResult<Value> {
        let url = format!("{}{path}", api_base(tokens));
        let resp = self
            .authed(reqwest::Method::POST, url, tokens)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let out: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        // 4xx 但有有效 JSON body（含 code 字段）→ 交给 activity_data 处理
        if !status.is_success() && out.get("code").is_none() {
            return Err(WmError::Upstream(format!("{path} -> {status}: {}", summarize(&out))));
        }
        Ok(out)
    }

    /// 查询某账号当前猫猫旅行状态。
    pub async fn buddy_travel_status(&self, tokens: &AuthTokens) -> WmResult<TravelStatus> {
        let resp = self.get_activity("/activity/growth/buddy/travel/status", tokens).await?;
        let _ = std::fs::write(
            workmami_paths().root.join("travel-status-debug.json"),
            serde_json::to_vec_pretty(&resp).unwrap_or_default(),
        );
        let data = activity_data(&resp)?;
        let secs = |keys: &[&str]| pick_f64(&data, keys).map(|x| x.round() as i64).unwrap_or(0);
        let loc = data.get("location").cloned().unwrap_or(Value::Null);
        Ok(TravelStatus {
            state: pick_str(&data, &["state", "status"]).unwrap_or_else(|| "idle".into()),
            location_name: pick_str(&loc, &["name", "location_name"]).unwrap_or_default(),
            depart_at: secs(&["depart_at", "departAt"]),
            arrive_at: secs(&["arrive_at", "arriveAt"]),
            server_now: secs(&["server_now", "serverNow"]),
            daily_limit_reached: pick_bool(&data, &["daily_limit_reached", "dailyLimitReached"]).unwrap_or(false),
        })
    }

    /// 拉取旅行地点配置（各地点 id / 名称 / 时长区间）。
    pub async fn buddy_travel_config(&self, tokens: &AuthTokens) -> WmResult<Vec<TravelLocation>> {
        let resp = self.get_activity("/activity/growth/buddy/travel/config", tokens).await?;
        let _ = std::fs::write(
            workmami_paths().root.join("travel-config-debug.json"),
            serde_json::to_vec_pretty(&resp).unwrap_or_default(),
        );
        let data = activity_data(&resp)?;
        let mut out = Vec::new();
        for obj in find_objects(&data, &["locations"]) {
            let id = pick_f64(&obj, &["id", "location_id", "locationId"])
                .map(|x| x.round() as i64)
                .unwrap_or(0);
            if id == 0 {
                continue;
            }
            let name = pick_str(&obj, &["name", "title", "location_name"]).unwrap_or_default();
            let dmin = pick_f64(&obj, &["duration_hours_min", "durationHoursMin"]).unwrap_or(0.0);
            let dmax = pick_f64(&obj, &["duration_hours_max", "durationHoursMax"]).unwrap_or(dmin);
            out.push(TravelLocation {
                id,
                name,
                duration_min_hours: dmin,
                duration_max_hours: dmax,
            });
        }
        Ok(out)
    }

    /// 派猫出发去某个地点。
    pub async fn buddy_travel_depart(&self, tokens: &AuthTokens, location_id: i64) -> WmResult<Value> {
        let resp = self
            .post_activity(
                "/activity/growth/buddy/travel/depart",
                tokens,
                &json!({ "location_id": location_id }),
            )
            .await?;
        activity_data(&resp)
    }

    /// 领取旅行奖励。
    pub async fn buddy_travel_claim(&self, tokens: &AuthTokens) -> WmResult<Value> {
        let resp = self
            .post_activity("/activity/growth/buddy/travel/claim", tokens, &json!({}))
            .await?;
        activity_data(&resp)
    }

    /// 拉取旅行历史记录（服务端真实台账，含每次奖励与领取时间）。
    /// 接口：GET /activity/growth/buddy/travel/records?page=&page_size=，返回按 travel_date 倒序。
    pub async fn buddy_travel_records(&self, tokens: &AuthTokens, page: i64, page_size: i64) -> WmResult<Vec<TravelRecord>> {
        let path = format!("/activity/growth/buddy/travel/records?page={page}&page_size={page_size}");
        let resp = self.get_activity(&path, tokens).await?;
        let data = activity_data(&resp)?;
        let mut out = Vec::new();
        if let Some(arr) = data.get("records").and_then(|r| r.as_array()) {
            for r in arr {
                let name = r
                    .get("location")
                    .and_then(|l| l.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();
                let secs = |keys: &[&str]| {
                    keys.iter()
                        .find_map(|k| r.get(*k).and_then(|x| x.as_f64()))
                        .map(|x| x.round() as i64)
                        .unwrap_or(0)
                };
                out.push(TravelRecord {
                    record_id: secs(&["record_id"]),
                    location_name: name,
                    travel_date: pick_str(r, &["travel_date"]).unwrap_or_default(),
                    depart_at: secs(&["depart_at"]),
                    arrive_at: secs(&["arrive_at"]),
                    claimed_at: secs(&["claimed_at"]),
                    reward_credit: pick_f64(r, &["reward_credit"]).unwrap_or(0.0),
                });
            }
        }
        Ok(out)
    }

    /// 查询积分：并行请求 summary / paid / free 三路资源接口，解析出各分类余额与到期时间，
    /// 再聚合为总额。国内版路径为 `/billing/meter/...`，404 时回落 `/v2/billing/meter/...`。
    pub async fn fetch_quota(&self, tokens: &AuthTokens) -> WmResult<QuotaSnapshot> {
        let now = Utc::now().timestamp_millis();
        let summary = self.post_quota(RESOURCE_SUMMARY, tokens, &json!({})).await;
        let paid = self.post_quota(RESOURCE_PAID, tokens, &paid_packages_body()).await;
        let free = self.post_quota(RESOURCE_FREE, tokens, &free_packages_body()).await;

        // 消费前记录：是否三路全失败 + 一个代表性错误信息。
        let all_failed = summary.is_err() && paid.is_err() && free.is_err();
        let err_msg = summary
            .as_ref()
            .err()
            .or_else(|| paid.as_ref().err())
            .or_else(|| free.as_ref().err())
            .map(|e| e.to_string());

        // 诊断落盘：保存三路原始响应，便于真机核对字段（无 token，仅积分数据）。
        let dump = json!({
            "ts": now,
            "summary": summary.as_ref().ok().cloned().unwrap_or(Value::Null),
            "paid": paid.as_ref().ok().cloned().unwrap_or(Value::Null),
            "free": free.as_ref().ok().cloned().unwrap_or(Value::Null),
        });
        let _ = std::fs::write(
            workmami_paths().root.join("quota-debug.json"),
            serde_json::to_vec_pretty(&dump).unwrap_or_default(),
        );

        // 明细优先（paid/free 带完整到期信息），再并入 summary 中未出现的资源包。
        let mut resources: Vec<QuotaResource> = Vec::new();
        for resp in [paid, free] {
            if let Ok(v) = resp {
                for obj in find_objects(&v, &["Accounts", "accounts"]) {
                    if let Some(r) = parse_resource(&obj, now) {
                        resources.push(r);
                    }
                }
            }
        }
        let seen: HashSet<String> = resources.iter().map(|r| r.package_code.clone()).collect();
        if let Ok(v) = summary {
            for obj in find_objects(&v, &["Packages", "packages"]) {
                if let Some(r) = parse_resource(&obj, now) {
                    if r.package_code.is_empty() || !seen.contains(&r.package_code) {
                        resources.push(r);
                    }
                }
            }
        }

        // 三路全部失败且无任何资源时，视为查询失败。
        if resources.is_empty() && all_failed {
            return Err(WmError::Upstream(
                err_msg.unwrap_or_else(|| "积分查询失败：无响应".into()),
            ));
        }

        let remaining: f64 = resources.iter().map(|r| r.remaining).sum();
        let total: f64 = resources.iter().map(|r| r.total).sum();
        let used: f64 = resources.iter().map(|r| r.used).sum();
        let soonest_expire_at = resources
            .iter()
            .filter(|r| r.remaining > 0.0)
            .filter_map(|r| r.expire_at)
            .min();
        let expiring_soon = resources.iter().any(|r| r.expiring_soon && r.remaining > 0.0);

        Ok(QuotaSnapshot {
            used,
            total,
            remaining,
            plan: String::new(),
            soonest_expire_at,
            expiring_soon,
            resources,
            updated_at: Utc::now(),
        })
    }

    /// 发一次积分资源请求，404 时回落到 `/v2` 前缀版本。
    async fn post_quota(&self, path: &str, tokens: &AuthTokens, body: &Value) -> WmResult<Value> {
        match self.post_json(path, tokens, body).await {
            Ok(v) => Ok(v),
            Err(e) if e.to_string().contains("404") => {
                self.post_json(&format!("/v2{path}"), tokens, body).await
            }
            Err(e) => Err(e),
        }
    }

    /// 平台奖励/购买积分的**发放批次**明细（来自 paid/free 资源接口，每个 account 一批）。
    pub async fn fetch_rewards(&self, tokens: &AuthTokens) -> WmResult<Vec<RewardItem>> {
        let now = Utc::now().timestamp_millis();
        let paid = self.post_quota(RESOURCE_PAID, tokens, &paid_packages_body()).await;
        let free = self.post_quota(RESOURCE_FREE, tokens, &free_packages_body()).await;
        let mut items: Vec<RewardItem> = Vec::new();
        for resp in [paid, free] {
            if let Ok(v) = resp {
                for obj in find_objects(&v, &["Accounts", "accounts"]) {
                    if let Some(it) = parse_reward(&obj, now) {
                        items.push(it);
                    }
                }
            }
        }
        // 仍有余量的优先、按到期升序（快过期的排前面）。
        items.sort_by(|a, b| {
            let ka = if a.remaining > 0.0 { 0 } else { 1 };
            let kb = if b.remaining > 0.0 { 0 } else { 1 };
            ka.cmp(&kb).then(
                a.expire_at.unwrap_or(i64::MAX).cmp(&b.expire_at.unwrap_or(i64::MAX)),
            )
        });
        Ok(items)
    }

    /// 积分消耗明细（请求级），按天数窗口分页拉取，返回按时间倒序的记录。
    pub async fn fetch_usage(&self, tokens: &AuthTokens, days: i64) -> WmResult<Vec<UsageItem>> {
        let days = days.clamp(1, 90);
        let today = Local::now().date_naive();
        let start = today - Duration::days(days - 1);
        let start_time = format!("{start} 00:00:00");
        let end_time = format!("{today} 23:59:59");

        let mut items: Vec<UsageItem> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut page = 1usize;
        loop {
            let body = json!({
                "startTime": start_time,
                "endTime": end_time,
                "pageNum": page,
                "pageSize": 1000,
            });
            let resp = self.post_url(REQUEST_USAGE_URL, tokens, &body).await?;
            if page == 1 {
                // 诊断落盘：首页原始响应，便于核对字段（无 token）。
                let _ = std::fs::write(
                    workmami_paths().root.join("usage-debug.json"),
                    serde_json::to_vec_pretty(&resp).unwrap_or_default(),
                );
            }
            let rows = resp
                .get("data")
                .and_then(|d| d.get("data"))
                .and_then(|a| a.as_array())
                .cloned()
                .unwrap_or_default();
            let raw_len = rows.len();
            let mut added = 0usize;
            for r in rows {
                if let Some(it) = parse_usage_row(&r) {
                    if seen.insert(format!("{}#{}", it.request_id, it.ts)) {
                        items.push(it);
                        added += 1;
                    }
                }
            }
            // 末页、无新增（服务端忽略分页）、或超过安全轮数即停。
            if raw_len < 1000 || added == 0 || page >= 30 {
                break;
            }
            page += 1;
        }
        items.sort_by(|a, b| b.ts.cmp(&a.ts));
        Ok(items)
    }

    /// 向固定 URL 发带鉴权头的 POST。
    async fn post_url(&self, url: &str, tokens: &AuthTokens, body: &Value) -> WmResult<Value> {
        let resp = self
            .authed(reqwest::Method::POST, url.to_string(), tokens)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let out: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        if !status.is_success() {
            return Err(WmError::Upstream(format!("{url} -> {status}: {}", summarize(&out))));
        }
        Ok(out)
    }

    /// 查询账号身份（GET /console/accounts，返回 data.accounts[]）。
    pub async fn fetch_status(&self, tokens: &AuthTokens) -> WmResult<Value> {
        self.get_json(USER_STATUS, tokens).await
    }

    /// 查询签到状态：返回 (今日是否已签, 连续天数, 原始)。
    pub async fn fetch_checkin_status(&self, tokens: &AuthTokens) -> WmResult<(bool, i64, Value)> {
        let v = self.post_json(CHECKIN_STATUS, tokens, &serde_json::json!({})).await?;
        let data = unwrap_data(&v);
        let checked = deep_bool(&data, &["today_checked_in", "checkedIn", "isCheckin", "checked", "signed", "hasCheckedIn", "isChecked", "todayChecked"])
            .unwrap_or(false);
        let streak = deep_i64(&data, &["streak_days", "streakDays", "streak", "continuousDays", "continueDays"]).unwrap_or(0);
        Ok((checked, streak, v))
    }

    /// 执行签到。返回 (成功?, 已获得积分, 原始响应)。
    /// 若已签到（code=10001 或 msg 含"已签到"），视为成功且 reward=0。
    pub async fn do_checkin(&self, tokens: &AuthTokens) -> WmResult<(bool, f64, Value)> {
        let url = format!("{}{CHECKIN_DO}", api_base(tokens));
        let resp = self
            .authed(reqwest::Method::POST, url, tokens)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        let data = unwrap_data(&body);
        let code = pick_i64(&body, &["code"]).unwrap_or(0);
        let msg = pick_str(&body, &["msg", "message"]).unwrap_or_default();
        // "已签到"视为成功（code 10001 或消息含"已签到"）
        let already = code == 10001 || msg.contains("已签到");
        let code_ok = code == 0 || already;
        let ok = (status.is_success() || already) && code_ok;
        let reward = deep_f64(&data, &["reward", "credits", "points", "amount", "gain", "addCredits", "creditNum", "addPoints", "checkinReward", "rewardCredits"]).unwrap_or(0.0);
        // 落一份原始响应，便于真机校准“到账积分”到底在哪个字段（WorkBuddy 不用通用键名）。
        let _ = std::fs::write(
            workmami_paths().root.join("checkin-debug.json"),
            serde_json::to_vec_pretty(&body).unwrap_or_default(),
        );
        Ok((ok, reward, body))
    }

    /// 刷新 access token（POST /v2/plugin/auth/token/refresh，header X-Refresh-Token）。
    pub async fn refresh_token(&self, tokens: &AuthTokens) -> WmResult<AuthTokens> {
        let url = format!("{}{TOKEN_REFRESH}", api_base(tokens));
        let mut req = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Refresh-Token", &tokens.refresh_token)
            .header("X-Auth-Refresh-Source", "plugin");
        let domain = if tokens.domain.is_empty() { "www.codebuddy.cn".to_string() } else { tokens.domain.clone() };
        req = req.header("X-Domain", &domain);
        if !tokens.uid.is_empty() {
            req = req.header("X-User-Id", &tokens.uid);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        if !status.is_success() {
            return Err(WmError::Credential(format!("刷新失败 {status}: {}", summarize(&body))));
        }
        let data = unwrap_data(&body);
        let access = pick_str(&data, &["accessToken", "access_token"]).ok_or_else(|| WmError::Credential("刷新响应缺 accessToken".into()))?;
        let refresh = pick_str(&data, &["refreshToken", "refresh_token"]).unwrap_or_else(|| tokens.refresh_token.clone());
        let now = Utc::now();
        let expires_in = pick_i64(&data, &["expiresIn", "expires_in"]).unwrap_or(2419200);
        Ok(AuthTokens {
            access_token: access,
            refresh_token: refresh,
            token_type: pick_str(&data, &["tokenType", "token_type"]).unwrap_or_else(|| "Bearer".into()),
            expires_at: Some(now.timestamp_millis() + expires_in * 1000),
            refresh_expires_at: pick_i64(&data, &["refreshExpiresAt", "refresh_expires_at"]).or(tokens.refresh_expires_at),
            scope: pick_str(&data, &["scope"]).unwrap_or_else(|| tokens.scope.clone()),
            domain: pick_str(&data, &["domain"]).unwrap_or_else(|| tokens.domain.clone()),
            uid: if tokens.uid.is_empty() { pick_str(&data, &["uid"]).unwrap_or_default() } else { tokens.uid.clone() },
        })
    }

    /// OAuth 设备流第 1 步：申请 state。返回 (state, authUrl)。
    pub async fn oauth_state(&self) -> WmResult<(String, String)> {
        let url = format!("{PLUGIN_API}{PLUGIN_PREFIX}{OAUTH_STATE}?platform={OAUTH_PLATFORM}");
        let resp = self
            .http
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        if !status.is_success() {
            return Err(WmError::Upstream(format!("auth/state -> {status}: {}", summarize(&body))));
        }
        let data = unwrap_data(&body);
        let state = pick_str(&data, &["state"])
            .ok_or_else(|| WmError::Upstream(format!("auth/state 响应缺少 state: {}", summarize(&body))))?;
        let auth_url = pick_str(&data, &["authUrl", "auth_url", "url"])
            .unwrap_or_else(|| format!("{PLUGIN_API}/login?state={state}"));
        Ok((state, auth_url))
    }

    /// OAuth 设备流第 2 步：轮询 token。未登录返回 None，成功返回 Some。
    pub async fn oauth_token(&self, state: &str) -> WmResult<Option<OAuthTokenData>> {
        let url = format!("{PLUGIN_API}{PLUGIN_PREFIX}{OAUTH_TOKEN}?state={state}");
        let resp = self
            .http
            .get(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .send()
            .await?;
        let body: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 && code != 200 {
            return Ok(None); // 用户尚未完成登录
        }
        let data = unwrap_data(&body);
        let access = match pick_str(&data, &["accessToken", "access_token"]) {
            Some(a) if !a.is_empty() => a,
            _ => return Ok(None),
        };
        let now = Utc::now().timestamp_millis();
        let expires_at = norm_ts(data.get("expiresAt").or_else(|| data.get("expires_at")))
            .or_else(|| data.get("expiresIn").or_else(|| data.get("expires_in")).and_then(|v| v.as_i64()).map(|e| now + e * 1000));
        Ok(Some(OAuthTokenData {
            access,
            refresh: pick_str(&data, &["refreshToken", "refresh_token"]).unwrap_or_default(),
            domain: pick_str(&data, &["domain"]).unwrap_or_else(|| "www.codebuddy.cn".into()),
            expires_at,
        }))
    }

    /// OAuth 设备流第 3 步：用 token 拉账号信息。返回 data 对象（uid/nickname/enterprise...）。
    pub async fn oauth_account(&self, state: &str, access: &str, domain: &str) -> WmResult<Value> {
        let url = format!("{PLUGIN_API}{PLUGIN_PREFIX}{OAUTH_ACCOUNT}?state={state}");
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {access}"))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .header("X-Domain", domain)
            .send()
            .await?;
        let body: Value = resp.json().await.unwrap_or_else(|_| Value::Null);
        Ok(unwrap_data(&body))
    }
}

/// OAuth token 换取结果。
pub struct OAuthTokenData {
    pub access: String,
    pub refresh: String,
    pub domain: String,
    pub expires_at: Option<i64>,
}

/// 平台奖励/购买积分的一个发放批次。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardItem {
    /// 分类展示名（映射后的中文名）。
    pub name: String,
    /// 下发原因（如"官方活动发放"）；无则空串。
    pub reason: String,
    pub used: f64,
    pub total: f64,
    pub remaining: f64,
    pub expire_at: Option<i64>,
    /// 使用中 / 已用完 / 已过期。
    pub status: String,
}

/// 一条请求级积分消耗记录。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageItem {
    pub time: String,
    pub ts: i64,
    pub request_id: String,
    pub credits: f64,
    pub model: String,
    pub client: String,
}

/// WorkBuddy 内置模型（来自 /v2/enterprises/personal/models，字段以官方客户端为准）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// 上游模型 id（如 wb-balanced），填入请求的 model 字段。
    pub id: String,
    /// 展示名（缺失时回落 id）。
    pub name: String,
    /// 上下文窗口（输入上限，token）。
    pub context_window: i64,
    /// 单次最大输出（token）。
    pub max_output_tokens: i64,
    pub supports_tool_call: bool,
    pub supports_images: bool,
    pub supports_reasoning: bool,
    /// 默认思考强度（low/medium/high/none，缺省为空）。
    pub default_effort: String,
    pub description: String,
}

/// 猫猫旅行的一个可选地点（来自 travel/config）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TravelLocation {
    pub id: i64,
    pub name: String,
    /// 预计出行时长下限（小时）。
    pub duration_min_hours: f64,
    /// 预计出行时长上限（小时）。
    pub duration_max_hours: f64,
}

/// 某账号当前猫猫旅行状态（来自 travel/status）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TravelStatus {
    /// idle / traveling / arrived
    pub state: String,
    pub location_name: String,
    /// 出发时间（unix 秒）。
    pub depart_at: i64,
    /// 到达（可领取）时间（unix 秒）。
    pub arrive_at: i64,
    /// 服务器当前时间（unix 秒），用于校准倒计时避免本地时钟漂移。
    pub server_now: i64,
    /// 服务端下发的“今日旅行次数已达上限”标志：为 true 时 state 仍是 idle，
    /// 但再派猫会被服务端拒绝，自动托管需跳过出发并在界面明示，否则用户以为没触发。
    pub daily_limit_reached: bool,
}

/// 单次旅行历史记录（来自 travel/records，服务端真实台账）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TravelRecord {
    pub record_id: i64,
    pub location_name: String,
    /// 服务端归属日期 YYYY-MM-DD（用于“今日”筛选）。
    pub travel_date: String,
    /// unix 秒；claimed_at=0 表示奖励尚未领取。
    pub depart_at: i64,
    pub arrive_at: i64,
    pub claimed_at: i64,
    pub reward_credit: f64,
}

/// 解析单个发放批次。
fn parse_reward(obj: &Value, now: i64) -> Option<RewardItem> {
    let total = num_with_slice(obj, TOTAL_KEYS).unwrap_or(0.0);
    let remaining = num_with_slice(obj, REMAIN_KEYS).unwrap_or(0.0);
    let used = num_with_slice(obj, USED_KEYS).unwrap_or((total - remaining).max(0.0));
    if total <= 0.0 && remaining <= 0.0 {
        return None;
    }
    let code = pick_str(obj, &["PackageCode", "packageCode"]).unwrap_or_default();
    let raw_name = pick_str(obj, &["PackageName", "packageName"]).unwrap_or_default();
    let name = package_display_name(&code, &raw_name);
    let reason = pick_str(
        obj,
        &[
            "GrantReason", "grantReason", "Reason", "reason", "Description", "description",
            "Source", "source", "Remark", "remark", "PackageName", "packageName",
        ],
    )
    .unwrap_or_default();
    let expire_at = resolve_expire_at(obj, now);
    let expired = expire_at.map(|e| e <= now).unwrap_or(false);
    let status = if expired {
        "已过期"
    } else if remaining <= 0.0 {
        "已用完"
    } else {
        "使用中"
    }
    .to_string();
    Some(RewardItem {
        name,
        reason,
        used: used.max(0.0),
        total: total.max(0.0),
        remaining: remaining.max(0.0),
        expire_at,
        status,
    })
}

/// 解析一条请求级用量记录。
fn parse_usage_row(v: &Value) -> Option<UsageItem> {
    let credits = v.get("credit").and_then(value_as_f64)?;
    if !credits.is_finite() || credits < 0.0 {
        return None;
    }
    let raw_time = v.get("requestTime").or_else(|| v.get("request_time"))?;
    let ts = parse_ts_ms(Some(raw_time))?;
    let time = raw_time
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| Local.timestamp_millis_opt(ts).single().map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default());
    Some(UsageItem {
        time,
        ts,
        request_id: pick_str(v, &["requestId", "request_id", "id", "RequestId"]).unwrap_or_default(),
        credits,
        model: pick_str(v, &["model", "Model"]).unwrap_or_else(|| "—".into()),
        client: pick_str(v, &["client", "clientType", "platform", "Client"]).unwrap_or_else(|| "—".into()),
    })
}

/// 秒/毫秒时间戳统一为毫秒。
fn norm_ts(v: Option<&Value>) -> Option<i64> {
    let mut ts: i64 = match v {
        Some(Value::String(s)) => s.trim().parse::<f64>().ok()? as i64,
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64))?,
        _ => return None,
    };
    if ts < 10_000_000_000 {
        ts *= 1000;
    }
    Some(ts)
}

// ---------- 积分资源解析（summary / paid / free 三路）----------

/// 付费包查询请求体。
fn paid_packages_body() -> Value {
    json!({
        "PageNumber": 1,
        "PageSize": 200,
        "Status": [0, 3],
        "PackageCodes": PAID_PACKAGE_CODES,
        "NeedRenewInfo": true,
    })
}

/// 免费包查询请求体（带当天时间窗）。
fn free_packages_body() -> Value {
    let now = Local::now();
    let day = now.date_naive();
    let start = day
        .and_hms_opt(0, 0, 0)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default();
    let end = day
        .and_hms_opt(23, 59, 59)
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default();
    json!({
        "PageNumber": 1,
        "PageSize": 200,
        "Status": [0, 3],
        "SlicePeriodStartTime": start,
        "SlicePeriodEndTime": end,
        "PackageCodes": FREE_PACKAGE_CODES,
    })
}

/// 递归查找第一个键名为 `keys` 之一的数组，返回其元素（拥有所有权）。
fn find_objects(v: &Value, keys: &[&str]) -> Vec<Value> {
    match v {
        Value::Object(map) => {
            for k in keys {
                if let Some(Value::Array(arr)) = map.get(*k) {
                    return arr.clone();
                }
            }
            for val in map.values() {
                let found = find_objects(val, keys);
                if !found.is_empty() {
                    return found;
                }
            }
            Vec::new()
        }
        Value::Array(arr) => {
            for val in arr {
                let found = find_objects(val, keys);
                if !found.is_empty() {
                    return found;
                }
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

const TOTAL_KEYS: &[&str] = &[
    "CycleCapacitySizePrecise",
    "CycleCapacitySize",
    "CycleTotalCapacity",
    "CapacitySizePrecise",
    "CapacitySize",
    "SlicePeriodCapacitySizePrecise",
    "SlicePeriodCapacitySize",
];
const REMAIN_KEYS: &[&str] = &[
    "CycleCapacityRemainPrecise",
    "CycleCapacityRemain",
    "CycleRemainCapacity",
    "CapacityRemainPrecise",
    "CapacityRemain",
    "SlicePeriodCapacityRemainPrecise",
    "SlicePeriodCapacityRemain",
];
const USED_KEYS: &[&str] = &[
    "CycleCapacityUsedPrecise",
    "CycleCapacityUsed",
    "CycleUsedCapacity",
    "CapacityUsedPrecise",
    "CapacityUsed",
    "SlicePeriodCapacityUsedPrecise",
    "SlicePeriodCapacityUsed",
];

/// 取数值：先看对象本身，再看 SlicePeriodUsageDetails[0]。
fn num_with_slice(obj: &Value, keys: &[&str]) -> Option<f64> {
    if let Some(x) = keys.iter().find_map(|k| obj.get(*k)).and_then(value_as_f64) {
        return Some(x);
    }
    for sk in ["SlicePeriodUsageDetails", "slicePeriodUsageDetails"] {
        if let Some(first) = obj.get(sk).and_then(|v| v.as_array()).and_then(|a| a.first()) {
            if let Some(x) = keys.iter().find_map(|k| first.get(*k)).and_then(value_as_f64) {
                return Some(x);
            }
        }
    }
    None
}

/// 解析到期时间：优先 DeductionEndTime/ExpiredTime；若它比 CycleEndTime 晚超过
/// 一年则视其为长期占位、改用 CycleEndTime；最终距 now 超两年则视为长期有效（None）。
fn resolve_expire_at(obj: &Value, now: i64) -> Option<i64> {
    let deduction = parse_ts_ms(obj.get(first_key(
        obj,
        &["DeductionEndTime", "deductionEndTime", "ExpiredTime", "expiredTime"],
    )?));
    let cycle = parse_ts_ms(obj.get(first_key(obj, &["CycleEndTime", "cycleEndTime"])?));
    let override_ms = EXPIRY_CYCLE_OVERRIDE_DAYS * 24 * 3600 * 1000;
    let expire = match (deduction, cycle) {
        (Some(d), Some(c)) if d.saturating_sub(c) > override_ms => Some(c),
        (Some(d), _) => Some(d),
        (None, cc) => cc,
    };
    let far_ms = FAR_FUTURE_EXPIRY_DAYS * 24 * 3600 * 1000;
    expire.filter(|v| v.saturating_sub(now) <= far_ms)
}

/// 返回对象中第一个存在的候选键名。
fn first_key<'a>(obj: &'a Value, keys: &[&'a str]) -> Option<&'a str> {
    keys.iter().copied().find(|k| obj.get(*k).is_some())
}

/// 解析时间戳为毫秒：支持秒/毫秒数字、RFC3339、"%Y-%m-%d %H:%M:%S"（本地）、纯日期。
fn parse_ts_ms(v: Option<&Value>) -> Option<i64> {
    let v = v?;
    if let Some(n) = value_as_f64(v) {
        let ms = if n.abs() < 10_000_000_000.0 { n * 1000.0 } else { n };
        return Some(ms.round() as i64);
    }
    let text = v.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(text) {
        return Some(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, fmt) {
            if let Some(local) = Local.from_local_datetime(&naive).single() {
                return Some(local.timestamp_millis());
            }
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        if let Some(naive) = date.and_hms_opt(23, 59, 59) {
            if let Some(local) = Local.from_local_datetime(&naive).single() {
                return Some(local.timestamp_millis());
            }
        }
    }
    None
}

/// 解析单个资源包对象为 QuotaResource；全零（无额度）返回 None。
fn parse_resource(obj: &Value, now: i64) -> Option<QuotaResource> {
    let total = num_with_slice(obj, TOTAL_KEYS).unwrap_or(0.0);
    let remaining = num_with_slice(obj, REMAIN_KEYS).unwrap_or(0.0);
    let used = num_with_slice(obj, USED_KEYS)
        .unwrap_or_else(|| (total - remaining).max(0.0));
    if total <= 0.0 && remaining <= 0.0 && used <= 0.0 {
        return None;
    }
    let code = pick_str(obj, &["PackageCode", "packageCode"]).unwrap_or_default();
    let raw_name = pick_str(obj, &["PackageName", "packageName"]).unwrap_or_default();
    let name = package_display_name(&code, &raw_name);
    let expire_at = resolve_expire_at(obj, now);
    let expired = expire_at.map(|e| e <= now).unwrap_or(false);
    let expiring_soon = expire_at
        .map(|e| e > now && e - now <= EXPIRING_SOON_DAYS * 24 * 3600 * 1000)
        .unwrap_or(false);
    Some(QuotaResource {
        package_code: code,
        name,
        used: used.max(0.0),
        total: total.max(0.0),
        remaining: remaining.max(0.0),
        expire_at,
        expired,
        expiring_soon,
    })
}

/// access token 是否临近过期（留 5 分钟余量）。
pub fn should_refresh(tokens: &AuthTokens) -> bool {
    match tokens.expires_at {
        Some(ms) => Utc::now().timestamp_millis() > ms - 300_000,
        None => false,
    }
}

pub fn ts_to_datetime(ms: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_else(Utc::now)
}

// ---------- 防御式解析工具 ----------

/// 很多接口把有效负载包在 data / result / body 里。
fn unwrap_data(v: &Value) -> Value {
    if let Some(obj) = v.as_object() {
        for k in ["data", "result", "body", "payload"] {
            if let Some(d) = obj.get(k) {
                if !d.is_null() {
                    return d.clone();
                }
            }
        }
    }
    v.clone()
}

/// 活动接口专用：检查 code==0 并提取 data 字段，code 非零视为上游业务错误。
fn activity_data(resp: &Value) -> WmResult<Value> {
    let code = resp.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    if code != 0 {
        let raw_msg = pick_str(resp, &["msg", "message"])
            .unwrap_or_else(|| format!("接口返回 code={code}"));
        // 翻译常见英文错误为中文
        let msg = match raw_msg.as_str() {
            "daily limit reached" => "今日旅行次数已用完，请明天再派".to_string(),
            "already traveling" => "猫猫正在旅行中，请等待归来".to_string(),
            "no active travel" => "当前没有进行中的旅行".to_string(),
            "reward already claimed" => "该奖励已领取过了".to_string(),
            _ => raw_msg,
        };
        return Err(WmError::Upstream(msg));
    }
    Ok(resp.get("data").cloned().unwrap_or(Value::Null))
}

fn pick_f64(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| v.get(*k)).and_then(value_as_f64)
}
fn pick_i64(v: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|k| v.get(*k)).and_then(|x| x.as_i64())
}
fn pick_bool(v: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|k| v.get(*k)).and_then(|x| x.as_bool())
}

/// 递归地在任意层级查找第一个匹配 key 的数值。
fn deep_f64(v: &Value, keys: &[&str]) -> Option<f64> {
    if let Some(x) = pick_f64(v, keys) {
        return Some(x);
    }
    match v {
        Value::Object(map) => map.values().find_map(|x| deep_f64(x, keys)),
        Value::Array(arr) => arr.iter().find_map(|x| deep_f64(x, keys)),
        _ => None,
    }
}
fn deep_i64(v: &Value, keys: &[&str]) -> Option<i64> {
    if let Some(x) = keys.iter().find_map(|k| v.get(*k)).and_then(|x| x.as_i64()) {
        return Some(x);
    }
    match v {
        Value::Object(map) => map.values().find_map(|x| deep_i64(x, keys)),
        Value::Array(arr) => arr.iter().find_map(|x| deep_i64(x, keys)),
        _ => None,
    }
}
fn deep_bool(v: &Value, keys: &[&str]) -> Option<bool> {
    if let Some(x) = pick_bool(v, keys) {
        return Some(x);
    }
    match v {
        Value::Object(map) => map.values().find_map(|x| deep_bool(x, keys)),
        Value::Array(arr) => arr.iter().find_map(|x| deep_bool(x, keys)),
        _ => None,
    }
}
fn pick_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(*k))
        .and_then(|x| x.as_str().map(|s| s.to_string()))
}

fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn summarize(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 300 {
        format!("{}…", &s[..300])
    } else {
        s
    }
}
