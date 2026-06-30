//! 会话模型
//!
//! 定义会话（Session）的数据结构，用于持久化对话上下文。
//! 会话以 session_key（platform:chatId[:threadId]）作为唯一标识。

use crate::types::message::{ChatType, SenderRole};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 会话
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Session {
    /// 会话键，格式 "platform:chatId" 或 "platform:chatId:threadId"
    pub key: String,
    /// 平台标识
    pub platform: String,
    /// 聊天 ID
    pub chat_id: String,
    /// 话题 ID（可选）
    pub thread_id: Option<String>,
    /// 创建时间戳（毫秒）
    pub created_at: i64,
    /// 最后更新时间戳（毫秒）
    pub updated_at: i64,
    /// 会话来源信息
    pub source: SessionSource,
    /// 重置策略
    #[serde(default)]
    pub reset_policy: ResetPolicy,
    /// 自定义元数据
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// 会话来源
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionSource {
    /// 平台标识
    pub platform: String,
    /// 聊天 ID
    pub chat_id: String,
    /// 聊天名称
    pub chat_name: Option<String>,
    /// 聊天类型
    pub chat_type: ChatType,
    /// 用户 ID
    pub user_id: Option<String>,
    /// 用户名
    pub user_name: Option<String>,
    /// 是否为机器人
    pub is_bot: bool,
    /// 平台特有用户名/句柄
    pub user_username: Option<String>,
    /// 发送者角色
    pub user_role: Option<SenderRole>,
}

/// 会话重置策略
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, Default, PartialEq)]
pub enum ResetPolicy {
    /// 从不重置
    #[default]
    Never,
    /// 1 小时后重置
    After1h,
    /// 24 小时后重置
    After24h,
    /// 50 条消息后重置
    After50Msgs,
    /// 每天重置
    Daily,
    /// 手动重置
    Manual,
}

impl Session {
    /// 计算会话键
    pub fn build_key(platform: &str, chat_id: &str, thread_id: Option<&str>) -> String {
        match thread_id {
            Some(tid) if !tid.is_empty() => format!("{}:{}:{}", platform, chat_id, tid),
            _ => format!("{}:{}", platform, chat_id),
        }
    }
}

/// 会话过滤器
#[derive(Debug, Default)]
pub struct SessionFilter {
    pub platform: Option<String>,
    pub active_within_minutes: Option<u64>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
