//! 插件系统
//!
//! 支持通过动态库（.so / .dylib / .dll）加载第三方 IM 适配器。
//!
//! # 架构
//!
//! 每个插件是一个包含 `plugin.yaml` 和编译好的动态库的子目录，
//! 放在 `{EASYBOT_HOME}/plugins/` 下。
//!
//! 加载流程：
//! 1. `PluginLoader` 扫描插件目录
//! 2. 读取 `plugin.yaml` 清单
//! 3. `dlopen` 动态库
//! 4. 验证 ABI 版本
//! 5. 创建适配器提取元信息
//! 6. 生成 `AdapterFactory` 闭包
//! 7. 注册到 `AdapterRegistry`

#[cfg(feature = "plugin-system")]
pub mod loader;
#[cfg(feature = "plugin-system")]
pub mod manifest;

#[cfg(feature = "plugin-system")]
pub use loader::*;
#[cfg(feature = "plugin-system")]
pub use manifest::*;
