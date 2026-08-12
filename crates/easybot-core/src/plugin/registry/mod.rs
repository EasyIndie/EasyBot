//! 插件注册表
//!
//! 抽象插件市场后端（Homebrew Taps 模型）。v1 提供 GitHub Releases 实现，
//! 未来可替换为自建后端（`StaticRegistry` 桩支持离线/空气隔离部署）。
//!
//! - [`PluginRegistry`]：目录 / 版本查询 / 下载三能力抽象
//! - [`GitHubRegistry`]：基于 GitHub Releases 的实现
//! - [`types`]：catalog.json 与 easybot-plugin.json 的数据结构

pub mod github;
pub mod types;

pub use github::GitHubRegistry;

use async_trait::async_trait;
use std::path::Path;
use types::{PluginArtifact, PluginCatalog, PluginRegistryError, PluginSource, PluginVersionMeta};

/// 插件注册表抽象
///
/// 实现必须 `Send + Sync`，可被 `PluginManager` 持有多源合并。
/// `catalog()` 的 5 分钟缓存、`versions_for` 的最新优先排序由实现保证。
#[async_trait]
pub trait PluginRegistry: Send + Sync {
    /// 获取市场目录索引
    async fn catalog(&self) -> Result<PluginCatalog, PluginRegistryError>;

    /// 查询插件在注册源中的版本列表（最新在前）
    async fn versions_for(
        &self,
        source: &PluginSource,
        limit: usize,
    ) -> Result<Vec<PluginVersionMeta>, PluginRegistryError>;

    /// 下载产物字节到目标路径
    ///
    /// 实现负责流式下载，并在返回前校验 `sha256` 与元数据一致。
    async fn download(
        &self,
        artifact: &PluginArtifact,
        dest: &Path,
    ) -> Result<(), PluginRegistryError>;
}
