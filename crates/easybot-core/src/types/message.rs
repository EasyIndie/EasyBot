//! 消息模型
//!
//! 定义入站消息（IM 平台 → 网关）、出站消息（网关 → IM 平台）、
//! 媒体附件、交互式按钮等数据模型。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 入站消息（从 IM 平台接收的消息）
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InboundMessage {
    /// 平台消息 ID
    pub id: String,
    /// 来源平台标识
    pub platform: String,

    // ── 消息类型与内容 ──
    /// 消息内容类型
    pub msg_type: MessageType,
    /// 消息文本内容
    pub text: Option<String>,
    /// 媒体附件
    pub media: Option<Vec<MediaAttachment>>,

    // ── 收发双方 ──
    /// 消息发送者
    #[serde(alias = "author")]
    pub sender: MessageSender,
    /// 接收该消息的机器人 ID（用于多 bot 路由）
    pub recipient: Option<String>,

    // ── 聊天上下文 ──
    /// 来源聊天 ID
    pub chat_id: String,
    /// 聊天名称
    pub chat_name: Option<String>,
    /// 聊天类型
    pub chat_type: ChatType,
    /// 所属服务器/频道 ID（Discord guild_id, QQ channel guild_id）
    pub guild_id: Option<String>,
    /// 话题/子线程 ID
    pub thread_id: Option<String>,
    /// 线程根消息 ID（飞书 root_id）
    pub root_id: Option<String>,

    // ── 时间 ──
    /// 消息时间戳（毫秒）
    pub timestamp: i64,

    // ── 交互 ──
    /// 斜杠命令
    pub command: Option<CommandData>,
    /// 按钮回调
    pub callback: Option<CallbackData>,
    /// 回复引用
    pub reply_to: Option<MessageReference>,
    /// @提及列表
    pub mentions: Option<Vec<MentionInfo>>,
    /// 是否 @了机器人（仅群聊场景有意义，None 表示不适用或未知）
    #[serde(default)]
    pub mentioned: Option<bool>,

    // ── 平台扩展 ──
    /// 平台特有元数据
    pub metadata: Option<serde_json::Value>,
}

/// 消息内容类型（平台无关）
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub enum MessageType {
    /// 纯文本
    Text,
    /// 图片
    Image,
    /// 音频
    Audio,
    /// 视频
    Video,
    /// 文件
    File,
    /// 贴纸
    Sticker,
    /// 动画/GIF
    Animation,
    /// 富文本（飞书 post）
    RichText,
    /// 交互式消息卡片
    Interactive,
    /// 分享（群/用户名片）
    Share,
    /// 位置
    Location,
    /// 联系人名片
    Contact,
    /// 链接预览
    Link,
    /// 系统消息（成员加入/离开等）
    System,
    /// 未识别或未知的消息类型
    Unknown,
}

/// 发送者角色
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub enum SenderRole {
    /// 管理员
    Admin,
    /// 普通成员
    Member,
    /// 群主/所有者
    Owner,
    /// 机器人/应用
    Bot,
    /// 匿名用户
    Anonymous,
}

/// 消息发送者（增强版，替换 MessageAuthor）
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MessageSender {
    /// 平台用户 ID
    pub id: String,
    /// 显示名称
    pub name: Option<String>,
    /// 平台特有用户名/句柄（telegram @username, discord username）
    pub username: Option<String>,
    /// 头像 URL
    pub avatar_url: Option<String>,
    /// 是否为机器人
    pub is_bot: bool,
    /// 发送者角色
    pub role: Option<SenderRole>,
    /// 语言代码（IETF tag，Telegram 特有）
    pub language_code: Option<String>,
}

/// @提及信息
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MentionInfo {
    /// 被提及的用户 ID（平台原始 ID）
    pub user_id: Option<String>,
    /// 被提及的用户名
    pub username: Option<String>,
    /// 提及范围：single / all / here
    pub scope: Option<String>,
}

/// 消息作者（已弃用，请使用 MessageSender）
#[deprecated(since = "0.3.0", note = "use MessageSender instead")]
pub type MessageAuthor = MessageSender;

/// 出站消息（发往 IM 平台的消息）
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OutboundMessage {
    /// 消息文本
    pub text: String,
    /// 文本解析模式
    #[serde(default)]
    pub parse_mode: ParseMode,
}

/// 发送文本消息参数
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SendTextParams {
    /// 目标聊天 ID
    pub chat_id: String,
    /// 消息内容
    pub message: OutboundMessage,
    /// 被回复消息 ID（可选）
    pub reply_to: Option<String>,
    /// 平台特有元数据
    pub metadata: Option<serde_json::Value>,
}

/// 发送媒体消息参数
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SendMediaParams {
    /// 目标聊天 ID
    pub chat_id: String,
    /// 媒体附件
    pub media: MediaAttachment,
    /// 文本说明（可选）
    pub text: Option<String>,
    /// 被回复消息 ID（可选）
    pub reply_to: Option<String>,
}

/// 发送交互式消息（带按钮）参数
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SendInteractiveParams {
    /// 目标聊天 ID
    pub chat_id: String,
    /// 消息文本
    pub text: String,
    /// 行内键盘
    pub keyboard: InlineKeyboard,
    /// 被回复消息 ID（可选）
    pub reply_to: Option<String>,
}

/// 编辑消息参数
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EditMessageParams {
    /// 目标聊天 ID
    pub chat_id: String,
    /// 平台消息 ID
    pub message_id: String,
    /// 新消息内容
    pub message: OutboundMessage,
    /// 更新后的键盘（可选）
    pub keyboard: Option<InlineKeyboard>,
}

/// 发送结果
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SendResult {
    #[schema(example = true)]
    pub success: bool,
    #[schema(example = "msg_abc123")]
    pub message_id: Option<String>,
    pub timestamp: Option<i64>,
    #[schema(example = "null")]
    pub error: Option<String>,
    #[schema(example = "null")]
    pub error_code: Option<String>,
    #[schema(example = false)]
    pub retryable: bool,
}

impl SendResult {
    /// 构造成功结果
    pub fn ok(message_id: String) -> Self {
        Self {
            success: true,
            message_id: Some(message_id),
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            error: None,
            error_code: None,
            retryable: false,
        }
    }

    /// 构造失败结果
    pub fn fail(error: impl Into<String>, retryable: bool) -> Self {
        Self {
            success: false,
            message_id: None,
            timestamp: None,
            error: Some(error.into()),
            error_code: None,
            retryable,
        }
    }
}

/// 编辑结果
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EditResult {
    pub success: bool,
    pub updated_at: Option<i64>,
    pub error: Option<String>,
}

/// 删除结果
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeleteResult {
    pub success: bool,
    pub error: Option<String>,
}

/// 流式草稿发送参数
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SendDraftParams {
    /// 目标聊天 ID
    pub chat_id: String,
    /// 已有消息 ID（更新草稿），None 则创建新消息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// 当前草稿文本
    pub text: String,
    /// 解析模式（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<ParseMode>,
    /// 被回复消息 ID（可选，仅新建时有效）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// 流式草稿结果
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DraftResult {
    pub success: bool,
    /// 消息 ID（新建时返回，更新时回传）
    pub message_id: Option<String>,
    pub error: Option<String>,
}

// ── 支持类型 ──

/// 聊天类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq)]
pub enum ChatType {
    /// 私聊
    Dm,
    /// 群组
    Group,
    /// 频道
    Channel,
    /// 话题/子线程
    Thread,
}

/// 媒体附件
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MediaAttachment {
    pub media_type: MediaType,
    pub url: Option<String>,
    pub data: Option<String>, // Base64 编码数据（小型文件）
    pub mime_type: String,
    pub filename: Option<String>,
    pub caption: Option<String>,
    pub thumbnail_url: Option<String>,
    pub file_size: Option<u64>,
    pub duration: Option<f64>, // 音频/视频时长（秒）
}

/// 媒体类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq)]
pub enum MediaType {
    Image,
    Audio,
    Video,
    Document,
    Sticker,
    Animation,
}

/// 斜杠命令
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CommandData {
    pub name: String,
    pub args: String,
}

/// 按钮回调
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CallbackData {
    pub data: String,
    pub message_id: String,
}

/// 消息引用（回复链）
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MessageReference {
    pub message_id: String,
    pub text: Option<String>,
}

/// 文本解析模式
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ParseMode {
    /// Markdown 格式
    Markdown,
    /// HTML 格式
    Html,
    /// 纯文本（不解析）
    #[default]
    None,
}

/// 行内键盘（按钮布局）
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct InlineKeyboard {
    pub rows: Vec<KeyboardRow>,
}

/// 键盘行
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct KeyboardRow {
    pub buttons: Vec<Button>,
}

/// 按钮
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Button {
    pub text: String,
    pub callback_data: Option<String>,
    pub url: Option<String>,
}

/// 按钮回调事件
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CallbackEvent {
    pub id: String,
    pub platform: String,
    pub chat_id: String,
    pub user_id: String,
    pub data: String,
    pub message_id: String,
    pub metadata: Option<serde_json::Value>,
}

/// 聊天信息
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChatInfo {
    pub chat_id: String,
    pub name: Option<String>,
    pub chat_type: ChatType,
    pub member_count: Option<u32>,
}

/// 聊天过滤器
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChatFilter {
    pub chat_type: Option<ChatType>,
    pub query: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mode_default_is_none() {
        // ParseMode 默认应为 None，保证不传 parse_mode 时不触发 Markdown 转义
        let mode = ParseMode::default();
        assert_eq!(mode, ParseMode::None);
    }

    #[test]
    fn test_parse_mode_serde_lowercase() {
        // API JSON 使用小写枚举值
        let json = r#""markdown""#;
        let mode: ParseMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode, ParseMode::Markdown);

        let json = r#""html""#;
        let mode: ParseMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode, ParseMode::Html);

        let json = r#""none""#;
        let mode: ParseMode = serde_json::from_str(json).unwrap();
        assert_eq!(mode, ParseMode::None);
    }

    #[test]
    fn test_parse_mode_serde_roundtrip() {
        let mode = ParseMode::Markdown;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""markdown""#);

        let mode = ParseMode::None;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, r#""none""#);
    }

    #[test]
    fn test_inbound_message_serde_alias_author() {
        // 兼容旧 JSON 格式：author → sender 通过 serde(alias) 反序列化
        let old_json = serde_json::json!({
            "id": "msg1",
            "platform": "test",
            "msg_type": "Text",
            "text": "hello",
            "author": {
                "id": "user1",
                "name": "Alice",
                "is_bot": false
            },
            "chat_id": "chat1",
            "chat_type": "Dm",
            "timestamp": 1000
        });
        let msg: InboundMessage = serde_json::from_value(old_json).unwrap();
        assert_eq!(msg.sender.id, "user1");
        assert_eq!(msg.sender.name, Some("Alice".to_string()));
        assert_eq!(msg.msg_type, MessageType::Text);
    }

    #[test]
    fn test_inbound_message_serde_new_format() {
        // 新 JSON 格式
        let new_json = serde_json::json!({
            "id": "msg1",
            "platform": "test",
            "msg_type": "Image",
            "text": "hello",
            "sender": {
                "id": "user1",
                "name": "Alice",
                "username": "alice_bot",
                "is_bot": false
            },
            "chat_id": "chat1",
            "chat_type": "Dm",
            "timestamp": 1000
        });
        let msg: InboundMessage = serde_json::from_value(new_json).unwrap();
        assert_eq!(msg.sender.id, "user1");
        assert_eq!(msg.sender.username, Some("alice_bot".to_string()));
        assert_eq!(msg.msg_type, MessageType::Image);
    }
}
