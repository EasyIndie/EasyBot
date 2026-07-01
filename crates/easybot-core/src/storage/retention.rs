//! TTL 保留策略
//!
//! 提供后台任务定期清理过期的会话和消息。
//! 支持 SQLite 和 PostgreSQL 两种后端。

use std::sync::Arc;
use tracing;

use super::{MessageStore, SessionStore, StoreError};

/// 保留策略配置
#[derive(Debug, Clone)]
pub struct RetentionConfig {
    /// 消息保留天数（默认 90）
    pub message_ttl_days: u64,
    /// 会话保留天数（默认 365）
    pub session_ttl_days: u64,
    /// 清理间隔秒数（默认 3600 = 1 小时）
    pub cleanup_interval_secs: u64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            message_ttl_days: 90,
            session_ttl_days: 365,
            cleanup_interval_secs: 3600,
        }
    }
}

/// TTL 清理后台工作器
///
/// 以固定间隔运行，删除早于配置天数的消息和会话。
pub struct RetentionWorker;

impl RetentionWorker {
    /// 启动 TTL 清理后台任务
    ///
    /// 当 `cleanup_interval_secs > 0` 时启用。
    /// 首个清理在 `T + interval` 后运行，避免启动时争用。
    pub fn start(
        message_store: Arc<dyn MessageStore>,
        session_store: Arc<dyn SessionStore>,
        config: RetentionConfig,
    ) {
        if config.cleanup_interval_secs == 0 {
            tracing::info!("TTL retention cleanup is disabled");
            return;
        }

        tracing::info!(
            "TTL retention worker started: messages>{}d, sessions>{}d, interval={}s",
            config.message_ttl_days,
            config.session_ttl_days,
            config.cleanup_interval_secs,
        );

        tokio::spawn(async move {
            // 首个清理在 T + interval 后运行
            tokio::time::sleep(std::time::Duration::from_secs(config.cleanup_interval_secs)).await;

            loop {
                let now = chrono::Utc::now().timestamp_millis();

                // 清理过期消息
                if config.message_ttl_days > 0 {
                    let cutoff = now - (config.message_ttl_days as i64 * 86_400_000);
                    match message_store.delete_expired_messages(cutoff).await {
                        Ok(count) => {
                            if count > 0 {
                                tracing::info!("TTL cleanup: deleted {} expired messages", count);
                            }
                        }
                        Err(StoreError::Database(e)) => {
                            tracing::warn!("TTL cleanup (messages) failed: {}", e);
                        }
                        Err(e) => {
                            tracing::warn!("TTL cleanup (messages) error: {}", e);
                        }
                    }
                }

                // 清理过期会话
                if config.session_ttl_days > 0 {
                    let cutoff = now - (config.session_ttl_days as i64 * 86_400_000);
                    match session_store.delete_expired_sessions(cutoff).await {
                        Ok(count) => {
                            if count > 0 {
                                tracing::info!("TTL cleanup: deleted {} expired sessions", count);
                            }
                        }
                        Err(StoreError::Database(e)) => {
                            tracing::warn!("TTL cleanup (sessions) failed: {}", e);
                        }
                        Err(e) => {
                            tracing::warn!("TTL cleanup (sessions) error: {}", e);
                        }
                    }
                }

                // 等待下一个清理周期
                tokio::time::sleep(std::time::Duration::from_secs(config.cleanup_interval_secs))
                    .await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::retention::{RetentionConfig, RetentionWorker};
    use crate::storage::sqlite::{
        SqliteMessageStore, SqliteSessionStore, create_pool, run_migrations,
    };
    use crate::storage::{MessageFilter, MessageRole, MessageStore, SessionStore, StoredMessage};
    use crate::types::message::ChatType;
    use crate::types::session::{ResetPolicy, Session, SessionSource};
    use std::sync::Arc;

    async fn create_test_stores() -> (SqliteMessageStore, SqliteSessionStore) {
        let pool = create_pool(std::path::Path::new(":memory:")).await.unwrap();
        run_migrations(&pool).await.unwrap();
        (
            SqliteMessageStore::new(pool.clone()),
            SqliteSessionStore::new(pool),
        )
    }

    fn make_old_session(key: &str, days_old: i64) -> Session {
        let old_time = chrono::Utc::now().timestamp_millis() - (days_old * 86_400_000);
        Session {
            key: key.to_string(),
            platform: "test".to_string(),
            chat_id: "1".to_string(),
            thread_id: None,
            created_at: old_time,
            updated_at: old_time,
            source: SessionSource {
                platform: "test".to_string(),
                chat_id: "1".to_string(),
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
        }
    }

    fn make_old_message(id: &str, days_old: i64) -> StoredMessage {
        let old_time = chrono::Utc::now().timestamp_millis() - (days_old * 86_400_000);
        StoredMessage {
            id: id.to_string(),
            session_key: "test:1".to_string(),
            platform: "test".to_string(),
            chat_id: "1".to_string(),
            role: MessageRole::User,
            text: Some("old".to_string()),
            raw_data: serde_json::json!({}),
            timestamp: old_time,
            created_at: old_time,
        }
    }

    fn make_recent_message(id: &str) -> StoredMessage {
        StoredMessage {
            id: id.to_string(),
            session_key: "test:1".to_string(),
            platform: "test".to_string(),
            chat_id: "1".to_string(),
            role: MessageRole::User,
            text: Some("recent".to_string()),
            raw_data: serde_json::json!({}),
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    #[tokio::test]
    async fn test_delete_expired_messages() {
        let (msg_store, _) = create_test_stores().await;

        msg_store
            .store_message(&make_old_message("old1", 100))
            .await
            .unwrap();
        msg_store
            .store_message(&make_old_message("old2", 200))
            .await
            .unwrap();
        msg_store
            .store_message(&make_recent_message("recent1"))
            .await
            .unwrap();

        let cutoff = chrono::Utc::now().timestamp_millis() - (50 * 86_400_000);
        let deleted = msg_store.delete_expired_messages(cutoff).await.unwrap();
        assert_eq!(deleted, 2, "should delete 2 old messages");

        let remaining = msg_store
            .list_messages(&MessageFilter::default())
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1, "should keep 1 recent message");
        assert_eq!(remaining[0].id, "recent1");
    }

    #[tokio::test]
    async fn test_delete_expired_sessions() {
        let (_, sess_store) = create_test_stores().await;

        sess_store
            .upsert_session(&make_old_session("old1", 400))
            .await
            .unwrap();
        sess_store
            .upsert_session(&make_old_session("old2", 500))
            .await
            .unwrap();
        sess_store
            .upsert_session(&make_old_session("recent1", 1))
            .await
            .unwrap();

        let cutoff = chrono::Utc::now().timestamp_millis() - (365 * 86_400_000);
        let deleted = sess_store.delete_expired_sessions(cutoff).await.unwrap();
        assert_eq!(deleted, 2, "should delete 2 old sessions");

        let remaining = sess_store.load_all_sessions().await.unwrap();
        assert_eq!(remaining.len(), 1, "should keep 1 recent session");
    }

    #[tokio::test]
    async fn test_retention_noop_when_zero_interval() {
        // Should not panic or start a task
        let (msg_store, sess_store) = create_test_stores().await;
        let config = RetentionConfig {
            cleanup_interval_secs: 0,
            ..Default::default()
        };
        RetentionWorker::start(Arc::new(msg_store), Arc::new(sess_store), config);
        // Just verify no crash
    }
}
