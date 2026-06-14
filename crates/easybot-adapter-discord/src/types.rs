//! Discord Bot API 和 Gateway 协议数据类型
//!
//! 定义用于反序列化 Discord REST API 响应及 WebSocket Gateway 消息的数据结构。
//! 涵盖 getMe、sendMessage、getChannel、MESSAGE_CREATE 等主要端点。

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ── Gateway Opcodes ──

pub(crate) const OP_DISPATCH: u8 = 0;
pub(crate) const OP_HEARTBEAT: u8 = 1;
pub(crate) const OP_IDENTIFY: u8 = 2;
pub(crate) const OP_RESUME: u8 = 6;
pub(crate) const OP_RECONNECT: u8 = 7;
pub(crate) const OP_INVALID_SESSION: u8 = 9;
pub(crate) const OP_HELLO: u8 = 10;
pub(crate) const OP_HEARTBEAT_ACK: u8 = 11;

// ── Gateway Intents ──

pub(crate) const INTENT_GUILDS: u64 = 1 << 0;
pub(crate) const INTENT_GUILD_MESSAGES: u64 = 1 << 9;
pub(crate) const INTENT_DIRECT_MESSAGES: u64 = 1 << 12;
pub(crate) const INTENT_GUILD_MESSAGE_TYPING: u64 = 1 << 13;
pub(crate) const INTENT_DIRECT_MESSAGE_TYPING: u64 = 1 << 14;
pub(crate) const INTENT_MESSAGE_CONTENT: u64 = 1 << 15;

/// Phase 3 默认 intents：接收群组/私聊消息及消息内容
pub(crate) const DEFAULT_INTENTS: u64 =
    INTENT_GUILD_MESSAGES | INTENT_DIRECT_MESSAGES | INTENT_MESSAGE_CONTENT;

// ── Gateway Payload (通用) ──

/// Gateway WebSocket 消息帧（收/发共用）
#[derive(Debug, Deserialize)]
pub(crate) struct GatewayPayload {
    pub op: u8,
    #[serde(default)]
    pub d: Option<serde_json::Value>,
    #[serde(default)]
    pub s: Option<u64>,
    #[serde(default)]
    pub t: Option<String>,
}

// ── Hello / Ready ──

/// Hello 事件的 data 字段
#[derive(Debug, Deserialize)]
pub(crate) struct HelloData {
    pub heartbeat_interval: u64,
}

/// Ready 事件的 data 字段
#[derive(Debug, Deserialize)]
pub(crate) struct ReadyData {
    pub v: u64,
    pub user: DiscordUser,
    pub session_id: String,
    #[serde(default)]
    pub resume_gateway_url: Option<String>,
}

// ── REST API 公共类型 ──

/// Discord 用户对象
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DiscordUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    pub bot: bool,
    #[serde(default)]
    pub avatar: Option<String>,
}

/// Discord 频道对象
#[derive(Debug, Deserialize)]
pub(crate) struct DiscordChannel {
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
}

/// Discord 消息对象（Gateway MESSAGE_CREATE & REST 响应共用）
#[derive(Debug, Deserialize)]
pub(crate) struct DiscordMessage {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    pub author: DiscordUser,
    #[serde(default)]
    pub content: Option<String>,
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub mention_everyone: bool,
    #[serde(default)]
    pub tts: bool,
    #[serde(rename = "type")]
    #[serde(default)]
    pub msg_type: u8,
}

/// 用于 Identify 的序列化结构
#[derive(Debug, Serialize)]
pub(crate) struct IdentifyPayload<'a> {
    pub token: &'a str,
    pub intents: u64,
    pub properties: IdentifyProperties<'a>,
}

/// Identify 中的连接属性
#[derive(Debug, Serialize)]
pub(crate) struct IdentifyProperties<'a> {
    #[serde(rename = "$os")]
    pub os: &'a str,
    #[serde(rename = "$browser")]
    pub browser: &'a str,
    #[serde(rename = "$device")]
    pub device: &'a str,
}

/// 用于发送心跳的序列化结构
#[derive(Debug, Serialize)]
pub(crate) struct HeartbeatPayload {
    pub op: u8,
    pub d: Option<u64>,
}
