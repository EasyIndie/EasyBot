//! EasyBot 插件 SDK
//!
//! 导出第三方适配器开发者需要的核心类型、trait 和 FFI 宏。
//! 插件开发者只需依赖此 crate 并使用 `declare_plugin!` 即可导出自定义适配器。

// FFI 模块需要 unsafe（no_mangle, extern "C" 等），豁免 workspace lint
#![allow(unsafe_code)]

mod ffi;
pub use ffi::EASYBOT_PLUGIN_ABI_VERSION;

// 重新导出核心类型
pub use easybot_core::types::adapter::{
    AdapterConfig, AdapterRuntimeConfig, AdapterState, AdapterStatusSummary, BotInfo, Capability,
    CapabilityLimits, CapabilityName, ConnectResult, HealthReport, HealthStatus, InitResult,
    PlatformAdapter,
};

#[allow(deprecated)]
pub use easybot_core::types::message::MessageAuthor;
pub use easybot_core::types::message::{
    CallbackEvent, ChatFilter, ChatInfo, ChatType, DeleteResult, EditMessageParams, EditResult,
    InboundMessage, MediaAttachment, MediaType, MentionInfo, MessageSender, MessageType,
    OutboundMessage, ParseMode, SendInteractiveParams, SendMediaParams, SendResult, SendTextParams,
    SenderRole,
};

pub use easybot_core::types::error::GatewayError;
pub use easybot_core::types::session::SessionSource;

pub use async_trait::async_trait;

/// 插件开发者一站式导入
pub mod prelude {
    pub use crate::{
        AdapterConfig, AdapterRuntimeConfig, AdapterState, AdapterStatusSummary, BotInfo,
        CallbackEvent, Capability, CapabilityLimits, CapabilityName, ChatFilter, ChatInfo,
        ChatType, ConnectResult, DeleteResult, EASYBOT_PLUGIN_ABI_VERSION, EditMessageParams,
        EditResult, GatewayError, HealthReport, HealthStatus, InboundMessage, InitResult,
        MediaAttachment, MediaType, MentionInfo, MessageSender, MessageType, OutboundMessage,
        ParseMode, PlatformAdapter, SendInteractiveParams, SendMediaParams, SendResult,
        SendTextParams, SenderRole, SessionSource, declare_plugin,
    };
    pub use async_trait::async_trait;
}
