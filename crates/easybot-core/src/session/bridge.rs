//! 会话桥接器
//!
//! 监听事件总线上的入站消息事件，自动创建或更新会话。
//! 是连接消息接收管道与会话管理的桥梁。

use std::sync::Arc;
use tracing::{info, warn};

use crate::bus::EventBus;
use crate::session::SessionManager;
use crate::types::message::InboundMessage;
use crate::types::session::{Session, SessionSource};

/// 会话桥接器
///
/// 订阅 `message.inbound` 事件，为每个入站消息创建或更新会话。
/// 作为一个后台任务运行，可通过可选 cancel 信号停止。
pub struct SessionBridge;

impl SessionBridge {
    /// 启动会话桥接器后台任务
    ///
    /// 订阅事件总线上的入站消息事件，持续处理入站消息并创建/更新会话。
    /// 运行时不会退出（除非 EventBus 关闭），随 tokio 运行时一起停止。
    pub fn start(event_bus: Arc<EventBus>, session_manager: Arc<SessionManager>) {
        let mut event_rx =
            event_bus.subscribe_many(&[crate::types::event::event_types::MESSAGE_INBOUND]);

        tokio::spawn(async move {
            info!("Session bridge started");

            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        if let Some(inbound) = Self::parse_inbound(&event.data) {
                            Self::handle_inbound(&session_manager, inbound).await;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Session bridge lagged by {} events", n);
                    }
                    Err(_) => {
                        info!("Session bridge stopped (event bus closed)");
                        break;
                    }
                }
            }
        });
    }

    /// 从事件数据中解析 InboundMessage
    fn parse_inbound(data: &serde_json::Value) -> Option<InboundMessage> {
        serde_json::from_value::<InboundMessage>(data.clone()).ok()
    }

    /// 处理入站消息：创建或更新会话
    async fn handle_inbound(session_manager: &SessionManager, msg: InboundMessage) {
        let key = Session::build_key(&msg.platform, &msg.chat_id, msg.thread_id.as_deref());

        let source = SessionSource {
            platform: msg.platform,
            chat_id: msg.chat_id,
            chat_name: msg.chat_name,
            chat_type: msg.chat_type,
            user_id: Some(msg.author.id),
            user_name: msg.author.name,
            is_bot: msg.author.is_bot,
        };

        let _session = session_manager.get_or_create(&key, source).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::session::SessionManager;
    use crate::types::event::GatewayEvent;
    use crate::types::message::{ChatType, MessageAuthor};
    use std::time::Duration;

    #[tokio::test]
    async fn test_bridge_creates_session() {
        let bus = Arc::new(EventBus::new());
        let sessions = Arc::new(SessionManager::new());

        SessionBridge::start(bus.clone(), sessions.clone());

        // 发布一个入站消息事件
        let msg = InboundMessage {
            id: "42".to_string(),
            platform: "telegram".to_string(),
            chat_id: "12345".to_string(),
            chat_name: Some("Test User".to_string()),
            chat_type: ChatType::Dm,
            text: Some("Hello".to_string()),
            author: MessageAuthor {
                id: "678".to_string(),
                name: Some("Test User".to_string()),
                is_bot: false,
            },
            timestamp: 1700000000000,
            media: None,
            command: None,
            callback: None,
            reply_to: None,
            thread_id: None,
            mentioned: None,
            is_group: false,
            metadata: None,
        };

        let event = GatewayEvent::new(
            crate::types::event::event_types::MESSAGE_INBOUND,
            "test",
            serde_json::to_value(&msg).unwrap(),
        );
        bus.publish(event);

        // 等待事件处理
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 验证会话已创建
        let session = sessions.get("telegram:12345");
        assert!(session.is_some(), "Session should be created by bridge");
        if let Some(s) = session {
            assert_eq!(s.platform, "telegram");
            assert_eq!(s.chat_id, "12345");
        }
    }

    #[tokio::test]
    async fn test_bridge_session_source_fields() {
        let bus = Arc::new(EventBus::new());
        let sessions = Arc::new(SessionManager::new());

        SessionBridge::start(bus.clone(), sessions.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;

        let msg = InboundMessage {
            id: "src-test".to_string(),
            platform: "test".to_string(),
            chat_id: "source-check".to_string(),
            chat_name: Some("Source Name".to_string()),
            chat_type: ChatType::Dm,
            text: Some("check".to_string()),
            author: MessageAuthor {
                id: "author-001".to_string(),
                name: Some("AuthorName".to_string()),
                is_bot: false,
            },
            timestamp: 1700000000000,
            media: None,
            command: None,
            callback: None,
            reply_to: None,
            thread_id: None,
            mentioned: None,
            is_group: false,
            metadata: None,
        };

        let event = GatewayEvent::new(
            crate::types::event::event_types::MESSAGE_INBOUND,
            "test",
            serde_json::to_value(&msg).unwrap(),
        );
        bus.publish(event);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let session = sessions.get("test:source-check").unwrap();
        assert_eq!(session.source.chat_name, Some("Source Name".to_string()));
        assert_eq!(session.source.user_id, Some("author-001".to_string()));
        assert_eq!(session.source.user_name, Some("AuthorName".to_string()));
    }
}
