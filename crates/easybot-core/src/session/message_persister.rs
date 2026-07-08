//! 消息持久化器
//!
//! 订阅事件总线上的入站消息事件，将消息持久化到存储层。
//! 与会话桥接器（SessionBridge）协同工作，各自专注：
//! - SessionBridge：创建/更新会话
//! - MessagePersister：持久化消息内容
//!
//! 消息先进入内存缓冲区，每隔 1 秒或缓冲区满（50 条）时批量写入存储层。
//! 写入失败时自动重试（最多 3 次，指数退避），重试耗尽后记录错误并丢弃该批次。

use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::bus::EventBus;
use crate::storage::{MessageStore, StoredMessage};
use crate::types::message::InboundMessage;

/// 刷新间隔：缓冲区非空时，每隔此间隔写入存储
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// 缓冲区容量：达到此数量立即刷新（不等定时器）
const BATCH_SIZE: usize = 50;

/// 最大重试次数
const MAX_RETRIES: u32 = 3;

/// 消息持久化器
///
/// 订阅 `message.inbound` 事件，将入站消息批量写入 MessageStore。
pub struct MessagePersister;

impl MessagePersister {
    /// 启动消息持久化后台任务
    ///
    /// 使用事件驱动的订阅流 + 定时刷新，无轮询、零空闲 CPU 消耗。
    pub fn start(event_bus: Arc<EventBus>, message_store: Arc<dyn MessageStore>) {
        let mut event_stream =
            event_bus.subscribe_many(&[crate::types::event::event_types::MESSAGE_INBOUND]);

        let buffer: Arc<Mutex<Vec<StoredMessage>>> =
            Arc::new(Mutex::new(Vec::with_capacity(BATCH_SIZE)));

        tokio::spawn(async move {
            info!(
                "Message persister started (event-driven, {}ms flush, batch={})",
                FLUSH_INTERVAL.as_millis(),
                BATCH_SIZE
            );

            let mut flush_timer = tokio::time::interval(FLUSH_INTERVAL);
            flush_timer.tick().await; // skip first immediate tick

            loop {
                tokio::select! {
                    event = event_stream.next() => {
                        match event {
                            Some(event) => {
                                if let Ok(inbound) =
                                    serde_json::from_value::<InboundMessage>(event.data)
                                {
                                    let stored = StoredMessage::from_inbound(&inbound);
                                    let should_flush = {
                                        let mut buf = buffer.lock().await;
                                        buf.push(stored);
                                        buf.len() >= BATCH_SIZE
                                    };
                                    if should_flush {
                                        flush_batch(&buffer, &message_store).await;
                                        flush_timer.reset();
                                    }
                                }
                            }
                            None => {
                                // 事件流关闭，最终刷新并退出
                                flush_batch(&buffer, &message_store).await;
                                info!("Message persister stopped (event bus closed)");
                                return;
                            }
                        }
                    }
                    _ = flush_timer.tick() => {
                        flush_batch(&buffer, &message_store).await;
                    }
                }
            }
        });
    }
}

/// 将缓冲区中的所有消息批量写入存储
///
/// 采用原子取出策略：先 `take` 缓冲区，再尝试写入。
/// 写入失败时最多重试 MAX_RETRIES 次（指数退避）。
/// 重试耗尽后，消息将丢失并记录错误。
async fn flush_batch(buffer: &Arc<Mutex<Vec<StoredMessage>>>, store: &Arc<dyn MessageStore>) {
    let batch: Vec<StoredMessage> = {
        let mut buf = buffer.lock().await;
        if buf.is_empty() {
            return;
        }
        std::mem::take(&mut *buf)
    };

    let count = batch.len();

    for attempt in 1..=MAX_RETRIES {
        match store.store_messages(&batch).await {
            Ok(()) => {
                if attempt > 1 {
                    info!(
                        "Message batch ({} msgs) persisted after {} retries",
                        count,
                        attempt - 1
                    );
                }
                return;
            }
            Err(e) if attempt < MAX_RETRIES => {
                let delay = Duration::from_millis(100 * attempt as u64);
                warn!(
                    "Failed to persist {} messages (attempt {}/{}): {} — retrying in {:?}",
                    count, attempt, MAX_RETRIES, e, delay,
                );
                tokio::time::sleep(delay).await;
            }
            Err(e) => {
                error!(
                    "Failed to persist {} messages after {} attempts: {} — batch discarded",
                    count, MAX_RETRIES, e,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::bus::EventBus;
    use crate::session::SessionManager;
    use crate::session::bridge::SessionBridge;
    use crate::session::message_persister::MessagePersister;
    use crate::storage::sqlite::{SqliteMessageStore, run_migrations};
    use crate::storage::{MessageFilter, MessageStore};
    use crate::types::event::event_types::MESSAGE_INBOUND;
    use crate::types::message::{ChatType, InboundMessage, MessageSender, MessageType};

    /// 创建测试用的入站消息
    fn test_inbound_message() -> InboundMessage {
        InboundMessage {
            id: uuid::Uuid::new_v4().to_string(),
            platform: "telegram".to_string(),
            msg_type: MessageType::Text,
            text: Some("Hello from test".to_string()),
            sender: MessageSender {
                id: "user1".to_string(),
                name: Some("Alice".to_string()),
                username: None,
                avatar_url: None,
                is_bot: false,
                role: None,
                language_code: None,
            },
            recipient: None,
            chat_id: "12345".to_string(),
            chat_name: Some("Test Chat".to_string()),
            chat_type: ChatType::Dm,
            guild_id: None,
            thread_id: None,
            root_id: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
            media: None,
            command: None,
            callback: None,
            reply_to: None,
            mentions: None,
            mentioned: None,
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
        let message_store: Arc<dyn MessageStore> = Arc::new(SqliteMessageStore::new(pool.clone()));

        MessagePersister::start(event_bus.clone(), message_store);

        tokio::time::sleep(Duration::from_millis(200)).await;

        let msg = test_inbound_message();
        publish_inbound(&event_bus, &msg);

        // 等待批量刷新（FLUSH_INTERVAL = 1s，加 200ms 余量）
        tokio::time::sleep(Duration::from_millis(1500)).await;

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
        let message_store: Arc<dyn MessageStore> = Arc::new(SqliteMessageStore::new(pool.clone()));

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
        assert!(
            stored_messages.is_empty(),
            "Should not store non-message events"
        );
    }

    #[tokio::test]
    async fn test_persister_multiple_messages() {
        let event_bus = Arc::new(EventBus::new());
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let message_store: Arc<dyn MessageStore> = Arc::new(SqliteMessageStore::new(pool.clone()));

        MessagePersister::start(event_bus.clone(), message_store);

        tokio::time::sleep(Duration::from_millis(200)).await;

        for i in 0..3 {
            let mut msg = test_inbound_message();
            msg.text = Some(format!("Message {}", i));
            publish_inbound(&event_bus, &msg);
        }

        // 等待批量刷新
        tokio::time::sleep(Duration::from_millis(1500)).await;

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

    // ── 全管线集成测试 ──

    #[tokio::test]
    async fn test_full_pipeline_inbound() {
        let event_bus = Arc::new(EventBus::new());
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let message_store: Arc<dyn MessageStore> = Arc::new(SqliteMessageStore::new(pool.clone()));
        let session_manager = Arc::new(SessionManager::new());

        // 启动管线组件
        SessionBridge::start(event_bus.clone(), session_manager.clone(), None);
        MessagePersister::start(event_bus.clone(), message_store.clone());

        tokio::time::sleep(Duration::from_millis(200)).await;

        // 发布入站消息
        let msg = InboundMessage {
            id: "pipe-001".to_string(),
            platform: "test".to_string(),
            msg_type: MessageType::Text,
            text: Some("pipeline test".to_string()),
            sender: MessageSender {
                id: "user-pipe".to_string(),
                name: Some("PipelineUser".to_string()),
                username: None,
                avatar_url: None,
                is_bot: false,
                role: None,
                language_code: None,
            },
            recipient: None,
            chat_id: "pipeline-chat".to_string(),
            chat_name: Some("Pipe Test".to_string()),
            chat_type: ChatType::Dm,
            guild_id: None,
            thread_id: None,
            root_id: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
            media: None,
            command: None,
            callback: None,
            reply_to: None,
            mentions: None,
            mentioned: None,
            metadata: None,
        };
        let event = crate::types::event::GatewayEvent {
            event_type: MESSAGE_INBOUND.to_string(),
            source: "test".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            data: serde_json::to_value(&msg).unwrap(),
            metadata: None,
        };
        event_bus.publish(event);

        // 等待批量刷新
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // 验证会话已创建
        let session = session_manager.get("test:pipeline-chat");
        assert!(session.is_some(), "SessionBridge should create session");
        assert_eq!(
            session.unwrap().source.chat_name,
            Some("Pipe Test".to_string())
        );

        // 验证消息已持久化
        let stored = SqliteMessageStore::new(pool)
            .list_messages(&MessageFilter {
                session_key: None,
                platform: Some("test".to_string()),
                chat_id: None,
                limit: Some(10),
                offset: None,
                before: None,
            })
            .await
            .unwrap();
        assert_eq!(stored.len(), 1, "MessagePersister should store 1 message");
        assert_eq!(stored[0].text.as_deref(), Some("pipeline test"));
    }

    #[tokio::test]
    async fn test_full_pipeline_non_inbound_ignored() {
        let event_bus = Arc::new(EventBus::new());
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let message_store: Arc<dyn MessageStore> = Arc::new(SqliteMessageStore::new(pool.clone()));
        let session_manager = Arc::new(SessionManager::new());

        SessionBridge::start(event_bus.clone(), session_manager.clone(), None);
        MessagePersister::start(event_bus.clone(), message_store.clone());

        tokio::time::sleep(Duration::from_millis(200)).await;

        // 发布非消息事件
        let event = crate::types::event::GatewayEvent {
            event_type: "adapter.connected".to_string(),
            source: "test".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            data: json!({"platform": "test"}),
            metadata: None,
        };
        event_bus.publish(event);

        tokio::time::sleep(Duration::from_millis(300)).await;

        // 会话和消息均不应被创建
        assert_eq!(session_manager.count(), 0, "No session should be created");
        let stored = SqliteMessageStore::new(pool)
            .list_messages(&MessageFilter::default())
            .await
            .unwrap();
        assert!(stored.is_empty(), "No message should be stored");
    }
}
