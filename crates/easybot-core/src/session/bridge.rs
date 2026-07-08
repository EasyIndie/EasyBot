//! 会话桥接器
//!
//! 监听事件总线上的入站消息事件，自动创建或更新会话。
//! 是连接消息接收管道与会话管理的桥梁。

use std::sync::Arc;
use tracing::{info, warn};

use crate::adapter::AdapterManager;
use crate::bus::EventBus;
use crate::session::SessionManager;
use crate::types::message::{InboundMessage, MessageType};
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
    ///
    /// `adapter_manager` 可选传入，用于在会话创建后异步富化 session source 信息
    /// （用户名、角色等）。传入 `None` 则跳过富化。
    pub fn start(
        event_bus: Arc<EventBus>,
        session_manager: Arc<SessionManager>,
        adapter_manager: Option<Arc<AdapterManager>>,
    ) {
        let mut event_rx = event_bus.subscribe(crate::types::event::event_types::MESSAGE_INBOUND);

        tokio::spawn(async move {
            info!("Session bridge started");

            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        if let Some(inbound) = Self::parse_inbound(&event.data) {
                            let sm = session_manager.clone();
                            let am = adapter_manager.clone();
                            Self::handle_inbound(sm, inbound, am).await;
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

    /// 处理入站消息：创建或更新会话，然后异步富化
    async fn handle_inbound(
        session_manager: Arc<SessionManager>,
        msg: InboundMessage,
        adapter_manager: Option<Arc<AdapterManager>>,
    ) {
        let platform = msg.platform.clone();
        let key = Session::build_key(&platform, &msg.chat_id, msg.thread_id.as_deref());

        let source = SessionSource {
            platform: platform.clone(),
            chat_id: msg.chat_id,
            chat_name: msg.chat_name,
            chat_type: msg.chat_type,
            user_id: Some(msg.sender.id),
            user_name: msg.sender.name,
            is_bot: msg.sender.is_bot,
            user_username: msg.sender.username,
            user_role: msg.sender.role,
        };

        let _session = session_manager.get_or_create(&key, source).await;

        // 记录最近消息（内联执行，不 spawn 额外任务）
        let msg_text = msg.text.clone().or_else(|| match msg.msg_type {
            MessageType::Image => Some("[图片]".to_string()),
            MessageType::Audio => Some("[音频]".to_string()),
            MessageType::Video => Some("[视频]".to_string()),
            MessageType::File => Some("[文件]".to_string()),
            MessageType::Sticker => Some("[贴纸]".to_string()),
            MessageType::Interactive => Some("[卡片]".to_string()),
            _ => None,
        });
        if let Some(text) = msg_text {
            session_manager
                .update_last_message(&key, Some(text), msg.timestamp)
                .await;
        }

        // 异步富化：内联执行，不 spawn 额外任务
        if let Some(ref am) = adapter_manager
            && let Some(current) = session_manager.get(&key)
            && let Some(enriched) = am.enrich_session(&platform, &current.source).await
            && (enriched.user_username.is_some()
                || enriched.user_role.is_some()
                || enriched.user_name.is_some()
                || enriched.chat_name.is_some())
        {
            session_manager.update_source_fields(&key, enriched).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::session::SessionManager;
    use crate::types::event::GatewayEvent;
    use crate::types::message::{ChatType, MessageSender, MessageType};
    use std::time::Duration;

    fn make_test_msg(
        id: &str,
        platform: &str,
        chat_id: &str,
        chat_name: Option<&str>,
        sender_id: &str,
        sender_name: Option<&str>,
    ) -> InboundMessage {
        InboundMessage {
            id: id.to_string(),
            platform: platform.to_string(),
            msg_type: MessageType::Text,
            text: Some("test".to_string()),
            sender: MessageSender {
                id: sender_id.to_string(),
                name: sender_name.map(|s| s.to_string()),
                username: None,
                avatar_url: None,
                is_bot: false,
                role: None,
                language_code: None,
            },
            recipient: None,
            chat_id: chat_id.to_string(),
            chat_name: chat_name.map(|s| s.to_string()),
            chat_type: ChatType::Dm,
            guild_id: None,
            thread_id: None,
            root_id: None,
            timestamp: 1700000000000,
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
    async fn test_bridge_creates_session() {
        let bus = Arc::new(EventBus::new());
        let sessions = Arc::new(SessionManager::new());

        SessionBridge::start(bus.clone(), sessions.clone(), None);

        // 发布一个入站消息事件
        let msg = make_test_msg(
            "42",
            "telegram",
            "12345",
            Some("Test User"),
            "678",
            Some("Test User"),
        );

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

        SessionBridge::start(bus.clone(), sessions.clone(), None);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let msg = make_test_msg(
            "src-test",
            "test",
            "source-check",
            Some("Source Name"),
            "author-001",
            Some("AuthorName"),
        );

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
