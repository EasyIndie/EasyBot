//! PostgreSQL 存储后端实现
//!
//! 基于 sqlx 的 PostgreSQL 持久化实现，提供 SessionStore 和 MessageStore。
//! 与 SQLite 后端保持相同的 trait 接口和行映射模式。

use async_trait::async_trait;
use sqlx::PgPool;

use super::{MessageFilter, MessageRole, MessageStore, SessionStore, StoreError, StoredMessage};
use crate::types::session::{ResetPolicy, Session, SessionFilter, SessionSource};

// ── Schema ──

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    key          VARCHAR(255) PRIMARY KEY,
    platform     VARCHAR(64) NOT NULL,
    chat_id      VARCHAR(255) NOT NULL,
    thread_id    VARCHAR(255),
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    source_json  TEXT NOT NULL,
    reset_policy VARCHAR(32) NOT NULL,
    metadata     JSONB NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_sessions_platform ON sessions(platform);
CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id           VARCHAR(255) PRIMARY KEY,
    session_key  VARCHAR(255) NOT NULL,
    platform     VARCHAR(64) NOT NULL,
    chat_id      VARCHAR(255) NOT NULL,
    role         VARCHAR(16) NOT NULL,
    text         TEXT,
    raw_data     JSONB NOT NULL,
    timestamp    BIGINT NOT NULL,
    created_at   BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_sk ON messages(session_key, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_pc ON messages(platform, chat_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_ct ON messages(created_at);
";

// ── 连接与迁移 ──

/// 创建 PostgreSQL 连接池
pub async fn create_pool(
    connection_string: &str,
    max_connections: u32,
) -> Result<PgPool, StoreError> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(connection_string)
        .await
        .map_err(|e| StoreError::Database(format!("Failed to connect to PostgreSQL: {}", e)))?;

    Ok(pool)
}

/// 运行数据库迁移（幂等）
pub async fn run_migrations(pool: &PgPool) -> Result<(), StoreError> {
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
    metadata: serde_json::Value,
}

impl SessionRow {
    fn into_session(self) -> Result<Session, StoreError> {
        let source: SessionSource = serde_json::from_str(&self.source_json)?;
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
            metadata: self.metadata,
        })
    }
}

/// 从 sqlx Row 反序列化 SessionRow
fn row_to_session(row: &sqlx::postgres::PgRow) -> Result<SessionRow, sqlx::Error> {
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
    })
}

// ── PgSessionStore ──

/// PostgreSQL 会话存储
pub struct PgSessionStore {
    pool: PgPool,
}

impl PgSessionStore {
    /// 创建新的 PostgreSQL 会话存储
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SessionStore for PgSessionStore {
    async fn upsert_session(&self, session: &Session) -> Result<(), StoreError> {
        let source_json = serde_json::to_string(&session.source)?;
        let metadata = serde_json::to_value(&session.metadata)?;
        let reset_policy = format!("{:?}", session.reset_policy);

        sqlx::query(
            "INSERT INTO sessions (key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (key) DO UPDATE SET
                updated_at = EXCLUDED.updated_at,
                source_json = EXCLUDED.source_json,
                reset_policy = EXCLUDED.reset_policy,
                metadata = EXCLUDED.metadata"
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_session(&self, key: &str) -> Result<Option<Session>, StoreError> {
        let rows = sqlx::query(
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata
             FROM sessions WHERE key = $1"
        )
        .bind(key)
        .fetch_all(&self.pool)
        .await?;

        match rows.first() {
            Some(row) => {
                let s = row_to_session(row)?;
                Ok(Some(s.into_session()?))
            }
            None => Ok(None),
        }
    }

    async fn delete_session(&self, key: &str) -> Result<bool, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE key = $1")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_expired_sessions(&self, before: i64) -> Result<u64, StoreError> {
        let result = sqlx::query("DELETE FROM sessions WHERE updated_at < $1")
            .bind(before)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn list_sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>, StoreError> {
        let mut sql = String::from(
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata
             FROM sessions WHERE 1=1"
        );

        if filter.platform.is_some() {
            sql.push_str(" AND platform = $1");
        }
        sql.push_str(" ORDER BY updated_at DESC");

        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }
        if let Some(offset) = filter.offset {
            sql.push_str(&format!(" OFFSET {}", offset));
        }

        let mut query = sqlx::query(&sql);
        if let Some(ref platform) = filter.platform {
            query = query.bind(platform);
        }

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
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*)::bigint FROM sessions")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    async fn load_all_sessions(&self) -> Result<Vec<Session>, StoreError> {
        let rows = sqlx::query(
            "SELECT key, platform, chat_id, thread_id, created_at, updated_at, source_json, reset_policy, metadata
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
    raw_data: serde_json::Value,
    timestamp: i64,
    created_at: i64,
}

fn row_to_stored_message(row: &sqlx::postgres::PgRow) -> Result<MessageRow, sqlx::Error> {
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

        Ok(StoredMessage {
            id: self.id,
            session_key: self.session_key,
            platform: self.platform,
            chat_id: self.chat_id,
            role,
            text: self.text,
            raw_data: self.raw_data,
            timestamp: self.timestamp,
            created_at: self.created_at,
        })
    }
}

// ── PgMessageStore ──

/// PostgreSQL 消息存储
pub struct PgMessageStore {
    pool: PgPool,
}

impl PgMessageStore {
    /// 创建新的 PostgreSQL 消息存储
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MessageStore for PgMessageStore {
    async fn store_message(&self, msg: &StoredMessage) -> Result<(), StoreError> {
        let role_str = match msg.role {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        };
        let raw_data = serde_json::to_value(&msg.raw_data)?;

        sqlx::query(
            "INSERT INTO messages (id, session_key, platform, chat_id, role, text, raw_data, timestamp, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (id) DO NOTHING"
        )
        .bind(&msg.id)
        .bind(&msg.session_key)
        .bind(&msg.platform)
        .bind(&msg.chat_id)
        .bind(role_str)
        .bind(&msg.text)
        .bind(&raw_data)
        .bind(msg.timestamp)
        .bind(msg.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn store_messages(&self, msgs: &[StoredMessage]) -> Result<(), StoreError> {
        for msg in msgs {
            self.store_message(msg).await?;
        }
        Ok(())
    }

    async fn list_messages(
        &self,
        filter: &MessageFilter,
    ) -> Result<Vec<StoredMessage>, StoreError> {
        let mut sql = String::from(
            "SELECT id, session_key, platform, chat_id, role, text, raw_data, timestamp, created_at
             FROM messages WHERE 1=1",
        );

        // Build SQL with parameter placeholders
        let mut param_values: Vec<String> = vec![];

        if let Some(ref key) = filter.session_key {
            sql.push_str(" AND session_key = $");
            let idx = param_values.len() + 1;
            sql.push_str(&idx.to_string());
            param_values.push(key.clone());
        }
        if let Some(ref platform) = filter.platform {
            sql.push_str(" AND platform = $");
            let idx = param_values.len() + 1;
            sql.push_str(&idx.to_string());
            param_values.push(platform.clone());
        }
        if let Some(ref chat_id) = filter.chat_id {
            sql.push_str(" AND chat_id = $");
            let idx = param_values.len() + 1;
            sql.push_str(&idx.to_string());
            param_values.push(chat_id.clone());
        }
        if let Some(before) = filter.before {
            sql.push_str(" AND timestamp < $");
            let idx = param_values.len() + 1;
            sql.push_str(&idx.to_string());
            param_values.push(before.to_string());
        }

        sql.push_str(" ORDER BY timestamp DESC");

        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }
        if let Some(offset) = filter.offset {
            sql.push_str(&format!(" OFFSET {}", offset));
        }

        let mut q = sqlx::query(&sql);
        for p in &param_values {
            q = q.bind(p);
        }

        let rows = q.fetch_all(&self.pool).await?;
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
        let result = sqlx::query("DELETE FROM messages WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn delete_expired_messages(&self, before: i64) -> Result<u64, StoreError> {
        let result = sqlx::query("DELETE FROM messages WHERE created_at < $1")
            .bind(before)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
#[cfg(feature = "integration-test")]
mod tests {
    use super::*;
    use crate::types::message::{ChatType, MessageAuthor};
    use crate::types::session::{ResetPolicy, SessionSource};

    /// 创建测试用 PostgreSQL 连接池
    /// 这些测试需要运行中的 PostgreSQL 实例：
    ///   docker run -d --name easybot-pg-test -e POSTGRES_DB=easybot_test -e POSTGRES_PASSWORD=easybot -p 5432:5432 postgres:16-alpine
    async fn create_test_pool() -> PgPool {
        let conn_str = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:easybot@localhost:5432/easybot_test".to_string()
        });
        let pool = create_pool(&conn_str, 2).await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

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
            },
            reset_policy: ResetPolicy::Never,
            metadata: serde_json::json!({}),
        }
    }

    fn make_test_inbound() -> InboundMessage {
        InboundMessage {
            id: "msg1".to_string(),
            platform: "telegram".to_string(),
            chat_id: "123".to_string(),
            chat_name: None,
            chat_type: ChatType::Dm,
            text: Some("Hello".to_string()),
            author: MessageAuthor {
                id: "user1".to_string(),
                name: Some("User".to_string()),
                is_bot: false,
            },
            timestamp: 1000000,
            media: None,
            command: None,
            callback: None,
            reply_to: None,
            thread_id: None,
            is_group: false,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn test_session_upsert_and_get() {
        let pool = create_test_pool().await;
        let store = PgSessionStore::new(pool);

        let session = make_test_session("pg:1", "telegram", "1");
        store.upsert_session(&session).await.unwrap();

        let loaded = store.get_session("pg:1").await.unwrap().unwrap();
        assert_eq!(loaded.key, "pg:1");
        assert_eq!(loaded.platform, "telegram");
    }

    #[tokio::test]
    async fn test_message_store_and_list() {
        let pool = create_test_pool().await;
        let store = PgMessageStore::new(pool);

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
    }
}
