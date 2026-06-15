//! 消息持久化器
//!
//! 订阅事件总线上的入站消息事件，将消息持久化到存储层。
//! 与会话桥接器（SessionBridge）协同工作，各自专注：
//! - SessionBridge：创建/更新会话
//! - MessagePersister：持久化消息内容

use std::sync::Arc;
use tracing::{info, warn};

use crate::bus::EventBus;
use crate::storage::{MessageStore, StoredMessage};
use crate::types::message::InboundMessage;

/// 消息持久化器
///
/// 订阅 `message.inbound` 事件，将每条入站消息写入 MessageStore。
pub struct MessagePersister;

impl MessagePersister {
    /// 启动消息持久化后台任务
    pub fn start(
        event_bus: Arc<EventBus>,
        message_store: Arc<dyn MessageStore>,
    ) {
        let mut event_rx = event_bus.subscribe_many(&[
            crate::types::event::event_types::MESSAGE_INBOUND,
        ]);

        tokio::spawn(async move {
            info!("Message persister started");

            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        if let Ok(inbound) = serde_json::from_value::<InboundMessage>(event.data) {
                            let stored = StoredMessage::from_inbound(&inbound);
                            if let Err(e) = message_store.store_message(&stored).await {
                                warn!("Failed to persist inbound message: {}", e);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Message persister lagged by {} events", n);
                    }
                    Err(_) => {
                        info!("Message persister stopped (event bus closed)");
                        break;
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;
    use serde_json::json;

    use crate::bus::EventBus;
    use crate::storage::sqlite::{SqliteMessageStore, run_migrations};
    use crate::storage::{MessageFilter, MessageStore};
    use crate::types::message::{InboundMessage, MessageAuthor, ChatType};
    use crate::types::event::event_types::MESSAGE_INBOUND;
    use super::MessagePersister;

    /// 创建测试用的入站消息
    fn test_inbound_message() -> InboundMessage {
        InboundMessage {
            id: uuid::Uuid::new_v4().to_string(),
            platform: "telegram".to_string(),
            chat_id: "12345".to_string(),
            chat_name: Some("Test Chat".to_string()),
            chat_type: ChatType::Dm,
            is_group: false,
            text: Some("Hello from test".to_string()),
            author: MessageAuthor {
                id: "user1".to_string(),
                name: Some("Alice".to_string()),
                is_bot: false,
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            media: None,
            command: None,
            callback: None,
            reply_to: None,
            thread_id: None,
            metadata: None,
        }
    }

    /// 将 InboundMessage 发布为 EventBus 事件
    fn publish_inbound(event_bus: &EventBus, msg: &InboundMessage) {
        let event = crate::types::event::GatewayEvent {
            event_type: MESSAGE_INBOUND.to_string(),
            source: "test".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            data: serde_json::to_value(msg).unwrap(),
            metadata: None,
        };
        event_bus.publish(event);
    }

    #[tokio::test]
    async fn test_persister_stores_inbound_message() {
        let event_bus = Arc::new(EventBus::new());
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let message_store: Arc<dyn crate::storage::MessageStore> =
            Arc::new(SqliteMessageStore::new(pool.clone()));

        MessagePersister::start(event_bus.clone(), message_store);

        tokio::time::sleep(Duration::from_millis(200)).await;

        let msg = test_inbound_message();
        publish_inbound(&event_bus, &msg);

        tokio::time::sleep(Duration::from_millis(500)).await;

        let stored_messages = SqliteMessageStore::new(pool)
            .list_messages(&MessageFilter {
                session_key: None,
                platform: Some("telegram".to_string()),
                chat_id: None,
                limit: Some(10),
                offset: None,
                before: None,
            })
            .await
            .unwrap();
        assert_eq!(stored_messages.len(), 1, "Should have stored 1 message");
        assert_eq!(stored_messages[0].platform, "telegram");
        assert_eq!(stored_messages[0].text.as_deref(), Some("Hello from test"));
    }

    #[tokio::test]
    async fn test_persister_ignores_non_inbound_events() {
        let event_bus = Arc::new(EventBus::new());
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let message_store: Arc<dyn crate::storage::MessageStore> =
            Arc::new(SqliteMessageStore::new(pool.clone()));

        MessagePersister::start(event_bus.clone(), message_store);

        tokio::time::sleep(Duration::from_millis(200)).await;

        // 发布非入站消息事件
        let event = crate::types::event::GatewayEvent {
            event_type: "adapter.connected".to_string(),
            source: "test".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            data: json!({"platform": "telegram"}),
            metadata: None,
        };
        event_bus.publish(event);

        tokio::time::sleep(Duration::from_millis(300)).await;

        let stored_messages = SqliteMessageStore::new(pool)
            .list_messages(&MessageFilter {
                session_key: None,
                platform: None,
                chat_id: None,
                limit: Some(10),
                offset: None,
                before: None,
            })
            .await
            .unwrap();
        assert!(stored_messages.is_empty(), "Should not store non-message events");
    }

    #[tokio::test]
    async fn test_persister_multiple_messages() {
        let event_bus = Arc::new(EventBus::new());
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let message_store: Arc<dyn crate::storage::MessageStore> =
            Arc::new(SqliteMessageStore::new(pool.clone()));

        MessagePersister::start(event_bus.clone(), message_store);

        tokio::time::sleep(Duration::from_millis(200)).await;

        for i in 0..3 {
            let mut msg = test_inbound_message();
            msg.text = Some(format!("Message {}", i));
            publish_inbound(&event_bus, &msg);
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let stored_messages = SqliteMessageStore::new(pool)
            .list_messages(&MessageFilter {
                session_key: None,
                platform: Some("telegram".to_string()),
                chat_id: None,
                limit: Some(10),
                offset: None,
                before: None,
            })
            .await
            .unwrap();
        assert_eq!(stored_messages.len(), 3, "Should have stored 3 messages");
    }
}
