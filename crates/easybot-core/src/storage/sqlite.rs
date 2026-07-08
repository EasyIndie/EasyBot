//! SQLite 存储后端实现
//!
//! 基于 sqlx 的 SQLite 持久化实现，提供 SessionStore 和 MessageStore。
//! 包含建表迁移和连接池初始化。

use async_trait::async_trait;
use sqlx::SqlitePool;

use super::{MessageFilter, MessageRole, MessageStore, SessionStore, StoreError, StoredMessage};
use crate::types::message::{InboundMessage, SendResult};
use crate::types::session::{ResetPolicy, Session, SessionFilter, SessionSource};

// ── Schema ──

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    key          TEXT PRIMARY KEY,
    platform     TEXT NOT NULL,
    chat_id      TEXT NOT NULL,
    thread_id    TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    source_json  TEXT NOT NULL,
    reset_policy TEXT NOT NULL,
    metadata     TEXT NOT NULL DEFAULT '{}',
    last_message TEXT,
    last_message_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_sessions_platform ON sessions(platform);
CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id           TEXT PRIMARY KEY,
    session_key  TEXT NOT NULL,
    platform     TEXT NOT NULL,
    chat_id      TEXT NOT NULL,
    role         TEXT NOT NULL,
    text         TEXT,
    raw_data     TEXT NOT NULL,
    timestamp    INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_sk ON messages(session_key, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_pc ON messages(platform, chat_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_ct ON messages(created_at);

CREATE TABLE IF NOT EXISTS api_keys (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    prefix        TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER,
    last_used_at  INTEGER,
    revoked       INTEGER NOT NULL DEFAULT 0,
    permissions   TEXT NOT NULL DEFAULT '[]',
    event_filters TEXT NOT NULL DEFAULT '[]',
    hash          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_keys_created ON api_keys(created_at DESC);
";

// ── 连接与迁移 ──

/// 创建 SQLite 连接池
///
/// 自动启用 WAL 模式、外键约束和忙超时。
/// 使用 `create_if_missing(true)` 确保数据库文件在不存在时自动创建。
pub async fn create_pool(db_path: &std::path::Path) -> Result<SqlitePool, StoreError> {
    // 确保父目录存在
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| StoreError::Database(format!("Failed to create db directory: {}", e)))?;
    }

    use sqlx::sqlite::SqliteConnectOptions;

    // `:memory:` 必须用 `SqlitePool::connect(":memory:")` 方式连接
    // 以确保池中所有连接共享同一个内存数据库（`in_memory(true)` 会创建独立连接）
    let is_memory = db_path.to_string_lossy() == ":memory:";
    if is_memory {
        let pool = SqlitePool::connect(":memory:")
            .await
            .map_err(|e| StoreError::Database(format!("Failed to connect to SQLite: {}", e)))?;
        // 内存库不需要 PRAGMA 优化
        return Ok(pool);
    }

    let connect_opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(connect_opts)
        .await
        .map_err(|e| StoreError::Database(format!("Failed to connect to SQLite: {}", e)))?;

    // 优化 SQLite 性能
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA busy_timeout=5000")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA synchronous=NORMAL")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .ok();

    Ok(pool)
}

/// 运行数据库迁移（幂等）
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), StoreError> {
    sqlx::query(SCHEMA_SQL).execute(pool).await?;
    Ok(())
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
}
