//! EasyBot 核心类型定义
//!
//! 本模块定义网关所有核心数据模型，包括消息、适配器接口、会话、事件、错误等。
//! 所有数据类型均实现 serde 序列化，以支持跨模块和跨网络传输。

pub mod message;
pub mod adapter;
pub mod session;
pub mod event;
pub mod error;
pub mod config;
