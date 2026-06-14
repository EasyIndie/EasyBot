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

/// 消息事件（入站，预留用于 Webhook/WebSocket 事件接收）
#[derive(Debug, Deserialize)]
pub struct FeishuMessageEvent {
    pub sender: FeishuSender,
    pub message: FeishuReceivedMessage,
}

#[derive(Debug, Deserialize)]
pub struct FeishuSender {
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
    pub msg_type: String,
    pub content: String,
    pub create_time: String,
}
