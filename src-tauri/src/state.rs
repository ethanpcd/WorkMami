//! 全局应用状态与可持久化设置。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::accounts::AccountStore;
use crate::api::ApiClient;
use crate::error::WmResult;
use crate::platform::workmami_paths;
use crate::usage::UsageStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    RoundRobin,
    Smart,
    Drain,
}

impl Default for Strategy {
    fn default() -> Self {
        Strategy::Smart
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProxySettings {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// 访问本地反代所需的 key；留空表示不校验
    pub api_key: String,
    pub strategy: Strategy,
    /// 上下文窗口（token），注入上游前据此裁剪
    pub context_window: u32,
    /// 默认思考强度：low/medium/high/none
    pub reasoning_effort: String,
    /// 默认模型（客户端未指定时）
    pub default_model: String,
    /// 会话粘性窗口（分钟）
    pub sticky_ttl_min: u32,
    /// failover 最大重试账号数
    pub max_retries: u32,
}

impl Default for ProxySettings {
    fn default() -> Self {
        ProxySettings {
            enabled: false,
            host: "127.0.0.1".into(),
            port: 8765,
            api_key: String::new(),
            strategy: Strategy::default(),
            context_window: 128_000,
            reasoning_effort: "medium".into(),
            default_model: "wb-balanced".into(),
            sticky_ttl_min: 30,
            max_retries: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckinSettings {
    pub auto_enabled: bool,
    /// 每日触发时间 "HH:MM"（本地）
    pub daily_time: String,
    pub notify: bool,
}

impl Default for CheckinSettings {
    fn default() -> Self {
        CheckinSettings { auto_enabled: false, daily_time: "09:00".into(), notify: true }
    }
}

/// 猫猫旅行（成长中心）自动托管设置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TravelSettings {
    /// 自动托管总开关：开着就自动派猫、到点自动领（客户端运行期间）。
    pub auto_enabled: bool,
    /// 自动派猫时选地点策略：longest（时长最长优先）/ shortest（最短）/ rotate（轮换）。
    pub location_policy: String,
}

impl Default for TravelSettings {
    fn default() -> Self {
        TravelSettings { auto_enabled: true, location_policy: "longest".into() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub proxy: ProxySettings,
    pub checkin: CheckinSettings,
    pub travel: TravelSettings,
    pub theme: String, // dark / light / system
    pub autostart: bool,
}

/// 余额快照（用于积分阶梯线）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalancePoint {
    pub ts: i64,
    pub uid: String,
    pub remaining: f64,
}

pub struct AppState {
    pub accounts: Arc<AccountStore>,
    pub usage: Arc<UsageStore>,
    pub api: Arc<ApiClient>,
    pub settings: RwLock<AppSettings>,
    /// 账号冷却到期时间（unix ms）
    pub cooldown: Mutex<HashMap<String, i64>>,
    /// 会话粘性：session_hash -> (uid, 到期 ms)
    pub sticky: Mutex<HashMap<String, (String, i64)>>,
    /// 轮询游标
    pub rr_cursor: Mutex<usize>,
    /// 反代运行控制
    pub proxy_shutdown: Mutex<Option<Arc<Notify>>>,
    pub proxy_running: Mutex<bool>,
    /// 反代实例代次：每次 start 递增。serve 任务退出时仅当自己仍是当前代次才清除 running，
    /// 避免旧实例优雅关闭退出时把新实例的运行状态误覆盖为 false。
    pub proxy_gen: Mutex<u64>,
    /// 余额快照历史
    pub balance_history: Mutex<Vec<BalancePoint>>,
    /// WorkBuddy 内置模型列表缓存（序列化的 ModelInfo，供反代 /v1/models 复用）。
    pub models_cache: Mutex<Vec<serde_json::Value>>,
    /// 本次运行会话内曾观测到"旅行中"的账号：用于区分"开着时到点→自动领"与"关着时到点→下次打开提醒"。
    pub travel_seen_traveling: Mutex<HashSet<String>>,
    /// 已就"离线期间到达"提醒过的账号，避免每轮重复弹提醒。
    pub travel_reminded: Mutex<HashSet<String>>,
    /// 轮换策略游标（uid -> 下次应选地点下标）。
    pub travel_rotate: Mutex<HashMap<String, usize>>,
    /// 本次运行会话内已执行过自动签到的日期（YYYY-MM-DD），用于"到点后首个 tick 触发一次"兼当日去重，
    /// 避免错过时刻不补签、以及同日重复触发。
    pub auto_checkin_done_date: Mutex<String>,
    /// 旅行自动托管"当日熔断"日期（YYYY-MM-DD）：连续多次确认全部账号今日已达派猫上限后，
    /// 置为当天，之后当日 travel_tick 直接早退、不再调用任何官方接口，避免高频轮询触发账号风控。
    pub travel_done_date: Mutex<String>,
    /// 本轮会话内连续观测到"所有账号今日都已完成（idle+达上限）"的次数，达阈值才触发当日熔断（防偶发误判）。
    pub travel_limit_streak: Mutex<i32>,
}

impl AppState {
    pub fn load() -> WmResult<Self> {
        let settings = Self::read_settings();
        let balance_history = Self::read_balance_history();
        Ok(AppState {
            accounts: Arc::new(AccountStore::load()?),
            usage: Arc::new(UsageStore::open()?),
            api: Arc::new(ApiClient::new()),
            settings: RwLock::new(settings),
            cooldown: Mutex::new(HashMap::new()),
            sticky: Mutex::new(HashMap::new()),
            rr_cursor: Mutex::new(0),
            proxy_shutdown: Mutex::new(None),
            proxy_running: Mutex::new(false),
            proxy_gen: Mutex::new(0),
            balance_history: Mutex::new(balance_history),
            models_cache: Mutex::new(Vec::new()),
            travel_seen_traveling: Mutex::new(HashSet::new()),
            travel_reminded: Mutex::new(HashSet::new()),
            travel_rotate: Mutex::new(HashMap::new()),
            auto_checkin_done_date: Mutex::new(String::new()),
            travel_done_date: Mutex::new(String::new()),
            travel_limit_streak: Mutex::new(0),
        })
    }

    fn read_settings() -> AppSettings {
        let path = workmami_paths().settings_file.clone();
        let mut s: AppSettings = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // 默认亮色：全新安装或旧配置里 theme 为空时回落为 light（light/dark/system 才是有效选项）。
        if s.theme.is_empty() {
            s.theme = "light".into();
        }
        s
    }

    pub fn save_settings(&self) -> WmResult<()> {
        let path = workmami_paths().settings_file.clone();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let s = self.settings.read().unwrap();
        std::fs::write(&path, serde_json::to_vec_pretty(&*s)?)?;
        Ok(())
    }

    pub fn proxy_settings(&self) -> ProxySettings {
        self.settings.read().unwrap().proxy.clone()
    }

    pub fn record_balance(&self, uid: &str, remaining: f64) {
        let snapshot = {
            let mut h = self.balance_history.lock().unwrap();
            h.push(BalancePoint { ts: Utc::now().timestamp_millis(), uid: uid.to_string(), remaining });
            if h.len() > 5000 {
                let excess = h.len() - 5000;
                h.drain(..excess);
            }
            serde_json::to_vec(&*h).unwrap_or_default()
        };
        // 落盘：余额快照必须跨重启保留，否则每次启动清空、趋势线永远只有几个点。
        let path = workmami_paths().root.join("balance-history.json");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&path, snapshot);
    }

    fn balance_history_file() -> std::path::PathBuf {
        workmami_paths().root.join("balance-history.json")
    }

    fn read_balance_history() -> Vec<BalancePoint> {
        std::fs::read(Self::balance_history_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn is_cooling(&self, uid: &str) -> bool {
        let now = Utc::now().timestamp_millis();
        let c = self.cooldown.lock().unwrap();
        c.get(uid).map(|t| *t > now).unwrap_or(false)
    }

    pub fn cool_down(&self, uid: &str, ms: i64) {
        let now = Utc::now().timestamp_millis();
        self.cooldown.lock().unwrap().insert(uid.to_string(), now + ms);
    }

    pub fn clear_cooling(&self, uid: &str) {
        self.cooldown.lock().unwrap().remove(uid);
    }

    pub fn sticky_account(&self, hash: &str, ttl_min: u32) -> Option<String> {
        let now = Utc::now().timestamp_millis();
        let mut m = self.sticky.lock().unwrap();
        if let Some((uid, exp)) = m.get(hash) {
            if *exp > now && !self_is_cooling(&self.cooldown, uid, now) {
                return Some(uid.clone());
            }
        }
        let _ttl = ttl_min;
        None
    }

    pub fn set_sticky(&self, hash: &str, uid: &str, ttl_min: u32) {
        let now = Utc::now().timestamp_millis();
        self.sticky.lock().unwrap().insert(hash.to_string(), (uid.to_string(), now + ttl_min as i64 * 60_000));
    }
}

fn self_is_cooling(cd: &Mutex<HashMap<String, i64>>, uid: &str, now: i64) -> bool {
    cd.lock().unwrap().get(uid).map(|t| *t > now).unwrap_or(false)
}

/// 供命令层使用的便捷时间类型别名。
pub type Now = DateTime<Utc>;
