//! # EasyBot 插件 SDK
//!
//! 编写 EasyBot **进程外插件**（独立可执行文件）所需的一切。
//!
//! ## 最小示例
//!
//! ```ignore
//! use easybot_plugin_sdk::prelude::*;
//!
//! #[derive(Default)]
//! struct MyAdapter;
//!
//! #[easybot_plugin_sdk::async_trait]
//! impl PlatformAdapter for MyAdapter {
//!     fn platform_name(&self) -> &str { "my-platform" }
//!     // ... 其余 trait 方法
//! }
//!
//! easybot_plugin_sdk::run_plugin!(MyAdapter, MyAdapter::default);
//! ```
//!
//! ## 运行方式
//!
//! 宿主把插件作为**子进程**启动，双方通过 stdin/stdout 逐行 JSON 协议
//! （`easybot-plugin-protocol`）通信：
//! `handshake → init → connect → send/事件回传 → shutdown`。
//!
//! 相比旧式 cdylib（`dlopen`）方式：插件崩溃不会拖垮宿主（进程隔离），
//! 且**静态链接的宿主也能加载插件**（静态二进制没有动态加载器，
//! 无法 `dlopen`）。
//!
//! ## 测试
//!
//! 依赖 `easybot-plugin-sdk = { features = ["testing"] }` 后可用：
//!
//! - `testing::PluginTestHost`——内存宿主（快，不启进程）；
//! - `testing::ProcessPluginTestHost`——进程外宿主（真实 stdio 协议）。
//!
//! 导出第三方适配器开发者需要的核心类型、trait 和运行时宏。
//! 插件开发者只需依赖此 crate 并调用 [`run_plugin!`] 即可。

// FFI 模块需要 unsafe（no_mangle, extern "C" 等），豁免 workspace lint

/// SDK ABI 版本常量（写入清单 `sdk_version`，安装时由宿主校验兼容性）。
pub const EASYBOT_PLUGIN_ABI_VERSION: u32 = 1;

// 进程外插件服务端运行时（`run_plugin!`）
pub mod runner;

/// 以**进程外插件**形式启动适配器（推荐方式）。
///
/// 插件 `main.rs` 只需：
/// ```ignore
/// fn main() { easybot_plugin_sdk::run_plugin!(MyAdapter, MyAdapter::new); }
/// ```
///
/// （旧式 `declare_plugin!` 的 cdylib 形态已废弃：Linux 静态宿主无法 dlopen。）
#[macro_export]
macro_rules! run_plugin {
    ($ty:ty, $ctor:expr) => {
        fn main() {
            let adapter: $ty = $ctor();
            $crate::runner::run_plugin(adapter);
        }
    };
}

// 测试宿主：`easybot-plugin-sdk = { ..., features = ["testing"] }` 时可用
#[cfg(feature = "testing")]
pub mod testing;

// 重新导出核心类型
pub use easybot_core::bus::EventBus;
pub use easybot_core::types::adapter::{
    AdapterConfig, AdapterRuntimeConfig, AdapterState, AdapterStatusSummary, BotInfo, Capability,
    CapabilityLimits, CapabilityName, ConnectErrorKind, ConnectResult, HealthReport, HealthStatus,
    InitResult, PlatformAdapter,
};
pub use easybot_core::types::event::{GatewayEvent, event_types};

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
        ChatType, ConnectErrorKind, ConnectResult, DeleteResult, EASYBOT_PLUGIN_ABI_VERSION,
        EditMessageParams, EditResult, EventBus, GatewayError, GatewayEvent, HealthReport,
        HealthStatus, InboundMessage, InitResult, MediaAttachment, MediaType, MentionInfo,
        MessageSender, MessageType, OutboundMessage, ParseMode, PlatformAdapter,
        SendInteractiveParams, SendMediaParams, SendResult, SendTextParams, SenderRole,
        SessionSource, event_types,
    };
    pub use async_trait::async_trait;
}
