//! updater 数据结构和错误类型
//!
//! 定义升级过程中使用的所有数据模型，包括版本信息、更新计划、
//! 平台目标映射和错误类型。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 目标平台对应的 Release artifact 名称
///
/// 映射规则与 `.github/workflows/release.yml` 的 build-matrix 一致。
pub struct PlatformAsset {
    /// Rust 标准 target triple，如 `x86_64-unknown-linux-musl`
    pub target_triple: &'static str,
    /// GitHub Release 中的 artifact 文件名
    pub asset_name: &'static str,
}

/// 获取当前运行平台的 target triple
///
/// 当平台不支持自动更新时返回 `UnsupportedPlatform` 错误。
pub fn current_target_triple() -> Result<&'static str, UpdateError> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => Ok("x86_64-unknown-linux-musl"),
        ("aarch64", "linux") => Ok("aarch64-unknown-linux-musl"),
        ("x86_64", "macos") => Ok("x86_64-apple-darwin"),
        ("aarch64", "macos") => Ok("aarch64-apple-darwin"),
        ("x86_64", "windows") => Ok("x86_64-pc-windows-msvc"),
        ("aarch64", "windows") => Ok("aarch64-pc-windows-msvc"),
        _ => Err(UpdateError::UnsupportedPlatform),
    }
}

/// 生成当前平台对应的 release artifact 文件名
///
/// 例: `easybot-x86_64-unknown-linux-musl`（Windows 追加 `.exe`）
pub fn current_asset_name() -> Result<String, UpdateError> {
    let triple = current_target_triple()?;
    Ok(if cfg!(target_os = "windows") {
        format!("easybot-{}.exe", triple)
    } else {
        format!("easybot-{}", triple)
    })
}

/// 版本比较
///
/// 使用 `semver` crate 进行严格的语义版本比较。
/// 返回 `true` 当 `newer` > `older`。
pub fn is_newer_than(newer: &str, older: &str) -> bool {
    let newer_v = semver::Version::parse(newer).ok();
    let older_v = semver::Version::parse(older).ok();
    match (newer_v, older_v) {
        (Some(n), Some(o)) => n > o,
        _ => false, // 版本解析失败时保守处理
    }
}

/// 从 GitHub tag name（如 `v0.1.0`）提取纯版本号
pub fn version_from_tag(tag: &str) -> Option<String> {
    tag.strip_prefix('v')
        .map(|s| s.to_string())
        .filter(|s| semver::Version::parse(s).is_ok())
}

// ══════════════════════════════════════════════════════════════════
// 错误类型
// ══════════════════════════════════════════════════════════════════

/// 更新过程中的所有可能错误
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("GitHub API rate limited (60 req/h). Set GITHUB_TOKEN env var to increase to 5000/h")]
    RateLimited,

    #[error("Already up to date (current: {0})")]
    AlreadyUpToDate(String),

    #[error("Cannot downgrade from {current} to {target}")]
    VersionDowngrade { current: String, target: String },

    #[error("SHA256 checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("Insufficient disk space: need {need} bytes, available {available}")]
    InsufficientDiskSpace { need: u64, available: u64 },

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Binary replacement failed: {0}")]
    BinaryReplaceFailed(String),

    #[error("Database migration failed: {0}")]
    MigrationFailed(String),

    #[error("New binary verification failed: {0}")]
    VerificationFailed(String),

    #[error("Unsupported platform for auto-update")]
    UnsupportedPlatform,

    #[error("Offline mode: cannot reach GitHub API")]
    OfflineMode,

    #[error("Backup failed: {0}")]
    BackupFailed(String),

    #[error("Rollback failed: {0}")]
    RollbackFailed(String),

    #[error("Plugin incompatible: {0}")]
    PluginIncompatible(String),

    #[error("Service unit update failed: {0}")]
    ServiceUpdateFailed(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

// ══════════════════════════════════════════════════════════════════
// 数据模型
// ══════════════════════════════════════════════════════════════════

/// GitHub Release 的 asset 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub size: u64,
    #[serde(rename = "browser_download_url")]
    pub download_url: String,
}

/// GitHub Release 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub html_url: String,
    pub body: String,
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

/// 版本清单文件内容（从 `easybot-version.json` 解析）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionManifest {
    pub version: String,
    pub tag: String,
    pub release_date: Option<String>,
    pub schema_version: i64,
    pub requires_db_migration: bool,
    pub migrations: Vec<MigrationInfo>,
    pub requires_config_migration: bool,
    pub config_changes: Vec<String>,
    pub breaking_changes: Vec<String>,
    pub plugin_abi_version: u32,
    pub min_upgradable_from: String,
}

/// 数据库迁移信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationInfo {
    pub version: i64,
    pub description: String,
}

/// 更新前预检结果
#[derive(Debug, Clone)]
pub struct PreCheckResult {
    pub network_ok: bool,
    pub disk_space_ok: bool,
    pub disk_space_required: u64,
    pub permissions_ok: bool,
    pub plugins_compatible: bool,
    pub incompatible_plugins: Vec<String>,
    pub is_offline: bool,
    pub is_docker: bool,
    pub is_dev_mode: bool,
    pub service_type: ServiceType,
}

/// 服务管理器类型
#[derive(Debug, Clone, PartialEq)]
pub enum ServiceType {
    None,
    Systemd,
    Launchd,
    Windows,
}

/// 更新计划（执行前完整预览）
#[derive(Debug, Clone)]
pub struct UpdatePlan {
    pub current_version: String,
    pub target_version: String,
    pub target_schema_version: i64,
    pub current_schema_version: i64,
    pub requires_db_migration: bool,
    pub db_migrations: Vec<MigrationInfo>,
    pub requires_config_migration: bool,
    pub config_changes: Vec<String>,
    pub breaking_changes: Vec<String>,
    pub plugin_incompatible: Vec<String>,
    pub binary_size: u64,
    pub checksum: String,
    pub requires_service_update: bool,
}

/// 更新结果
#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub old_version: String,
    pub new_version: String,
    pub backup_path: PathBuf,
    pub db_backup_path: Option<PathBuf>,
    pub migrations_applied: usize,
}

/// 更新备份清单
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub timestamp: i64,
    pub from_version: String,
    pub to_version: String,
    pub from_schema_version: i64,
    pub to_schema_version: i64,
    pub binary_backup: Option<String>,
    pub db_backup: Option<String>,
    pub config_backup: Option<String>,
    pub migrations_applied: Vec<i64>,
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_from_tag() {
        assert_eq!(version_from_tag("v0.1.0"), Some("0.1.0".into()));
        assert_eq!(version_from_tag("0.1.0"), None); // 缺 v 前缀
        assert_eq!(version_from_tag("v1"), None); // 非完整 semver
    }

    #[test]
    fn test_version_comparison_newer() {
        assert!(is_newer_than("0.1.0", "0.0.16"));
        assert!(is_newer_than("1.0.0", "0.9.99"));
        assert!(is_newer_than("0.0.17", "0.0.16"));
    }

    #[test]
    fn test_version_comparison_older() {
        assert!(!is_newer_than("0.0.16", "0.1.0"));
        assert!(!is_newer_than("0.9.0", "1.0.0"));
    }

    #[test]
    fn test_version_comparison_equal() {
        assert!(!is_newer_than("0.0.16", "0.0.16"));
    }

    #[test]
    fn test_version_comparison_invalid() {
        assert!(!is_newer_than("not-a-version", "0.0.16"));
        assert!(!is_newer_than("0.0.16", "not-a-version"));
    }

    #[test]
    fn test_target_triple_returns_supported() {
        let result = current_target_triple();
        assert!(result.is_ok());
        let triple = result.unwrap();
        // 在测试运行的平台上验证
        assert!(
            triple.contains("linux") || triple.contains("apple") || triple.contains("windows"),
            "triple should match a known OS: {}",
            triple
        );
    }

    #[test]
    fn test_asset_name_includes_platform() {
        let name = current_asset_name().unwrap();
        assert!(
            name.starts_with("easybot-"),
            "asset should start with easybot-"
        );
    }

    #[test]
    fn test_error_display() {
        let err = UpdateError::UnsupportedPlatform;
        assert_eq!(format!("{}", err), "Unsupported platform for auto-update");

        let err = UpdateError::ChecksumMismatch {
            expected: "abc".into(),
            actual: "def".into(),
        };
        assert!(format!("{}", err).contains("abc"));
        assert!(format!("{}", err).contains("def"));
    }
}
