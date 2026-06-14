//! EasyBot 核心库
//!
//! 包含网关核心逻辑：数据类型、消息总线、会话管理、适配器管理、认证授权、配置加载。

pub mod types;
pub mod bus;
pub mod session;
pub mod adapter;
pub mod auth;
#[cfg(feature = "plugin-system")]
pub mod plugin;
pub mod config;
pub mod storage;
pub mod webhook;

/// 重新导出核心类型，方便外部 crate 使用
pub use types::message::*;
pub use types::adapter::*;
pub use types::session::*;
pub use types::event::*;
pub use types::error::*;
pub use types::config::*;
