use std::error::Error;
use std::fmt;

use serde::Serialize;

/// 统一的库错误类型。向前端序列化时带上可读 message 与分类 code。
/// Display 手写（不用 thiserror derive），便于 Http 变体把 reqwest 原始英文错误
/// 转成中文友好提示；新增变体时需在下方 fmt 分支里同步补展示文案。
#[derive(Debug)]
pub enum WmError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Db(rusqlite::Error),
    Http(reqwest::Error),
    Keyring(keyring::Error),
    Zip(zip::result::ZipError),
    Scheduler(String),
    Crypto(String),
    NotFound(String),
    Credential(String),
    Config(String),
    Upstream(String),
    Other(String),
}

impl fmt::Display for WmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            WmError::Io(e) => format!("IO 错误: {e}"),
            WmError::Json(e) => format!("JSON 错误: {e}"),
            WmError::Db(e) => format!("SQLite 错误: {e}"),
            WmError::Http(e) => humanize_reqwest(e),
            WmError::Keyring(e) => format!("钥匙串错误: {e}"),
            WmError::Zip(e) => format!("压缩错误: {e}"),
            WmError::Scheduler(e) => format!("调度错误: {e}"),
            WmError::Crypto(e) => format!("加解密错误: {e}"),
            WmError::NotFound(e) => format!("未找到: {e}"),
            WmError::Credential(e) => format!("凭证不可用: {e}"),
            WmError::Config(e) => format!("配置错误: {e}"),
            WmError::Upstream(e) => format!("上游错误: {e}"),
            WmError::Other(e) => format!("其它: {e}"),
        };
        f.write_str(&s)
    }
}

impl Error for WmError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            WmError::Io(e) => Some(e),
            WmError::Json(e) => Some(e),
            WmError::Db(e) => Some(e),
            WmError::Http(e) => Some(e),
            WmError::Keyring(e) => Some(e),
            WmError::Zip(e) => Some(e),
            _ => None,
        }
    }
}

macro_rules! impl_from {
    ($variant:ident, $ty:ty) => {
        impl From<$ty> for WmError {
            fn from(e: $ty) -> Self {
                WmError::$variant(e)
            }
        }
    };
}

impl_from!(Io, std::io::Error);
impl_from!(Json, serde_json::Error);
impl_from!(Db, rusqlite::Error);
impl_from!(Http, reqwest::Error);
impl_from!(Keyring, keyring::Error);
impl_from!(Zip, zip::result::ZipError);

/// reqwest 错误转中文友好提示：连接层失败（请求没发出去/超时）直接透传
/// "error sending request for url(...)" 英文原文会让用户误以为程序坏了，其实是网络抖动。
fn humanize_reqwest(e: &reqwest::Error) -> String {
    if e.is_request() || e.is_connect() || e.is_timeout() {
        "网络连接失败，请检查网络后重试".to_string()
    } else if e.is_body() || e.is_decode() {
        "网络响应异常，请稍后重试".to_string()
    } else {
        format!("HTTP 错误: {e}")
    }
}

impl From<tokio_cron_scheduler::JobSchedulerError> for WmError {
    fn from(e: tokio_cron_scheduler::JobSchedulerError) -> Self {
        WmError::Scheduler(e.to_string())
    }
}

/// 给前端的错误负载。
#[derive(Debug, Serialize)]
pub struct ErrPayload {
    pub code: String,
    pub message: String,
}

impl WmError {
    fn code(&self) -> &'static str {
        match self {
            WmError::Io(_) => "io",
            WmError::Json(_) => "json",
            WmError::Db(_) => "db",
            WmError::Http(_) => "http",
            WmError::Keyring(_) => "keyring",
            WmError::Zip(_) => "zip",
            WmError::Scheduler(_) => "scheduler",
            WmError::Crypto(_) => "crypto",
            WmError::NotFound(_) => "not_found",
            WmError::Credential(_) => "credential",
            WmError::Config(_) => "config",
            WmError::Upstream(_) => "upstream",
            WmError::Other(_) => "other",
        }
    }
}

/// WmError 直接作为 tauri command 错误返回（序列化为 {code,message}）。
impl Serialize for WmError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ErrPayload { code: self.code().to_string(), message: self.to_string() }.serialize(s)
    }
}

/// 兼容旧代码：转成 ErrPayload。
impl From<WmError> for ErrPayload {
    fn from(e: WmError) -> Self {
        ErrPayload { code: e.code().to_string(), message: e.to_string() }
    }
}

pub type WmResult<T> = Result<T, WmError>;
