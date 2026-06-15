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

/// 消息事件数据（AT_MESSAGE_CREATE / C2C_MESSAGE_CREATE / GROUP_AT_MESSAGE_CREATE）
#[derive(Debug, Deserialize)]
pub struct QqMessageEvent {
    pub id: String,
    pub channel_id: String,
    pub guild_id: Option<String>,
    pub group_id: Option<String>,
    pub content: Option<String>,
    pub author: QqMessageAuthor,
    pub timestamp: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct QqMessageAuthor {
    pub id: String,
    pub username: Option<String>,
    pub bot: bool,
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
    /// 群聊@消息
    pub const GROUP_AT_MESSAGE: u32 = 1 << 25;
    /// 私聊消息
    pub const C2C_MESSAGE: u32 = 1 << 30;
    /// 频道@消息
    pub const AT_MESSAGE: u32 = 1 << 9;
}
