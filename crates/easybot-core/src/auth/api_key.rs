//! API Key 管理
//!
//! 使用 argon2id 哈希存储 API Key，SHA-256 仅用于快速索引查找。
//! Key 本身不持久化明文，仅在创建时返回一次。
//! Phase 4: 从 SHA-256 升级到 argon2id (PHC 格式)
//! Phase 4: 接入 SQLite 持久化，重启不丢失

use crate::types::event::event_types;
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
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
    /// 允许接收的 WebSocket 事件类型列表。
    /// 空列表表示接收所有事件（向后兼容）。
    pub event_filters: Vec<String>,
}

/// 认证信息（验证成功后返回）
#[derive(Debug, Clone)]
pub struct AuthInfo {
    pub id: String,
    pub name: String,
    pub permissions: Vec<String>,
    /// 允许接收的 WebSocket 事件类型列表。
    /// 空列表表示接收所有事件（向后兼容）。
    pub event_filters: Vec<String>,
}

/// API Key 管理器
///
/// 管理 API Key 的生成、验证、吊销和删除。
/// Key 的哈希值使用 argon2id 存储，原始 Key 只在创建时返回一次。
///
/// **索引策略**: 运行时创建的 Key 按 SHA-256(raw_key) 索引实现 O(1) 快速查找。
/// 从 SQLite 加载的历史 Key 无法计算 SHA-256（raw_key 已丢失），
/// 通过 Argon2 遍历验证（O(n)，n 通常 < 100，完全可接受）。
pub struct ApiKeyManager {
    /// SHA-256(raw_key) → StoredKey（运行时创建的 Key，快速索引）
    keys: RwLock<HashMap<String, StoredKey>>,
    /// 从 SQLite 加载的历史 Key（无 SHA-256 索引，验证时遍历）
    loaded: RwLock<Vec<StoredKey>>,
    /// SQLite 连接池（None = 纯内存模式）
    pool: Option<SqlitePool>,
}

#[derive(Clone)]
struct StoredKey {
    info: ApiKeyInfo,
    /// Argon2 PHC 格式哈希字符串 (e.g. $argon2id$v=19$m=65536,t=3,p=4$...)
    hash: String,
}

impl ApiKeyManager {
    /// 创建新的 API Key 管理器
    ///
    /// 传入 `Some(pool)` 启用 SQLite 持久化（生产模式）。
    /// 传入 `None` 使用纯内存存储（测试模式）。
    pub fn new(pool: Option<SqlitePool>) -> Self {
        Self {
            keys: RwLock::new(HashMap::new()),
            loaded: RwLock::new(Vec::new()),
            pool,
        }
    }

    /// 从 SQLite 加载已有 Key 到内存（启动时调用）
    pub async fn load_from_db(&self) {
        let pool = match &self.pool {
            Some(p) => p,
            None => return,
        };
        let rows = sqlx::query(
                "SELECT id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, event_filters, hash FROM api_keys"
            )
            .fetch_all(pool)
            .await;

        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Failed to load API keys from DB: {}", e);
                return;
            }
        };

        use sqlx::Row;
        let mut loaded = self.loaded.write().await;
        for row in &rows {
            let permissions_str: String = row.get("permissions");
            let event_filters_str: String = row.get("event_filters");
            let revoked_int: i64 = row.get("revoked");
            let permissions: Vec<String> =
                serde_json::from_str(&permissions_str).unwrap_or_default();
            let event_filters: Vec<String> =
                serde_json::from_str(&event_filters_str).unwrap_or_default();
            loaded.push(StoredKey {
                info: ApiKeyInfo {
                    id: row.get("id"),
                    name: row.get("name"),
                    prefix: row.get("prefix"),
                    created_at: row.get("created_at"),
                    expires_at: row.get("expires_at"),
                    last_used_at: row.get("last_used_at"),
                    revoked: revoked_int != 0,
                    permissions,
                    event_filters,
                },
                hash: row.get("hash"),
            });
        }
        tracing::info!("Loaded {} API keys from database", loaded.len());
    }

    /// 创建新的 API Key
    ///
    /// 返回 (key_id, raw_key)。raw_key 仅在创建时返回，不再持久化存储。
    ///
    /// `event_filters` 指定该 Key 允许接收的 WebSocket 事件类型。
    /// 传入空数组表示接收全部事件。
    /// name == "dev" 的 Key 不持久化（每次启动重建）。
    pub async fn create_key(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: Option<i64>,
        event_filters: Vec<String>,
    ) -> Result<(String, String), String> {
        // 校验 event_filters
        let known_events = event_types::all();
        for ef in &event_filters {
            if !known_events.contains(&ef.as_str()) {
                return Err(format!("Unknown event type: '{}'", ef));
            }
        }
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

        let now = chrono::Utc::now().timestamp_millis();
        let info = ApiKeyInfo {
            id: key_id.clone(),
            name: name.to_string(),
            prefix: prefix.clone(),
            created_at: now,
            expires_at,
            last_used_at: None,
            revoked: false,
            permissions: permissions.clone(),
            event_filters: event_filters.clone(),
        };

        let stored = StoredKey {
            info,
            hash: phc_hash.clone(),
        };

        // 内存索引（SHA-256 快速查找）
        let index_hash = sha256_index(&raw_key);
        self.keys.write().await.insert(index_hash, stored);

        // 持久化到 SQLite（dev key 不持久化）
        if name != "dev"
            && let Some(pool) = &self.pool
        {
            let perms_json = serde_json::to_string(&permissions).unwrap_or_default();
            let filters_json = serde_json::to_string(&event_filters).unwrap_or_default();
            let _ = sqlx::query(
                    "INSERT INTO api_keys (id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, event_filters, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
                )
                .bind(&key_id).bind(name).bind(&prefix).bind(now).bind(expires_at).bind(None::<i64>).bind(0).bind(&perms_json).bind(&filters_json).bind(&phc_hash)
                .execute(pool)
                .await;
        }

        Ok((key_id, raw_key))
    }

    /// 验证 API Key
    ///
    /// 优先使用 SHA-256 快速定位（运行时创建的 Key），
    /// 未命中则遍历 DB 加载的历史 Key 并用 Argon2 验证。
    pub async fn authenticate(&self, key: &str) -> Result<AuthInfo, String> {
        let index_hash = sha256_index(key);

        // 快速路径：SHA-256 索引查找
        {
            let keys = self.keys.read().await;
            if let Some(stored) = keys.get(&index_hash) {
                return Self::verify_and_build_auth(stored, key).await;
            }
        }

        // 慢速路径：遍历 DB 加载的历史 Key，逐个 Argon2 验证
        {
            let loaded = self.loaded.read().await;
            for stored in loaded.iter() {
                if stored.info.revoked {
                    continue;
                }
                if let Some(expires) = stored.info.expires_at
                    && chrono::Utc::now().timestamp_millis() > expires
                {
                    continue;
                }
                // 尝试 Argon2 验证
                let phc_hash = stored.hash.clone();
                let key_owned = key.to_string();
                let verified = tokio::task::spawn_blocking(move || {
                    let parsed_hash = PasswordHash::new(&phc_hash).map_err(|e| e.to_string())?;
                    let argon2 = Argon2::default();
                    argon2
                        .verify_password(key_owned.as_bytes(), &parsed_hash)
                        .map_err(|_| "Invalid API key".to_string())
                })
                .await
                .map_err(|e| format!("Task join error: {}", e))?;

                if verified.is_ok() {
                    // 二次确认未吊销（防止 Argon2 验证期间被并发吊销）
                    if stored.info.revoked {
                        return Err("API key has been revoked".to_string());
                    }
                    let auth_info = AuthInfo {
                        id: stored.info.id.clone(),
                        name: stored.info.name.clone(),
                        permissions: stored.info.permissions.clone(),
                        event_filters: stored.info.event_filters.clone(),
                    };
                    // 加入 keys HashMap 以便下次快速查找
                    self.keys.write().await.insert(index_hash, stored.clone());
                    return Ok(auth_info);
                }
            }
        }

        Err("Invalid API key".to_string())
    }

    /// Argon2 验证并构建 AuthInfo
    async fn verify_and_build_auth(stored: &StoredKey, key: &str) -> Result<AuthInfo, String> {
        if stored.info.revoked {
            return Err("API key has been revoked".to_string());
        }

        if let Some(expires) = stored.info.expires_at
            && chrono::Utc::now().timestamp_millis() > expires
        {
            return Err("API key has expired".to_string());
        }

        let auth_info = AuthInfo {
            id: stored.info.id.clone(),
            name: stored.info.name.clone(),
            permissions: stored.info.permissions.clone(),
            event_filters: stored.info.event_filters.clone(),
        };
        let phc_hash = stored.hash.clone();
        let key_owned = key.to_string();

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
        let mut found = false;
        // 更新快速索引（self.keys）
        {
            let mut keys = self.keys.write().await;
            for stored in keys.values_mut() {
                if stored.info.id == key_id {
                    stored.info.revoked = true;
                    found = true;
                    break;
                }
            }
        }
        // 同时更新 DB 加载列表（self.loaded），确保两处状态一致
        {
            let mut loaded = self.loaded.write().await;
            for stored in loaded.iter_mut() {
                if stored.info.id == key_id {
                    stored.info.revoked = true;
                    found = true;
                    break;
                }
            }
        }

        if !found {
            return false;
        }

        // 持久化到 SQLite
        if let Some(pool) = &self.pool {
            let _ = sqlx::query("UPDATE api_keys SET revoked = 1 WHERE id = ?1")
                .bind(key_id)
                .execute(pool)
                .await;
        }

        true
    }

    /// 永久删除已吊销的 API Key
    ///
    /// 仅允许删除已吊销的 Key，防止误删活跃 Key。
    pub async fn delete_key(&self, key_id: &str) -> bool {
        // 检查是否已吊销
        let mut revoked = false;
        {
            let keys = self.keys.read().await;
            if let Some(stored) = keys.values().find(|s| s.info.id == key_id) {
                revoked = stored.info.revoked;
            }
        }
        if !revoked {
            let loaded = self.loaded.read().await;
            if let Some(stored) = loaded.iter().find(|s| s.info.id == key_id) {
                revoked = stored.info.revoked;
            }
        }

        if !revoked {
            return false; // 不允许删除未吊销的 Key
        }

        // 从内存中移除
        {
            let mut keys = self.keys.write().await;
            keys.retain(|_, s| s.info.id != key_id);
        }
        {
            let mut loaded = self.loaded.write().await;
            loaded.retain(|s| s.info.id != key_id);
        }

        // 从 SQLite 中删除
        if let Some(pool) = &self.pool {
            let _ = sqlx::query("DELETE FROM api_keys WHERE id = ?1")
                .bind(key_id)
                .execute(pool)
                .await;
        }

        true
    }

    /// 列出所有 API Key（合并内存和 DB 加载的）
    pub async fn list_keys(&self) -> Vec<ApiKeyInfo> {
        let keys = self.keys.read().await;
        let loaded = self.loaded.read().await;

        let mut all: Vec<ApiKeyInfo> = keys.values().map(|s| s.info.clone()).collect();
        // 追加 DB 加载的 Key（去重：以 id 为准）
        let seen: std::collections::HashSet<String> = all.iter().map(|k| k.id.clone()).collect();
        for s in loaded.iter() {
            if !seen.contains(&s.info.id) {
                all.push(s.info.clone());
            }
        }
        all
    }
}

impl Default for ApiKeyManager {
    fn default() -> Self {
        Self::new(None)
    }
}

/// 计算 API Key 的 SHA-256 哈希（仅用于快速索引，不用于密码验证）
fn sha256_index(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_authenticate() {
        let mgr = ApiKeyManager::new(None);
        let (id, key) = mgr
            .create_key("test", vec!["message:send".to_string()], None, vec![])
            .await
            .unwrap();

        assert!(!id.is_empty());
        assert!(key.starts_with("eb_"));

        let auth = mgr.authenticate(&key).await.unwrap();
        assert_eq!(auth.name, "test");
        assert_eq!(auth.permissions, vec!["message:send"]);
        assert!(auth.event_filters.is_empty());
    }

    #[tokio::test]
    async fn test_revoke_key() {
        let mgr = ApiKeyManager::new(None);
        let (id, key) = mgr.create_key("test", vec![], None, vec![]).await.unwrap();

        assert!(mgr.revoke_key(&id).await);
        assert!(mgr.authenticate(&key).await.is_err());
    }

    #[tokio::test]
    async fn test_delete_revoked_key() {
        let mgr = ApiKeyManager::new(None);
        let (id, _key) = mgr.create_key("test", vec![], None, vec![]).await.unwrap();

        // 未吊销不能删除
        assert!(!mgr.delete_key(&id).await);

        // 吊销后可删除
        assert!(mgr.revoke_key(&id).await);
        assert!(mgr.delete_key(&id).await);

        // 列表里不再有
        assert!(mgr.list_keys().await.is_empty());
    }

    #[tokio::test]
    async fn test_invalid_key() {
        let mgr = ApiKeyManager::new(None);
        assert!(mgr.authenticate("invalid_key").await.is_err());
    }

    #[tokio::test]
    async fn test_expired_key() {
        let mgr = ApiKeyManager::new(None);
        let (_id, key) = mgr
            .create_key("expired", vec![], Some(1), vec![])
            .await
            .unwrap();
        // expires_at is 1ms after epoch — definitely expired
        assert!(mgr.authenticate(&key).await.is_err());
    }

    #[tokio::test]
    async fn test_create_key_with_event_filters() {
        let mgr = ApiKeyManager::new(None);
        let filters = vec!["message.inbound".to_string(), "message.sent".to_string()];
        let (_id, key) = mgr
            .create_key(
                "filtered",
                vec!["messagesread".to_string()],
                None,
                filters.clone(),
            )
            .await
            .unwrap();

        let auth = mgr.authenticate(&key).await.unwrap();
        assert_eq!(auth.event_filters, filters);
    }

    #[tokio::test]
    async fn test_create_key_with_invalid_event_filters() {
        let mgr = ApiKeyManager::new(None);
        let result = mgr
            .create_key("bad", vec![], None, vec!["nonexistent.event".to_string()])
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown event type"));
    }

    #[tokio::test]
    async fn test_create_key_empty_event_filters_all_events() {
        let mgr = ApiKeyManager::new(None);
        let (_id, key) = mgr
            .create_key("all", vec!["*".to_string()], None, vec![])
            .await
            .unwrap();

        let auth = mgr.authenticate(&key).await.unwrap();
        // Empty event_filters = receive all events (backward compatible)
        assert!(auth.event_filters.is_empty());
    }
}
