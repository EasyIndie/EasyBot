//! Telegram Bot API 响应类型
//!
//! 定义用于反序列化 Telegram Bot API 响应数据的数据结构。
//! 仅涵盖当前使用的端点（getMe、getUpdates、sendMessage）。
//!
//! #!\[allow(dead_code)\]：所有字段仅用于 JSON 反序列化，读取部分由使用方决定

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Telegram API 通用响应包装
#[derive(Debug, Deserialize)]
pub(crate) struct TelegramApiResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
    pub error_code: Option<i32>,
}

/// getUpdates 返回的更新对象
#[derive(Debug, Deserialize)]
pub(crate) struct TelegramUpdate {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<TelegramMessage>,
    /// 群组成员变更事件（管理员升/降级等）
    #[serde(default)]
    pub chat_member: Option<TelegramChatMemberUpdated>,
}

/// getChatAdministrators 返回的管理员条目 / chat_member 事件中的成员信息
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct TelegramChatMember {
    pub status: String,
    pub user: TelegramUser,
    #[serde(default)]
    pub is_anonymous: Option<bool>,
}

/// chat_member 更新事件（getUpdates 的 chat_member 字段）
#[derive(Debug, Deserialize)]
pub(crate) struct TelegramChatMemberUpdated {
    pub chat: TelegramChat,
    pub from: TelegramUser,
    pub old_chat_member: TelegramChatMember,
    pub new_chat_member: TelegramChatMember,
}

/// Telegram 消息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramMessage {
    pub message_id: i64,
    #[serde(default)]
    pub from: Option<TelegramUser>,
    pub chat: TelegramChat,
    /// Unix 时间戳
    pub date: i64,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub entities: Option<Vec<TelegramMessageEntity>>,
    #[serde(default)]
    pub reply_to_message: Option<Box<TelegramMessage>>,
    #[serde(default)]
    pub caption: Option<String>,
    // ── 媒体字段 ──
    /// 图片（最大分辨率在最后）
    #[serde(default)]
    pub photo: Option<Vec<TelegramPhotoSize>>,
    /// 通用文件
    #[serde(default)]
    pub document: Option<TelegramDocument>,
    /// 视频
    #[serde(default)]
    pub video: Option<TelegramVideo>,
    /// 音频
    #[serde(default)]
    pub audio: Option<TelegramAudio>,
    /// 语音消息
    #[serde(default)]
    pub voice: Option<TelegramVoice>,
    /// 贴纸
    #[serde(default)]
    pub sticker: Option<TelegramSticker>,
    /// 动画/GIF
    #[serde(default)]
    pub animation: Option<TelegramAnimation>,
    /// 视频笔记
    #[serde(default)]
    pub video_note: Option<TelegramVideoNote>,
}

/// Telegram 用户
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub language_code: Option<String>,
}

/// Telegram 聊天
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

/// 消息实体（用于检测命令、格式化等）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramMessageEntity {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub offset: i64,
    pub length: i64,
}

/// Bot 信息（getMe 响应）
#[derive(Debug, Deserialize)]
pub(crate) struct TelegramBotInfo {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

// ─── Telegram 媒体消息类型 ─────────────────────────

/// 照片尺寸
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramPhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i64,
    pub height: i64,
    #[serde(default)]
    pub file_size: Option<i64>,
}

/// 文档/文件
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramDocument {
    pub file_id: String,
    pub file_unique_id: String,
    #[serde(default)]
    pub thumbnail: Option<TelegramPhotoSize>,
    #[serde(default)]
    pub file_name: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<i64>,
}

/// 视频
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramVideo {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i64,
    pub height: i64,
    pub duration: i64,
    #[serde(default)]
    pub thumbnail: Option<TelegramPhotoSize>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<i64>,
}

/// 音频
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramAudio {
    pub file_id: String,
    pub file_unique_id: String,
    pub duration: i64,
    #[serde(default)]
    pub performer: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<i64>,
}

/// 语音
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramVoice {
    pub file_id: String,
    pub file_unique_id: String,
    pub duration: i64,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<i64>,
}

/// 贴纸
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramSticker {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i64,
    pub height: i64,
    pub is_animated: bool,
    #[serde(default)]
    pub is_video: bool,
    #[serde(default)]
    pub thumbnail: Option<TelegramPhotoSize>,
    #[serde(default)]
    pub file_size: Option<i64>,
}

/// 动画/GIF
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramAnimation {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i64,
    pub height: i64,
    pub duration: i64,
    #[serde(default)]
    pub thumbnail: Option<TelegramPhotoSize>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<i64>,
}

/// 视频笔记（圆形视频）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TelegramVideoNote {
    pub file_id: String,
    pub file_unique_id: String,
    pub length: i64,
    pub duration: i64,
    #[serde(default)]
    pub thumbnail: Option<TelegramPhotoSize>,
    #[serde(default)]
    pub file_size: Option<i64>,
}
