//! 本地加密 token 存储（替代 macOS/Windows 钥匙串）。
//! 使用 AES-256-GCM 加密，密钥由 HKDF-SHA256 从「应用主密钥 + 机器指纹」派生。
//! 存储路径：{app_data}/tokens.vault
//! 格式：[12B nonce][ciphertext + 16B tag]

use std::collections::HashMap;
use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

use crate::error::{WmError, WmResult};
use crate::platform::workmami_paths;
use crate::wbauth::AuthTokens;

/// 应用主密钥（硬编码在二进制中，配合机器指纹保证跨机不可解密）。
const MASTER_SECRET: &[u8; 32] = b"WorkMami2026!token_vault_master!";

fn vault_path() -> PathBuf {
    workmami_paths().root.join("tokens.vault")
}

/// 派生密钥：HKDF-SHA256(ikm=MASTER_SECRET, salt=machine_id, info="workmami-vault-v1")
fn derive_key() -> [u8; 32] {
    let machine_id = machine_fingerprint();
    let hk = Hkdf::<Sha256>::new(Some(&machine_id), MASTER_SECRET);
    let mut okm = [0u8; 32];
    hk.expand(b"workmami-vault-v1", &mut okm)
        .expect("HKDF expand should not fail for 32-byte output");
    okm
}

/// 机器指纹：username + hostname，取 SHA-256。
fn machine_fingerprint() -> [u8; 32] {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let input = format!("{user}@{host}");
    use sha2::Digest;
    let digest = sha2::Sha256::digest(input.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// 加载全部 token（解密失败返回空 map）。
fn load_all() -> HashMap<String, String> {
    let path = vault_path();
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => return HashMap::new(),
    };
    if data.len() < 12 {
        return HashMap::new();
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let key = derive_key();
    let cipher = Aes256Gcm::new_from_slice(&key)
        .expect("AES key length should be valid");
    let nonce = Nonce::from_slice(nonce_bytes);
    let plaintext = match cipher.decrypt(nonce, ciphertext) {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!("token vault 解密失败（可能换机器或密钥已变更），需重新授权");
            return HashMap::new();
        }
    };
    serde_json::from_slice(&plaintext).unwrap_or_default()
}

/// 保存全部 token（加密写入）。
fn save_all(map: &HashMap<String, String>) -> WmResult<()> {
    let plaintext = serde_json::to_vec(map)?;
    let key = derive_key();
    let cipher = Aes256Gcm::new_from_slice(&key)
        .expect("AES key length should be valid");
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_ref())
        .map_err(|_| WmError::Crypto("AES 加密失败".into()))?;
    let mut data = Vec::with_capacity(12 + ciphertext.len());
    data.extend_from_slice(&nonce_bytes);
    data.extend_from_slice(&ciphertext);
    let path = vault_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&path, &data)?;
    // 设置文件权限 0600（仅当前用户可读写）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 存储单个账号的 token。
pub fn store_token(uid: &str, tokens: &AuthTokens) -> WmResult<()> {
    let json = serde_json::to_string(tokens)?;
    let mut map = load_all();
    map.insert(uid.to_string(), json);
    save_all(&map)
}

/// 读取单个账号的 token。
pub fn load_token(uid: &str) -> Option<AuthTokens> {
    let map = load_all();
    let json = map.get(uid)?;
    serde_json::from_str(json).ok()
}

/// 删除单个账号的 token。
pub fn delete_token(uid: &str) -> WmResult<()> {
    let mut map = load_all();
    map.remove(uid);
    save_all(&map)
}

/// 从旧钥匙串迁移到 vault（一次性，迁移后旧条目删除）。
pub fn migrate_from_keyring(service: &str, uids: &[String]) {
    for uid in uids {
        // 如果 vault 中已有则跳过
        let map = load_all();
        if map.contains_key(uid.as_str()) {
            continue;
        }
        // 尝试从 keyring 读取
        let entry = match keyring::Entry::new(service, uid) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let Ok(json) = entry.get_password() {
            let mut map = load_all();
            map.insert(uid.clone(), json);
            let _ = save_all(&map);
            let _ = entry.delete_credential();
            tracing::info!("已迁移 uid={uid} 从钥匙串到 vault");
        }
    }
}
