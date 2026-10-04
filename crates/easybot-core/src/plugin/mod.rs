//! 插件系统（**进程外**）
//!
//! 支持通过**独立可执行文件**加载第三方 IM 适配器：宿主 `spawn` 子进程，
//! 双方经 stdin/stdout 逐行 JSON（`easybot-plugin-protocol`）通信。
//!
//! 每个插件是 `{EASYBOT_HOME}/plugins/<name>/` 下的一个子目录，
//! 含 `plugin.yaml` 与入口可执行文件。
//!
//! 加载流程（[`loader::PluginLoader`]）：
//! 1. 扫描插件目录，读取 `plugin.yaml`
//! 2. 旧式 cdylib 清单（仅 `library`）→ `LegacyCdylibPlugin`（附迁移指引）
//! 3. 定位入口可执行文件（`command`）并校验可执行位
//! 4. 有 `plugin.sig.json` → 对入口文件字节验签 + 发布者信任
//! 5. 生成 `AdapterFactory`（首次创建时 spawn 子进程并 `init`）
//! 6. 注册到 `AdapterRegistry`
//!
//! **不使用 `dlopen`**：官方 Linux 发行版是 musl 全静态二进制，没有动态加载器。

#[cfg(feature = "plugin-system")]
pub mod error;
#[cfg(feature = "plugin-system")]
pub mod install;
#[cfg(feature = "plugin-system")]
pub mod ipc;
#[cfg(feature = "plugin-system")]
pub mod loader;
#[cfg(feature = "plugin-system")]
pub mod manager;
#[cfg(feature = "plugin-system")]
pub mod manifest;
#[cfg(feature = "plugin-system")]
pub mod registry;
#[cfg(feature = "plugin-system")]
pub mod signing;

#[cfg(feature = "plugin-system")]
pub use ipc::IpcPluginAdapter;
#[cfg(feature = "plugin-system")]
pub use loader::*;
#[cfg(feature = "plugin-system")]
pub use manager::*;
#[cfg(feature = "plugin-system")]
pub use manifest::*;
