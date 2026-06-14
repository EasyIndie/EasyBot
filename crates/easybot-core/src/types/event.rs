//! 事件模型
//!
//! 定义网关内部事件总线的数据类型。
//! 事件用于组件间解耦通信，支持广播到多个订阅者。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 网关内部事件
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GatewayEvent {
    /// 事件类型，如 "message.inbound", "adapter.connected"
    #[serde(rename = "event")]
    pub event_type: String,
    /// 事件来源标识
    pub source: String,
    /// 事件时间戳（毫秒）
    pub timestamp: i64,
    /// 事件数据（按类型不同）
    pub data: serde_json::Value,
    /// 事件元数据
    pub metadata: Option<EventMetadata>,
}

/// 事件元数据
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EventMetadata {
    /// 关联 ID，用于追踪消息链路
    pub correlation_id: Option<String>,
    /// 关联的会话键
    pub session_key: Option<String>,
}

impl GatewayEvent {
    /// 构造新事件
    pub fn new(
        event_type: impl Into<String>,
        source: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            event_type: event_type.into(),
            source: source.into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            data,
            metadata: None,
        }
    }

    /// 设置元数据
    pub fn with_metadata(mut self, metadata: EventMetadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// 标准事件类型常量
pub mod event_types {
    /// 收到 IM 平台消息
    pub const MESSAGE_INBOUND: &str = "message.inbound";
    /// 消息发送成功
    pub const MESSAGE_SENT: &str = "message.sent";
    /// 消息发送失败
    pub const MESSAGE_FAILED: &str = "message.failed";
    /// 适配器连接成功
    pub const ADAPTER_CONNECTED: &str = "adapter.connected";
    /// 适配器断开连接
    pub const ADAPTER_DISCONNECTED: &str = "adapter.disconnected";
    /// 适配器异常
    pub const ADAPTER_ERROR: &str = "adapter.error";
    /// 收到按钮回调
    pub const CALLBACK_RECEIVED: &str = "callback.received";
    /// 网关启动
    pub const GATEWAY_STARTED: &str = "gateway.started";
    /// 网关关闭
    pub const GATEWAY_STOPPING: &str = "gateway.stopping";
    /// 配置变更
    pub const CONFIG_CHANGED: &str = "config.changed";

    /// 返回所有预定义事件类型列表
    pub fn all() -> &'static [&'static str] {
        &[
            MESSAGE_INBOUND,
            MESSAGE_SENT,
            MESSAGE_FAILED,
            ADAPTER_CONNECTED,
            ADAPTER_DISCONNECTED,
            ADAPTER_ERROR,
            CALLBACK_RECEIVED,
            GATEWAY_STARTED,
            GATEWAY_STOPPING,
            CONFIG_CHANGED,
        ]
    }
}
