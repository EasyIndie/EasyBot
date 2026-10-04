//! API Key 管理
//!
//! 使用 argon2id 哈希存储 API Key，SHA-256 仅用于快速索引查找。
//! Key 本身不持久化明文，仅在创建时返回一次。
//! Phase 4: 从 SHA-256 升级到 argon2id (PHC 格式)
//! Phase 4: 接入 SQLite 持久化，重启不丢失

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
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

mod audit;
mod billing;
mod grants;
mod keys;

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
mod tests;
