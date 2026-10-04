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
/// 对带版本记录的数据库执行版本化增量迁移。
/// 调用 `migration::run_migrations()` 逐版执行并追踪版本，
/// 返回本次实际执行的迁移列表（空 = 无待迁移）。
pub async fn run_migrations(
    pool: &SqlitePool,
) -> Result<Vec<crate::storage::migration::AppliedMigration>, StoreError> {
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
                // 收紧权限属于纵深防御，不应成为打开数据库的硬性前提。
                // Windows Docker Desktop 的 bind mount 等文件系统会拒绝对
                // 已存在文件执行 chmod 并返回 EPERM（os error 1），此时继续
                // 打开数据库并依赖底层文件系统 ACL，而不是退回到内存库
                // （否则会丢失持久化数据与 API key 认证）。
                if let Err(error) =
                    tokio::fs::set_permissions(db_path, std::fs::Permissions::from_mode(0o600))
                        .await
                {
                    tracing::warn!(
                        path = %db_path.display(),
                        %error,
                        "Could not tighten SQLite database permissions; continuing with existing filesystem permissions"
                    );
                }
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
    custom_name: Option<String>,
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
            custom_name: self.custom_name,
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
        custom_name: row.try_get("custom_name")?,
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
            "INSERT INTO sessions (key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata, last_message, last_message_at, custom_name)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET
                updated_at = excluded.updated_at,
                source_json = excluded.source_json,
                reset_policy = excluded.reset_policy,
                metadata = excluded.metadata,
                last_message = excluded.last_message,
                last_message_at = excluded.last_message_at,
                custom_name = excluded.custom_name"
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
        .bind(&session.custom_name)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_session(&self, key: &str) -> Result<Option<Session>, StoreError> {
        let row = sqlx::query(
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata, last_message, last_message_at, custom_name
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
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata, last_message, last_message_at, custom_name \
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
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata, last_message, last_message_at, custom_name
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
mod tests;
