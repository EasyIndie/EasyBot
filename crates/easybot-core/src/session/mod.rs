//! 会话管理
//!
//! 会话管理器负责会话的创建、查找、更新、删除等生命周期管理。
//! 会话以 "platform:chatId[:threadId]" 为键。

mod manager;
mod bridge;
mod message_persister;
pub use manager::*;
pub use bridge::*;
pub use message_persister::*;
