//! 适配器管理
//!
//! 适配器管理器负责适配器工厂的注册、适配器的创建/启动/停止、
//! 健康检查和状态查询。

mod manager;
mod registry;
pub use manager::*;
pub use registry::*;
