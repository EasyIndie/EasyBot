//! 飞书开放平台 API 类型定义
//!
//! # 注意
//! 部分类型字段暂未使用，保留以支持未来入站消息处理和 API 序列化。
#![allow(dead_code)]

use serde::Deserialize;

/// 通用 API 响应包装
#[derive(Debug, Deserialize)]
pub struct FeishuApiResponse<T> {
    pub code: i64,
    pub msg: Option<String>,
    pub data: Option<T>,
}

/// Token 响应
#[derive(Debug, Deserialize)]
pub struct FeishuTokenResponse {
    pub code: i64,
    pub msg: Option<String>,
    pub tenant_access_token: Option<String>,
    pub expire: Option<u64>,
}

/// 发送消息响应数据
#[derive(Debug, Deserialize)]
pub struct FeishuSendMessageData {
    pub message_id: String,
    pub root_id: Option<String>,
    pub parent_id: Option<String>,
    pub create_time: Option<String>,
}

/// 上传文件响应数据
#[derive(Debug, Deserialize)]
pub struct FeishuUploadData {
    pub file_key: Option<String>,
}

/// 群聊信息
#[derive(Debug, Deserialize)]
pub struct FeishuChatInfo {
    pub chat_id: String,
    pub name: String,
    pub chat_type: String,
    pub member_count: u64,
    pub description: Option<String>,
}

/// 群聊列表响应
#[derive(Debug, Deserialize)]
pub struct FeishuListChatData {
    pub items: Vec<FeishuChatListItem>,
    pub page_token: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Deserialize)]
pub struct FeishuChatListItem {
    pub chat_id: String,
    pub name: String,
    pub chat_type: String,
    pub member_count: u64,
}

/// 消息事件类型常量
pub const EVENT_MESSAGE_RECEIVE_V1: &str = "im.message.receive_v1";

/// 入站事件：im.message.receive_v1 的 event 字段
#[derive(Debug, Deserialize)]
pub struct FeishuMessageReceiveEvent {
    pub sender: FeishuMessageSender,
    pub message: FeishuReceivedMessage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_message_receive_event_group() {
        let json = serde_json::json!({
            "sender": {
                "sender_id": {
                    "open_id": "ou_abc123",
                    "union_id": "on_xyz",
                    "user_id": "u_456"
                },
                "sender_type": "user",
                "tenant_key": "t_789"
            },
            "message": {
                "message_id": "om_xxx111",
                "root_id": "",
                "parent_id": "",
                "chat_id": "oc_5a64b50e",
                "chat_type": "group",
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "create_time": "1603977298000",
                "mentions": []
            }
        });

        let event: FeishuMessageReceiveEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.sender.sender_id.open_id, "ou_abc123");
        assert_eq!(event.sender.sender_type, "user");
        assert_eq!(event.message.message_id, "om_xxx111");
        assert_eq!(event.message.chat_id, "oc_5a64b50e");
        assert_eq!(event.message.chat_type, "group");
        assert_eq!(event.message.msg_type, "text");
        assert_eq!(event.message.content, r#"{"text":"hello"}"#);
        assert_eq!(event.message.create_time, "1603977298000");
        assert_eq!(event.message.root_id, Some("".to_string()));
        assert_eq!(event.message.parent_id, Some("".to_string()));
    }

    #[test]
    fn test_deserialize_message_receive_event_p2p() {
        let json = serde_json::json!({
            "sender": {
                "sender_id": {
                    "open_id": "ou_p2p_user"
                },
                "sender_type": "user"
            },
            "message": {
                "message_id": "om_p2p_msg",
                "root_id": null,
                "parent_id": null,
                "chat_id": "oc_p2p_chat",
                "chat_type": "p2p",
                "message_type": "image",
                "content": "{\"image_key\":\"img_abc\"}",
                "create_time": "1603977300000"
            }
        });

        let event: FeishuMessageReceiveEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.message.chat_type, "p2p");
        assert_eq!(event.message.msg_type, "image");
        assert!(event.message.root_id.is_none());
        assert!(event.message.parent_id.is_none());
    }

    #[test]
    fn test_deserialize_message_receive_event_minimal() {
        // 最小化事件（只有必填字段）
        let json = serde_json::json!({
            "sender": {
                "sender_id": {
                    "open_id": "ou_min"
                },
                "sender_type": "app"
            },
            "message": {
                "message_id": "om_min",
                "chat_id": "oc_min",
                "chat_type": "p2p",
                "message_type": "text",
                "content": "plain text",
                "create_time": "1000"
            }
        });

        let event: FeishuMessageReceiveEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.sender.sender_id.open_id, "ou_min");
        assert_eq!(event.sender.sender_type, "app");
        assert_eq!(event.message.msg_type, "text");
    }

    #[test]
    fn test_deserialize_unknown_chat_type() {
        let json = serde_json::json!({
            "sender": {
                "sender_id": { "open_id": "ou_test" },
                "sender_type": "user"
            },
            "message": {
                "message_id": "om_test",
                "chat_id": "oc_test",
                "chat_type": "unknown_type",
                "message_type": "text",
                "content": "{}",
                "create_time": "0"
            }
        });

        let event: FeishuMessageReceiveEvent = serde_json::from_value(json).unwrap();
        assert_eq!(event.message.chat_type, "unknown_type");
    }

    #[test]
    fn test_deserialize_invalid_json_should_fail() {
        let result = serde_json::from_value::<FeishuMessageReceiveEvent>(serde_json::json!({
            "sender": "invalid"
        }));
        assert!(result.is_err());
    }
}

#[derive(Debug, Deserialize)]
pub struct FeishuMessageSender {
    pub sender_id: FeishuSenderId,
    pub sender_type: String,
}

#[derive(Debug, Deserialize)]
pub struct FeishuSenderId {
    pub open_id: String,
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FeishuReceivedMessage {
    pub message_id: String,
    pub root_id: Option<String>,
    pub parent_id: Option<String>,
    pub chat_id: String,
    pub chat_type: String,
    #[serde(rename = "message_type")]
    pub msg_type: String,
    pub content: String,
    pub create_time: String,
}
