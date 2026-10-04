//! # EasyBot 进程外插件协议
//!
//! 宿主与插件进程之间通过 **stdin/stdout 逐行 JSON** 通信（不依赖 dlopen）。
//! 本 crate 定义双方共享的报文类型、方法名与生命周期数据结构。
//!
//! ## 报文形态
//!
//! ```text
//! 宿主 → 插件（请求）: {"id":1,"method":"send","params":{...}}
//! 插件 → 宿主（成功）: {"id":1,"ok":true,"result":{...}}
//! 插件 → 宿主（失败）: {"id":1,"ok":false,"error":{"message":"…","kind":"transient"}}
//! 插件 → 宿主（通知）: {"method":"event","params":{...}}       // 无 id
//! ```
//!
//! 设计约束：错误分类 [`ErrorKind`] 与宿主健康监测的 `GatewayError` 分类对齐，
//! 便于上层复用既有的"瞬态重试 / 永久停用"逻辑。

#![deny(missing_docs)]

mod lifecycle;
mod message;

pub use lifecycle::*;
pub use message::*;

/// 当前协议版本。握手时由插件回传，宿主校验兼容性。
pub const PROTOCOL_VERSION: u32 = 1;
