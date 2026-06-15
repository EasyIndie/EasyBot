//! 会话管理
//!
//! 会话管理器负责会话的创建、查找、更新、删除等生命周期管理。
//! 会话以 "platform:chatId[:threadId]" 为键。

mod bridge;
mod manager;
mod message_persister;
pub use bridge::*;
pub use manager::*;
pub use message_persister::*;
