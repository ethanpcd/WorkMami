//! 账号注册表：元数据落 accounts.json，token 落本地加密 vault 文件。

use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{WmError, WmResult};
use crate::platform::workmami_paths;
use crate::wbauth::AuthTokens;

/// 单个积分资源包（分类）明细：注册赠送 / 平台奖励 / 购买 / 版本用量等，各自带到期时间。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuotaResource {
    pub package_code: String,
    /// 已映射成中文展示名（命中官方商品码表时用官方名）。
    pub name: String,
    pub used: f64,
    pub total: f64,
    pub remaining: f64,
    /// 到期时间（毫秒）；None = 长期有效 / 未知。
    #[serde(default)]
    pub expire_at: Option<i64>,
    #[serde(default)]
    pub expired: bool,
    /// 7 天内到期。
    #[serde(default)]
    pub expiring_soon: bool,
}

/// 积分快照：聚合总额 + 分类明细（含到期）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub used: f64,
    pub total: f64,
    pub remaining: f64,
    #[serde(default)]
    pub plan: String,
    /// 各分类中最近的到期时间（毫秒），供智能轮询优先消耗快过期账号；None = 无近期到期。
    #[serde(default)]
    pub soonest_expire_at: Option<i64>,
    /// 是否存在 7 天内到期且仍有余量的积分。
    #[serde(default)]
    pub expiring_soon: bool,
    /// 按资源包/分类的明细。
    #[serde(default)]
    pub resources: Vec<QuotaResource>,
    pub updated_at: DateTime<Utc>,
}

/// 落盘的账号元数据（不含 token）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub uid: String,
    #[serde(default)]
    pub uin: String,
    #[serde(default)]
    pub account_type: String,
    #[serde(default)]
    pub nickname: String,
    /// 来源：scan / oauth / manual
    pub source: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub quota: Option<QuotaSnapshot>,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub today_checked_in: bool,
    #[serde(default)]
    pub streak_days: i64,
    /// token 是否可本机解密获得（否则需重新授权）
    pub has_local_token: bool,
    pub created_at: DateTime<Utc>,
}

fn default_true() -> bool {
    true
}

/// 内存中的注册表。
pub struct AccountStore {
    inner: RwLock<Vec<Account>>,
}

impl AccountStore {
    pub fn load() -> WmResult<Self> {
        let path = workmami_paths().accounts_file.clone();
        let list: Vec<Account> = if path.exists() {
            let raw = std::fs::read(&path)?;
            serde_json::from_slice(&raw)?
        } else {
            Vec::new()
        };
        Ok(Self { inner: RwLock::new(list) })
    }

    fn persist(&self) -> WmResult<()> {
        let path = workmami_paths().accounts_file.clone();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let guard = self.inner.read().unwrap();
        let json = serde_json::to_vec_pretty(&*guard)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    pub fn all(&self) -> Vec<Account> {
        self.inner.read().unwrap().clone()
    }

    pub fn get(&self, uid: &str) -> Option<Account> {
        self.inner.read().unwrap().iter().find(|a| a.uid == uid).cloned()
    }

    pub fn enabled_accounts(&self) -> Vec<Account> {
        self.inner.read().unwrap().iter().filter(|a| a.enabled).cloned().collect()
    }

    fn upsert_locked(list: &mut Vec<Account>, acc: Account) {
        match list.iter_mut().find(|a| a.uid == acc.uid) {
            Some(existing) => *existing = acc,
            None => list.push(acc),
        }
    }

    pub fn upsert(&self, acc: Account) -> WmResult<()> {
        {
            let mut guard = self.inner.write().unwrap();
            Self::upsert_locked(&mut guard, acc);
        }
        self.persist()
    }

    pub fn remove(&self, uid: &str) -> WmResult<()> {
        {
            let mut guard = self.inner.write().unwrap();
            guard.retain(|a| a.uid != uid);
        }
        let _ = delete_token(uid);
        self.persist()
    }

    pub fn set_enabled(&self, uid: &str, enabled: bool) -> WmResult<()> {
        {
            let mut guard = self.inner.write().unwrap();
            if let Some(a) = guard.iter_mut().find(|a| a.uid == uid) {
                a.enabled = enabled;
            }
        }
        self.persist()
    }

    pub fn set_tags(&self, uid: &str, tags: Vec<String>) -> WmResult<()> {
        {
            let mut guard = self.inner.write().unwrap();
            if let Some(a) = guard.iter_mut().find(|a| a.uid == uid) {
                a.tags = tags;
            }
        }
        self.persist()
    }

    pub fn update_quota(&self, uid: &str, quota: QuotaSnapshot) -> WmResult<()> {
        {
            let mut guard = self.inner.write().unwrap();
            if let Some(a) = guard.iter_mut().find(|a| a.uid == uid) {
                a.quota = Some(quota);
            }
        }
        self.persist()
    }

    pub fn update_checkin(&self, uid: &str, checked: bool, streak: i64) -> WmResult<()> {
        {
            let mut guard = self.inner.write().unwrap();
            if let Some(a) = guard.iter_mut().find(|a| a.uid == uid) {
                a.today_checked_in = checked;
                a.streak_days = streak;
            }
        }
        self.persist()
    }

    pub fn touch_used(&self, uid: &str) -> WmResult<()> {
        {
            let mut guard = self.inner.write().unwrap();
            if let Some(a) = guard.iter_mut().find(|a| a.uid == uid) {
                a.last_used_at = Some(Utc::now());
            }
        }
        self.persist()
    }
}

/// 保存 token 到加密 vault。
pub fn store_token(uid: &str, tokens: &AuthTokens) -> WmResult<()> {
    crate::vault::store_token(uid, tokens)
}

/// 从加密 vault 读取 token。
pub fn load_token(uid: &str) -> Option<AuthTokens> {
    crate::vault::load_token(uid)
}

fn delete_token(uid: &str) -> WmResult<()> {
    crate::vault::delete_token(uid)
}

/// 通过 OAuth 授权新增/更新一个账号（token 为明文返回）。
pub fn upsert_from_oauth(
    store: &AccountStore,
    uid: &str,
    nickname: &str,
    account_type: &str,
    tokens: AuthTokens,
) -> WmResult<Account> {
    store_token(uid, &tokens)?;
    let mut acc = store.get(uid).unwrap_or_else(|| Account {
        uid: uid.to_string(),
        uin: String::new(),
        account_type: account_type.to_string(),
        nickname: nickname.to_string(),
        source: "oauth".into(),
        tags: Vec::new(),
        enabled: true,
        quota: None,
        last_used_at: None,
        today_checked_in: false,
        streak_days: 0,
        has_local_token: true,
        created_at: Utc::now(),
    });
    acc.nickname = nickname.to_string();
    acc.account_type = account_type.to_string();
    acc.has_local_token = true;
    if acc.source == "scan" {
        acc.source = "oauth".into();
    }
    store.upsert(acc.clone())?;
    Ok(acc)
}
