//! Telegram Bot API 响应类型
//!
//! 定义用于反序列化 Telegram Bot API 响应数据的数据结构。
//! 仅涵盖当前使用的端点（getMe、getUpdates、sendMessage）。
//!
//! #!\[allow(dead_code)\]：所有字段仅用于 JSON 反序列化，读取部分由使用方决定

#![allow(dead_code)]

use serde::Deserialize;

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
}

/// Telegram 消息
#[derive(Debug, Deserialize, Clone)]
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
}

/// Telegram 用户
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct TelegramUser {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

/// Telegram 聊天
#[derive(Debug, Deserialize, Clone)]
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
#[derive(Debug, Deserialize, Clone)]
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
