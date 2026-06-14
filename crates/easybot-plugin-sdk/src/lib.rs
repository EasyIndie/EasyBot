//! EasyBot 插件 SDK
//!
//! 导出第三方适配器开发者需要的核心类型、trait 和 FFI 宏。
//! 插件开发者只需依赖此 crate 并使用 `declare_plugin!` 即可导出自定义适配器。

mod ffi;
pub use ffi::EASYBOT_PLUGIN_ABI_VERSION;

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

/// 插件开发者一站式导入
pub mod prelude {
    pub use crate::{
        declare_plugin, EASYBOT_PLUGIN_ABI_VERSION,
        PlatformAdapter, AdapterConfig, AdapterRuntimeConfig,
        Capability, CapabilityName, CapabilityLimits,
        AdapterState, HealthStatus, HealthReport,
        ConnectResult, InitResult, BotInfo,
        AdapterStatusSummary,
        InboundMessage, OutboundMessage,
        SendTextParams, SendMediaParams, SendInteractiveParams,
        EditMessageParams, SendResult, EditResult, DeleteResult,
        ChatInfo, ChatFilter, ChatType, MessageAuthor,
        MediaAttachment, MediaType, ParseMode, CallbackEvent,
        GatewayError, SessionSource,
    };
    pub use async_trait::async_trait;
}
