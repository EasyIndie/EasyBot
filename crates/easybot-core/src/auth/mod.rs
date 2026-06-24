//! 认证授权模块
//!
//! 提供 API Key 的生成、验证、管理功能，以及权限检查。

mod api_key;
pub mod permissions;
pub use api_key::*;
