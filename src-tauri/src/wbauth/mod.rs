//! WorkBuddy 登录文件解析与 `$wbEncrypted` 信封加解密。
//!
//! 落盘结构（workbuddy-desktop.info）：
//! - account / accounts / allAccounts：账号元数据（uid、uin、type…，昵称/手机号加密）
//! - auth：OAuth token 段，accessToken / refreshToken 为信封加密
//!
//! 加密链路：
//!   field 信封 --(master key 32B)--> 明文
//!   keyblob.wrapped --(protector key 32B)--> master key
//! protector key 由官方 daemon/钥匙串下发，本机不一定可解析；解析失败时上层
//! 降级为“一键重新网页授权”，不影响反代使用（新 token 为明文返回）。

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use zeroize::Zeroize;

use crate::error::{WmError, WmResult};
use crate::platform::workbuddy_paths;

use base64::engine::general_purpose::STANDARD as B64;

/// 信封结构：{"suite":1,"keyId":"...","nonce":"b64","authTag":"b64","ciphertext":"b64"}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub suite: u8,
    #[serde(rename = "keyId")]
    pub key_id: String,
    pub nonce: String,
    #[serde(rename = "authTag")]
    pub auth_tag: String,
    pub ciphertext: String,
    /// asym-v1 方案会带 scheme 字段
    #[serde(default, rename = "scheme")]
    pub scheme: Option<String>,
}

/// 被加密字段包装：{"$wbEncrypted":1,"envelope":"<base64 json>"} 或直接内联信封字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncField {
    #[serde(rename = "$wbEncrypted")]
    pub wb_encrypted: u8,
    #[serde(default)]
    pub scheme: Option<String>,
    #[serde(default)]
    pub envelope: Option<String>,
    // 内联信封（部分场景）
    #[serde(default)]
    pub suite: Option<u8>,
    #[serde(default, rename = "nonce")]
    pub nonce: Option<String>,
    #[serde(default, rename = "authTag")]
    pub auth_tag: Option<String>,
    #[serde(default, rename = "ciphertext")]
    pub ciphertext: Option<String>,
}

impl EncField {
    /// 归一化出信封（envelope 为 base64(JSON) 或内联字段）。
    pub fn to_envelope(&self) -> WmResult<Envelope> {
        if let Some(env_b64) = &self.envelope {
            let json = B64
                .decode(env_b64.trim_end_matches('\n'))
                .map_err(|e| WmError::Crypto(format!("envelope base64: {e}")))?;
            let env: Envelope = serde_json::from_slice(&json)
                .map_err(|e| WmError::Crypto(format!("envelope json: {e}")))?;
            return Ok(env);
        }
        match (&self.nonce, &self.auth_tag, &self.ciphertext, self.suite) {
            (Some(n), Some(t), Some(c), Some(s)) => Ok(Envelope {
                suite: s,
                key_id: String::new(),
                nonce: n.clone(),
                auth_tag: t.clone(),
                ciphertext: c.clone(),
                scheme: self.scheme.clone(),
            }),
            _ => Err(WmError::Crypto("无法解析加密字段".into())),
        }
    }
}

/// 用 32 字节密钥解密信封，返回明文字节。
pub fn decrypt_envelope(key: &[u8; 32], env: &Envelope) -> WmResult<Vec<u8>> {
    if env.suite != 1 {
        return Err(WmError::Crypto(format!("不支持的加密 suite={}", env.suite)));
    }
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| WmError::Crypto(format!("aes key: {e}")))?;
    let nonce_b = B64
        .decode(&env.nonce)
        .map_err(|e| WmError::Crypto(format!("nonce b64: {e}")))?;
    let tag = B64
        .decode(&env.auth_tag)
        .map_err(|e| WmError::Crypto(format!("tag b64: {e}")))?;
    let ct = B64
        .decode(&env.ciphertext)
        .map_err(|e| WmError::Crypto(format!("ct b64: {e}")))?;
    if nonce_b.len() != 12 {
        return Err(WmError::Crypto(format!("nonce 长度 {} != 12", nonce_b.len())));
    }
    // aes-gcm 期望 ciphertext || tag
    let mut payload = ct;
    payload.extend_from_slice(&tag);
    let nonce = Nonce::from_slice(&nonce_b);
    cipher
        .decrypt(nonce, payload.as_ref())
        .map_err(|e| WmError::Crypto(format!("gcm decrypt 失败(密钥不匹配?): {e}")))
}

/// 用 32 字节密钥加密明文，产出信封（写回官方布局时使用）。
pub fn encrypt_envelope(key: &[u8; 32], plaintext: &[u8]) -> WmResult<Envelope> {
    use rand::RngCore;
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| WmError::Crypto(format!("aes key: {e}")))?;
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let payload = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| WmError::Crypto(format!("gcm encrypt: {e}")))?;
    // 末尾 16 字节为 tag
    let tag = payload[payload.len() - 16..].to_vec();
    let ct = payload[..payload.len() - 16].to_vec();
    Ok(Envelope {
        suite: 1,
        key_id: derive_key_id(key),
        nonce: B64.encode(nonce_bytes),
        auth_tag: B64.encode(tag),
        ciphertext: B64.encode(ct),
        scheme: None,
    })
}

/// keyId = sha256(key) 前 8 字节的 hex（与官方 deriveAtRestKeyId 约定一致，若不符仅影响自检）。
pub fn derive_key_id(key: &[u8; 32]) -> String {
    let mut h = Sha256::new();
    h.update(key);
    let d = h.finalize();
    hex::encode(&d[..8])
}

/// keyblob 文件结构。
#[derive(Debug, Clone, Deserialize)]
pub struct KeyBlob {
    pub version: u8,
    #[serde(rename = "keyId")]
    pub key_id: String,
    pub slots: Vec<KeySlot>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeySlot {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "protectorKeyId")]
    pub protector_key_id: Option<String>,
    pub wrapped: String,
}

/// 用 protector key 解出 keyblob 中的 master key。
pub fn unwrap_master_key(protector: &[u8; 32], blob: &KeyBlob) -> WmResult<[u8; 32]> {
    for slot in &blob.slots {
        if slot.kind != "static-v1" {
            continue;
        }
        let json = match B64.decode(&slot.wrapped) {
            Ok(j) => j,
            Err(_) => continue,
        };
        let env: Envelope = match serde_json::from_slice(&json) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let Some(pk) = &slot.protector_key_id {
            if pk != &env.key_id {
                continue;
            }
        }
        match decrypt_envelope(protector, &env) {
            Ok(pt) => {
                if pt.len() == 32 {
                    let mut k = [0u8; 32];
                    k.copy_from_slice(&pt);
                    return Ok(k);
                }
            }
            Err(_) => continue,
        }
    }
    Err(WmError::Credential("protector key 无法解开 keyblob".into()))
}

/// master key 解析结果。
#[derive(Clone)]
pub struct MasterKey {
    pub key: [u8; 32],
    pub source: String,
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

/// 尽力解析 master key：
/// 1) ~/.workbuddy-key-fallback/.master.key（明文降级文件）
/// 2) connector-keys/*.key 作为 protector 逐个尝试解 keyblob
/// 3) keyring 中候选 service 名
/// 全部失败返回 None，上层走“重新授权”。
pub fn resolve_master_key() -> Option<MasterKey> {
    let paths = workbuddy_paths();

    // 1) 明文 master key 降级文件
    let mk_file = paths.key_fallback.join(".master.key");
    if let Some(k) = read_key_file(&mk_file) {
        return Some(MasterKey { key: k, source: "fallback-master-key".into() });
    }

    // 载入 keyblob
    let blob = load_keyblob(&paths.keyblob)?;

    // 2) connector-keys 作为 protector 候选
    let ck_dir = paths.key_fallback.join("connector-keys");
    if let Ok(rd) = std::fs::read_dir(&ck_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("key") {
                continue;
            }
            if let Some(prot) = read_key_file(&p) {
                if let Ok(master) = unwrap_master_key(&prot, &blob) {
                    return Some(MasterKey { key: master, source: "connector-protector".into() });
                }
            }
        }
    }

    // 3) keyring 候选 service
    for svc in ["WorkBuddy", "CodeBuddy", "com.workbuddy.workbuddy", "WorkBuddy Safe Storage"] {
        if let Some(prot) = keyring_get(svc, "at-rest-protector") {
            if let Ok(master) = unwrap_master_key(&prot, &blob) {
                return Some(MasterKey { key: master, source: "keychain-protector".into() });
            }
        }
    }

    None
}

fn read_key_file(path: &Path) -> Option<[u8; 32]> {
    let raw = std::fs::read(path).ok()?;
    let trimmed: Vec<u8> = raw.iter().cloned().filter(|b| !b.is_ascii_whitespace()).collect();
    // 可能是 hex 或 base64 或裸 32 字节
    if trimmed.len() == 64 {
        if let Ok(hexed) = hex::decode(&trimmed) {
            if hexed.len() == 32 {
                let mut k = [0u8; 32];
                k.copy_from_slice(&hexed);
                return Some(k);
            }
        }
    }
    if let Ok(b) = B64.decode(&trimmed) {
        if b.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&b);
            return Some(k);
        }
    }
    if raw.len() == 32 {
        let mut k = [0u8; 32];
        k.copy_from_slice(&raw);
        return Some(k);
    }
    None
}

fn load_keyblob(path: &Path) -> Option<KeyBlob> {
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn keyring_get(service: &str, account: &str) -> Option<[u8; 32]> {
    let entry = keyring::Entry::new(service, account).ok()?;
    let s = entry.get_password().ok()?;
    let bytes = s.into_bytes();
    if bytes.len() == 32 {
        let mut k = [0u8; 32];
        k.copy_from_slice(&bytes);
        Some(k)
    } else {
        read_key_bytes(&bytes)
    }
}

fn read_key_bytes(bytes: &[u8]) -> Option<[u8; 32]> {
    let trimmed: Vec<u8> = bytes.iter().cloned().filter(|b| !b.is_ascii_whitespace()).collect();
    if trimmed.len() == 64 {
        let hexed = hex::decode(&trimmed).ok()?;
        if hexed.len() == 32 {
            let mut k = [0u8; 32];
            k.copy_from_slice(&hexed);
            return Some(k);
        }
    }
    let b = B64.decode(&trimmed).ok()?;
    if b.len() == 32 {
        let mut k = [0u8; 32];
        k.copy_from_slice(&b);
        return Some(k);
    }
    None
}

/// desktop.info 顶层结构（只声明我们关心的字段，其余保留）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopInfo {
    pub account: serde_json::Value,
    pub auth: serde_json::Value,
    #[serde(default)]
    pub accounts: Option<serde_json::Value>,
    #[serde(default)]
    pub allAccounts: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// 从 desktop.info 解出的明文 token 段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub refresh_expires_at: Option<i64>,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub domain: String,
    /// 账号 uid，用于 X-User-Id 请求头（网页授权登录时写入）。
    #[serde(default)]
    pub uid: String,
}

/// 解析 desktop.info 并尝试解密 auth 段。
pub fn load_and_decrypt() -> WmResult<(DesktopInfo, Option<AuthTokens>)> {
    let paths = workbuddy_paths();
    let file = paths
        .existing_info_file()
        .ok_or_else(|| WmError::NotFound("未找到 workbuddy-desktop.info".into()))?;
    let raw = std::fs::read(&file)?;
    let info: DesktopInfo = serde_json::from_slice(&raw)
        .map_err(|e| WmError::Crypto(format!("desktop.info 解析失败: {e}")))?;

    let tokens = match resolve_master_key() {
        Some(mk) => decrypt_auth_section(&info.auth, &mk.key).ok(),
        None => None,
    };
    Ok((info, tokens))
}

fn decrypt_auth_section(auth: &serde_json::Value, key: &[u8; 32]) -> WmResult<AuthTokens> {
    let obj = auth
        .as_object()
        .ok_or_else(|| WmError::Crypto("auth 段缺失".into()))?;

    let access_token = decrypt_field(obj.get("accessToken")).unwrap_or_default();
    let refresh_token = decrypt_field(obj.get("refreshToken")).unwrap_or_default();
    if access_token.is_empty() {
        return Err(WmError::Crypto("accessToken 解密失败".into()));
    }
    let token_type = str_field(obj, "tokenType").unwrap_or_else(|| "Bearer".into());
    Ok(AuthTokens {
        access_token,
        refresh_token,
        token_type,
        expires_at: num_field(obj, "expiresAt"),
        refresh_expires_at: num_field(obj, "refreshExpiresAt"),
        scope: str_field(obj, "scope").unwrap_or_default(),
        domain: str_field(obj, "domain").unwrap_or_default(),
        uid: str_field(obj, "uid").unwrap_or_default(),
    })
}

fn decrypt_field(v: Option<&serde_json::Value>) -> Option<String> {
    let v = v?;
    // 明文
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    // 加密字段
    let enc: EncField = serde_json::from_value(v.clone()).ok()?;
    let env = enc.to_envelope().ok()?;
    // 需要一个 key —— 由调用方注入；这里用 thread-local 缓存避免层层传参
    let key = CURRENT_MASTER_KEY.with(|k| *k.borrow())?;
    let pt = decrypt_envelope(&key, &env).ok()?;
    String::from_utf8(pt).ok()
}

thread_local! {
    static CURRENT_MASTER_KEY: std::cell::RefCell<Option<[u8; 32]>> = const { std::cell::RefCell::new(None) };
}

/// 在给定 master key 作用域内执行闭包（供 decrypt_field 取用）。
pub fn with_master_key<R>(key: &[u8; 32], f: impl FnOnce() -> R) -> R {
    CURRENT_MASTER_KEY.with(|c| *c.borrow_mut() = Some(*key));
    let r = f();
    CURRENT_MASTER_KEY.with(|c| *c.borrow_mut() = None);
    r
}

fn str_field(obj: &serde_json::Map<String, serde_json::Value>, k: &str) -> Option<String> {
    obj.get(k).and_then(|v| v.as_str()).map(|s| s.to_string())
}
fn num_field(obj: &serde_json::Map<String, serde_json::Value>, k: &str) -> Option<i64> {
    obj.get(k).and_then(|v| v.as_i64())
}

/// 便捷：解析账号元数据数组（accounts / allAccounts），返回 (uid, uin, type)。
pub fn list_account_meta(info: &DesktopInfo) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    collect_accounts(info.accounts.as_ref(), &mut out);
    if out.is_empty() {
        collect_accounts(info.allAccounts.as_ref(), &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn collect_accounts(arr: Option<&serde_json::Value>, out: &mut Vec<(String, String, String)>) {
    if let Some(serde_json::Value::Array(list)) = arr {
        for a in list {
            let uid = a.get("uid").and_then(|v| v.as_str()).unwrap_or_default();
            let uin = a
                .get("uin")
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            let ty = a.get("type").and_then(|v| v.as_str()).unwrap_or("personal");
            if !uid.is_empty() {
                out.push((uid.to_string(), uin, ty.to_string()));
            }
        }
    }
}
