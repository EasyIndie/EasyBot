//! QQ 频道机器人 API 类型定义
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ── Gateway WebSocket ──

/// Gateway OpCode
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QqOpCode {
    #[serde(rename = "0")]
    Dispatch = 0,
    #[serde(rename = "1")]
    Heartbeat = 1,
    #[serde(rename = "2")]
    Identify = 2,
    #[serde(rename = "6")]
    Resume = 6,
    #[serde(rename = "7")]
    Reconnect = 7,
    #[serde(rename = "9")]
    InvalidSession = 9,
    #[serde(rename = "10")]
    Hello = 10,
    #[serde(rename = "11")]
    HeartbeatAck = 11,
}

/// Gateway 消息帧
#[derive(Debug, Deserialize, Serialize)]
pub struct GatewayPayload<T = serde_json::Value> {
    pub op: u8,
    #[serde(default)]
    pub d: Option<T>,
    #[serde(default)]
    pub s: Option<u64>,
    #[serde(default)]
    pub t: Option<String>,
}

/// Hello 消息
#[derive(Debug, Deserialize)]
pub struct HelloData {
    pub heartbeat_interval: u64,
}

/// Identify 消息
#[derive(Debug, Serialize)]
pub struct IdentifyData {
    pub token: String,
    pub intents: u32,
    pub shard: Vec<u32>,
}

/// Ready 事件
#[derive(Debug, Deserialize)]
pub struct ReadyData {
    pub version: u64,
    pub session_id: String,
    pub user: QqUser,
    pub shard: Vec<u32>,
}

/// Resume 消息
#[derive(Debug, Serialize)]
pub struct ResumeData {
    pub token: String,
    pub session_id: String,
    pub seq: u64,
}

// ── Gateway 响应 ──

#[derive(Debug, Deserialize)]
pub struct GatewayResponse {
    pub url: String,
}

// ── 用户/机器人 ──

#[derive(Debug, Deserialize, Clone)]
pub struct QqUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub bot: Option<bool>,
    #[serde(default)]
    pub avatar: Option<String>,
}

// ── 消息 ──

/// 频道消息事件数据（AT_MESSAGE_CREATE）
#[derive(Debug, Deserialize)]
pub struct QqChannelMessageEvent {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    pub author: QqMessageAuthor,
    pub timestamp: String,
}

/// 群聊消息事件数据（GROUP_AT_MESSAGE_CREATE — 旧协议，仅 @消息）
#[derive(Debug, Deserialize)]
pub struct QqGroupMessageEvent {
    pub id: String,
    pub group_openid: String,
    #[serde(default)]
    pub content: Option<String>,
    pub author: QqGroupMessageAuthor,
    pub timestamp: String,
}

/// 群聊全量消息事件数据（GROUP_MESSAGE_CREATE — 2026 新协议，全部群消息）
#[derive(Debug, Deserialize)]
pub struct QqGroupMessageCreateEvent {
    pub id: String,
    pub group_openid: String,
    #[serde(default)]
    pub content: Option<String>,
    pub author: QqGroupMessageAuthor,
    pub timestamp: String,
    /// @提及列表，通过 mentions[].is_you 判断是否 @了机器人
    #[serde(default)]
    pub mentions: Vec<QqMention>,
    /// 消息场景信息
    #[serde(default)]
    pub message_scene: Option<QqMessageScene>,
    /// 消息类型（0=文本）
    #[serde(default)]
    pub message_type: Option<u32>,
}

/// @提及信息
#[derive(Debug, Deserialize, Clone)]
pub struct QqMention {
    /// 是否 @了当前机器人
    pub is_you: bool,
    /// 提及范围（single / all / here）
    #[serde(default)]
    pub scope: Option<String>,
    /// 被提及的用户名
    #[serde(default)]
    pub username: Option<String>,
}

/// 消息场景
#[derive(Debug, Deserialize, Clone)]
pub struct QqMessageScene {
    /// 消息来源（default 等）
    #[serde(default)]
    pub source: Option<String>,
    /// 扩展字段
    #[serde(default)]
    pub ext: Option<Vec<String>>,
}

/// 私聊消息事件数据（C2C_MESSAGE_CREATE）
#[derive(Debug, Deserialize)]
pub struct QqC2cMessageEvent {
    pub id: String,
    #[serde(default)]
    pub content: Option<String>,
    pub author: QqC2cMessageAuthor,
    pub timestamp: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QqMessageAuthor {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub bot: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QqGroupMessageAuthor {
    pub member_openid: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QqC2cMessageAuthor {
    pub user_openid: String,
}

/// 发送消息请求体
#[derive(Debug, Serialize)]
pub struct QqSendMessageRequest {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_type: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// 发送消息响应
#[derive(Debug, Deserialize)]
pub struct QqSendMessageResponse {
    pub id: String,
    pub timestamp: Option<String>,
}

// ── 键盘（交互式消息）──

/// QQ 消息键盘（顶层包装）
#[derive(Debug, Serialize)]
pub struct QqKeyboard {
    pub content: QqKeyboardContent,
}

/// 键盘内容
#[derive(Debug, Serialize)]
pub struct QqKeyboardContent {
    pub rows: Vec<QqKeyboardRow>,
}

/// 键盘行
#[derive(Debug, Serialize)]
pub struct QqKeyboardRow {
    pub buttons: Vec<QqKeyboardButton>,
}

/// 键盘按钮
#[derive(Debug, Serialize)]
pub struct QqKeyboardButton {
    pub id: String,
    pub render_data: QqButtonRenderData,
    pub action: QqButtonAction,
}

/// 按钮渲染数据
#[derive(Debug, Serialize)]
pub struct QqButtonRenderData {
    pub label: String,
    pub visited_label: String,
    pub style: u32,
}

/// 按钮动作
#[derive(Debug, Serialize)]
pub struct QqButtonAction {
    #[serde(rename = "type")]
    pub action_type: u32,
    pub permission: QqButtonPermission,
    pub data: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub enter: bool,
}

/// 按钮权限
#[derive(Debug, Serialize)]
pub struct QqButtonPermission {
    #[serde(rename = "type")]
    pub permission_type: u32,
}

/// QQ 频道服务器（Guild）对象 — GET /users/@me/guilds 响应
#[derive(Debug, Deserialize)]
pub struct QqGuild {
    pub id: String,
    pub name: String,
}

/// 频道信息
#[derive(Debug, Deserialize)]
pub struct QqChannelInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub channel_type: u32,
}

/// 群信息
#[derive(Debug, Deserialize)]
pub struct QqGroupInfo {
    pub group_openid: String,
    pub group_name: Option<String>,
}

/// API 错误响应
#[derive(Debug, Deserialize)]
pub struct QqApiError {
    pub code: u64,
    pub message: String,
}

// ── 意图 (Intents) ──

pub mod intents {
    /// 群聊消息（2026 起涵盖 GROUP_AT_MESSAGE_CREATE 和 GROUP_MESSAGE_CREATE）
    pub const GROUP_AT_MESSAGE: u32 = 1 << 25;
    /// 私聊消息
    pub const C2C_MESSAGE: u32 = 1 << 30;
    /// 频道消息
    pub const AT_MESSAGE: u32 = 1 << 9;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qq_user_deserialize_with_bot_field() {
        let json = r#"{"id":"123","username":"test","bot":true}"#;
        let user: QqUser = serde_json::from_str(json).unwrap();
        assert_eq!(user.id, "123");
        assert_eq!(user.username, "test");
        assert_eq!(user.bot, Some(true));
    }

    #[test]
    fn test_qq_user_deserialize_without_bot_field() {
        // 新统一平台响应可能不含 bot 字段
        let json = r#"{"id":"456","username":"TestBot","avatar":"http://example.com/a.png"}"#;
        let user: QqUser = serde_json::from_str(json).unwrap();
        assert_eq!(user.id, "456");
        assert_eq!(user.bot, None);
        assert_eq!(user.avatar, Some("http://example.com/a.png".to_string()));
    }

    #[test]
    fn test_channel_message_event_deserialize() {
        let json = r#"{
            "id": "msg1",
            "channel_id": "ch123",
            "guild_id": "guild456",
            "content": "hello",
            "author": {"id": "u789", "username": "user", "bot": false},
            "timestamp": "2026-01-01T00:00:00+08:00"
        }"#;
        let msg: QqChannelMessageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, "msg1");
        assert_eq!(msg.channel_id, "ch123");
        assert_eq!(msg.content, Some("hello".to_string()));
        assert_eq!(msg.author.id, "u789");
        assert_eq!(msg.author.username, Some("user".to_string()));
    }

    #[test]
    fn test_channel_message_event_without_guild_id() {
        let json = r#"{
            "id": "msg2",
            "channel_id": "ch789",
            "content": null,
            "author": {"id": "u012"},
            "timestamp": "2026-01-01T00:00:00+08:00"
        }"#;
        let msg: QqChannelMessageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(msg.channel_id, "ch789");
        assert!(msg.guild_id.is_none());
        assert!(msg.content.is_none());
        assert_eq!(msg.author.username, None);
        assert!(!msg.author.bot); // default
    }

    #[test]
    fn test_group_message_event_deserialize() {
        let json = r#"{
            "id": "gmsg1",
            "group_openid": "ABCD1234EFGH5678",
            "content": "@bot hello",
            "author": {"member_openid": "MEMBER001"},
            "timestamp": "2026-01-01T00:00:00+08:00"
        }"#;
        let msg: QqGroupMessageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, "gmsg1");
        assert_eq!(msg.group_openid, "ABCD1234EFGH5678");
        assert_eq!(msg.content, Some("@bot hello".to_string()));
        assert_eq!(msg.author.member_openid, "MEMBER001");
    }

    #[test]
    fn test_c2c_message_event_deserialize() {
        let json = r#"{
            "id": "c2cmsg1",
            "content": "private message",
            "author": {"user_openid": "USER001"},
            "timestamp": "2026-01-01T00:00:00+08:00"
        }"#;
        let msg: QqC2cMessageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, "c2cmsg1");
        assert_eq!(msg.author.user_openid, "USER001");
        assert_eq!(msg.content, Some("private message".to_string()));
    }

    #[test]
    fn test_c2c_message_event_without_content() {
        let json = r#"{
            "id": "c2cmsg2",
            "author": {"user_openid": "USER002"},
            "timestamp": "2026-01-01T00:00:00+08:00"
        }"#;
        let msg: QqC2cMessageEvent = serde_json::from_str(json).unwrap();
        assert!(msg.content.is_none());
    }

    #[test]
    fn test_send_message_request_serialize() {
        let req = QqSendMessageRequest {
            content: "hello".into(),
            msg_type: Some(0),
            msg_id: Some("reply123".into()),
            image: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""content":"hello""#));
        assert!(json.contains(r#""msg_type":0"#));
        assert!(json.contains(r#""msg_id":"reply123""#));
        assert!(!json.contains("image"));
    }

    #[test]
    fn test_send_message_response_deserialize() {
        let json = r#"{"id": "send1", "timestamp": "2026-01-01T00:00:00+08:00"}"#;
        let resp: QqSendMessageResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "send1");
        assert!(resp.timestamp.is_some());
    }

    #[test]
    fn test_group_message_create_event_deserialize() {
        let json = r#"{
            "id": "ROBOT1.0_abc123",
            "group_openid": "GROUP_OPENID_001",
            "content": "hello everyone",
            "author": {"member_openid": "MEMBER002"},
            "timestamp": "2026-05-08T13:24:53+08:00",
            "mentions": [
                {"is_you": true, "scope": "single", "username": "EasyBot"}
            ],
            "message_scene": {"source": "default", "ext": ["msg_idx=REFIDX_001"]},
            "message_type": 0
        }"#;
        let msg: QqGroupMessageCreateEvent = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, "ROBOT1.0_abc123");
        assert_eq!(msg.group_openid, "GROUP_OPENID_001");
        assert_eq!(msg.content, Some("hello everyone".to_string()));
        assert_eq!(msg.author.member_openid, "MEMBER002");
        assert_eq!(msg.mentions.len(), 1);
        assert!(msg.mentions[0].is_you);
        assert_eq!(msg.mentions[0].scope, Some("single".to_string()));
        assert_eq!(msg.mentions[0].username, Some("EasyBot".to_string()));
        assert!(msg.message_scene.is_some());
        assert_eq!(msg.message_type, Some(0));
    }

    #[test]
    fn test_group_message_create_event_no_mentions() {
        let json = r#"{
            "id": "ROBOT1.0_noat",
            "group_openid": "GROUP_NOAT",
            "content": "just a message",
            "author": {"member_openid": "MEMBER003"},
            "timestamp": "2026-05-08T14:00:00+08:00"
        }"#;
        let msg: QqGroupMessageCreateEvent = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, "ROBOT1.0_noat");
        assert!(msg.mentions.is_empty());
        assert!(msg.message_scene.is_none());
        assert!(msg.message_type.is_none());
    }

    #[test]
    fn test_qq_guild_deserialize() {
        let json = r#"{"id":"guild123","name":"我的频道"}"#;
        let guild: QqGuild = serde_json::from_str(json).unwrap();
        assert_eq!(guild.id, "guild123");
        assert_eq!(guild.name, "我的频道");
    }

    #[test]
    fn test_keyboard_serialization_callback_button() {
        let keyboard = QqKeyboard {
            content: QqKeyboardContent {
                rows: vec![QqKeyboardRow {
                    buttons: vec![QqKeyboardButton {
                        id: "btn_0_0".to_string(),
                        render_data: QqButtonRenderData {
                            label: "点击我".to_string(),
                            visited_label: "点击我".to_string(),
                            style: 1,
                        },
                        action: QqButtonAction {
                            action_type: 2,
                            permission: QqButtonPermission { permission_type: 2 },
                            data: "/start".to_string(),
                            enter: false,
                        },
                    }],
                }],
            },
        };
        let json = serde_json::to_string(&keyboard).unwrap();
        assert!(json.contains(r#""id":"btn_0_0""#));
        assert!(json.contains(r#""label":"点击我""#));
        assert!(json.contains(r#""type":2"#)); // action type (callback)
        assert!(json.contains(r#""data":"/start""#));
        assert!(!json.contains(r#""enter":true"#));
    }

    #[test]
    fn test_keyboard_serialization_url_button() {
        let keyboard = QqKeyboard {
            content: QqKeyboardContent {
                rows: vec![QqKeyboardRow {
                    buttons: vec![QqKeyboardButton {
                        id: "btn_0_0".to_string(),
                        render_data: QqButtonRenderData {
                            label: "打开链接".to_string(),
                            visited_label: "打开链接".to_string(),
                            style: 1,
                        },
                        action: QqButtonAction {
                            action_type: 0, // URL jump
                            permission: QqButtonPermission { permission_type: 2 },
                            data: "https://example.com".to_string(),
                            enter: false,
                        },
                    }],
                }],
            },
        };
        let json = serde_json::to_string(&keyboard).unwrap();
        assert!(json.contains(r#""type":0"#)); // action type (URL jump)
        assert!(json.contains(r#""data":"https://example.com""#));
    }

    #[test]
    fn test_keyboard_serialization_multi_row() {
        let keyboard = QqKeyboard {
            content: QqKeyboardContent {
                rows: vec![
                    QqKeyboardRow {
                        buttons: vec![QqKeyboardButton {
                            id: "btn_0_0".to_string(),
                            render_data: QqButtonRenderData {
                                label: "按钮1".to_string(),
                                visited_label: "按钮1".to_string(),
                                style: 1,
                            },
                            action: QqButtonAction {
                                action_type: 2,
                                permission: QqButtonPermission { permission_type: 2 },
                                data: "cb_1".to_string(),
                                enter: false,
                            },
                        }],
                    },
                    QqKeyboardRow {
                        buttons: vec![
                            QqKeyboardButton {
                                id: "btn_1_0".to_string(),
                                render_data: QqButtonRenderData {
                                    label: "按钮2".to_string(),
                                    visited_label: "按钮2".to_string(),
                                    style: 1,
                                },
                                action: QqButtonAction {
                                    action_type: 2,
                                    permission: QqButtonPermission { permission_type: 2 },
                                    data: "cb_2".to_string(),
                                    enter: false,
                                },
                            },
                            QqKeyboardButton {
                                id: "btn_1_1".to_string(),
                                render_data: QqButtonRenderData {
                                    label: "按钮3".to_string(),
                                    visited_label: "按钮3".to_string(),
                                    style: 1,
                                },
                                action: QqButtonAction {
                                    action_type: 2,
                                    permission: QqButtonPermission { permission_type: 2 },
                                    data: "cb_3".to_string(),
                                    enter: false,
                                },
                            },
                        ],
                    },
                ],
            },
        };
        let json = serde_json::to_string(&keyboard).unwrap();
        // 验证两行都存在
        assert!(json.contains(r#""id":"btn_0_0""#));
        assert!(json.contains(r#""id":"btn_1_0""#));
        assert!(json.contains(r#""id":"btn_1_1""#));
        assert!(json.contains(r#""label":"按钮1""#));
        assert!(json.contains(r#""label":"按钮2""#));
        assert!(json.contains(r#""label":"按钮3""#));
    }
}
