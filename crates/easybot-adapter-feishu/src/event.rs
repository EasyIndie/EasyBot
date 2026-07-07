//! 飞书事件处理模块
//!
//! 接收来自飞书 WebSocket 事件订阅的事件，解析为 `InboundMessage`，
//! 并通过 EventBus 发布到系统内部。

use easybot_core::bus::EventBus;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::message::{
    ChatType, InboundMessage, MessageSender, MessageType, SenderRole,
};

use crate::types::*;

/// 处理 `im.message.receive_v1` 事件
///
/// 解析消息内容、发送者、聊天信息，构建 `InboundMessage` 并发布到 EventBus。
/// `sender_role` 是可选的群成员角色，由调用方（lib.rs）预先解析。
pub async fn handle_message_receive(
    event_data: serde_json::Value,
    event_bus: &EventBus,
    _bot_id: &str,
    sender_role: Option<SenderRole>,
) {
    let receive_event: FeishuMessageReceiveEvent = match serde_json::from_value(event_data) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("飞书消息事件解析失败: {}", e);
            return;
        }
    };

    // 在移出字段前序列化原始数据（用于 metadata）
    let raw_payload = serde_json::to_value(&receive_event).ok();

    let sender_id = receive_event.sender.sender_id.open_id;
    let message = receive_event.message;

    // 解析消息内容
    // 飞书 text 消息的 content 是 JSON 字符串: `{"text":"hello"}`
    let text = if message.msg_type == "text" {
        serde_json::from_str::<serde_json::Value>(&message.content)
            .ok()
            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))
            .unwrap_or(message.content.clone())
    } else {
        // 非 text 消息，记录消息类型但不提取内容
        tracing::debug!("飞书非文本消息: msg_type={}", message.msg_type);
        message.content.clone()
    };

    // 判断聊天类型
    let chat_type = match message.chat_type.as_str() {
        "group" => ChatType::Group,
        "p2p" => ChatType::Dm,
        _ => {
            tracing::warn!("飞书未知聊天类型: {}", message.chat_type);
            return;
        }
    };

    // 解析时间戳（飞书 create_time 是毫秒时间戳字符串）
    let timestamp: i64 = message
        .create_time
        .parse()
        .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());

    let inbound = InboundMessage {
        id: message.message_id,
        platform: "feishu".to_string(),
        msg_type: MessageType::Text,
        chat_id: message.chat_id,
        chat_type,
        guild_id: None,
        root_id: None,
        mentions: None,
        chat_name: None,
        text: Some(text),
        sender: MessageSender {
            id: sender_id.clone(),
            name: Some(sender_id),
            username: None,
            avatar_url: None,
            is_bot: receive_event.sender.sender_type == "app",
            role: sender_role,
            language_code: None,
        },
        recipient: None,
        timestamp,
        media: None,
        command: None,
        callback: None,
        reply_to: None,
        thread_id: None,
        mentioned: None,
        metadata: raw_payload,
    };

    let event = GatewayEvent::new(
        easybot_core::types::event::event_types::MESSAGE_INBOUND,
        "feishu",
        serde_json::to_value(&inbound).unwrap_or_default(),
    );

    event_bus.publish(event);
    tracing::info!(
        "飞书消息已处理: chat={}, type={}",
        inbound.chat_id,
        message.msg_type
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use easybot_core::bus::EventBus;

    fn make_event_data(chat_type: &str, msg_type: &str, content: &str) -> serde_json::Value {
        serde_json::json!({
            "sender": {
                "sender_id": { "open_id": "ou_test_user" },
                "sender_type": "user"
            },
            "message": {
                "message_id": "om_test_msg",
                "chat_id": "oc_test_chat",
                "chat_type": chat_type,
                "message_type": msg_type,
                "content": content,
                "create_time": "1603977298000"
            }
        })
    }

    #[tokio::test]
    async fn test_handle_text_message_group() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("message.inbound");

        let data = make_event_data("group", "text", r#"{"text":"hello"}"#);
        handle_message_receive(data, &bus, "bot_id", None).await;

        // 验证事件被发布
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("event should be published")
            .expect("no error");

        assert_eq!(event.event_type, "message.inbound");
        let msg: InboundMessage = serde_json::from_value(event.data).unwrap();
        assert_eq!(msg.platform, "feishu");
        assert_eq!(msg.chat_id, "oc_test_chat");
        assert_eq!(msg.text.as_deref(), Some("hello"));
        assert_eq!(msg.chat_type, ChatType::Group);
        assert_eq!(msg.chat_type, ChatType::Group);
        assert_eq!(msg.sender.id, "ou_test_user");
        assert_eq!(msg.id, "om_test_msg");
    }

    #[tokio::test]
    async fn test_handle_text_message_p2p() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("message.inbound");

        let data = make_event_data("p2p", "text", r#"{"text":"hi"}"#);
        handle_message_receive(data, &bus, "bot_id", None).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("event should be published")
            .expect("no error");

        let msg: InboundMessage = serde_json::from_value(event.data).unwrap();
        assert_eq!(msg.text.as_deref(), Some("hi"));
        assert_eq!(msg.chat_type, ChatType::Dm);
        assert_eq!(msg.chat_type, ChatType::Dm);
    }

    #[tokio::test]
    async fn test_handle_image_message() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("message.inbound");

        let data = make_event_data("group", "image", r#"{"image_key":"img_abc"}"#);
        handle_message_receive(data, &bus, "bot_id", None).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("event should be published")
            .expect("no error");

        let msg: InboundMessage = serde_json::from_value(event.data).unwrap();
        // 非 text 消息，content 原样保留
        assert_eq!(msg.text.as_deref(), Some(r#"{"image_key":"img_abc"}"#));
    }

    #[tokio::test]
    async fn test_handle_unknown_chat_type_should_not_publish() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("message.inbound");

        let data = make_event_data("unknown", "text", r#"{"text":"x"}"#);
        handle_message_receive(data, &bus, "bot_id", None).await;

        // 不应该有事件被发布
        let result = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(
            result.is_err(),
            "should not publish event for unknown chat type"
        );
    }

    #[tokio::test]
    async fn test_handle_malformed_content_uses_raw() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("message.inbound");

        // content 不是有效 JSON
        let data = make_event_data("group", "text", "not_json");
        handle_message_receive(data, &bus, "bot_id", None).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("event should be published")
            .expect("no error");

        let msg: InboundMessage = serde_json::from_value(event.data).unwrap();
        // 解析失败时使用原始 content
        assert_eq!(msg.text.as_deref(), Some("not_json"));
    }

    #[tokio::test]
    async fn test_handle_invalid_event_data() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("message.inbound");

        // 无效的事件数据（没有 message 字段）
        let invalid_data = serde_json::json!({"invalid": true});
        handle_message_receive(invalid_data, &bus, "bot_id", None).await;

        // 不应该发布事件
        let result = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(result.is_err(), "should not publish event for invalid data");
    }

    #[tokio::test]
    async fn test_handle_missing_msg_type_error() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("message.inbound");

        // 缺少 message_type 字段（使用旧字段名 msg_type 而不是 message_type）
        let data = serde_json::json!({
            "sender": {
                "sender_id": { "open_id": "ou_test" },
                "sender_type": "user"
            },
            "message": {
                "message_id": "om_test",
                "chat_id": "oc_test",
                "chat_type": "group",
                "msg_type": "text",
                "content": "{}",
                "create_time": "0"
            }
        });
        handle_message_receive(data, &bus, "bot_id", None).await;

        // 使用旧字段名 msg_type 会导致解析失败，不应发布事件
        let result = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(
            result.is_err(),
            "should not publish event when message_type field is missing"
        );
    }

    #[tokio::test]
    async fn test_timestamp_parse_fallback() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("message.inbound");

        let mut data = make_event_data("group", "text", r#"{"text":"fallback"}"#);
        // 非法时间戳
        if let Some(msg) = data.get_mut("message")
            && let Some(obj) = msg.as_object_mut()
        {
            obj.insert("create_time".to_string(), serde_json::json!("not_a_number"));
        }
        handle_message_receive(data, &bus, "bot_id", None).await;

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("event should be published")
            .expect("no error");

        let msg: InboundMessage = serde_json::from_value(event.data).unwrap();
        // 时间戳应回退为当前时间
        let now = chrono::Utc::now().timestamp_millis();
        assert!(msg.timestamp > 0 && msg.timestamp <= now);
    }
}
