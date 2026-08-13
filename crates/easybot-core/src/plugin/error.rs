//! 插件管理器错误
//!
//! `PluginManager` 编排安装/卸载/启停/更新时的统一错误类型。
//! 组合了注册表错误、加载器错误、签名错误与本地流水线校验错误。

use crate::plugin::loader::PluginError;
use crate::plugin::registry::types::PluginRegistryError;
use crate::plugin::signing::SigningError;
use crate::updater::types::UpdateError;

/// 插件管理器操作过程中的所有可能错误
#[derive(Debug, thiserror::Error)]
pub enum PluginManagerError {
    #[error("Plugin not found: {0}")]
    NotFound(String),

    #[error("Plugin '{0}' already installed")]
    AlreadyInstalled(String),

    #[error("Plugin name '{0}' is invalid (must match [A-Za-z0-9_-])")]
    InvalidName(String),

    #[error(
        "Library file name '{0}' is invalid (must be a bare file name without path separators)"
    )]
    InvalidLibrary(String),

    #[error("Artifact URL '{0}' is not an allowed download origin (https + GitHub host required)")]
    InvalidArtifactUrl(String),

    #[error("Platform {triple} is not supported by plugin {name}")]
    UnsupportedPlatform { name: String, triple: String },

    #[error("Plugin {name} requires EasyBot {range}, current version is {current}")]
    EasyBotVersionRequirement {
        name: String,
        range: String,
        current: String,
    },

    #[error("Plugin {name} uses ABI v{got}, host expects v{expected}")]
    AbiMismatch {
        name: String,
        expected: u32,
        got: u32,
    },

    #[error(
        "Installed version {installed} is newer than available {available} — refusing downgrade"
    )]
    DowngradeNotAllowed {
        name: String,
        installed: String,
        available: String,
    },

    #[error("No version '{version}' of '{name}' found in registry")]
    VersionNotFound { name: String, version: String },

    #[error("Plugin '{0}' has no signature and signature verification is required")]
    SignatureRequired(String),

    #[error("Publisher '{0}' is not trusted")]
    UntrustedPublisher(String),

    #[error("Failed to stop adapter '{platform}': {detail}")]
    StopFailed { platform: String, detail: String },

    #[error("Registry error: {0}")]
    Registry(#[from] PluginRegistryError),

    #[error("Loader error: {0}")]
    Loader(#[from] PluginError),

    #[error("Signing error: {0}")]
    Signing(#[from] SigningError),

    #[error("Update error: {0}")]
    Update(#[from] UpdateError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("{0}")]
    Other(String),
}

impl PluginManagerError {
    /// 是否可重试（瞬态网络/限流错误）
    pub fn is_retryable(&self) -> bool {
        matches!(self, PluginManagerError::Registry(e) if e.is_retryable())
    }
}
