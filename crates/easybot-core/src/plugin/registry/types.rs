//! 插件注册表数据类型
//!
//! 市场目录（`catalog.json`）与插件版本元数据（`easybot-plugin.json`）的
//! 反序列化结构。所有字段均来自**不可信网络来源**，使用侧必须校验
//! （见 `install.rs` 的 name 白名单、`sha256` 比对与签名校验）。
//!
//! 采用 Homebrew Taps 模型：一个 `PluginCatalog` 对应一个注册源，
//! `PluginManager` 可合并多源；插件名支持 `publisher/name` 限定。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 市场目录索引（catalog.json）的 schema 版本
pub const CATALOG_SCHEMA_VERSION: u32 = 1;

/// 插件版本元数据（easybot-plugin.json）的 schema 版本
pub const PLUGIN_META_SCHEMA_VERSION: u32 = 1;

/// 插件目录项（catalog.json 中的一条）
///
/// 指向插件的源码仓库，版本信息由 `easybot-plugin.json`（Release asset）提供。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSource {
    /// 插件名（唯一，与 `publisher` 组成 `publisher/name` 限定）
    pub name: String,
    /// 发布者标识（对应 `trusted_publishers` 中的发布者）
    pub publisher: String,
    /// 源码仓库所属组织/用户
    pub owner: String,
    /// 源码仓库名
    pub repo: String,
    /// 人类可读的显示名称
    #[serde(default)]
    pub display_name: Option<String>,
    /// 功能描述
    #[serde(default)]
    pub description: Option<String>,
    /// 分类标签
    #[serde(default)]
    pub tags: Vec<String>,
    /// 发布者是否通过官方验证（徽标；**只证身份，不证安全**）
    #[serde(default)]
    pub verified: bool,
}

/// 市场目录索引（catalog.json）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCatalog {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    pub plugins: Vec<PluginSource>,
}

impl PluginCatalog {
    /// 按 `publisher/name`（或仅 name）查找插件目录项
    pub fn find(&self, publisher: Option<&str>, name: &str) -> Option<&PluginSource> {
        self.plugins
            .iter()
            .find(|p| p.name == name && publisher.is_none_or(|publisher| p.publisher == publisher))
    }
}

/// 插件发布渠道
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginChannel {
    /// 稳定渠道（默认）
    #[default]
    Stable,
    /// 预览渠道
    Beta,
}

/// 插件对宿主的兼容性要求
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginRequirements {
    /// 宿主 EasyBot 版本范围（semver range，如 `>=0.0.28`）
    #[serde(default)]
    pub easybot: Option<String>,
}

/// 单个目标平台（target triple）的产物
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginArtifact {
    /// 产物下载 URL（GitHub browser_download_url）
    pub url: String,
    /// 产物字节数
    #[serde(default)]
    pub size: u64,
    /// 产物字节的 SHA256（十六进制小写）
    pub sha256: String,
    /// base64 编码的 ed25519 签名（覆盖产物字节本身）
    ///
    /// `None` 表示发布者未签名——lenient（dev）模式可加载并告警，
    /// strict（prod）模式拒绝。
    #[serde(default)]
    pub signature: Option<String>,
    /// base64 编码的 ed25519 验证公钥（随元数据分发，HTTPS 拉取）
    ///
    /// 安装时用这把公钥验签；发布者信任 = 公钥指纹 ∈ 配置
    /// `trusted_publishers` ∪ 用户 `.trust`。公钥随元数据走 TLS，
    /// 篡改元数据中的公钥即需伪造对应私钥签名，因此可信。
    #[serde(default)]
    pub public_key: Option<String>,
    /// 动态库文件名（相对插件目录）；缺省按平台规则推断
    #[serde(default)]
    pub library: Option<String>,
}

/// 插件版本元数据（easybot-plugin.json，Release asset）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginVersionMeta {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    pub name: String,
    /// 语义化版本号（如 `1.0.0`）
    pub version: String,
    /// 编译所用 SDK 的 ABI 版本
    #[serde(rename = "sdkVersion")]
    pub sdk_version: u32,
    pub publisher: String,
    /// 对应的 Git tag（如 `v1.0.0`）
    pub tag: String,
    /// 发布渠道（默认 stable）
    #[serde(default)]
    pub channel: PluginChannel,
    /// 对宿主的兼容性要求
    #[serde(default)]
    pub requires: Option<PluginRequirements>,
    /// 已废弃标记（v1 预留，仅展示）
    #[serde(default)]
    pub deprecated: bool,
    /// 各目标平台的产物；只包含实际发布的平台（OBS 模型：插件可仅支持部分平台）
    pub artifacts: HashMap<String, PluginArtifact>,
}

/// 解析 `publisher/name` 限定的插件名
///
/// 返回 `(publisher, name)`。两段均不允许为空。
pub fn parse_qualified_name(input: &str) -> Option<(String, String)> {
    let (publisher, name) = input.trim().split_once('/')?;
    let publisher = publisher.trim();
    let name = name.trim();
    if publisher.is_empty() || name.is_empty() {
        return None;
    }
    Some((publisher.to_string(), name.to_string()))
}

// ══════════════════════════════════════════════════════════════════
// 错误类型
// ══════════════════════════════════════════════════════════════════

/// 注册表操作过程中的所有可能错误
#[derive(Debug, thiserror::Error)]
pub enum PluginRegistryError {
    #[error("Registry '{0}' not found")]
    RegistryNotFound(String),

    #[error("Plugin '{0}' not found in registry")]
    PluginNotFound(String),

    #[error("No version metadata found for plugin '{0}'")]
    NoVersionFound(String),

    #[error("Platform {triple} is not supported by plugin {name}")]
    UnsupportedPlatform { name: String, triple: String },

    #[error("SHA256 checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("GitHub API rate limited. Set GITHUB_TOKEN env var to raise the limit")]
    RateLimited,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl PluginRegistryError {
    /// 是否可重试（瞬态网络/限流错误）
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            PluginRegistryError::NetworkError(_) | PluginRegistryError::RateLimited
        )
    }
}
