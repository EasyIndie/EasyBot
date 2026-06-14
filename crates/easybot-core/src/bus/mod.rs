//! 消息总线
//!
//! 基于 tokio broadcast channel 的事件发布/订阅系统。
//! 每个事件类型有独立的广播通道，支持一对多分发。

mod event_bus;
pub use event_bus::*;
