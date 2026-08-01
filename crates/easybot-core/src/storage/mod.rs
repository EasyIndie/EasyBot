//! 持久化存储抽象层
#![allow(missing_docs)]
//!
//! 定义存储 trait 和共享模型，支持 SQLite 和未来 PostgreSQL 后端。
//! SessionManager 使用 SessionStore 做持久化写入；
//! MessageStore 用于消息历史存储。

pub mod migration;
pub mod postgres;
pub mod retention;
pub mod sqlite;

use crate::types::message::{InboundMessage, SendResult};
use crate::types::session::{Session, SessionFilter};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ── StoreError ──

/// 存储层统一错误
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        StoreError::Database(e.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Serialization(e.to_string())
    }
}

// ── SessionStore Trait ──

/// 会话存储接口
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// 创建或更新会话
    async fn upsert_session(&self, session: &Session) -> Result<(), StoreError>;

    /// 获取单个会话
    async fn get_session(&self, key: &str) -> Result<Option<Session>, StoreError>;

    /// 删除会话
    async fn delete_session(&self, key: &str) -> Result<bool, StoreError>;

    /// 列出会话（支持过滤）
    async fn list_sessions(&self, filter: &SessionFilter) -> Result<Vec<Session>, StoreError>;

    /// 统计会话数量
    async fn count_sessions(&self) -> Result<i64, StoreError>;

    /// 加载所有会话（启动时使用）
    async fn load_all_sessions(&self) -> Result<Vec<Session>, StoreError>;

    /// 删除 updated_at 早于 before 的过期会话
    /// 返回删除的行数。默认实现返回 0（不执行清理）。
    async fn delete_expired_sessions(&self, _before: i64) -> Result<u64, StoreError> {
        Ok(0)
    }
}

// ── MessageStore Trait ──

/// 消息存储接口
#[async_trait]
pub trait MessageStore: Send + Sync {
    /// Verify that the backing message database is reachable.
    async fn storage_ready(&self) -> bool {
        self.list_messages(&MessageFilter {
            limit: Some(1),
            ..MessageFilter::default()
        })
        .await
        .is_ok()
    }

    /// Persist an outbound delivery intent before contacting the external platform.
    async fn prepare_outbound_delivery(
        &self,
        _delivery: &OutboundDelivery,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database(
            "outbound delivery journal is not supported by this store".into(),
        ))
    }

    /// Atomically store the final platform result and its message-history row.
    async fn finalize_outbound_delivery(
        &self,
        _delivery_id: &str,
        _state: OutboundDeliveryState,
        _result: &serde_json::Value,
        _message: Option<&StoredMessage>,
    ) -> Result<(), StoreError> {
        Err(StoreError::Database(
            "outbound delivery journal is not supported by this store".into(),
        ))
    }

    async fn unpublished_outbound_events(
        &self,
        _limit: usize,
    ) -> Result<Vec<OutboundEvent>, StoreError> {
        Ok(Vec::new())
    }

    async fn mark_outbound_event_published(&self, _delivery_id: &str) -> Result<(), StoreError> {
        Ok(())
    }

    async fn list_outbound_deliveries(
        &self,
        _actor_id: &str,
        _limit: usize,
    ) -> Result<Vec<OutboundDeliveryRecord>, StoreError> {
        Ok(Vec::new())
    }

    async fn reconcile_outbound_delivery(
        &self,
        _delivery_id: &str,
        _actor_id: &str,
        _state: OutboundDeliveryState,
        _evidence: &str,
        _reconciled_by: &str,
    ) -> Result<bool, StoreError> {
        Ok(false)
    }

    async fn list_outbound_deliveries_by_session(
        &self,
        _platform: &str,
        _chat_id: &str,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<OutboundDeliveryRecord>, StoreError> {
        Ok(Vec::new())
    }

    async fn delete_outbound_deliveries_by_session(
        &self,
        _platform: &str,
        _chat_id: &str,
    ) -> Result<u64, StoreError> {
        Ok(0)
    }

    async fn delete_expired_outbound_deliveries(&self, _before: i64) -> Result<u64, StoreError> {
        Ok(0)
    }

    async fn outbound_delivery_stats(
        &self,
        _stale_before: i64,
    ) -> Result<OutboundDeliveryStats, StoreError> {
        Ok(OutboundDeliveryStats::default())
    }

    /// 存储一条消息
    async fn store_message(&self, msg: &StoredMessage) -> Result<(), StoreError>;

    /// 批量存储消息
    async fn store_messages(&self, msgs: &[StoredMessage]) -> Result<(), StoreError>;

    /// 列出消息（支持过滤/分页）
    async fn list_messages(&self, filter: &MessageFilter)
    -> Result<Vec<StoredMessage>, StoreError>;

    /// 删除单条消息
    async fn delete_message(&self, id: &str) -> Result<bool, StoreError>;

    /// Delete all message bodies and raw payloads belonging to one session.
    async fn delete_messages_by_session(&self, session_key: &str) -> Result<u64, StoreError>;

    /// 删除 created_at 早于 before 的过期消息
    /// 返回删除的行数。默认实现返回 0（不执行清理）。
    async fn delete_expired_messages(&self, _before: i64) -> Result<u64, StoreError> {
        Ok(0)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutboundDeliveryState {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundDelivery {
    pub id: String,
    pub actor_id: String,
    pub idempotency_key: Option<String>,
    pub platform: String,
    pub chat_id: String,
    pub request_json: serde_json::Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundEvent {
    pub delivery_id: String,
    pub platform: String,
    pub chat_id: String,
    pub state: OutboundDeliveryState,
    pub result_json: serde_json::Value,
    pub completed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundDeliveryRecord {
    pub id: String,
    pub actor_id: String,
    pub idempotency_key: Option<String>,
    pub platform: String,
    pub chat_id: String,
    pub request_json: serde_json::Value,
    pub state: String,
    pub result_json: Option<serde_json::Value>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub reconciliation_evidence: Option<String>,
    pub reconciled_by: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboundDeliveryStats {
    pub pending: u64,
    pub stale_pending: u64,
    pub unpublished_events: u64,
}

// ── StoredMessage ──

/// 消息方向
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    /// 用户发送的（入站）
    User,
    /// 机器人回复的（出站）
    Assistant,
}

/// 已存储的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    /// 消息 ID（平台 ID 或合成 ID）
    pub id: String,
    /// 关联的会话键
    pub session_key: String,
    /// 平台标识
    pub platform: String,
    /// 聊天 ID
    pub chat_id: String,
    /// 消息方向
    pub role: MessageRole,
    /// 文本内容（提取用于查询）
    pub text: Option<String>,
    /// 完整原始消息 JSON
    pub raw_data: serde_json::Value,
    /// 原始消息时间戳（毫秒）
    pub timestamp: i64,
    /// 存储时间戳（毫秒）
    pub created_at: i64,
}

impl StoredMessage {
    /// 从入站消息构建
    pub fn from_inbound(msg: &InboundMessage) -> Self {
        let session_key = Session::build_key(&msg.platform, &msg.chat_id, msg.thread_id.as_deref());
        let raw = serde_json::to_value(msg).unwrap_or_default();
        Self {
            id: format!("inbound:{}:{}", msg.platform, msg.id),
            session_key,
            platform: msg.platform.to_string(),
            chat_id: msg.chat_id.clone(),
            role: MessageRole::User,
            text: msg.text.clone(),
            raw_data: raw,
            timestamp: msg.timestamp,
            created_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// 从出站发送结果构建
    pub fn from_outbound(
        platform: &str,
        chat_id: &str,
        thread_id: Option<&str>,
        text: &str,
        result: &SendResult,
    ) -> Self {
        let session_key = Session::build_key(platform, chat_id, thread_id);
        let raw = serde_json::json!({
            "text": text,
            "result": result,
            "platform": platform,
            "chat_id": chat_id,
        });
        Self {
            // Platform message IDs are not guaranteed to be globally unique
            // (and mocks/legacy platforms may reuse them across chats). Keep
            // them in raw_data.result, but always allocate an independent
            // local record ID so one outbound send can never overwrite another.
            id: format!("outbound:{}", uuid::Uuid::now_v7()),
            session_key,
            platform: platform.to_string(),
            chat_id: chat_id.to_string(),
            role: MessageRole::Assistant,
            text: Some(text.to_string()),
            raw_data: raw,
            timestamp: result
                .timestamp
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
            created_at: chrono::Utc::now().timestamp_millis(),
        }
    }
}

// ── MessageFilter ──

/// 消息查询过滤器
#[derive(Debug, Default)]
pub struct MessageFilter {
    /// 按会话键过滤
    pub session_key: Option<String>,
    /// 按平台过滤
    pub platform: Option<String>,
    /// 按聊天 ID 过滤
    pub chat_id: Option<String>,
    /// 分页大小
    pub limit: Option<usize>,
    /// 分页偏移（基于 ID 的 offset，配合 before 使用游标分页）
    pub offset: Option<usize>,
    /// 返回此时间戳之前的消息（游标分页）
    pub before: Option<i64>,
}
