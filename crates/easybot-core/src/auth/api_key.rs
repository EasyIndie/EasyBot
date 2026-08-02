//! API Key 管理
//!
//! 使用 argon2id 哈希存储 API Key，SHA-256 仅用于快速索引查找。
//! Key 本身不持久化明文，仅在创建时返回一次。
//! Phase 4: 从 SHA-256 升级到 argon2id (PHC 格式)
//! Phase 4: 接入 SQLite 持久化，重启不丢失

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use futures::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use uuid::Uuid;

/// API Key 信息
#[derive(Debug, Clone)]
pub struct ApiKeyInfo {
    pub id: String,
    /// Stable caller identity shared by all rotations of this credential.
    pub subject_id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked: bool,
    pub permissions: Vec<String>,
    /// Per-subject request quota. Rotated credentials inherit the same window.
    pub requests_per_minute: Option<u32>,
}

/// 认证信息（验证成功后返回）
#[derive(Debug, Clone)]
pub struct AuthInfo {
    pub id: String,
    pub subject_id: String,
    pub name: String,
    pub permissions: Vec<String>,
    pub requests_per_minute: Option<u32>,
}

/// A server-owned authorization grant for one stable caller subject.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct TargetGrant {
    pub id: String,
    pub subject_id: String,
    pub platform: String,
    pub chat_id: String,
    pub actions: Vec<String>,
    pub created_at: i64,
    pub created_by: String,
}

pub mod target_actions {
    pub const INBOUND_READ: &str = "inbound:read";
    pub const MESSAGES_READ: &str = "messages:read";
    pub const MESSAGES_SEND: &str = "messages:send";
    pub const SESSIONS_READ: &str = "sessions:read";
    pub const SESSIONS_MANAGE: &str = "sessions:manage";

    pub const ALL: &[&str] = &[
        INBOUND_READ,
        MESSAGES_READ,
        MESSAGES_SEND,
        SESSIONS_READ,
        SESSIONS_MANAGE,
    ];
}

/// Immutable management audit event. Hashes form an ordered chain so deletion
/// or modification is detectable during verification.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: i64,
    pub actor_id: String,
    pub action: String,
    pub resource: String,
    pub metadata: serde_json::Value,
    pub previous_hash: String,
    pub event_hash: String,
}

/// Durable hourly API usage suitable for invoice reconciliation.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct UsageRecord {
    pub key_id: String,
    pub subject_id: String,
    /// UTC Unix timestamp in milliseconds, truncated to the hour.
    pub bucket_start: i64,
    /// HTTP status class (2, 3, 4 or 5).
    pub status_class: i32,
    pub request_count: i64,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct BillingEvent {
    pub provider: String,
    pub event_id: String,
    pub event_type: String,
    pub object_id: String,
    pub customer_ref: String,
    pub amount_minor: i64,
    pub currency: String,
    pub occurred_at: i64,
    pub received_at: i64,
    pub event_hash: String,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ApiKeyRotationTransition {
    pub source_id: String,
    pub replacement_id: String,
    pub state: String,
    pub created_at: i64,
    pub source_revoked: bool,
    pub replacement_revoked: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BillingEventFilter<'a> {
    pub customer_ref: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub event_type: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillingEventWrite {
    Created,
    Duplicate,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotencyReservation {
    Acquired,
    Replay { status: u16, response_json: String },
    InProgress,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaConsumption {
    pub allowed: bool,
    pub remaining: u32,
    pub retry_after_secs: u64,
}

/// API Key 管理器
///
/// 管理 API Key 的生成、验证、吊销和删除。
/// Key 的哈希值使用 argon2id 存储，原始 Key 只在创建时返回一次。
///
/// **索引策略**: 运行时创建的 Key 按 SHA-256(raw_key) 索引实现 O(1) 快速查找。
/// 从 SQLite 加载的历史 Key 无法计算 SHA-256（raw_key 已丢失），
/// 先用公开随机前缀定位极小候选集，再用 Argon2 完成最终验证。
pub struct ApiKeyManager {
    /// SHA-256(raw_key) → StoredKey（运行时创建的 Key，快速索引）
    keys: RwLock<HashMap<String, StoredKey>>,
    /// 从 SQLite 加载的历史 Key（无 SHA-256 索引，验证时遍历）
    loaded: RwLock<Vec<StoredKey>>,
    /// SQLite 连接池（None = 纯内存模式）
    pool: Option<SqlitePool>,
    audit_events: RwLock<Vec<AuditEvent>>,
    /// Current durable chain head. Production does not retain the full ledger in memory.
    audit_head: RwLock<Option<String>>,
    audit_lock: Mutex<()>,
    /// Serializes successful persisted-key promotion with revoke/delete so a
    /// stale Argon2 result can never resurrect a credential.
    key_lifecycle_lock: Mutex<()>,
    metering_healthy: AtomicBool,
    quota_healthy: AtomicBool,
    quota_windows: Mutex<HashMap<String, VecDeque<Instant>>>,
}

#[derive(Clone)]
struct StoredKey {
    info: ApiKeyInfo,
    /// Argon2 PHC 格式哈希字符串 (e.g. $argon2id$v=19$m=65536,t=3,p=4$...)
    hash: String,
    /// Whether this credential belongs to the manageable API Key inventory.
    /// Ephemeral admin sessions authenticate normally but never appear there.
    manageable: bool,
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
            audit_events: RwLock::new(Vec::new()),
            audit_head: RwLock::new(None),
            audit_lock: Mutex::new(()),
            key_lifecycle_lock: Mutex::new(()),
            metering_healthy: AtomicBool::new(true),
            quota_healthy: AtomicBool::new(true),
            quota_windows: Mutex::new(HashMap::new()),
        }
    }

    /// 从 SQLite 加载已有 Key 到内存（启动时调用）
    pub async fn load_from_db(&self) {
        let pool = match &self.pool {
            Some(p) => p,
            None => return,
        };
        let now = chrono::Utc::now().timestamp_millis();
        let rows = sqlx::query(
                "SELECT id, subject_id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, requests_per_minute, hash FROM api_keys WHERE revoked = 0 AND (expires_at IS NULL OR expires_at > ?1)"
            )
            .bind(now)
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
        loaded.clear();
        for row in &rows {
            let permissions_str: String = row.get("permissions");
            let revoked_int: i64 = row.get("revoked");
            let permissions: Vec<String> =
                serde_json::from_str(&permissions_str).unwrap_or_default();
            loaded.push(StoredKey {
                info: ApiKeyInfo {
                    id: row.get("id"),
                    subject_id: row.get("subject_id"),
                    name: row.get("name"),
                    prefix: row.get("prefix"),
                    created_at: row.get("created_at"),
                    expires_at: row.get("expires_at"),
                    last_used_at: row.get("last_used_at"),
                    revoked: revoked_int != 0,
                    permissions,
                    requests_per_minute: row
                        .get::<Option<i64>, _>("requests_per_minute")
                        .map(|v| v as u32),
                },
                hash: row.get("hash"),
                manageable: true,
            });
        }
        tracing::info!("Loaded {} API keys from database", loaded.len());

        match sqlx::query_as::<_, (String, i64)>(
            "SELECT head_hash, event_count FROM audit_chain_state WHERE singleton = 1",
        )
        .fetch_optional(pool)
        .await
        {
            Ok(Some((head, count))) if count > 0 => *self.audit_head.write().await = Some(head),
            Ok(Some(_)) | Ok(None) => *self.audit_head.write().await = None,
            Err(error) => tracing::warn!(%error, "failed to load audit chain head"),
        }
    }

    /// 创建新的 API Key
    ///
    /// 返回 (key_id, raw_key)。raw_key 仅在创建时返回，不再持久化存储。
    ///
    /// Keys created through this method are durable when a storage pool is configured.
    /// Short-lived, memory-only credentials must use `create_ephemeral_key` explicitly.
    pub async fn create_key(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: Option<i64>,
    ) -> Result<(String, String), String> {
        self.create_key_with_quota(name, permissions, expires_at, None)
            .await
    }

    /// Create an API key with an optional independently enforced minute quota.
    pub async fn create_key_with_quota(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: Option<i64>,
        requests_per_minute: Option<u32>,
    ) -> Result<(String, String), String> {
        self.create_key_internal(
            name,
            permissions,
            expires_at,
            requests_per_minute,
            true,
            None,
            None,
        )
        .await
    }

    /// Create a replacement credential that preserves the caller identity.
    pub async fn create_rotated_key(
        &self,
        source: &ApiKeyInfo,
        expires_at: i64,
    ) -> Result<(String, String), String> {
        self.create_key_internal(
            &source.name,
            source.permissions.clone(),
            Some(expires_at),
            source.requests_per_minute,
            true,
            Some(source.subject_id.clone()),
            Some(source.id.clone()),
        )
        .await
    }

    /// Create a short-lived key that is never written to SQLite.
    pub async fn create_ephemeral_key(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: i64,
    ) -> Result<(String, String), String> {
        const MAX_ACTIVE_PER_EPHEMERAL_NAME: usize = 8;
        // Remove expired ephemeral/session keys while issuing a replacement.
        let now = chrono::Utc::now().timestamp_millis();
        self.keys
            .write()
            .await
            .retain(|_, stored| stored.info.expires_at.is_none_or(|expires| expires > now));
        let created = self
            .create_key_internal(name, permissions, Some(expires_at), None, false, None, None)
            .await?;
        let created_index = sha256_index(&created.1);
        // Browser reloads and repeated logins must not create an unbounded set of
        // simultaneously valid management credentials. Keep a small bounded set
        // so multiple tabs/operators still work, evicting the oldest sessions.
        let mut keys = self.keys.write().await;
        let mut same_name = keys
            .iter()
            .filter(|(_, stored)| !stored.manageable && stored.info.name == name)
            .map(|(index, stored)| {
                (
                    index.clone(),
                    index == &created_index,
                    stored.info.created_at,
                )
            })
            .collect::<Vec<_>>();
        same_name.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.2.cmp(&left.2)));
        for (index, _, _) in same_name.into_iter().skip(MAX_ACTIVE_PER_EPHEMERAL_NAME) {
            keys.remove(&index);
        }
        Ok(created)
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_key_internal(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: Option<i64>,
        requests_per_minute: Option<u32>,
        persist: bool,
        subject_id: Option<String>,
        rotation_source_id: Option<String>,
    ) -> Result<(String, String), String> {
        let is_rotation = rotation_source_id.is_some();
        if permissions.len() > 32 {
            return Err("permissions exceed policy limits".into());
        }
        let unique_permissions: std::collections::HashSet<_> = permissions.iter().collect();
        if unique_permissions.len() != permissions.len() {
            return Err("permissions must not contain duplicates".into());
        }
        let key_id = Uuid::new_v4().to_string();
        let subject_id = subject_id.unwrap_or_else(|| key_id.clone());
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
            subject_id: subject_id.clone(),
            name: name.to_string(),
            prefix: prefix.clone(),
            created_at: now,
            expires_at,
            last_used_at: None,
            revoked: false,
            permissions: permissions.clone(),
            requests_per_minute,
        };

        let stored = StoredKey {
            info,
            hash: phc_hash.clone(),
            manageable: persist,
        };

        // 内存索引（SHA-256 快速查找）
        let index_hash = sha256_index(&raw_key);
        self.keys.write().await.insert(index_hash.clone(), stored);

        // Persist every durable user-managed API key. Ephemeral management
        // sessions authenticate from memory and are deliberately excluded.
        if persist && let Some(pool) = &self.pool {
            let perms_json = serde_json::to_string(&permissions).map_err(|e| e.to_string())?;
            let mut connection = match pool.acquire().await {
                Ok(connection) => connection,
                Err(error) => {
                    self.keys.write().await.remove(&index_hash);
                    return Err(format!("failed to acquire API key database: {error}"));
                }
            };
            if let Err(error) = sqlx::query("BEGIN IMMEDIATE")
                .execute(&mut *connection)
                .await
            {
                self.keys.write().await.remove(&index_hash);
                return Err(format!("failed to begin API key transaction: {error}"));
            }
            let active_count = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM api_keys WHERE revoked = 0 AND (expires_at IS NULL OR expires_at > ?1)",
            )
            .bind(now)
            .fetch_one(&mut *connection)
            .await;
            match active_count {
                Ok(count) if count >= if is_rotation { 101 } else { 100 } => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    self.keys.write().await.remove(&index_hash);
                    return Err(if is_rotation {
                        "API Key rotation transition slot is occupied".into()
                    } else {
                        "active API Key limit reached (100)".into()
                    });
                }
                Err(error) => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    self.keys.write().await.remove(&index_hash);
                    return Err(format!("failed to count active API keys: {error}"));
                }
                Ok(_) => {}
            }
            if let Err(error) = sqlx::query(
                    "INSERT INTO api_keys (id, subject_id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, requests_per_minute, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
                )
                .bind(&key_id).bind(&subject_id).bind(name).bind(&prefix).bind(now).bind(expires_at).bind(None::<i64>).bind(0).bind(&perms_json).bind(requests_per_minute.map(i64::from)).bind(&phc_hash)
                .execute(&mut *connection)
                .await
            {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                self.keys.write().await.remove(&index_hash);
                return Err(format!("failed to persist API key: {error}"));
            }
            if let Some(source_id) = &rotation_source_id
                && let Err(error) = sqlx::query(
                    "INSERT INTO api_key_rotation_transitions(source_id,replacement_id,state,created_at) VALUES (?1,?2,'created',?3)",
                )
                .bind(source_id)
                .bind(&key_id)
                .bind(now)
                .execute(&mut *connection)
                .await
            {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                self.keys.write().await.remove(&index_hash);
                return Err(format!("failed to persist API key rotation transition: {error}"));
            }
            if let Err(error) = sqlx::query("COMMIT").execute(&mut *connection).await {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                self.keys.write().await.remove(&index_hash);
                return Err(format!("failed to commit API key transaction: {error}"));
            }
        }

        Ok((key_id, raw_key))
    }

    pub async fn mark_rotation_prepared(
        &self,
        source_id: &str,
        replacement_id: &str,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        let updated = sqlx::query(
            "UPDATE api_key_rotation_transitions SET state='prepared' WHERE source_id=?1 AND replacement_id=?2 AND state='created'",
        )
        .bind(source_id)
        .bind(replacement_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        if updated.rows_affected() != 1 {
            return Err("API key rotation transition was not in created state".into());
        }
        Ok(())
    }

    pub async fn clear_rotation_transition(
        &self,
        source_id: &str,
        replacement_id: &str,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        sqlx::query(
            "DELETE FROM api_key_rotation_transitions WHERE source_id=?1 AND replacement_id=?2",
        )
        .bind(source_id)
        .bind(replacement_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn rotation_transition_count(&self) -> Result<i64, String> {
        let Some(pool) = &self.pool else {
            return Ok(0);
        };
        sqlx::query_scalar("SELECT COUNT(*) FROM api_key_rotation_transitions")
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn rotation_transitions(&self) -> Result<Vec<ApiKeyRotationTransition>, String> {
        let Some(pool) = &self.pool else {
            return Ok(Vec::new());
        };
        use sqlx::Row as _;
        let rows = sqlx::query(
            "SELECT t.source_id,t.replacement_id,t.state,t.created_at,
                    s.revoked AS source_revoked,r.revoked AS replacement_revoked
             FROM api_key_rotation_transitions t
             JOIN api_keys s ON s.id=t.source_id
             JOIN api_keys r ON r.id=t.replacement_id
             ORDER BY t.created_at,t.source_id",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| ApiKeyRotationTransition {
                source_id: row.get("source_id"),
                replacement_id: row.get("replacement_id"),
                state: row.get("state"),
                created_at: row.get("created_at"),
                source_revoked: row.get::<i64, _>("source_revoked") != 0,
                replacement_revoked: row.get::<i64, _>("replacement_revoked") != 0,
            })
            .collect())
    }

    /// Atomically resolve a crash-left rotation transition. `cancel` is only
    /// valid before the prepared audit gate; `complete` is only valid after it.
    pub async fn reconcile_rotation_transition(
        &self,
        source_id: &str,
        replacement_id: &str,
        action: &str,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Err("durable API key storage is required".into());
        };
        let _lifecycle = self.key_lifecycle_lock.lock().await;
        let mut connection = pool.acquire().await.map_err(|error| error.to_string())?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
        let outcome = async {
            let state = sqlx::query_scalar::<_, String>(
                "SELECT state FROM api_key_rotation_transitions WHERE source_id=?1 AND replacement_id=?2",
            )
            .bind(source_id)
            .bind(replacement_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;
            let target_id = match (action, state.as_str()) {
                ("cancel", "created") => replacement_id,
                ("complete", "prepared") => source_id,
                _ => {
                    return Err(sqlx::Error::Protocol(
                        "rotation action does not match transition state".into(),
                    ));
                }
            };
            let updated = sqlx::query("UPDATE api_keys SET revoked=1 WHERE id=?1 AND revoked=0")
                .bind(target_id)
                .execute(&mut *connection)
                .await?;
            if updated.rows_affected() != 1 {
                return Err(sqlx::Error::Protocol(
                    "rotation reconciliation target is not active".into(),
                ));
            }
            sqlx::query(
                "DELETE FROM api_key_rotation_transitions WHERE source_id=?1 AND replacement_id=?2",
            )
            .bind(source_id)
            .bind(replacement_id)
            .execute(&mut *connection)
            .await?;
            Ok::<_, sqlx::Error>(target_id.to_string())
        }
        .await;
        match outcome {
            Ok(revoked_id) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(|error| error.to_string())?;
                self.keys
                    .write()
                    .await
                    .retain(|_, stored| stored.info.id != revoked_id);
                self.loaded
                    .write()
                    .await
                    .retain(|stored| stored.info.id != revoked_id);
                Ok(())
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error.to_string())
            }
        }
    }

    /// 验证 API Key
    ///
    /// 优先使用 SHA-256 快速定位（运行时创建的 Key），
    /// 未命中则遍历 DB 加载的历史 Key 并用 Argon2 验证。
    pub async fn authenticate(&self, key: &str) -> Result<AuthInfo, String> {
        let Some(prefix) = api_key_prefix(key) else {
            return Err("Invalid API key".to_string());
        };
        let index_hash = sha256_index(key);

        // 快速路径：SHA-256 索引查找
        let indexed = {
            let keys = self.keys.read().await;
            keys.get(&index_hash).cloned()
        };
        if let Some(stored) = indexed {
            let auth = Self::verify_and_build_auth(&stored, key).await?;
            self.record_successful_use(&auth.id).await;
            return Ok(auth);
        }

        // Persisted keys cannot be SHA-256 indexed after restart because the raw
        // secret is intentionally unavailable. The random public prefix narrows
        // Argon2 work to collision candidates instead of all historical keys.
        {
            // Clone candidates so Argon2 verification never holds an async read
            // lock and usage updates cannot deadlock.
            let loaded = self
                .loaded
                .read()
                .await
                .iter()
                .filter(|stored| stored.info.prefix == prefix)
                .cloned()
                .collect::<Vec<_>>();
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
                    let auth_info = self
                        .promote_verified_loaded_key(index_hash, &stored.info.id)
                        .await?;
                    self.record_successful_use(&auth_info.id).await;
                    return Ok(auth_info);
                }
            }
        }

        Err("Invalid API key".to_string())
    }

    async fn promote_verified_loaded_key(
        &self,
        index_hash: String,
        key_id: &str,
    ) -> Result<AuthInfo, String> {
        // Re-check authoritative current state while serialized with
        // revoke/delete. The Argon2 candidate passed here is a stale clone.
        let _lifecycle = self.key_lifecycle_lock.lock().await;
        let current = self
            .loaded
            .read()
            .await
            .iter()
            .find(|candidate| candidate.info.id == key_id)
            .cloned();
        let Some(current) = current else {
            return Err("Invalid API key".to_string());
        };
        if current.info.revoked
            || current
                .info
                .expires_at
                .is_some_and(|expires| chrono::Utc::now().timestamp_millis() > expires)
        {
            return Err("Invalid API key".to_string());
        }
        let auth_info = AuthInfo {
            id: current.info.id.clone(),
            subject_id: current.info.subject_id.clone(),
            name: current.info.name.clone(),
            permissions: current.info.permissions.clone(),
            requests_per_minute: current.info.requests_per_minute,
        };
        self.keys.write().await.insert(index_hash, current);
        Ok(auth_info)
    }

    /// Atomically consume one request from a subject-level 60-second window.
    /// Persistent managers keep the window in auth.db so restarts cannot reset it.
    pub async fn consume_subject_quota(
        &self,
        subject_id: &str,
        limit: u32,
    ) -> Result<QuotaConsumption, String> {
        const WINDOW_MS: i64 = 60_000;
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Some(pool) = &self.pool {
            let mut connection = pool
                .acquire()
                .await
                .map_err(|error| format!("failed to acquire quota database: {error}"))?;
            sqlx::query("BEGIN IMMEDIATE")
                .execute(&mut *connection)
                .await
                .map_err(|error| format!("failed to begin quota transaction: {error}"))?;
            let outcome = async {
                sqlx::query("DELETE FROM api_quota_events WHERE occurred_at <= ?1")
                .bind(now_ms - WINDOW_MS)
                .execute(&mut *connection)
                .await?;
                let (count, oldest): (i64, Option<i64>) = sqlx::query_as(
                    "SELECT COUNT(*), MIN(occurred_at) FROM api_quota_events WHERE subject_id = ?1",
                )
                .bind(subject_id)
                .fetch_one(&mut *connection)
                .await?;
                if count >= i64::from(limit) {
                    return Ok::<_, sqlx::Error>((false, count, oldest));
                }
                sqlx::query(
                    "INSERT INTO api_quota_events (id, subject_id, occurred_at) VALUES (?1, ?2, ?3)",
                )
                .bind(Uuid::now_v7().to_string())
                .bind(subject_id)
                .bind(now_ms)
                .execute(&mut *connection)
                .await?;
                Ok((true, count + 1, oldest.or(Some(now_ms))))
            }
            .await;
            match outcome {
                Ok((allowed, count, oldest)) => {
                    sqlx::query("COMMIT")
                        .execute(&mut *connection)
                        .await
                        .map_err(|error| format!("failed to commit quota transaction: {error}"))?;
                    let retry_after_secs = oldest
                        .map(|oldest| {
                            ((WINDOW_MS - (now_ms - oldest)).max(1) as u64).div_ceil(1_000)
                        })
                        .unwrap_or(60)
                        .max(1);
                    return Ok(QuotaConsumption {
                        allowed,
                        remaining: if allowed {
                            limit.saturating_sub(count as u32)
                        } else {
                            0
                        },
                        retry_after_secs,
                    });
                }
                Err(error) => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    return Err(format!("failed to persist quota decision: {error}"));
                }
            }
        }

        let mut windows = self.quota_windows.lock().await;
        let timestamps = windows.entry(subject_id.to_string()).or_default();
        let now = Instant::now();
        while timestamps
            .front()
            .is_some_and(|timestamp| now.duration_since(*timestamp).as_secs() >= 60)
        {
            timestamps.pop_front();
        }
        if timestamps.len() >= limit as usize {
            let retry_after_secs = timestamps
                .front()
                .map(|oldest| 60u64.saturating_sub(now.duration_since(*oldest).as_secs()))
                .unwrap_or(60)
                .max(1);
            return Ok(QuotaConsumption {
                allowed: false,
                remaining: 0,
                retry_after_secs,
            });
        }
        timestamps.push_back(now);
        Ok(QuotaConsumption {
            allowed: true,
            remaining: limit.saturating_sub(timestamps.len() as u32),
            retry_after_secs: 60,
        })
    }

    /// Update the in-memory and persisted last-used timestamp after successful
    /// authentication. Persistence failure must not turn valid authentication
    /// into an outage, but is logged for operators.
    async fn record_successful_use(&self, key_id: &str) {
        let now = chrono::Utc::now().timestamp_millis();
        const PERSIST_INTERVAL_MS: i64 = 60_000;
        let mut should_persist = false;
        {
            let mut keys = self.keys.write().await;
            for stored in keys.values_mut().filter(|stored| stored.info.id == key_id) {
                should_persist |= stored
                    .info
                    .last_used_at
                    .is_none_or(|last| now - last >= PERSIST_INTERVAL_MS);
                stored.info.last_used_at = Some(now);
            }
        }
        {
            let mut loaded = self.loaded.write().await;
            for stored in loaded.iter_mut().filter(|stored| stored.info.id == key_id) {
                should_persist |= stored
                    .info
                    .last_used_at
                    .is_none_or(|last| now - last >= PERSIST_INTERVAL_MS);
                stored.info.last_used_at = Some(now);
            }
        }
        // A one-minute write-behind window avoids one database write per API
        // request while keeping operator-visible activity reasonably fresh.
        if should_persist
            && let Some(pool) = &self.pool
            && let Err(error) = sqlx::query("UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2")
                .bind(now)
                .bind(key_id)
                .execute(pool)
                .await
        {
            tracing::warn!(key_id, %error, "Failed to persist API key usage timestamp");
        }
    }

    /// Argon2 验证并构建 AuthInfo
    async fn verify_and_build_auth(stored: &StoredKey, key: &str) -> Result<AuthInfo, String> {
        // SECURITY: Use unified error message to prevent user enumeration
        // (distinguishing revoked vs invalid keys leaks information)
        if stored.info.revoked {
            return Err("Invalid API key".to_string());
        }

        // SECURITY: Use unified error message to prevent user enumeration
        if let Some(expires) = stored.info.expires_at
            && chrono::Utc::now().timestamp_millis() > expires
        {
            return Err("Invalid API key".to_string());
        }

        let auth_info = AuthInfo {
            id: stored.info.id.clone(),
            subject_id: stored.info.subject_id.clone(),
            name: stored.info.name.clone(),
            permissions: stored.info.permissions.clone(),
            requests_per_minute: stored.info.requests_per_minute,
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
    pub async fn revoke_key(&self, key_id: &str) -> Result<bool, String> {
        let _lifecycle = self.key_lifecycle_lock.lock().await;
        // Persist first. A failed durable revocation must never be reported as
        // successful while the key could become active again after restart.
        if let Some(pool) = &self.pool {
            let result =
                sqlx::query("UPDATE api_keys SET revoked = 1 WHERE id = ?1 AND revoked = 0")
                    .bind(key_id)
                    .execute(pool)
                    .await
                    .map_err(|error| format!("failed to persist API key revocation: {error}"))?;
            if result.rows_affected() != 1 {
                return Ok(false);
            }
        } else {
            let found = self
                .keys
                .read()
                .await
                .values()
                .any(|stored| stored.info.id == key_id)
                || self
                    .loaded
                    .read()
                    .await
                    .iter()
                    .any(|stored| stored.info.id == key_id);
            if !found {
                return Ok(false);
            }
        }

        if self.pool.is_some() {
            self.keys
                .write()
                .await
                .retain(|_, stored| stored.info.id != key_id);
            self.loaded
                .write()
                .await
                .retain(|stored| stored.info.id != key_id);
        } else {
            for stored in self.keys.write().await.values_mut() {
                if stored.info.id == key_id {
                    stored.info.revoked = true;
                }
            }
            for stored in self.loaded.write().await.iter_mut() {
                if stored.info.id == key_id {
                    stored.info.revoked = true;
                }
            }
        }
        Ok(true)
    }

    /// Revoke a memory-only session without consulting the durable API Key table.
    pub async fn revoke_ephemeral_key(&self, key_id: &str) -> bool {
        let mut keys = self.keys.write().await;
        let before = keys.len();
        keys.retain(|_, stored| stored.manageable || stored.info.id != key_id);
        keys.len() != before
    }

    /// 永久删除已吊销的 API Key
    ///
    /// 仅允许删除已吊销的 Key，防止误删活跃 Key。
    pub async fn delete_key(&self, key_id: &str) -> Result<bool, String> {
        let _lifecycle = self.key_lifecycle_lock.lock().await;
        if let Some(pool) = &self.pool {
            let deleted = sqlx::query("DELETE FROM api_keys WHERE id = ?1 AND revoked = 1")
                .bind(key_id)
                .execute(pool)
                .await
                .map_err(|error| format!("failed to persist API key deletion: {error}"))?;
            if deleted.rows_affected() != 1 {
                return Ok(false);
            }
            self.keys
                .write()
                .await
                .retain(|_, stored| stored.info.id != key_id);
            self.loaded
                .write()
                .await
                .retain(|stored| stored.info.id != key_id);
            return Ok(true);
        }
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
            return Ok(false); // 不允许删除未吊销的 Key
        }

        // Delete durably before removing the in-memory copy. Otherwise a DB
        // failure could be reported as success and the key would reappear.
        // 从内存中移除
        {
            let mut keys = self.keys.write().await;
            keys.retain(|_, s| s.info.id != key_id);
        }
        {
            let mut loaded = self.loaded.write().await;
            loaded.retain(|s| s.info.id != key_id);
        }

        Ok(true)
    }

    /// 列出所有 API Key（合并内存和 DB 加载的）
    pub async fn list_keys(&self) -> Vec<ApiKeyInfo> {
        self.list_keys_result().await.unwrap_or_default()
    }

    pub async fn list_keys_result(&self) -> Result<Vec<ApiKeyInfo>, String> {
        self.key_infos_page(10_000, 0).await
    }

    pub async fn key_infos_page(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ApiKeyInfo>, String> {
        if let Some(pool) = &self.pool {
            use sqlx::Row as _;
            let rows = sqlx::query(
                "SELECT id, subject_id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, requests_per_minute FROM api_keys ORDER BY created_at DESC, id LIMIT ?1 OFFSET ?2",
            )
            .bind(limit.min(10_000) as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
            let mut all = rows
                .into_iter()
                .map(|row| {
                    Ok(ApiKeyInfo {
                        id: row.get("id"),
                        subject_id: row.get("subject_id"),
                        name: row.get("name"),
                        prefix: row.get("prefix"),
                        created_at: row.get("created_at"),
                        expires_at: row.get("expires_at"),
                        last_used_at: row.get("last_used_at"),
                        revoked: row.get::<i64, _>("revoked") != 0,
                        permissions: serde_json::from_str(&row.get::<String, _>("permissions"))
                            .map_err(|error| error.to_string())?,
                        requests_per_minute: row
                            .get::<Option<i64>, _>("requests_per_minute")
                            .map(|value| value as u32),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let persisted = all
                .iter()
                .map(|key| key.id.clone())
                .collect::<std::collections::HashSet<_>>();
            let keys = self.keys.read().await;
            all.extend(
                keys.values()
                    .filter(|stored| stored.manageable && !persisted.contains(&stored.info.id))
                    .map(|stored| stored.info.clone()),
            );
            return Ok(all);
        }
        let keys = self.keys.read().await;
        let loaded = self.loaded.read().await;

        let mut all: Vec<ApiKeyInfo> = keys
            .values()
            .filter(|stored| stored.manageable)
            .map(|stored| stored.info.clone())
            .collect();
        // 追加 DB 加载的 Key（去重：以 id 为准）
        let seen: std::collections::HashSet<String> = all.iter().map(|k| k.id.clone()).collect();
        for s in loaded.iter().filter(|stored| stored.manageable) {
            if !seen.contains(&s.info.id) {
                all.push(s.info.clone());
            }
        }
        Ok(all)
    }

    pub async fn find_key_info(&self, key_id: &str) -> Result<Option<ApiKeyInfo>, String> {
        if let Some(pool) = &self.pool {
            use sqlx::Row as _;
            let row = sqlx::query(
                "SELECT id, subject_id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, requests_per_minute FROM api_keys WHERE id = ?1",
            )
            .bind(key_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?;
            return row
                .map(|row| {
                    Ok(ApiKeyInfo {
                        id: row.get("id"),
                        subject_id: row.get("subject_id"),
                        name: row.get("name"),
                        prefix: row.get("prefix"),
                        created_at: row.get("created_at"),
                        expires_at: row.get("expires_at"),
                        last_used_at: row.get("last_used_at"),
                        revoked: row.get::<i64, _>("revoked") != 0,
                        permissions: serde_json::from_str(&row.get::<String, _>("permissions"))
                            .map_err(|error| error.to_string())?,
                        requests_per_minute: row
                            .get::<Option<i64>, _>("requests_per_minute")
                            .map(|value| value as u32),
                    })
                })
                .transpose();
        }
        Ok(self
            .list_keys()
            .await
            .into_iter()
            .find(|key| key.id == key_id))
    }

    pub async fn active_key_count(&self) -> Result<i64, String> {
        if let Some(pool) = &self.pool {
            return sqlx::query_scalar(
                "SELECT COUNT(*) FROM api_keys WHERE revoked = 0 AND (expires_at IS NULL OR expires_at > ?1)",
            )
            .bind(chrono::Utc::now().timestamp_millis())
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string());
        }
        let now = chrono::Utc::now().timestamp_millis();
        Ok(self
            .list_keys()
            .await
            .iter()
            .filter(|key| !key.revoked && key.expires_at.is_none_or(|expires| expires > now))
            .count() as i64)
    }

    /// Append a tamper-evident audit event. Metadata must already be redacted.
    pub async fn record_audit(
        &self,
        actor_id: &str,
        action: &str,
        resource: &str,
        metadata: serde_json::Value,
    ) -> Result<AuditEvent, String> {
        let _guard = self.audit_lock.lock().await;
        let timestamp = chrono::Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();
        let metadata_json = serde_json::to_string(&metadata).map_err(|e| e.to_string())?;
        let previous_hash = if self.pool.is_some() {
            self.audit_head
                .read()
                .await
                .clone()
                .unwrap_or_else(|| "GENESIS".to_string())
        } else {
            self.audit_events
                .read()
                .await
                .last()
                .map(|event| event.event_hash.clone())
                .unwrap_or_else(|| "GENESIS".to_string())
        };
        let event_hash = audit_hash(
            &id,
            timestamp,
            actor_id,
            action,
            resource,
            &metadata_json,
            &previous_hash,
        );
        let event = AuditEvent {
            id,
            timestamp,
            actor_id: actor_id.to_string(),
            action: action.to_string(),
            resource: resource.to_string(),
            metadata,
            previous_hash,
            event_hash,
        };
        if let Some(pool) = &self.pool {
            let mut transaction = pool.begin().await.map_err(|e| e.to_string())?;
            sqlx::query("INSERT INTO audit_events (id, timestamp, actor_id, action, resource, metadata_json, previous_hash, event_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
                .bind(&event.id).bind(event.timestamp).bind(&event.actor_id).bind(&event.action)
                .bind(&event.resource).bind(&metadata_json).bind(&event.previous_hash).bind(&event.event_hash)
                .execute(&mut *transaction).await.map_err(|e| e.to_string())?;
            let updated = sqlx::query(
                "UPDATE audit_chain_state SET head_hash = ?1, event_count = event_count + 1 WHERE singleton = 1 AND head_hash = ?2",
            )
            .bind(&event.event_hash)
            .bind(&event.previous_hash)
            .execute(&mut *transaction)
            .await
            .map_err(|e| e.to_string())?;
            if updated.rows_affected() != 1 {
                return Err("audit chain anchor does not match current head".into());
            }
            transaction.commit().await.map_err(|e| e.to_string())?;
        }
        if self.pool.is_some() {
            *self.audit_head.write().await = Some(event.event_hash.clone());
        } else {
            self.audit_events.write().await.push(event.clone());
        }
        Ok(event)
    }

    pub async fn list_audit_events(&self, limit: usize) -> Vec<AuditEvent> {
        self.query_audit_events(limit).await.unwrap_or_default()
    }

    pub async fn query_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, String> {
        if let Some(pool) = &self.pool {
            let rows = sqlx::query(
                "SELECT id, timestamp, actor_id, action, resource, metadata_json, previous_hash, event_hash FROM audit_events ORDER BY timestamp DESC, rowid DESC LIMIT ?1",
            )
            .bind(limit.min(1_000) as i64)
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
            return rows
                .into_iter()
                .map(|row| audit_event_from_row(&row))
                .collect();
        }
        Ok(self
            .audit_events
            .read()
            .await
            .iter()
            .rev()
            .take(limit.min(1_000))
            .cloned()
            .collect())
    }

    pub async fn verify_audit_chain(&self) -> bool {
        let _guard = self.audit_lock.lock().await;
        if let Some(pool) = &self.pool {
            let mut rows = sqlx::query(
                "SELECT id, timestamp, actor_id, action, resource, metadata_json, previous_hash, event_hash FROM audit_events ORDER BY timestamp, rowid",
            )
            .fetch(pool);
            let mut previous = "GENESIS".to_string();
            let mut count = 0_i64;
            loop {
                let row = match rows.try_next().await {
                    Ok(Some(row)) => row,
                    Ok(None) => break,
                    Err(error) => {
                        tracing::error!(%error, "failed to stream audit ledger for verification");
                        return false;
                    }
                };
                let event = match audit_event_from_row(&row) {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::error!(%error, "invalid persisted audit event");
                        return false;
                    }
                };
                if !audit_event_follows(&event, &previous) {
                    return false;
                }
                previous = event.event_hash;
                count += 1;
            }
            let anchor = sqlx::query_as::<_, (String, i64)>(
                "SELECT head_hash, event_count FROM audit_chain_state WHERE singleton = 1",
            )
            .fetch_optional(pool)
            .await;
            return matches!(anchor, Ok(Some((head, anchor_count))) if head == previous && anchor_count == count);
        }
        let events = self.audit_events.read().await;
        let mut previous = "GENESIS".to_string();
        for event in events.iter() {
            if !audit_event_follows(event, &previous) {
                return false;
            }
            previous = event.event_hash.clone();
        }
        true
    }

    /// Atomically add one authenticated request to the durable hourly ledger.
    pub async fn record_usage(
        &self,
        key_id: &str,
        subject_id: &str,
        status: u16,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        let now = chrono::Utc::now().timestamp_millis();
        let hour_ms = 60 * 60 * 1_000;
        let bucket_start = now - now.rem_euclid(hour_ms);
        let status_class = i32::from(status / 100).clamp(1, 5);
        let result = async {
            let mut transaction = pool.begin().await?;
            for statement in [
                "INSERT INTO api_usage_hourly (key_id, subject_id, bucket_start, status_class, request_count) \
                 VALUES (?1, ?2, ?3, ?4, 1) \
                 ON CONFLICT(key_id, bucket_start, status_class) \
                 DO UPDATE SET request_count = request_count + 1, subject_id = excluded.subject_id",
                "INSERT INTO api_usage_integrity (key_id, subject_id, bucket_start, status_class, request_count) \
                 VALUES (?1, ?2, ?3, ?4, 1) \
                 ON CONFLICT(key_id, bucket_start, status_class) \
                 DO UPDATE SET request_count = request_count + 1, subject_id = excluded.subject_id",
            ] {
                sqlx::query(statement)
                    .bind(key_id)
                    .bind(subject_id)
                    .bind(bucket_start)
                    .bind(status_class)
                    .execute(&mut *transaction)
                    .await?;
            }
            let anchor = sqlx::query(
                "UPDATE api_usage_ledger_state SET total_requests = total_requests + 1 WHERE singleton = 1",
            )
            .execute(&mut *transaction)
            .await?;
            if anchor.rows_affected() != 1 {
                return Err(sqlx::Error::Protocol("usage ledger anchor is missing".into()));
            }
            transaction.commit().await
        }
        .await;
        match result {
            Ok(_) => {
                self.metering_healthy.store(true, Ordering::Release);
                Ok(())
            }
            Err(error) => {
                self.metering_healthy.store(false, Ordering::Release);
                Err(error.to_string())
            }
        }
    }

    /// Validate the mutable usage aggregate against an independently updated
    /// mirror and cumulative request anchor before any invoice export.
    pub async fn verify_usage_ledger_integrity(&self) -> Result<bool, String> {
        let Some(pool) = &self.pool else {
            return Err("durable usage storage is required".into());
        };
        let mismatch: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT key_id, subject_id, bucket_start, status_class, request_count FROM api_usage_hourly
                EXCEPT
                SELECT key_id, subject_id, bucket_start, status_class, request_count FROM api_usage_integrity
             ) OR EXISTS(
                SELECT key_id, subject_id, bucket_start, status_class, request_count FROM api_usage_integrity
                EXCEPT
                SELECT key_id, subject_id, bucket_start, status_class, request_count FROM api_usage_hourly
             )",
        )
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
        if mismatch != 0 {
            return Ok(false);
        }
        let totals = sqlx::query_as::<_, (i64, i64)>(
            "SELECT COALESCE(SUM(u.request_count), 0), s.total_requests
             FROM api_usage_ledger_state s LEFT JOIN api_usage_hourly u ON 1 = 1
             WHERE s.singleton = 1 GROUP BY s.total_requests",
        )
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(matches!(totals, Some((actual, anchor)) if actual == anchor))
    }

    /// Read a bounded UTC time range from the durable usage ledger.
    pub async fn usage_records(
        &self,
        from: i64,
        to: i64,
        key_id: Option<&str>,
        subject_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<UsageRecord>, String> {
        let Some(pool) = &self.pool else {
            return Ok(Vec::new());
        };
        use sqlx::Row as _;
        let rows = if let Some(key_id) = key_id {
            sqlx::query(
                "SELECT key_id, subject_id, bucket_start, status_class, request_count \
                 FROM api_usage_hourly WHERE bucket_start >= ?1 AND bucket_start < ?2 \
                 AND key_id = ?3 ORDER BY bucket_start, key_id, status_class LIMIT ?4 OFFSET ?5",
            )
            .bind(from)
            .bind(to)
            .bind(key_id)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await
        } else if let Some(subject_id) = subject_id {
            sqlx::query(
                "SELECT key_id, subject_id, bucket_start, status_class, request_count \
                 FROM api_usage_hourly WHERE bucket_start >= ?1 AND bucket_start < ?2 \
                 AND subject_id = ?3 ORDER BY bucket_start, key_id, status_class LIMIT ?4 OFFSET ?5",
            )
            .bind(from)
            .bind(to)
            .bind(subject_id)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await
        } else {
            sqlx::query(
                "SELECT key_id, subject_id, bucket_start, status_class, request_count \
                 FROM api_usage_hourly WHERE bucket_start >= ?1 AND bucket_start < ?2 \
                 ORDER BY bucket_start, key_id, status_class LIMIT ?3 OFFSET ?4",
            )
            .bind(from)
            .bind(to)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await
        }
        .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| UsageRecord {
                key_id: row.get("key_id"),
                subject_id: row.get("subject_id"),
                bucket_start: row.get("bucket_start"),
                status_class: row.get("status_class"),
                request_count: row.get("request_count"),
            })
            .collect())
    }

    pub async fn usage_total(
        &self,
        from: i64,
        to: i64,
        key_id: Option<&str>,
        subject_id: Option<&str>,
    ) -> Result<i64, String> {
        let Some(pool) = &self.pool else {
            return Ok(0);
        };
        let total = if let Some(key_id) = key_id {
            sqlx::query_scalar(
                "SELECT COALESCE(SUM(request_count), 0) FROM api_usage_hourly
                 WHERE bucket_start >= ?1 AND bucket_start < ?2 AND key_id = ?3",
            )
            .bind(from)
            .bind(to)
            .bind(key_id)
            .fetch_one(pool)
            .await
        } else if let Some(subject_id) = subject_id {
            sqlx::query_scalar(
                "SELECT COALESCE(SUM(request_count), 0) FROM api_usage_hourly
                 WHERE bucket_start >= ?1 AND bucket_start < ?2 AND subject_id = ?3",
            )
            .bind(from)
            .bind(to)
            .bind(subject_id)
            .fetch_one(pool)
            .await
        } else {
            sqlx::query_scalar(
                "SELECT COALESCE(SUM(request_count), 0) FROM api_usage_hourly
                 WHERE bucket_start >= ?1 AND bucket_start < ?2",
            )
            .bind(from)
            .bind(to)
            .fetch_one(pool)
            .await
        };
        total.map_err(|error| error.to_string())
    }

    pub async fn record_billing_event(
        &self,
        mut event: BillingEvent,
    ) -> Result<BillingEventWrite, String> {
        let Some(pool) = &self.pool else {
            return Err("durable billing storage is required".into());
        };
        event.received_at = chrono::Utc::now().timestamp_millis();
        event.event_hash = billing_event_hash(&event);
        let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO billing_events \
             (provider,event_id,event_type,object_id,customer_ref,amount_minor,currency,occurred_at,received_at,event_hash) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        )
        .bind(&event.provider).bind(&event.event_id).bind(&event.event_type)
        .bind(&event.object_id).bind(&event.customer_ref).bind(event.amount_minor)
        .bind(&event.currency).bind(event.occurred_at).bind(event.received_at)
        .bind(&event.event_hash).execute(&mut *transaction).await.map_err(|error| error.to_string())?;
        if inserted.rows_affected() == 1 {
            let updated = sqlx::query(
                "UPDATE billing_ledger_state SET event_count = event_count + 1 WHERE singleton = 1",
            )
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() != 1 {
                return Err("billing ledger anchor is missing".into());
            }
            transaction
                .commit()
                .await
                .map_err(|error| error.to_string())?;
            return Ok(BillingEventWrite::Created);
        }
        use sqlx::Row as _;
        let existing =
            sqlx::query("SELECT event_hash FROM billing_events WHERE provider=?1 AND event_id=?2")
                .bind(&event.provider)
                .bind(&event.event_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|error| error.to_string())?;
        let disposition = if existing.get::<String, _>("event_hash") == event.event_hash {
            BillingEventWrite::Duplicate
        } else {
            BillingEventWrite::Conflict
        };
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        Ok(disposition)
    }

    pub async fn reserve_idempotency(
        &self,
        key_id: &str,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<IdempotencyReservation, String> {
        let Some(pool) = &self.pool else {
            return Err("durable idempotency storage is required".into());
        };
        let now = chrono::Utc::now().timestamp_millis();
        let expires_at = now + 24 * 60 * 60 * 1_000;
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM api_idempotency WHERE expires_at <= ?1")
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO api_idempotency \
             (key_id,idempotency_key,request_hash,state,created_at,expires_at) \
             VALUES (?1,?2,?3,'pending',?4,?5)",
        )
        .bind(key_id)
        .bind(idempotency_key)
        .bind(request_hash)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if inserted.rows_affected() == 1 {
            tx.commit().await.map_err(|e| e.to_string())?;
            return Ok(IdempotencyReservation::Acquired);
        }
        use sqlx::Row as _;
        let row = sqlx::query(
            "SELECT request_hash,state,http_status,response_json FROM api_idempotency \
             WHERE key_id=?1 AND idempotency_key=?2",
        )
        .bind(key_id)
        .bind(idempotency_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let existing_hash: String = row.get("request_hash");
        let state: String = row.get("state");
        let status: Option<i64> = row.get("http_status");
        let response: Option<String> = row.get("response_json");
        tx.commit().await.map_err(|e| e.to_string())?;
        if existing_hash != request_hash {
            Ok(IdempotencyReservation::Conflict)
        } else if state == "completed" {
            Ok(IdempotencyReservation::Replay {
                status: status.unwrap_or(500) as u16,
                response_json: response.unwrap_or_else(|| "{}".into()),
            })
        } else {
            Ok(IdempotencyReservation::InProgress)
        }
    }

    pub async fn complete_idempotency(
        &self,
        key_id: &str,
        idempotency_key: &str,
        request_hash: &str,
        status: u16,
        response_json: &str,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Err("durable idempotency storage is required".into());
        };
        let result = sqlx::query(
            "UPDATE api_idempotency SET state='completed',http_status=?1,response_json=?2 \
             WHERE key_id=?3 AND idempotency_key=?4 AND request_hash=?5 AND state='pending'",
        )
        .bind(i64::from(status))
        .bind(response_json)
        .bind(key_id)
        .bind(idempotency_key)
        .bind(request_hash)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
        if result.rows_affected() != 1 {
            return Err("idempotency reservation was not pending".into());
        }
        Ok(())
    }

    pub async fn billing_events(
        &self,
        from: i64,
        to: i64,
        filter: BillingEventFilter<'_>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<BillingEvent>, String> {
        let Some(pool) = &self.pool else {
            return Ok(Vec::new());
        };
        use sqlx::Row as _;
        let mut query =
            sqlx::QueryBuilder::new("SELECT * FROM billing_events WHERE occurred_at >= ");
        query
            .push_bind(from)
            .push(" AND occurred_at < ")
            .push_bind(to);
        if let Some(customer_ref) = filter.customer_ref {
            query.push(" AND customer_ref = ").push_bind(customer_ref);
        }
        if let Some(provider) = filter.provider {
            query.push(" AND provider = ").push_bind(provider);
        }
        if let Some(event_type) = filter.event_type {
            query.push(" AND event_type = ").push_bind(event_type);
        }
        query
            .push(" ORDER BY occurred_at DESC, provider, event_id LIMIT ")
            .push_bind(limit as i64)
            .push(" OFFSET ")
            .push_bind(offset as i64);
        let rows = query
            .build()
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| BillingEvent {
                provider: row.get("provider"),
                event_id: row.get("event_id"),
                event_type: row.get("event_type"),
                object_id: row.get("object_id"),
                customer_ref: row.get("customer_ref"),
                amount_minor: row.get("amount_minor"),
                currency: row.get("currency"),
                occurred_at: row.get("occurred_at"),
                received_at: row.get("received_at"),
                event_hash: row.get("event_hash"),
            })
            .collect())
    }

    pub async fn billing_event_count(
        &self,
        from: i64,
        to: i64,
        filter: BillingEventFilter<'_>,
    ) -> Result<i64, String> {
        let Some(pool) = &self.pool else {
            return Ok(0);
        };
        let mut query =
            sqlx::QueryBuilder::new("SELECT COUNT(*) FROM billing_events WHERE occurred_at >= ");
        query
            .push_bind(from)
            .push(" AND occurred_at < ")
            .push_bind(to);
        if let Some(customer_ref) = filter.customer_ref {
            query.push(" AND customer_ref = ").push_bind(customer_ref);
        }
        if let Some(provider) = filter.provider {
            query.push(" AND provider = ").push_bind(provider);
        }
        if let Some(event_type) = filter.event_type {
            query.push(" AND event_type = ").push_bind(event_type);
        }
        query
            .build_query_scalar()
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())
    }

    /// Verify immutable financial event fields against their stored content hashes.
    pub fn verify_billing_event_integrity(events: &[BillingEvent]) -> bool {
        events
            .iter()
            .all(|event| billing_event_hash(event) == event.event_hash)
    }

    /// Verify every persisted financial event and the independent row-count
    /// anchor so deletion cannot be hidden by returning only the remaining rows.
    pub async fn verify_billing_ledger_integrity(&self) -> Result<bool, String> {
        let Some(pool) = &self.pool else {
            return Err("durable billing storage is required".into());
        };
        use sqlx::Row as _;
        let mut rows =
            sqlx::query("SELECT * FROM billing_events ORDER BY received_at, rowid").fetch(pool);
        let mut count = 0_i64;
        while let Some(row) = rows.try_next().await.map_err(|error| error.to_string())? {
            let event = BillingEvent {
                provider: row.get("provider"),
                event_id: row.get("event_id"),
                event_type: row.get("event_type"),
                object_id: row.get("object_id"),
                customer_ref: row.get("customer_ref"),
                amount_minor: row.get("amount_minor"),
                currency: row.get("currency"),
                occurred_at: row.get("occurred_at"),
                received_at: row.get("received_at"),
                event_hash: row.get("event_hash"),
            };
            if billing_event_hash(&event) != event.event_hash {
                return Ok(false);
            }
            count += 1;
        }
        let anchor = sqlx::query_scalar::<_, i64>(
            "SELECT event_count FROM billing_ledger_state WHERE singleton = 1",
        )
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(anchor == Some(count))
    }

    /// Verify that the durable authentication/billing store is reachable.
    pub async fn storage_ready(&self) -> bool {
        match &self.pool {
            Some(pool) => sqlx::query_scalar::<_, i64>("SELECT 1")
                .fetch_one(pool)
                .await
                .is_ok(),
            // In-memory auth is valid for tests/development.
            None => true,
        }
    }

    pub async fn schema_version(&self) -> Result<i64, String> {
        let Some(pool) = &self.pool else {
            return Ok(0);
        };
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())
    }

    /// Whether the most recent durable usage write succeeded.
    pub fn metering_ready(&self) -> bool {
        self.metering_healthy.load(Ordering::Acquire)
    }

    pub fn quota_ready(&self) -> bool {
        self.quota_healthy.load(Ordering::Acquire)
    }

    pub fn set_quota_ready(&self, ready: bool) {
        self.quota_healthy.store(ready, Ordering::Release);
    }

    /// When metering is unhealthy, perform a real SQLite write probe so
    /// readiness can recover after transient disk/database failures.
    pub async fn probe_metering_ready(&self) -> bool {
        if self.metering_ready() {
            return true;
        }
        let Some(pool) = &self.pool else {
            self.metering_healthy.store(true, Ordering::Release);
            return true;
        };
        let result = async {
            let mut transaction = pool.begin().await?;
            let bucket = chrono::Utc::now().timestamp_millis();
            sqlx::query(
                "INSERT INTO api_usage_hourly(key_id, subject_id, bucket_start, status_class, request_count) \
                 VALUES ('__metering_probe__', '__metering_probe__', ?1, 0, 0)",
            )
            .bind(bucket)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "DELETE FROM api_usage_hourly WHERE key_id = '__metering_probe__' AND bucket_start = ?1",
            )
            .bind(bucket)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await
        }
        .await
        .is_ok();
        self.metering_healthy.store(result, Ordering::Release);
        result
    }

    pub async fn create_target_grant(
        &self,
        subject_id: &str,
        platform: &str,
        chat_id: &str,
        actions: Vec<String>,
        created_by: &str,
    ) -> Result<TargetGrant, String> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "target authorization requires durable storage".to_string())?;
        let platform = platform.trim().to_ascii_lowercase();
        let chat_id = chat_id.trim();
        if subject_id.trim().is_empty() || platform.is_empty() || chat_id.is_empty() {
            return Err("subject_id, platform and chat_id are required".into());
        }
        if platform.len() > 64 || chat_id.len() > 255 {
            return Err("platform or chat_id exceeds policy limits".into());
        }
        if actions.is_empty() || actions.len() > target_actions::ALL.len() {
            return Err("at least one target action is required".into());
        }
        let unique: std::collections::HashSet<_> = actions.iter().collect();
        if unique.len() != actions.len()
            || actions
                .iter()
                .any(|action| !target_actions::ALL.contains(&action.as_str()))
        {
            return Err("target actions are invalid or duplicated".into());
        }
        let subject_exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_keys WHERE subject_id = ?1")
                .bind(subject_id)
                .fetch_one(pool)
                .await
                .map_err(|error| error.to_string())?;
        if subject_exists == 0 {
            return Err("subject does not exist".into());
        }
        let grant = TargetGrant {
            id: Uuid::now_v7().to_string(),
            subject_id: subject_id.to_string(),
            platform,
            chat_id: chat_id.to_string(),
            actions,
            created_at: chrono::Utc::now().timestamp_millis(),
            created_by: created_by.to_string(),
        };
        let actions_json = serde_json::to_string(&grant.actions).map_err(|e| e.to_string())?;
        sqlx::query(
            "INSERT INTO target_grants(id,subject_id,platform,chat_id,actions,created_at,created_by) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
        )
        .bind(&grant.id)
        .bind(&grant.subject_id)
        .bind(&grant.platform)
        .bind(&grant.chat_id)
        .bind(actions_json)
        .bind(grant.created_at)
        .bind(&grant.created_by)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(grant)
    }

    pub async fn list_target_grants(&self, subject_id: &str) -> Result<Vec<TargetGrant>, String> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "target authorization requires durable storage".to_string())?;
        use sqlx::Row as _;
        let rows = sqlx::query(
            "SELECT id,subject_id,platform,chat_id,actions,created_at,created_by \
             FROM target_grants WHERE subject_id = ?1 ORDER BY platform,chat_id,id",
        )
        .bind(subject_id)
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                let actions_json: String = row.try_get("actions").map_err(|e| e.to_string())?;
                Ok(TargetGrant {
                    id: row.try_get("id").map_err(|e| e.to_string())?,
                    subject_id: row.try_get("subject_id").map_err(|e| e.to_string())?,
                    platform: row.try_get("platform").map_err(|e| e.to_string())?,
                    chat_id: row.try_get("chat_id").map_err(|e| e.to_string())?,
                    actions: serde_json::from_str(&actions_json).map_err(|e| e.to_string())?,
                    created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
                    created_by: row.try_get("created_by").map_err(|e| e.to_string())?,
                })
            })
            .collect()
    }

    pub async fn delete_target_grant(
        &self,
        subject_id: &str,
        grant_id: &str,
    ) -> Result<bool, String> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "target authorization requires durable storage".to_string())?;
        sqlx::query("DELETE FROM target_grants WHERE id = ?1 AND subject_id = ?2")
            .bind(grant_id)
            .bind(subject_id)
            .execute(pool)
            .await
            .map(|result| result.rows_affected() == 1)
            .map_err(|error| error.to_string())
    }

    pub async fn target_authorized(
        &self,
        subject_id: &str,
        platform: &str,
        chat_id: &str,
        action: &str,
    ) -> Result<bool, String> {
        if !target_actions::ALL.contains(&action) {
            return Err("unknown target action".into());
        }
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "target authorization requires durable storage".to_string())?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT actions FROM target_grants WHERE subject_id = ?1 \
             AND (platform = ?2 OR platform = '*') AND (chat_id = ?3 OR chat_id = '*')",
        )
        .bind(subject_id)
        .bind(platform.trim().to_ascii_lowercase())
        .bind(chat_id.trim())
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(rows.into_iter().any(|json| {
            serde_json::from_str::<Vec<String>>(&json)
                .is_ok_and(|actions| actions.iter().any(|candidate| candidate == action))
        }))
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

fn api_key_prefix(key: &str) -> Option<&str> {
    if key.len() != 35
        || !key.starts_with("eb_")
        || !key[3..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    key.get(..8)
}

fn audit_hash(
    id: &str,
    timestamp: i64,
    actor: &str,
    action: &str,
    resource: &str,
    metadata: &str,
    previous: &str,
) -> String {
    let mut hasher = Sha256::new();
    for part in [
        id,
        &timestamp.to_string(),
        actor,
        action,
        resource,
        metadata,
        previous,
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn audit_event_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<AuditEvent, String> {
    use sqlx::Row as _;
    let metadata_json: String = row.try_get("metadata_json").map_err(|e| e.to_string())?;
    Ok(AuditEvent {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        timestamp: row.try_get("timestamp").map_err(|e| e.to_string())?,
        actor_id: row.try_get("actor_id").map_err(|e| e.to_string())?,
        action: row.try_get("action").map_err(|e| e.to_string())?,
        resource: row.try_get("resource").map_err(|e| e.to_string())?,
        metadata: serde_json::from_str(&metadata_json).map_err(|e| e.to_string())?,
        previous_hash: row.try_get("previous_hash").map_err(|e| e.to_string())?,
        event_hash: row.try_get("event_hash").map_err(|e| e.to_string())?,
    })
}

fn audit_event_follows(event: &AuditEvent, previous: &str) -> bool {
    let Ok(metadata_json) = serde_json::to_string(&event.metadata) else {
        return false;
    };
    event.previous_hash == previous
        && event.event_hash
            == audit_hash(
                &event.id,
                event.timestamp,
                &event.actor_id,
                &event.action,
                &event.resource,
                &metadata_json,
                &event.previous_hash,
            )
}

fn billing_event_hash(event: &BillingEvent) -> String {
    let mut hasher = Sha256::new();
    for part in [
        event.provider.as_str(),
        event.event_id.as_str(),
        event.event_type.as_str(),
        event.object_id.as_str(),
        event.customer_ref.as_str(),
        &event.amount_minor.to_string(),
        event.currency.as_str(),
        &event.occurred_at.to_string(),
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_authenticate() {
        let mgr = ApiKeyManager::new(None);
        let (id, key) = mgr
            .create_key("test", vec!["message:send".to_string()], None)
            .await
            .unwrap();

        assert!(!id.is_empty());
        assert!(key.starts_with("eb_"));

        let auth = mgr.authenticate(&key).await.unwrap();
        assert_eq!(auth.name, "test");
        assert_eq!(auth.permissions, vec!["message:send"]);
        let keys = mgr.list_keys().await;
        assert!(keys[0].last_used_at.is_some());
    }

    #[tokio::test]
    async fn ephemeral_sessions_are_authenticatable_hidden_and_bounded() {
        let manager = ApiKeyManager::new(None);
        let expires_at = chrono::Utc::now().timestamp_millis() + 60 * 60 * 1_000;
        let mut sessions = Vec::new();
        for _ in 0..10 {
            sessions.push(
                manager
                    .create_ephemeral_key("admin-session", vec!["*".into()], expires_at)
                    .await
                    .unwrap()
                    .1,
            );
        }

        assert!(manager.list_keys().await.is_empty());
        assert!(manager.authenticate(sessions.last().unwrap()).await.is_ok());
        let valid_count =
            futures::future::join_all(sessions.iter().map(|session| manager.authenticate(session)))
                .await
                .into_iter()
                .filter(Result::is_ok)
                .count();
        assert_eq!(valid_count, 8);
    }

    #[test]
    fn api_key_locator_rejects_malformed_or_unbounded_secrets() {
        assert_eq!(
            api_key_prefix("eb_0123456789abcdef0123456789abcdef"),
            Some("eb_01234")
        );
        for invalid in [
            "invalid_key",
            "eb_0123",
            "eb_0123456789abcdef0123456789abcdeg",
            "EB_0123456789abcdef0123456789abcdef",
        ] {
            assert_eq!(api_key_prefix(invalid), None);
        }
    }

    #[tokio::test]
    async fn test_authentication_persists_last_used_at() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let mgr = ApiKeyManager::new(Some(pool.clone()));
        let (id, key) = mgr
            .create_key_with_quota("customer", vec!["messagesread".into()], None, Some(250))
            .await
            .unwrap();

        mgr.authenticate(&key).await.unwrap();

        use sqlx::Row as _;
        let row =
            sqlx::query("SELECT last_used_at, requests_per_minute FROM api_keys WHERE id = ?1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let last_used_at: Option<i64> = row.get("last_used_at");
        assert!(last_used_at.is_some());
        let quota: Option<i64> = row.get("requests_per_minute");
        assert_eq!(quota, Some(250));

        let reloaded = ApiKeyManager::new(Some(pool));
        reloaded.load_from_db().await;
        let auth = reloaded.authenticate(&key).await.unwrap();
        assert_eq!(auth.requests_per_minute, Some(250));
    }

    #[tokio::test]
    async fn stale_argon_result_cannot_resurrect_revoked_loaded_key() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let issuer = ApiKeyManager::new(Some(pool.clone()));
        let (key_id, raw_key) = issuer.create_key("customer", vec![], None).await.unwrap();
        let reloaded = ApiKeyManager::new(Some(pool));
        reloaded.load_from_db().await;

        let stale_index = sha256_index(&raw_key);
        reloaded.loaded.write().await[0].info.revoked = true;
        let result = reloaded
            .promote_verified_loaded_key(stale_index.clone(), &key_id)
            .await;
        assert_eq!(result.unwrap_err(), "Invalid API key");
        assert!(!reloaded.keys.read().await.contains_key(&stale_index));
    }

    #[tokio::test]
    async fn restart_keeps_inactive_key_history_out_of_authentication_memory() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let issuer = ApiKeyManager::new(Some(pool.clone()));
        let (revoked_id, _) = issuer.create_key("revoked", vec![], None).await.unwrap();
        issuer.revoke_key(&revoked_id).await.unwrap();
        issuer.create_key("expired", vec![], Some(1)).await.unwrap();

        let reloaded = ApiKeyManager::new(Some(pool));
        reloaded.load_from_db().await;
        assert!(reloaded.loaded.read().await.is_empty());
        let history = reloaded.list_keys_result().await.unwrap();
        assert_eq!(history.len(), 2);
        assert!(
            history
                .iter()
                .any(|key| key.id == revoked_id && key.revoked)
        );
        assert!(reloaded.delete_key(&revoked_id).await.unwrap());
        assert_eq!(reloaded.list_keys_result().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn concurrent_key_creation_cannot_oversell_active_limit() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        sqlx::query(
            "WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 99)
             INSERT INTO api_keys(id,subject_id,name,prefix,created_at,revoked,permissions,hash)
             SELECT 'seed-' || value, 'subject-' || value, 'seed', 'eb_seed', 1, 0, '[]', 'unused' FROM n",
        )
        .execute(&pool)
        .await
        .unwrap();
        let first = ApiKeyManager::new(Some(pool.clone()));
        let second = ApiKeyManager::new(Some(pool.clone()));
        let (a, b) = tokio::join!(
            first.create_key("concurrent-a", vec![], None),
            second.create_key("concurrent-b", vec![], None),
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM api_keys WHERE revoked = 0 AND (expires_at IS NULL OR expires_at > ?1)",
            )
            .bind(chrono::Utc::now().timestamp_millis())
            .fetch_one(&pool)
            .await
            .unwrap(),
            100
        );
    }

    #[tokio::test]
    async fn full_capacity_allows_only_one_safe_rotation_transition() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        sqlx::query(
            "WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 99)
             INSERT INTO api_keys(id,subject_id,name,prefix,created_at,revoked,permissions,hash)
             SELECT 'seed-' || value, 'subject-' || value, 'seed', 'eb_seed', 1, 0, '[]', 'unused' FROM n",
        )
        .execute(&pool)
        .await
        .unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        let (source_id, _) = manager.create_key("source", vec![], None).await.unwrap();
        let source = manager.find_key_info(&source_id).await.unwrap().unwrap();
        let (replacement_id, _) = manager
            .create_rotated_key(&source, chrono::Utc::now().timestamp_millis() + 86_400_000)
            .await
            .unwrap();
        assert_eq!(manager.active_key_count().await.unwrap(), 101);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM api_key_rotation_transitions WHERE source_id=?1 AND replacement_id=?2",
            )
            .bind(&source_id)
            .bind(&replacement_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            "created"
        );

        assert!(
            manager
                .create_key("normal-overflow", vec![], None)
                .await
                .is_err()
        );
        assert!(
            manager
                .create_rotated_key(&source, chrono::Utc::now().timestamp_millis() + 86_400_000)
                .await
                .is_err()
        );

        manager
            .mark_rotation_prepared(&source_id, &replacement_id)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM api_key_rotation_transitions WHERE source_id=?1",
            )
            .bind(&source_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            "prepared"
        );
        assert!(manager.revoke_key(&source_id).await.unwrap());
        manager
            .clear_rotation_transition(&source_id, &replacement_id)
            .await
            .unwrap();
        assert_eq!(manager.active_key_count().await.unwrap(), 100);
        assert!(
            manager
                .find_key_info(&replacement_id)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_key_rotation_transitions")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn rotation_reconciliation_enforces_state_specific_atomic_action() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool));
        let (source_id, _) = manager.create_key("source", vec![], None).await.unwrap();
        let source = manager.find_key_info(&source_id).await.unwrap().unwrap();

        let (cancelled_id, _) = manager
            .create_rotated_key(&source, chrono::Utc::now().timestamp_millis() + 86_400_000)
            .await
            .unwrap();
        assert!(
            manager
                .reconcile_rotation_transition(&source_id, &cancelled_id, "complete")
                .await
                .is_err()
        );
        manager
            .reconcile_rotation_transition(&source_id, &cancelled_id, "cancel")
            .await
            .unwrap();
        assert!(
            !manager
                .find_key_info(&source_id)
                .await
                .unwrap()
                .unwrap()
                .revoked
        );
        assert!(
            manager
                .find_key_info(&cancelled_id)
                .await
                .unwrap()
                .unwrap()
                .revoked
        );

        let (completed_id, _) = manager
            .create_rotated_key(&source, chrono::Utc::now().timestamp_millis() + 86_400_000)
            .await
            .unwrap();
        manager
            .mark_rotation_prepared(&source_id, &completed_id)
            .await
            .unwrap();
        manager
            .reconcile_rotation_transition(&source_id, &completed_id, "complete")
            .await
            .unwrap();
        assert!(
            manager
                .find_key_info(&source_id)
                .await
                .unwrap()
                .unwrap()
                .revoked
        );
        assert!(
            !manager
                .find_key_info(&completed_id)
                .await
                .unwrap()
                .unwrap()
                .revoked
        );
        assert!(manager.rotation_transitions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn durable_subject_quota_survives_manager_restart() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let first = ApiKeyManager::new(Some(pool.clone()));
        assert!(
            first
                .consume_subject_quota("customer-subject", 2)
                .await
                .unwrap()
                .allowed
        );
        assert!(
            first
                .consume_subject_quota("customer-subject", 2)
                .await
                .unwrap()
                .allowed
        );

        let restarted = ApiKeyManager::new(Some(pool));
        let decision = restarted
            .consume_subject_quota("customer-subject", 2)
            .await
            .unwrap();
        assert!(!decision.allowed);
        assert_eq!(decision.remaining, 0);
        assert!(decision.retry_after_secs > 0 && decision.retry_after_secs <= 60);
    }

    #[tokio::test]
    async fn durable_subject_quota_does_not_oversell_under_concurrency() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = std::sync::Arc::new(ApiKeyManager::new(Some(pool)));
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let manager = manager.clone();
            tasks.push(tokio::spawn(async move {
                manager
                    .consume_subject_quota("concurrent-subject", 5)
                    .await
                    .unwrap()
                    .allowed
            }));
        }
        let mut allowed = 0;
        for task in tasks {
            allowed += usize::from(task.await.unwrap());
        }
        assert_eq!(allowed, 5);
    }

    #[tokio::test]
    async fn test_revoke_key() {
        let mgr = ApiKeyManager::new(None);
        let (id, key) = mgr.create_key("test", vec![], None).await.unwrap();

        assert!(mgr.revoke_key(&id).await.unwrap());
        assert!(mgr.authenticate(&key).await.is_err());
    }

    #[tokio::test]
    async fn test_delete_revoked_key() {
        let mgr = ApiKeyManager::new(None);
        let (id, _key) = mgr.create_key("test", vec![], None).await.unwrap();

        // 未吊销不能删除
        assert!(!mgr.delete_key(&id).await.unwrap());

        // 吊销后可删除
        assert!(mgr.revoke_key(&id).await.unwrap());
        assert!(mgr.delete_key(&id).await.unwrap());

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
        let (_id, key) = mgr.create_key("expired", vec![], Some(1)).await.unwrap();
        // expires_at is 1ms after epoch — definitely expired
        assert!(mgr.authenticate(&key).await.is_err());
    }

    #[tokio::test]
    async fn test_create_key_rejects_duplicate_policy_entries() {
        let mgr = ApiKeyManager::new(None);
        let duplicate_permissions = mgr
            .create_key(
                "duplicate-permissions",
                vec!["messagesread".to_string(), "messagesread".to_string()],
                None,
            )
            .await;
        assert!(duplicate_permissions.is_err());
    }

    #[tokio::test]
    async fn test_audit_chain_persists_and_detects_tampering() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        manager
            .record_audit(
                "actor-1",
                "api_key.created",
                "api_key:1",
                serde_json::json!({"plan":"starter"}),
            )
            .await
            .unwrap();
        manager
            .record_audit(
                "actor-1",
                "api_key.revoked",
                "api_key:1",
                serde_json::json!({}),
            )
            .await
            .unwrap();
        assert!(manager.verify_audit_chain().await);

        let reloaded = ApiKeyManager::new(Some(pool.clone()));
        reloaded.load_from_db().await;
        assert!(reloaded.audit_events.read().await.is_empty());
        assert_eq!(reloaded.list_audit_events(10).await.len(), 2);
        assert!(reloaded.verify_audit_chain().await);
        reloaded
            .record_audit(
                "actor-2",
                "api_key.deleted",
                "api_key:1",
                serde_json::json!({}),
            )
            .await
            .unwrap();
        assert_eq!(reloaded.list_audit_events(10).await.len(), 3);
        assert!(reloaded.verify_audit_chain().await);

        sqlx::query("UPDATE audit_events SET metadata_json = ?1 WHERE action = ?2")
            .bind(r#"{"plan":"enterprise"}"#)
            .bind("api_key.created")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!reloaded.verify_audit_chain().await);
    }

    #[tokio::test]
    async fn audit_chain_anchor_detects_tail_deletion_and_full_truncation() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        for action in ["first", "second"] {
            manager
                .record_audit("actor", action, "resource", serde_json::json!({}))
                .await
                .unwrap();
        }
        assert!(manager.verify_audit_chain().await);

        sqlx::query("DELETE FROM audit_events WHERE action = 'second'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!manager.verify_audit_chain().await);

        sqlx::query("DELETE FROM audit_events")
            .execute(&pool)
            .await
            .unwrap();
        let reloaded = ApiKeyManager::new(Some(pool));
        reloaded.load_from_db().await;
        assert!(!reloaded.verify_audit_chain().await);
    }

    #[tokio::test]
    async fn usage_ledger_is_atomic_persistent_and_filterable() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));

        manager
            .record_usage("key-a", "subject-a", 200)
            .await
            .unwrap();
        manager
            .record_usage("key-a", "subject-a", 201)
            .await
            .unwrap();
        manager
            .record_usage("key-a", "subject-a", 429)
            .await
            .unwrap();
        manager
            .record_usage("key-b", "subject-b", 500)
            .await
            .unwrap();
        manager
            .record_usage("key-a-rotated", "subject-a", 200)
            .await
            .unwrap();

        let now = chrono::Utc::now().timestamp_millis();
        let records = manager
            .usage_records(
                now - 3_600_000,
                now + 3_600_000,
                Some("key-a"),
                None,
                100,
                0,
            )
            .await
            .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records.iter().map(|r| r.request_count).sum::<i64>(), 3);
        assert_eq!(records[0].status_class, 2);
        assert_eq!(records[0].request_count, 2);
        assert_eq!(records[1].status_class, 4);

        let subject_records = manager
            .usage_records(
                now - 3_600_000,
                now + 3_600_000,
                None,
                Some("subject-a"),
                100,
                0,
            )
            .await
            .unwrap();
        assert_eq!(
            subject_records
                .iter()
                .map(|record| record.request_count)
                .sum::<i64>(),
            4
        );
        assert!(
            subject_records
                .iter()
                .all(|record| record.subject_id == "subject-a")
        );
        let key_ids = subject_records
            .iter()
            .map(|record| record.key_id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            key_ids,
            std::collections::HashSet::from(["key-a", "key-a-rotated"])
        );
        let first_page = manager
            .usage_records(
                now - 3_600_000,
                now + 3_600_000,
                None,
                Some("subject-a"),
                1,
                0,
            )
            .await
            .unwrap();
        let second_page = manager
            .usage_records(
                now - 3_600_000,
                now + 3_600_000,
                None,
                Some("subject-a"),
                1,
                1,
            )
            .await
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(second_page.len(), 1);
        assert_ne!(
            (first_page[0].key_id.as_str(), first_page[0].status_class),
            (second_page[0].key_id.as_str(), second_page[0].status_class)
        );
        assert_eq!(
            manager
                .usage_total(now - 3_600_000, now + 3_600_000, None, Some("subject-a"))
                .await
                .unwrap(),
            4
        );
        assert!(manager.verify_usage_ledger_integrity().await.unwrap());
        sqlx::query(
            "UPDATE api_usage_hourly SET request_count = request_count + 1 WHERE key_id = 'key-a'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(!manager.verify_usage_ledger_integrity().await.unwrap());
    }

    #[tokio::test]
    async fn usage_ledger_integrity_detects_row_deletion_from_both_copies() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        manager.record_usage("key", "subject", 200).await.unwrap();
        assert!(manager.verify_usage_ledger_integrity().await.unwrap());
        sqlx::query("DELETE FROM api_usage_hourly")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM api_usage_integrity")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!manager.verify_usage_ledger_integrity().await.unwrap());
    }

    #[tokio::test]
    async fn usage_subject_survives_credential_purge() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool));
        let (key_id, raw_key) = manager
            .create_key("purged-customer", vec![], None)
            .await
            .unwrap();
        let subject_id = manager.authenticate(&raw_key).await.unwrap().subject_id;
        manager
            .record_usage(&key_id, &subject_id, 200)
            .await
            .unwrap();
        assert!(manager.revoke_key(&key_id).await.unwrap());
        assert!(manager.delete_key(&key_id).await.unwrap());

        let now = chrono::Utc::now().timestamp_millis();
        let records = manager
            .usage_records(
                now - 3_600_000,
                now + 3_600_000,
                None,
                Some(&subject_id),
                100,
                0,
            )
            .await
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key_id, key_id);
        assert_eq!(records[0].subject_id, subject_id);
        assert_eq!(records[0].request_count, 1);
    }

    #[tokio::test]
    async fn storage_readiness_detects_closed_pool() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        assert!(manager.storage_ready().await);
        pool.close().await;
        assert!(!manager.storage_ready().await);
    }

    #[tokio::test]
    async fn metering_failure_blocks_and_real_write_probe_recovers() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        sqlx::query("DROP TABLE api_usage_hourly")
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            manager
                .record_usage("customer", "customer", 200)
                .await
                .is_err()
        );
        assert!(!manager.metering_ready());
        assert!(!manager.probe_metering_ready().await);
        sqlx::query(
            "CREATE TABLE api_usage_hourly (
                key_id TEXT NOT NULL,
                subject_id TEXT NOT NULL,
                bucket_start INTEGER NOT NULL,
                status_class INTEGER NOT NULL,
                request_count INTEGER NOT NULL CHECK (request_count >= 0),
                PRIMARY KEY (key_id, bucket_start, status_class)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(manager.probe_metering_ready().await);
        assert!(manager.metering_ready());
    }

    #[tokio::test]
    async fn billing_events_are_idempotent_and_conflicting_replays_are_rejected() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        let event = BillingEvent {
            provider: "stripe".into(),
            event_id: "evt_1".into(),
            event_type: "payment_succeeded".into(),
            object_id: "pi_1".into(),
            customer_ref: "customer-1".into(),
            amount_minor: 9900,
            currency: "USD".into(),
            occurred_at: 1_700_000_000_000,
            received_at: 0,
            event_hash: String::new(),
        };
        assert_eq!(
            manager.record_billing_event(event.clone()).await.unwrap(),
            BillingEventWrite::Created
        );
        assert_eq!(
            manager.record_billing_event(event.clone()).await.unwrap(),
            BillingEventWrite::Duplicate
        );
        let mut conflict = event.clone();
        conflict.amount_minor = 10_000;
        assert_eq!(
            manager.record_billing_event(conflict).await.unwrap(),
            BillingEventWrite::Conflict
        );
        let rows = manager
            .billing_events(
                1_600_000_000_000,
                1_800_000_000_000,
                BillingEventFilter {
                    customer_ref: Some("customer-1"),
                    provider: Some("stripe"),
                    event_type: Some("payment_succeeded"),
                },
                10,
                0,
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].amount_minor, 9900);
        for (event_id, event_type, occurred_at) in [
            ("evt_refund", "refund_succeeded", 1_700_000_000_001),
            ("evt_chargeback", "chargeback_opened", 1_700_000_000_002),
        ] {
            let mut followup = event.clone();
            followup.event_id = event_id.into();
            followup.event_type = event_type.into();
            followup.object_id = format!("object-{event_id}");
            followup.occurred_at = occurred_at;
            assert_eq!(
                manager.record_billing_event(followup).await.unwrap(),
                BillingEventWrite::Created
            );
        }
        let first_page = manager
            .billing_events(
                1_600_000_000_000,
                1_800_000_000_000,
                BillingEventFilter {
                    customer_ref: Some("customer-1"),
                    provider: Some("stripe"),
                    event_type: None,
                },
                2,
                0,
            )
            .await
            .unwrap();
        let second_page = manager
            .billing_events(
                1_600_000_000_000,
                1_800_000_000_000,
                BillingEventFilter {
                    customer_ref: Some("customer-1"),
                    provider: Some("stripe"),
                    event_type: None,
                },
                2,
                2,
            )
            .await
            .unwrap();
        assert_eq!(first_page.len(), 2);
        assert_eq!(second_page.len(), 1);
        assert_eq!(
            manager
                .billing_event_count(
                    1_600_000_000_000,
                    1_800_000_000_000,
                    BillingEventFilter {
                        customer_ref: Some("customer-1"),
                        provider: Some("stripe"),
                        event_type: None,
                    },
                )
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            manager
                .billing_event_count(
                    1_600_000_000_000,
                    1_800_000_000_000,
                    BillingEventFilter {
                        customer_ref: Some("customer-1"),
                        provider: Some("stripe"),
                        event_type: Some("refund_succeeded"),
                    },
                )
                .await
                .unwrap(),
            1
        );
        let all_events = manager
            .billing_events(
                1_600_000_000_000,
                1_800_000_000_000,
                BillingEventFilter {
                    customer_ref: Some("customer-1"),
                    provider: Some("stripe"),
                    event_type: None,
                },
                100,
                0,
            )
            .await
            .unwrap();
        assert!(ApiKeyManager::verify_billing_event_integrity(&all_events));
        assert!(manager.verify_billing_ledger_integrity().await.unwrap());
        sqlx::query(
            "UPDATE billing_events SET amount_minor = amount_minor + 1 WHERE event_id = 'evt_1'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let tampered = manager
            .billing_events(
                1_600_000_000_000,
                1_800_000_000_000,
                BillingEventFilter {
                    customer_ref: Some("customer-1"),
                    provider: Some("stripe"),
                    event_type: Some("payment_succeeded"),
                },
                10,
                0,
            )
            .await
            .unwrap();
        assert!(!ApiKeyManager::verify_billing_event_integrity(&tampered));
        assert!(!manager.verify_billing_ledger_integrity().await.unwrap());
    }

    #[tokio::test]
    async fn billing_ledger_anchor_detects_deleted_valid_event() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        manager
            .record_billing_event(BillingEvent {
                provider: "provider".into(),
                event_id: "event-to-delete".into(),
                event_type: "invoice_paid".into(),
                object_id: "invoice-1".into(),
                customer_ref: "customer-1".into(),
                amount_minor: 500,
                currency: "CNY".into(),
                occurred_at: 1_700_000_000_000,
                received_at: 0,
                event_hash: String::new(),
            })
            .await
            .unwrap();
        assert!(manager.verify_billing_ledger_integrity().await.unwrap());
        sqlx::query("DELETE FROM billing_events WHERE event_id = 'event-to-delete'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(!manager.verify_billing_ledger_integrity().await.unwrap());
    }

    #[tokio::test]
    async fn idempotency_reservation_replays_and_rejects_changed_requests() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool.clone()));
        assert_eq!(
            manager
                .reserve_idempotency("key-1", "request-123", "hash-a")
                .await
                .unwrap(),
            IdempotencyReservation::Acquired
        );
        assert_eq!(
            manager
                .reserve_idempotency("key-1", "request-123", "hash-a")
                .await
                .unwrap(),
            IdempotencyReservation::InProgress
        );
        assert_eq!(
            manager
                .reserve_idempotency("key-1", "request-123", "hash-b")
                .await
                .unwrap(),
            IdempotencyReservation::Conflict
        );
        manager
            .complete_idempotency(
                "key-1",
                "request-123",
                "hash-a",
                200,
                r#"{"id":"message-1"}"#,
            )
            .await
            .unwrap();
        assert_eq!(
            manager
                .reserve_idempotency("key-1", "request-123", "hash-a")
                .await
                .unwrap(),
            IdempotencyReservation::Replay {
                status: 200,
                response_json: r#"{"id":"message-1"}"#.into()
            }
        );
        let reloaded = ApiKeyManager::new(Some(pool));
        assert!(matches!(
            reloaded
                .reserve_idempotency("key-1", "request-123", "hash-a")
                .await
                .unwrap(),
            IdempotencyReservation::Replay { .. }
        ));
    }

    #[tokio::test]
    async fn target_grants_are_subject_scoped_action_scoped_and_fail_closed() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        crate::storage::sqlite::run_migrations(&pool).await.unwrap();
        let manager = ApiKeyManager::new(Some(pool));
        let (_, raw_key) = manager
            .create_key("target-owner", vec!["websocketconnect".into()], None)
            .await
            .unwrap();
        let subject = manager.authenticate(&raw_key).await.unwrap().subject_id;
        let grant = manager
            .create_target_grant(
                &subject,
                "QQ",
                "group-a",
                vec![target_actions::INBOUND_READ.into()],
                "admin",
            )
            .await
            .unwrap();

        assert!(
            manager
                .target_authorized(&subject, "qq", "group-a", target_actions::INBOUND_READ,)
                .await
                .unwrap()
        );
        assert!(
            !manager
                .target_authorized(&subject, "qq", "group-a", target_actions::MESSAGES_READ,)
                .await
                .unwrap()
        );
        assert!(
            !manager
                .target_authorized(&subject, "qq", "group-b", target_actions::INBOUND_READ,)
                .await
                .unwrap()
        );
        assert_eq!(manager.list_target_grants(&subject).await.unwrap().len(), 1);
        assert!(
            manager
                .delete_target_grant(&subject, &grant.id)
                .await
                .unwrap()
        );
        assert!(
            !manager
                .target_authorized(&subject, "qq", "group-a", target_actions::INBOUND_READ,)
                .await
                .unwrap()
        );
    }
}
