//! SQLite 存储后端实现
//!
//! 基于 sqlx 的 SQLite 持久化实现，提供 SessionStore 和 MessageStore。
//! 包含建表迁移和连接池初始化。

use async_trait::async_trait;
use sqlx::SqlitePool;

use super::{
    MessageFilter, MessageRole, MessageStore, OutboundDelivery, OutboundDeliveryRecord,
    OutboundDeliveryState, OutboundDeliveryStats, OutboundEvent, SessionStore, StoreError,
    StoredMessage,
};
use crate::types::message::{InboundMessage, SendResult};
use crate::types::session::{ResetPolicy, Session, SessionFilter, SessionSource};

// ── Schema ──

/// 运行数据库迁移（版本化）
///
/// 从旧版幂等 CREATE TABLE 升级为版本化增量迁移。
/// 调用 `migration::run_migrations()` 逐版执行并追踪版本。
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), StoreError> {
    crate::storage::migration::run_migrations(pool).await
}

// ── 连接与迁移 ──

/// 创建 SQLite 连接池
///
/// 自动启用 WAL 模式、外键约束和忙超时。
/// 使用 `create_if_missing(true)` 确保数据库文件在不存在时自动创建。
pub async fn create_pool(db_path: &std::path::Path) -> Result<SqlitePool, StoreError> {
    if tokio::fs::symlink_metadata(db_path)
        .await
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(StoreError::Database(
            "Refusing to open SQLite database through a symbolic link".into(),
        ));
    }
    // 确保父目录存在
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| StoreError::Database(format!("Failed to create db directory: {}", e)))?;
    }

    use sqlx::sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    };

    // `:memory:` 必须用 `SqlitePool::connect(":memory:")` 方式连接
    // 以确保池中所有连接共享同一个内存数据库（`in_memory(true)` 会创建独立连接）
    let is_memory = db_path.to_string_lossy() == ":memory:";
    if is_memory {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .map_err(|e| StoreError::Database(format!("Failed to connect to SQLite: {}", e)))?;
        // 内存库不需要 PRAGMA 优化
        return Ok(pool);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        match tokio::fs::metadata(db_path).await {
            Ok(metadata) => {
                if !metadata.is_file() {
                    return Err(StoreError::Database(
                        "SQLite database path is not a regular file".into(),
                    ));
                }
                tokio::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))
                    .await
                    .map_err(|error| {
                        StoreError::Database(format!(
                            "Failed to secure existing SQLite database: {error}"
                        ))
                    })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(db_path)
                    .map_err(|error| {
                        StoreError::Database(format!(
                            "Failed to securely create SQLite database: {error}"
                        ))
                    })?;
            }
            Err(error) => {
                return Err(StoreError::Database(format!(
                    "Failed to inspect SQLite database: {error}"
                )));
            }
        }
    }

    let connect_opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(!cfg!(unix))
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_opts)
        .await
        .map_err(|e| StoreError::Database(format!("Failed to connect to SQLite: {}", e)))?;

    // 优化 SQLite 性能
    // 注意：auto_vacuum 必须在 journal_mode=WAL 之前设置，否则
    // 在已存在的数据库上（WAL 模式创建了数据库文件后）设置
    // auto_vacuum 会被静默忽略，导致 incremental_vacuum 成为空操作。
    sqlx::query("PRAGMA auto_vacuum=INCREMENTAL")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA busy_timeout=5000")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA synchronous=FULL")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .ok();

    Ok(pool)
}

/// 创建第二个 SQLite 连接池（指向同一数据库，用于读写分离）
///
/// 与 `create_pool` 创建的池共享同一个 SQLite 数据库文件。
/// 两池间通过 WAL 模式的并发读写能力协同工作——写入不阻塞读取。
pub async fn create_shared_pool(db_path: &std::path::Path) -> Result<SqlitePool, StoreError> {
    if tokio::fs::symlink_metadata(db_path)
        .await
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(StoreError::Database(
            "Refusing to open SQLite database through a symbolic link".into(),
        ));
    }
    use sqlx::sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    };

    let is_memory = db_path.to_string_lossy() == ":memory:";
    if is_memory {
        return SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .map_err(|e| StoreError::Database(format!("Failed to connect to SQLite: {}", e)));
    }

    let connect_opts = SqliteConnectOptions::new()
        .filename(db_path)
        .read_only(false)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_opts)
        .await
        .map_err(|e| StoreError::Database(format!("Failed to connect secondary pool: {}", e)))?;

    Ok(pool)
}

// ── Session 行类型 ──

/// 会话行（用于 sqlx 反序列化）
struct SessionRow {
    key: String,
    platform: String,
    chat_id: String,
    thread_id: Option<String>,
    created_at: i64,
    updated_at: i64,
    source_json: String,
    reset_policy: String,
    metadata: String,
    last_message: Option<String>,
    last_message_at: Option<i64>,
}

impl SessionRow {
    fn into_session(self) -> Result<Session, StoreError> {
        let source: SessionSource = serde_json::from_str(&self.source_json)?;
        let metadata: serde_json::Value =
            serde_json::from_str(&self.metadata).unwrap_or(serde_json::json!({}));
        let reset_policy = match self.reset_policy.as_str() {
            "Never" => ResetPolicy::Never,
            "After1h" => ResetPolicy::After1h,
            "After24h" => ResetPolicy::After24h,
            "After50Msgs" => ResetPolicy::After50Msgs,
            "Daily" => ResetPolicy::Daily,
            "Manual" => ResetPolicy::Manual,
            _ => ResetPolicy::Never,
        };

        Ok(Session {
            key: self.key,
            platform: self.platform,
            chat_id: self.chat_id,
            thread_id: self.thread_id,
            created_at: self.created_at,
            updated_at: self.updated_at,
            source,
            reset_policy,
            metadata,
            last_message: self.last_message,
            last_message_at: self.last_message_at,
        })
    }
}

/// 从 sqlx Row 手动反序列化 SessionRow
fn row_to_session(row: &sqlx::sqlite::SqliteRow) -> Result<SessionRow, sqlx::Error> {
    use sqlx::Row as _;
    Ok(SessionRow {
        key: row.try_get("key")?,
        platform: row.try_get("platform")?,
        chat_id: row.try_get("chat_id")?,
        thread_id: row.try_get("thread_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        source_json: row.try_get("source_json")?,
        reset_policy: row.try_get("reset_policy")?,
        metadata: row.try_get("metadata")?,
        last_message: row.try_get("last_message")?,
        last_message_at: row.try_get("last_message_at")?,
    })
}

// ── SqliteSessionStore ──

/// SQLite 会话存储
pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    /// 创建新的 SQLite 会话存储
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn upsert_session(&self, session: &Session) -> Result<(), StoreError> {
        let source_json = serde_json::to_string(&session.source)?;
        let metadata = serde_json::to_string(&session.metadata)?;
        let reset_policy = format!("{:?}", session.reset_policy);

        sqlx::query(
            "INSERT INTO sessions (key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata, last_message, last_message_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET
                updated_at = excluded.updated_at,
                source_json = excluded.source_json,
                reset_policy = excluded.reset_policy,
                metadata = excluded.metadata,
                last_message = excluded.last_message,
                last_message_at = excluded.last_message_at"
        )
        .bind(&session.key)
        .bind(&session.platform)
        .bind(&session.chat_id)
        .bind(&session.thread_id)
        .bind(session.created_at)
        .bind(session.updated_at)
        .bind(&source_json)
        .bind(&reset_policy)
        .bind(&metadata)
        .bind(&session.last_message)
        .bind(session.last_message_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_session(&self, key: &str) -> Result<Option<Session>, StoreError> {
        let row = sqlx::query(
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata, last_message, last_message_at
             FROM sessions WHERE key = ?"
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(ref r) => {
                let s = row_to_session(r)?;
                Ok(Some(s.into_session()?))
            }
            None => Ok(None),
        }
    }

    async fn delete_session(&self, key: &str) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE key = ?")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>, StoreError> {
        let mut builder = sqlx::QueryBuilder::new(
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata, last_message, last_message_at \
             FROM sessions WHERE 1=1",
        );

        if let Some(ref platform) = filter.platform {
            builder.push(" AND platform = ").push_bind(platform);
        }
        builder.push(" ORDER BY updated_at DESC");

        if let Some(limit) = filter.limit {
            builder.push(" LIMIT ").push_bind(limit as i64);
        }
        if let Some(offset) = filter.offset {
            builder.push(" OFFSET ").push_bind(offset as i64);
        }

        let query = builder.build();
        let rows = query.fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                let s = row_to_session(row)?;
                s.into_session()
                    .map_err(|e| sqlx::Error::Protocol(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    async fn count_sessions(&self) -> Result<i64, StoreError> {
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    async fn delete_expired_sessions(&self, before: i64) -> Result<u64, StoreError> {
        // 分批删除，避免单条 DELETE 锁定表太久导致慢查询
        let mut total = 0u64;
        const CHUNK: i64 = 500;
        loop {
            let result = sqlx::query(
                "DELETE FROM sessions WHERE rowid IN (SELECT rowid FROM sessions WHERE updated_at < ? LIMIT ?)",
            )
            .bind(before)
            .bind(CHUNK)
            .execute(&self.pool)
            .await?;
            let affected = result.rows_affected();
            total += affected;
            if affected < CHUNK as u64 {
                break;
            }
        }
        Ok(total)
    }

    async fn load_all_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let rows = sqlx::query(
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata, last_message, last_message_at
             FROM sessions"
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|row| {
                let s = row_to_session(row)?;
                s.into_session()
                    .map_err(|e| sqlx::Error::Protocol(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }
}

// ── 消息行类型 ──

/// 消息行（用于 sqlx 反序列化）
struct MessageRow {
    id: String,
    session_key: String,
    platform: String,
    chat_id: String,
    role: String,
    text: Option<String>,
    raw_data: String,
    timestamp: i64,
    created_at: i64,
}

fn row_to_stored_message(row: &sqlx::sqlite::SqliteRow) -> Result<MessageRow, sqlx::Error> {
    use sqlx::Row as _;
    Ok(MessageRow {
        id: row.try_get("id")?,
        session_key: row.try_get("session_key")?,
        platform: row.try_get("platform")?,
        chat_id: row.try_get("chat_id")?,
        role: row.try_get("role")?,
        text: row.try_get("text")?,
        raw_data: row.try_get("raw_data")?,
        timestamp: row.try_get("timestamp")?,
        created_at: row.try_get("created_at")?,
    })
}

impl MessageRow {
    fn into_stored(self) -> Result<StoredMessage, StoreError> {
        let role = match self.role.as_str() {
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            _ => MessageRole::Assistant,
        };
        let raw_data: serde_json::Value = serde_json::from_str(&self.raw_data)?;

        Ok(StoredMessage {
            id: self.id,
            session_key: self.session_key,
            platform: self.platform,
            chat_id: self.chat_id,
            role,
            text: self.text,
            raw_data,
            timestamp: self.timestamp,
            created_at: self.created_at,
        })
    }
}

// ── SqliteMessageStore ──

/// SQLite 消息存储
pub struct SqliteMessageStore {
    pool: SqlitePool,
}

impl SqliteMessageStore {
    /// 创建新的 SQLite 消息存储
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MessageStore for SqliteMessageStore {
    async fn prepare_outbound_delivery(
        &self,
        delivery: &OutboundDelivery,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO outbound_deliveries
             (id, actor_id, idempotency_key, platform, chat_id, request_json, state, created_at)
             VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)",
        )
        .bind(&delivery.id)
        .bind(&delivery.actor_id)
        .bind(&delivery.idempotency_key)
        .bind(&delivery.platform)
        .bind(&delivery.chat_id)
        .bind(serde_json::to_string(&delivery.request_json)?)
        .bind(delivery.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn finalize_outbound_delivery(
        &self,
        delivery_id: &str,
        state: OutboundDeliveryState,
        result: &serde_json::Value,
        message: Option<&StoredMessage>,
    ) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        if let Some(msg) = message {
            let role = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
            };
            sqlx::query(
                "INSERT OR IGNORE INTO messages
                 (id, session_key, platform, chat_id, role, text, raw_data, timestamp, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&msg.id)
            .bind(&msg.session_key)
            .bind(&msg.platform)
            .bind(&msg.chat_id)
            .bind(role)
            .bind(&msg.text)
            .bind(serde_json::to_string(&msg.raw_data)?)
            .bind(msg.timestamp)
            .bind(msg.created_at)
            .execute(&mut *tx)
            .await?;
        }
        let state = match state {
            OutboundDeliveryState::Succeeded => "succeeded",
            OutboundDeliveryState::Failed => "failed",
        };
        let updated = sqlx::query(
            "UPDATE outbound_deliveries SET state = ?, result_json = ?, completed_at = ?
             WHERE id = ? AND state = 'pending'",
        )
        .bind(state)
        .bind(serde_json::to_string(result)?)
        .bind(chrono::Utc::now().timestamp_millis())
        .bind(delivery_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::Database(
                "outbound delivery is missing or already finalized".into(),
            ));
        }
        tx.commit().await?;
        Ok(())
    }

    async fn unpublished_outbound_events(
        &self,
        limit: usize,
    ) -> Result<Vec<OutboundEvent>, StoreError> {
        let rows: Vec<(String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT id, platform, chat_id, state, result_json, completed_at
             FROM outbound_deliveries WHERE state != 'pending' AND event_published = 0
             ORDER BY completed_at, id LIMIT ?",
        )
        .bind(limit.min(1000) as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(delivery_id, platform, chat_id, state, result_json, completed_at)| {
                    Ok(OutboundEvent {
                        delivery_id,
                        platform,
                        chat_id,
                        state: if state == "succeeded" {
                            OutboundDeliveryState::Succeeded
                        } else {
                            OutboundDeliveryState::Failed
                        },
                        result_json: serde_json::from_str(&result_json)?,
                        completed_at,
                    })
                },
            )
            .collect()
    }

    async fn mark_outbound_event_published(&self, delivery_id: &str) -> Result<(), StoreError> {
        let updated = sqlx::query(
            "UPDATE outbound_deliveries SET event_published = 1
             WHERE id = ? AND state != 'pending'",
        )
        .bind(delivery_id)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::NotFound(format!(
                "completed outbound delivery {delivery_id}"
            )));
        }
        Ok(())
    }

    async fn list_outbound_deliveries(
        &self,
        actor_id: &str,
        limit: usize,
    ) -> Result<Vec<OutboundDeliveryRecord>, StoreError> {
        type Row = (
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
            Option<String>,
            i64,
            Option<i64>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, actor_id, idempotency_key, platform, chat_id, request_json, state,
                    result_json, created_at, completed_at, reconciliation_evidence, reconciled_by
             FROM outbound_deliveries WHERE actor_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(actor_id)
        .bind(limit.clamp(1, 200) as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(OutboundDeliveryRecord {
                    id: row.0,
                    actor_id: row.1,
                    idempotency_key: row.2,
                    platform: row.3,
                    chat_id: row.4,
                    request_json: serde_json::from_str(&row.5)?,
                    state: row.6,
                    result_json: row
                        .7
                        .map(|value| serde_json::from_str(&value))
                        .transpose()?,
                    created_at: row.8,
                    completed_at: row.9,
                    reconciliation_evidence: row.10,
                    reconciled_by: row.11,
                })
            })
            .collect()
    }

    async fn list_outbound_deliveries_by_session(
        &self,
        platform: &str,
        chat_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<OutboundDeliveryRecord>, StoreError> {
        type Row = (
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
            Option<String>,
            i64,
            Option<i64>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, actor_id, idempotency_key, platform, chat_id, request_json, state,
                    result_json, created_at, completed_at, reconciliation_evidence, reconciled_by
             FROM outbound_deliveries WHERE platform = ? AND chat_id = ?
             ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(platform)
        .bind(chat_id)
        .bind(limit.clamp(1, 1001) as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(OutboundDeliveryRecord {
                    id: row.0,
                    actor_id: row.1,
                    idempotency_key: row.2,
                    platform: row.3,
                    chat_id: row.4,
                    request_json: serde_json::from_str(&row.5)?,
                    state: row.6,
                    result_json: row
                        .7
                        .map(|value| serde_json::from_str(&value))
                        .transpose()?,
                    created_at: row.8,
                    completed_at: row.9,
                    reconciliation_evidence: row.10,
                    reconciled_by: row.11,
                })
            })
            .collect()
    }

    async fn delete_outbound_deliveries_by_session(
        &self,
        platform: &str,
        chat_id: &str,
    ) -> Result<u64, StoreError> {
        Ok(
            sqlx::query("DELETE FROM outbound_deliveries WHERE platform = ? AND chat_id = ?")
                .bind(platform)
                .bind(chat_id)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }

    async fn delete_expired_outbound_deliveries(&self, before: i64) -> Result<u64, StoreError> {
        Ok(
            sqlx::query("DELETE FROM outbound_deliveries WHERE created_at < ?")
                .bind(before)
                .execute(&self.pool)
                .await?
                .rows_affected(),
        )
    }

    async fn outbound_delivery_stats(
        &self,
        stale_before: i64,
    ) -> Result<OutboundDeliveryStats, StoreError> {
        let (pending, stale_pending, unpublished): (i64, i64, i64) = sqlx::query_as(
            "SELECT
                COALESCE(SUM(CASE WHEN state = 'pending' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN state = 'pending' AND created_at < ? THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN state != 'pending' AND event_published = 0 THEN 1 ELSE 0 END), 0)
             FROM outbound_deliveries",
        )
        .bind(stale_before)
        .fetch_one(&self.pool)
        .await?;
        Ok(OutboundDeliveryStats {
            pending: pending as u64,
            stale_pending: stale_pending as u64,
            unpublished_events: unpublished as u64,
        })
    }

    async fn reconcile_outbound_delivery(
        &self,
        delivery_id: &str,
        actor_id: &str,
        state: OutboundDeliveryState,
        evidence: &str,
        reconciled_by: &str,
    ) -> Result<bool, StoreError> {
        let state = match state {
            OutboundDeliveryState::Succeeded => "succeeded",
            OutboundDeliveryState::Failed => "failed",
        };
        let now = chrono::Utc::now().timestamp_millis();
        let result = sqlx::query(
            "UPDATE outbound_deliveries
             SET state = ?, result_json = ?, completed_at = ?, reconciliation_evidence = ?,
                 reconciled_by = ?, event_published = 0
             WHERE id = ? AND actor_id = ? AND state = 'pending'",
        )
        .bind(state)
        .bind(serde_json::to_string(&serde_json::json!({
            "manually_reconciled": true,
            "state": state,
        }))?)
        .bind(now)
        .bind(evidence)
        .bind(reconciled_by)
        .bind(delivery_id)
        .bind(actor_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn store_message(&self, msg: &StoredMessage) -> Result<(), StoreError> {
        let role_str = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };
        let raw_json = serde_json::to_string(&msg.raw_data)?;

        sqlx::query(
            "INSERT OR IGNORE INTO messages (id, session_key, platform, chat_id, role, text, raw_data, timestamp, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&msg.id)
        .bind(&msg.session_key)
        .bind(&msg.platform)
        .bind(&msg.chat_id)
        .bind(role_str)
        .bind(&msg.text)
        .bind(&raw_json)
        .bind(msg.timestamp)
        .bind(msg.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn store_messages(&self, msgs: &[StoredMessage]) -> Result<(), StoreError> {
        // 使用事务包装批量写入，减少单条提交开销和 WAL 写入放大
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::Database(format!("Failed to begin transaction: {}", e)))?;
        for msg in msgs {
            let role_str = match msg.role {
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
            };
            let raw_json = serde_json::to_string(&msg.raw_data)?;

            sqlx::query(
                "INSERT OR IGNORE INTO messages (id, session_key, platform, chat_id, role, text, raw_data, timestamp, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&msg.id)
            .bind(&msg.session_key)
            .bind(&msg.platform)
            .bind(&msg.chat_id)
            .bind(role_str)
            .bind(&msg.text)
            .bind(&raw_json)
            .bind(msg.timestamp)
            .bind(msg.created_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .map_err(|e| StoreError::Database(format!("Failed to commit batch insert: {}", e)))?;
        Ok(())
    }

    async fn list_messages(
        &self,
        filter: &MessageFilter,
    ) -> Result<Vec<StoredMessage>, StoreError> {
        let mut builder = sqlx::QueryBuilder::new(
            "SELECT id, session_key, platform, chat_id, role, text, raw_data, timestamp, created_at \
             FROM messages WHERE 1=1",
        );

        if let Some(ref key) = filter.session_key {
            builder.push(" AND session_key = ").push_bind(key);
        }
        if let Some(ref platform) = filter.platform {
            builder.push(" AND platform = ").push_bind(platform);
        }
        if let Some(ref chat_id) = filter.chat_id {
            builder.push(" AND chat_id = ").push_bind(chat_id);
        }
        if let Some(before) = filter.before {
            builder.push(" AND timestamp < ").push_bind(before);
        }

        builder.push(" ORDER BY timestamp DESC");

        if let Some(limit) = filter.limit {
            builder.push(" LIMIT ").push_bind(limit as i64);
        }
        if let Some(offset) = filter.offset {
            builder.push(" OFFSET ").push_bind(offset as i64);
        }

        let query = builder.build();
        let rows = query.fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                let r = row_to_stored_message(row)?;
                r.into_stored()
                    .map_err(|e| sqlx::Error::Protocol(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    async fn delete_message(&self, id: &str) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM messages WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_messages_by_session(&self, session_key: &str) -> Result<u64, StoreError> {
        let result = sqlx::query("DELETE FROM messages WHERE session_key = ?")
            .bind(session_key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn delete_expired_messages(&self, before: i64) -> Result<u64, StoreError> {
        // 分批删除，避免单条 DELETE 锁定表太久导致慢查询
        let mut total = 0u64;
        const CHUNK: i64 = 500;
        loop {
            let result = sqlx::query(
                "DELETE FROM messages WHERE rowid IN (SELECT rowid FROM messages WHERE created_at < ? LIMIT ?)",
            )
            .bind(before)
            .bind(CHUNK)
            .execute(&self.pool)
            .await?;
            let affected = result.rows_affected();
            total += affected;
            if affected < CHUNK as u64 {
                break;
            }
        }
        Ok(total)
    }
}

// ── 辅助函数（用于外部代码构建存储消息） ──

/// 从入站消息构建存储消息并持久化
pub async fn persist_inbound_message(
    store: &dyn MessageStore,
    msg: &InboundMessage,
) -> Result<(), StoreError> {
    let stored = StoredMessage::from_inbound(msg);
    store.store_message(&stored).await
}

/// 从出站发送结果构建存储消息并持久化
pub async fn persist_outbound_message(
    store: &dyn MessageStore,
    platform: &str,
    chat_id: &str,
    text: &str,
    result: &SendResult,
) -> Result<(), StoreError> {
    let stored = StoredMessage::from_outbound(platform, chat_id, None, text, result);
    store.store_message(&stored).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::{ChatType, MessageSender, MessageType};
    use crate::types::session::{ResetPolicy, SessionSource};

    fn make_test_session(key: &str, platform: &str, chat_id: &str) -> Session {
        Session {
            key: key.to_string(),
            platform: platform.to_string(),
            chat_id: chat_id.to_string(),
            thread_id: None,
            created_at: 1000,
            updated_at: 1000,
            source: SessionSource {
                platform: platform.to_string(),
                chat_id: chat_id.to_string(),
                chat_name: None,
                chat_type: ChatType::Dm,
                user_id: None,
                user_name: None,
                is_bot: false,
                user_username: None,
                user_role: None,
            },
            reset_policy: ResetPolicy::Never,
            metadata: serde_json::json!({}),
            last_message: None,
            last_message_at: None,
        }
    }

    async fn create_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    // ── SessionStore 测试 ──

    #[tokio::test]
    async fn test_session_upsert_and_get() {
        let pool = create_test_pool().await;
        let store = SqliteSessionStore::new(pool);

        let session = make_test_session("tg:1", "telegram", "1");
        store.upsert_session(&session).await.unwrap();

        let loaded = store.get_session("tg:1").await.unwrap().unwrap();
        assert_eq!(loaded.key, "tg:1");
        assert_eq!(loaded.platform, "telegram");
        assert_eq!(loaded.chat_id, "1");
    }

    #[tokio::test]
    async fn test_session_get_nonexistent() {
        let pool = create_test_pool().await;
        let store = SqliteSessionStore::new(pool);

        let result = store.get_session("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_session_delete() {
        let pool = create_test_pool().await;
        let store = SqliteSessionStore::new(pool);

        store
            .upsert_session(&make_test_session("tg:1", "telegram", "1"))
            .await
            .unwrap();
        assert!(store.delete_session("tg:1").await.unwrap());
        assert!(!store.delete_session("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_session_load_all() {
        let pool = create_test_pool().await;
        let store = SqliteSessionStore::new(pool);

        store
            .upsert_session(&make_test_session("a:1", "telegram", "1"))
            .await
            .unwrap();
        store
            .upsert_session(&make_test_session("b:2", "discord", "2"))
            .await
            .unwrap();

        let all = store.load_all_sessions().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_session_list_filter() {
        let pool = create_test_pool().await;
        let store = SqliteSessionStore::new(pool);

        store
            .upsert_session(&make_test_session("tg:1", "telegram", "1"))
            .await
            .unwrap();
        store
            .upsert_session(&make_test_session("dc:2", "discord", "2"))
            .await
            .unwrap();

        let filter = SessionFilter {
            platform: Some("telegram".to_string()),
            active_within_minutes: None,
            limit: None,
            offset: None,
        };
        let list = store.list_sessions(&filter).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].platform, "telegram");
    }

    #[tokio::test]
    async fn test_session_upsert_preserves_created_at() {
        let pool = create_test_pool().await;
        let store = SqliteSessionStore::new(pool);

        let mut session = make_test_session("tg:1", "telegram", "1");
        session.created_at = 100;
        session.updated_at = 100;
        store.upsert_session(&session).await.unwrap();

        // 第二次 upsert 只更新 updated_at
        let mut updated = session.clone();
        updated.updated_at = 200;
        store.upsert_session(&updated).await.unwrap();

        let loaded = store.get_session("tg:1").await.unwrap().unwrap();
        assert_eq!(loaded.created_at, 100, "created_at should not change");
        assert_eq!(loaded.updated_at, 200, "updated_at should be updated");
    }

    // ── MessageStore 测试 ──

    fn make_test_inbound() -> InboundMessage {
        InboundMessage {
            id: "msg1".to_string(),
            platform: "telegram".to_string().into(),
            msg_type: MessageType::Text,
            text: Some("Hello".to_string()),
            sender: MessageSender {
                id: "user1".to_string(),
                name: Some("User".to_string()),
                username: None,
                avatar_url: None,
                is_bot: false,
                role: None,
                language_code: None,
            },
            recipient: None,
            chat_id: "123".to_string(),
            chat_name: None,
            chat_type: ChatType::Dm,
            guild_id: None,
            thread_id: None,
            root_id: None,
            timestamp: 1000000,
            media: None,
            command: None,
            callback: None,
            reply_to: None,
            mentions: None,
            mentioned: None,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn test_message_store_and_list() {
        let pool = create_test_pool().await;
        let store = SqliteMessageStore::new(pool);

        let inbound = make_test_inbound();
        let stored = StoredMessage::from_inbound(&inbound);
        store.store_message(&stored).await.unwrap();

        let filter = MessageFilter {
            session_key: Some("telegram:123".to_string()),
            platform: None,
            chat_id: None,
            limit: Some(10),
            offset: None,
            before: None,
        };
        let msgs = store.list_messages(&filter).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text.as_deref(), Some("Hello"));
        assert_eq!(msgs[0].role, MessageRole::User);
    }

    #[tokio::test]
    async fn outbound_delivery_finalization_is_atomic_with_message_history() {
        let pool = create_test_pool().await;
        let store = SqliteMessageStore::new(pool.clone());
        let delivery = OutboundDelivery {
            id: "delivery-1".into(),
            actor_id: "customer-key".into(),
            idempotency_key: Some("request-123".into()),
            platform: "telegram".into(),
            chat_id: "123".into(),
            request_json: serde_json::json!({"text": "hello"}),
            created_at: 1_000_000,
        };
        store.prepare_outbound_delivery(&delivery).await.unwrap();

        let send_result = SendResult {
            success: true,
            message_id: Some("platform-message-1".into()),
            timestamp: Some(1_000_001),
            error: None,
            error_code: None,
            retryable: false,
        };
        let message = StoredMessage::from_outbound("telegram", "123", None, "hello", &send_result);
        store
            .finalize_outbound_delivery(
                &delivery.id,
                OutboundDeliveryState::Succeeded,
                &serde_json::to_value(&send_result).unwrap(),
                Some(&message),
            )
            .await
            .unwrap();

        let state: String =
            sqlx::query_scalar("SELECT state FROM outbound_deliveries WHERE id = ?")
                .bind(&delivery.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let message_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id = ?")
            .bind(&message.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(state, "succeeded");
        assert_eq!(message_count, 1);

        let unpublished = store.unpublished_outbound_events(10).await.unwrap();
        assert_eq!(unpublished.len(), 1);
        assert_eq!(unpublished[0].delivery_id, delivery.id);
        assert_eq!(unpublished[0].state, OutboundDeliveryState::Succeeded);
        store
            .mark_outbound_event_published(&delivery.id)
            .await
            .unwrap();
        assert!(
            store
                .unpublished_outbound_events(10)
                .await
                .unwrap()
                .is_empty()
        );

        let duplicate = store
            .finalize_outbound_delivery(
                &delivery.id,
                OutboundDeliveryState::Succeeded,
                &serde_json::json!({}),
                Some(&message),
            )
            .await;
        assert!(duplicate.is_err(), "a delivery can only be finalized once");

        let pending = OutboundDelivery {
            id: "delivery-pending".into(),
            actor_id: "customer-key".into(),
            idempotency_key: None,
            platform: "telegram".into(),
            chat_id: "456".into(),
            request_json: serde_json::json!({"text": "uncertain"}),
            created_at: 2_000_000,
        };
        store.prepare_outbound_delivery(&pending).await.unwrap();
        let stats = store.outbound_delivery_stats(3_000_000).await.unwrap();
        assert_eq!(stats.pending, 1);
        assert_eq!(stats.stale_pending, 1);
        assert!(
            store
                .list_outbound_deliveries("another-customer", 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !store
                .reconcile_outbound_delivery(
                    &pending.id,
                    "another-customer",
                    OutboundDeliveryState::Succeeded,
                    "platform search found message 123",
                    "another-customer",
                )
                .await
                .unwrap()
        );
        assert!(
            store
                .reconcile_outbound_delivery(
                    &pending.id,
                    "customer-key",
                    OutboundDeliveryState::Succeeded,
                    "platform search found message 123",
                    "customer-key",
                )
                .await
                .unwrap()
        );
        assert!(
            !store
                .reconcile_outbound_delivery(
                    &pending.id,
                    "customer-key",
                    OutboundDeliveryState::Failed,
                    "second finalization must be rejected",
                    "customer-key",
                )
                .await
                .unwrap()
        );
        let records = store
            .list_outbound_deliveries("customer-key", 10)
            .await
            .unwrap();
        let reconciled = records.iter().find(|row| row.id == pending.id).unwrap();
        assert_eq!(reconciled.state, "succeeded");
        assert_eq!(
            reconciled.reconciliation_evidence.as_deref(),
            Some("platform search found message 123")
        );
        let stats = store.outbound_delivery_stats(3_000_000).await.unwrap();
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.stale_pending, 0);
        assert_eq!(stats.unpublished_events, 1);
    }

    #[tokio::test]
    async fn test_message_store_multiple() {
        let pool = create_test_pool().await;
        let store = SqliteMessageStore::new(pool);

        for i in 0..5 {
            let mut inbound = make_test_inbound();
            inbound.id = format!("msg{}", i);
            inbound.text = Some(format!("Message {}", i));
            inbound.timestamp = 1000000 + i;
            let stored = StoredMessage::from_inbound(&inbound);
            store.store_message(&stored).await.unwrap();
        }

        let filter = MessageFilter {
            session_key: Some("telegram:123".to_string()),
            platform: None,
            chat_id: None,
            limit: Some(3),
            offset: None,
            before: None,
        };
        let msgs = store.list_messages(&filter).await.unwrap();
        assert_eq!(msgs.len(), 3);
        // Should be newest first (timestamp desc)
        assert_eq!(msgs[0].text.as_deref(), Some("Message 4"));
    }

    #[tokio::test]
    async fn test_message_delete() {
        let pool = create_test_pool().await;
        let store = SqliteMessageStore::new(pool);

        let stored = StoredMessage::from_inbound(&make_test_inbound());
        store.store_message(&stored).await.unwrap();

        assert!(store.delete_message(&stored.id).await.unwrap());
        assert!(!store.delete_message("nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn test_inbound_to_stored_message() {
        let inbound = make_test_inbound();
        let stored = StoredMessage::from_inbound(&inbound);

        assert_eq!(stored.role, MessageRole::User);
        assert_eq!(stored.session_key, "telegram:123");
        assert_eq!(stored.platform, "telegram");
        assert_eq!(stored.chat_id, "123");
        assert!(stored.id.starts_with("inbound:"));
    }

    #[tokio::test]
    async fn test_outbound_to_stored_message() {
        let result = SendResult::ok("out_msg_1".to_string());
        let stored = StoredMessage::from_outbound("telegram", "123", None, "Reply", &result);

        assert_eq!(stored.role, MessageRole::Assistant);
        assert_eq!(stored.session_key, "telegram:123");
        assert!(stored.id.starts_with("outbound:"));
        assert_eq!(stored.text.as_deref(), Some("Reply"));
    }

    #[test]
    fn outbound_local_ids_are_unique_when_platform_reuses_message_id() {
        let result = SendResult {
            success: true,
            message_id: Some("platform-shared-id".into()),
            timestamp: Some(1),
            error: None,
            error_code: None,
            retryable: false,
        };
        let first = StoredMessage::from_outbound("telegram", "chat-1", None, "one", &result);
        let second = StoredMessage::from_outbound("telegram", "chat-2", None, "two", &result);
        assert_ne!(first.id, second.id);
        assert_eq!(first.raw_data["result"]["message_id"], "platform-shared-id");
        assert_eq!(
            second.raw_data["result"]["message_id"],
            "platform-shared-id"
        );
    }

    #[tokio::test]
    async fn test_field_specific_query() {
        let pool = create_test_pool().await;
        let store = SqliteMessageStore::new(pool);

        // Store messages for two different chats
        let mut msg1 = make_test_inbound();
        msg1.chat_id = "111".to_string();
        msg1.text = Some("Chat 111 msg".to_string());
        store
            .store_message(&StoredMessage::from_inbound(&msg1))
            .await
            .unwrap();

        let mut msg2 = make_test_inbound();
        msg2.chat_id = "222".to_string();
        msg2.text = Some("Chat 222 msg".to_string());
        store
            .store_message(&StoredMessage::from_inbound(&msg2))
            .await
            .unwrap();

        // Filter by chat_id
        let filter = MessageFilter {
            session_key: None,
            platform: Some("telegram".to_string()),
            chat_id: Some("111".to_string()),
            limit: Some(10),
            offset: None,
            before: None,
        };
        let msgs = store.list_messages(&filter).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text.as_deref(), Some("Chat 111 msg"));
    }

    #[tokio::test]
    async fn test_migration_adds_api_key_quota_to_existing_database() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE api_keys (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, prefix TEXT NOT NULL,
                created_at INTEGER NOT NULL, expires_at INTEGER, last_used_at INTEGER,
                revoked INTEGER NOT NULL DEFAULT 0, permissions TEXT NOT NULL DEFAULT '[]',
                event_filters TEXT NOT NULL DEFAULT '[]', hash TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE api_usage_hourly (
                key_id TEXT NOT NULL, bucket_start INTEGER NOT NULL,
                status_class INTEGER NOT NULL, request_count INTEGER NOT NULL,
                PRIMARY KEY (key_id, bucket_start, status_class)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO api_usage_hourly (key_id, bucket_start, status_class, request_count)
             VALUES ('legacy-key', 0, 2, 7)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO api_keys
             (id, name, prefix, created_at, revoked, permissions, event_filters, hash)
             VALUES ('legacy-key', 'legacy', 'eb_old', 1, 0, '[]', '[]', 'hash')",
        )
        .execute(&pool)
        .await
        .unwrap();

        run_migrations(&pool).await.unwrap();
        run_migrations(&pool).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
                .fetch_one(&pool)
                .await
                .unwrap(),
            crate::storage::migration::SCHEMA_VERSION
        );
        use sqlx::Row as _;
        let columns = sqlx::query("PRAGMA table_info(api_keys)")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(
            columns
                .iter()
                .any(|row| { row.get::<String, _>("name") == "requests_per_minute" })
        );
        let subject_id: String =
            sqlx::query_scalar("SELECT subject_id FROM api_keys WHERE id = 'legacy-key'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(subject_id, "legacy-key");
        let usage_subject: String = sqlx::query_scalar(
            "SELECT subject_id FROM api_usage_hourly WHERE key_id = 'legacy-key'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(usage_subject, "legacy-key");
        let delivery_columns = sqlx::query_scalar::<_, String>(
            "SELECT name FROM pragma_table_info('outbound_deliveries')",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        for expected in [
            "event_published",
            "reconciliation_evidence",
            "reconciled_by",
        ] {
            assert!(delivery_columns.iter().any(|column| column == expected));
        }
    }

    #[tokio::test]
    async fn failed_multi_step_migration_rolls_back_prior_schema_changes() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE api_keys (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, prefix TEXT NOT NULL,
                created_at INTEGER NOT NULL, expires_at INTEGER, last_used_at INTEGER,
                revoked INTEGER NOT NULL DEFAULT 0, permissions TEXT NOT NULL DEFAULT '[]',
                event_filters TEXT NOT NULL DEFAULT '[]', hash TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("CREATE VIEW api_usage_hourly AS SELECT 'blocked' AS key_id")
            .execute(&pool)
            .await
            .unwrap();

        assert!(run_migrations(&pool).await.is_err());
        let columns =
            sqlx::query_scalar::<_, String>("SELECT name FROM pragma_table_info('api_keys')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(!columns.iter().any(|column| column == "subject_id"));
        assert!(!columns.iter().any(|column| column == "requests_per_minute"));
    }

    #[tokio::test]
    async fn migration_refuses_database_from_newer_schema_version() {
        let pool = SqlitePool::connect(":memory:").await.unwrap();
        sqlx::query("CREATE TABLE future_data(value TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO future_data VALUES ('preserve-me')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE _schema_version (
                version INTEGER NOT NULL,
                applied_at INTEGER NOT NULL,
                description TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO _schema_version VALUES (999, 1, 'future schema')")
            .execute(&pool)
            .await
            .unwrap();

        let error = run_migrations(&pool).await.unwrap_err().to_string();
        assert!(error.contains("newer than supported"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
                .fetch_one(&pool)
                .await
                .unwrap(),
            999
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT value FROM future_data")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "preserve-me"
        );
    }

    #[tokio::test]
    async fn file_pool_applies_durable_security_pragmas_to_every_connection() {
        let path = std::env::temp_dir().join(format!("easybot-pool-{}.db", uuid::Uuid::new_v4()));
        let pool = create_pool(&path).await.unwrap();
        let mut connections = Vec::new();
        for _ in 0..5 {
            connections.push(pool.acquire().await.unwrap());
        }
        for connection in &mut connections {
            let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            assert_eq!(foreign_keys, 1);
            assert_eq!(busy_timeout, 5_000);
            assert_eq!(synchronous, 2);
        }
        sqlx::query("CREATE TABLE permission_probe(value TEXT)")
            .execute(&mut **connections.first_mut().unwrap())
            .await
            .unwrap();
        sqlx::query("INSERT INTO permission_probe VALUES ('secret')")
            .execute(&mut **connections.first_mut().unwrap())
            .await
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for protected_path in [
                path.clone(),
                std::path::PathBuf::from(format!("{}-wal", path.display())),
                std::path::PathBuf::from(format!("{}-shm", path.display())),
            ] {
                assert_eq!(
                    std::fs::metadata(&protected_path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600,
                    "{}",
                    protected_path.display()
                );
            }
        }
        drop(connections);
        pool.close().await;
        for suffix in ["", "-wal", "-shm"] {
            let _ = tokio::fs::remove_file(format!("{}{suffix}", path.display())).await;
        }
    }
}
