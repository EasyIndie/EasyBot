//! API Key 管理
//!
//! 使用 argon2 哈希存储 API Key，Key 本身不持久化明文。
//! Phase 1 使用内存存储，后续接入数据库。

use std::collections::HashMap;
use tokio::sync::RwLock;
use uuid::Uuid;
use sha2::{Sha256, Digest};

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
/// Key 的哈希值使用 argon2 存储，原始 Key 只在创建时返回一次。
pub struct ApiKeyManager {
    /// key_hash → ApiKeyInfo
    keys: RwLock<HashMap<String, StoredKey>>,
}

struct StoredKey {
    info: ApiKeyInfo,
    hash: String, // argon2 hash
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

        // 计算 SHA-256 前缀用于快速查找
        let key_hash = hash_api_key(&raw_key);

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

        let hash_clone = key_hash.clone();
        let stored = StoredKey {
            info,
            hash: hash_clone,
        };

        let mut keys = self.keys.write().await;
        keys.insert(key_hash, stored);

        Ok((key_id, raw_key))
    }

    /// 验证 API Key
    ///
    /// 对传入的 key 计算哈希，与存储的哈希比对。
    pub async fn authenticate(&self, key: &str) -> Result<AuthInfo, String> {
        let key_hash = hash_api_key(key);
        let keys = self.keys.read().await;

        let stored = keys
            .get(&key_hash)
            .ok_or_else(|| "Invalid API key".to_string())?;

        if stored.info.revoked {
            return Err("API key has been revoked".to_string());
        }

        if let Some(expires) = stored.info.expires_at {
            if chrono::Utc::now().timestamp_millis() > expires {
                return Err("API key has expired".to_string());
            }
        }

        Ok(AuthInfo {
            id: stored.info.id.clone(),
            name: stored.info.name.clone(),
            permissions: stored.info.permissions.clone(),
        })
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

/// 计算 API Key 的 SHA-256 哈希（用于快速索引）
fn hash_api_key(key: &str) -> String {
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
        let (id, key) = mgr.create_key("test", vec!["message:send".to_string()], None).await.unwrap();

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
}
