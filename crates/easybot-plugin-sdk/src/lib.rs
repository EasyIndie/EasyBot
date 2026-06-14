//! EasyBot 插件 SDK
//!
//! 导出第三方适配器开发者需要的核心类型和 trait。
//! 插件开发者只需依赖此 crate 即可实现 PlatformAdapter。

// 重新导出核心类型
pub use easybot_core::types::adapter::{
    PlatformAdapter, AdapterConfig, AdapterRuntimeConfig,
    Capability, CapabilityName, CapabilityLimits,
    AdapterState, HealthStatus, HealthReport,
    ConnectResult, InitResult, BotInfo,
    AdapterStatusSummary,
};

pub use easybot_core::types::message::{
    InboundMessage, OutboundMessage,
    SendTextParams, SendMediaParams, SendInteractiveParams,
    EditMessageParams, SendResult, EditResult, DeleteResult,
    ChatInfo, ChatFilter,
    ChatType, MessageAuthor, MediaAttachment, MediaType,
    ParseMode, CallbackEvent,
};

pub use easybot_core::types::error::GatewayError;
pub use easybot_core::types::session::SessionSource;

pub use async_trait::async_trait;
