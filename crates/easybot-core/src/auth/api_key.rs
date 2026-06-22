//! API Key 管理
//!
//! 使用 argon2id 哈希存储 API Key，SHA-256 仅用于快速索引查找。
//! Key 本身不持久化明文，仅在创建时返回一次。
//! Phase 4: 从 SHA-256 升级到 argon2id (PHC 格式)

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::password_hash::rand_core::OsRng;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tokio::sync::RwLock;
use uuid::Uuid;

/// API Key 信息
#[derive(Debug, Clone)]
pub struct ApiKeyInfo {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked: bool,
    pub permissions: Vec<String>,
}

/// 认证信息（验证成功后返回）
#[derive(Debug, Clone)]
pub struct AuthInfo {
    pub id: String,
    pub name: String,
    pub permissions: Vec<String>,
}

/// API Key 管理器
///
/// 管理 API Key 的生成、验证、吊销。
/// Key 的哈希值使用 argon2id 存储，原始 Key 只在创建时返回一次。
/// SHA-256 用作 HashMap 的快速索引键。
pub struct ApiKeyManager {
    /// key_hash(SHA-256) → StoredKey
    keys: RwLock<HashMap<String, StoredKey>>,
}

struct StoredKey {
    info: ApiKeyInfo,
    /// Argon2 PHC 格式哈希字符串 (e.g. $argon2id$v=19$m=65536,t=3,p=4$...)
    hash: String,
}

impl ApiKeyManager {
    /// 创建新的 API Key 管理器
    pub fn new() -> Self {
        Self {
            keys: RwLock::new(HashMap::new()),
        }
    }

    /// 创建新的 API Key
    ///
    /// 返回 (key_id, raw_key)。raw_key 仅在创建时返回，不再持久化存储。
    pub async fn create_key(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: Option<i64>,
    ) -> Result<(String, String), String> {
        let key_id = Uuid::new_v4().to_string();
        let raw_key = format!("eb_{}", Uuid::new_v4().to_string().replace("-", ""));
        let prefix = raw_key.chars().take(8).collect::<String>();

        // 生成 Argon2 哈希 (CPU 密集型，使用 spawn_blocking)
        let salt = SaltString::generate(&mut OsRng);
        let raw_key_clone = raw_key.clone();
        let phc_hash = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let argon2 = Argon2::default();
            argon2
                .hash_password(raw_key_clone.as_bytes(), &salt)
                .map(|h| h.to_string())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))??;

        // SHA-256 仅作为索引键
        let index_hash = sha256_index(&raw_key);

        let now = chrono::Utc::now().timestamp_millis();
        let info = ApiKeyInfo {
            id: key_id.clone(),
            name: name.to_string(),
            prefix,
            created_at: now,
            expires_at,
            last_used_at: None,
            revoked: false,
            permissions,
        };

        let stored = StoredKey {
            info,
            hash: phc_hash,
        };

        let mut keys = self.keys.write().await;
        keys.insert(index_hash, stored);

        Ok((key_id, raw_key))
    }

    /// 验证 API Key
    ///
    /// 使用 SHA-256 快速定位，再用 Argon2 验证密码。
    pub async fn authenticate(&self, key: &str) -> Result<AuthInfo, String> {
        let index_hash = sha256_index(key);
        let keys = self.keys.read().await;

        let stored = keys
            .get(&index_hash)
            .ok_or_else(|| "Invalid API key".to_string())?;

        if stored.info.revoked {
            return Err("API key has been revoked".to_string());
        }

        if let Some(expires) = stored.info.expires_at {
            if chrono::Utc::now().timestamp_millis() > expires {
                return Err("API key has expired".to_string());
            }
        }

        // 提前克隆所需数据，释放锁
        let auth_info = AuthInfo {
            id: stored.info.id.clone(),
            name: stored.info.name.clone(),
            permissions: stored.info.permissions.clone(),
        };
        let phc_hash = stored.hash.clone();
        let key_owned = key.to_string();
        drop(keys);

        // Argon2 验证 (CPU 密集型，使用 spawn_blocking)
        tokio::task::spawn_blocking(move || {
            let parsed_hash = PasswordHash::new(&phc_hash).map_err(|e| e.to_string())?;
            let argon2 = Argon2::default();
            argon2
                .verify_password(key_owned.as_bytes(), &parsed_hash)
                .map_err(|_| "Invalid API key".to_string())?;
            Ok(auth_info)
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }

    /// 吊销 API Key
    pub async fn revoke_key(&self, key_id: &str) -> bool {
        let mut keys = self.keys.write().await;
        for stored in keys.iter_mut() {
            if stored.1.info.id == key_id {
                stored.1.info.revoked = true;
                return true;
            }
        }
        false
    }

    /// 列出所有 API Key
    pub async fn list_keys(&self) -> Vec<ApiKeyInfo> {
        let keys = self.keys.read().await;
        keys.values().map(|s| s.info.clone()).collect()
    }
}

impl Default for ApiKeyManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 计算 API Key 的 SHA-256 哈希（仅用于快速索引，不用于密码验证）
fn sha256_index(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_authenticate() {
        let mgr = ApiKeyManager::new();
        let (id, key) = mgr
            .create_key("test", vec!["message:send".to_string()], None)
            .await
            .unwrap();

        assert!(!id.is_empty());
        assert!(key.starts_with("eb_"));

        let auth = mgr.authenticate(&key).await.unwrap();
        assert_eq!(auth.name, "test");
        assert_eq!(auth.permissions, vec!["message:send"]);
    }

    #[tokio::test]
    async fn test_revoke_key() {
        let mgr = ApiKeyManager::new();
        let (id, key) = mgr.create_key("test", vec![], None).await.unwrap();

        assert!(mgr.revoke_key(&id).await);
        assert!(mgr.authenticate(&key).await.is_err());
    }

    #[tokio::test]
    async fn test_invalid_key() {
        let mgr = ApiKeyManager::new();
        assert!(mgr.authenticate("invalid_key").await.is_err());
    }

    #[tokio::test]
    async fn test_expired_key() {
        let mgr = ApiKeyManager::new();
        let (_id, key) = mgr.create_key("expired", vec![], Some(1)).await.unwrap();
        // expires_at is 1ms after epoch — definitely expired
        assert!(mgr.authenticate(&key).await.is_err());
    }
}
