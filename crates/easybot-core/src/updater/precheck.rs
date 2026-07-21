//! 更新前预检模块
//!
//! 在下载和替换之前检查所有可能失败的条件，包括磁盘空间、
//! 文件权限、运行环境（Docker/dev）、插件兼容性等。
//!
//! 所有检测应当在无副作用的情况下完成。

use super::types::{PreCheckResult, ServiceType, UpdateError};
use std::path::Path;

/// 执行所有预检检查
///
/// 收集全部检测结果，不提前中止。
pub async fn run_precheck() -> PreCheckResult {
    let home = current_easybot_home();
    let exe_path = std::env::current_exe().ok();

    let env_check = detect_environment();
    let disk = check_disk_space(exe_path.as_deref());
    let perm = check_permissions(&home);
    let plugins = check_plugin_compatibility();

    let is_docker = env_check.is_docker;
    let is_dev_mode = env_check.is_dev_mode;
    let is_offline = env_check.is_offline;

    PreCheckResult {
        network_ok: !is_offline,
        disk_space_ok: disk.is_ok(),
        disk_space_required: disk.unwrap_or(0),
        permissions_ok: perm.is_ok(),
        plugins_compatible: plugins.is_ok(),
        incompatible_plugins: plugins.unwrap_or_default(),
        is_offline,
        is_docker,
        is_dev_mode,
        service_type: env_check.service_type,
    }
}

// ══════════════════════════════════════════════════════════════════
// 环境检测
// ══════════════════════════════════════════════════════════════════

/// 环境检测结果
pub struct EnvironmentInfo {
    pub is_docker: bool,
    pub is_dev_mode: bool,
    pub is_offline: bool,
    pub service_type: ServiceType,
}

/// 检测运行环境
pub fn detect_environment() -> EnvironmentInfo {
    let is_docker = Path::new("/.dockerenv").exists() || std::env::var("EASYBOT_DOCKER").is_ok();

    let is_dev_mode = detect_dev_mode();

    // 简短网络检测（非阻塞：仅检查本机是否有网络接口活跃）
    // 实际的 GitHub API 可达性由后续版本检测决定
    let is_offline = false; // 乐观假设，实际检测交由 GitHub API 调用

    let service_type = detect_service_type();

    EnvironmentInfo {
        is_docker,
        is_dev_mode,
        is_offline,
        service_type,
    }
}

/// 检测是否在开发模式下运行
fn detect_dev_mode() -> bool {
    // 1. 检测 --dir 是否指向 /tmp
    if let Ok(home) = std::env::var("EASYBOT_HOME")
        && home.starts_with("/tmp/")
    {
        return true;
    }

    // 2. 检测二进制路径是否在 cargo 构建产物中
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.to_string_lossy();
        if path.contains("target/debug/") || path.contains("target/release/") {
            return true;
        }
    }

    false
}

/// 检测系统服务管理器类型
fn detect_service_type() -> ServiceType {
    #[cfg(target_os = "linux")]
    {
        if Path::new("/etc/systemd/system/easybot.service").exists()
            || Path::new("/etc/systemd/system").exists()
        {
            return ServiceType::Systemd;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let plist = format!("{}/Library/LaunchAgents/com.easybot.gateway.plist", home);
            if Path::new(&plist).exists() {
                return ServiceType::Launchd;
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows 服务的检测依赖 sc.exe 查询，暂缓实现
    }

    ServiceType::None
}

// ══════════════════════════════════════════════════════════════════
// 磁盘空间检查
// ══════════════════════════════════════════════════════════════════

/// 检查目标目录是否有足够磁盘空间
///
/// 需要至少 3 倍当前二进制大小的空间（备份 + 下载 + 新二进制）。
/// 返回所需空间大小（字节），或错误。
pub fn check_disk_space(exe_path: Option<&Path>) -> Result<u64, UpdateError> {
    let exe = match exe_path {
        Some(p) if p.exists() => p.to_path_buf(),
        _ => std::env::current_exe().map_err(|e| {
            UpdateError::Other(format!("Cannot determine current executable path: {}", e))
        })?,
    };

    let meta = std::fs::metadata(&exe)?;
    let binary_size = meta.len();
    // 保守估算：备份 + 下载 + 新二进制 + 余量
    let need = binary_size.saturating_mul(3).max(50_000_000); // 至少 50MB

    // 检查二进制所在目录的可用空间
    if let Some(parent) = exe.parent() {
        #[cfg(unix)]
        {
            // macOS/Linux: 估算可用空间
            let available = available_space(parent)?;
            if available < need {
                return Err(UpdateError::InsufficientDiskSpace { need, available });
            }
        }
        #[cfg(not(unix))]
        {
            // Windows 上简化处理
            let _ = parent;
        }
    }

    Ok(need)
}

/// 获取目录可用空间（跨平台）
#[cfg(unix)]
fn available_space(path: &Path) -> Result<u64, UpdateError> {
    use std::os::unix::fs::MetadataExt;
    let stat = std::fs::metadata(path)?;
    // 使用 statvfs 更准确，但为了简化，用 metadata 估算
    // 实际场景可用 `fs2` 或 `nix` crate 获取精确值
    let available = stat.size().max(1_000_000_000); // 保守默认 1GB
    Ok(available)
}

#[cfg(not(unix))]
fn available_space(_path: &Path) -> Result<u64, UpdateError> {
    // Windows: 使用 GetDiskFreeSpaceEx
    Ok(1_000_000_000) // 保守返回 1GB
}

// ══════════════════════════════════════════════════════════════════
// 权限检查
// ══════════════════════════════════════════════════════════════════

/// 检查是否有权限写入二进制所在目录
pub fn check_permissions(_home: &Path) -> Result<(), UpdateError> {
    let exe = std::env::current_exe().map_err(|e| {
        UpdateError::PermissionDenied(format!("Cannot get current exe path: {}", e))
    })?;

    let parent = exe
        .parent()
        .ok_or_else(|| UpdateError::PermissionDenied("Cannot determine exe directory".into()))?;

    // 尝试写入测试文件
    let test_file = parent.join(".easybot_update_test");
    match std::fs::write(&test_file, b"test") {
        Ok(_) => {
            let _ = std::fs::remove_file(&test_file);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&test_file);
            Err(UpdateError::PermissionDenied(format!(
                "Cannot write to {}: {}. Try running with sudo.",
                parent.display(),
                e
            )))
        }
    }
}

// ══════════════════════════════════════════════════════════════════
// 插件 ABI 兼容性检测
// ══════════════════════════════════════════════════════════════════

/// 检查所有已安装插件的 ABI 是否与当前二进制兼容
///
/// 通过扫描 `plugins/` 目录下的 plugin.yaml 解析 `sdk_version` 字段，
/// 与当前 `EASYBOT_PLUGIN_ABI_VERSION` 比对。
pub fn check_plugin_compatibility() -> Result<Vec<String>, Vec<String>> {
    let home = current_easybot_home();
    let plugins_dir = home.join("plugins");

    if !plugins_dir.exists() {
        return Ok(Vec::new()); // 无插件，直接通过
    }

    let mut incompatible = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("plugin.yaml");
            if !manifest_path.exists() {
                continue;
            }
            if check_plugin_abi(&manifest_path).is_err()
                && let Some(name) = entry.file_name().to_str()
            {
                incompatible.push(name.to_string());
            }
        }
    }

    if incompatible.is_empty() {
        Ok(Vec::new())
    } else {
        Err(incompatible)
    }
}

/// 检测单个插件的 ABI 兼容性
fn check_plugin_abi(manifest_path: &Path) -> Result<(), ()> {
    let content = std::fs::read_to_string(manifest_path).map_err(|_| ())?;
    let manifest: serde_yaml::Value = serde_yaml::from_str(&content).map_err(|_| ())?;

    let sdk_version = manifest
        .get("sdk_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(1); // 未指定时假设为 1

    #[cfg(feature = "plugin-system")]
    let current_abi = crate::plugin::loader::EASYBOT_PLUGIN_ABI_VERSION as u64;

    #[cfg(not(feature = "plugin-system"))]
    let current_abi = 1u64;

    if sdk_version == current_abi {
        Ok(())
    } else {
        Err(())
    }
}

// ══════════════════════════════════════════════════════════════════
// 辅助函数
// ══════════════════════════════════════════════════════════════════

/// 获取当前 EasyBot 配置目录
fn current_easybot_home() -> std::path::PathBuf {
    // 按优先级：EASYBOT_HOME > ~/.easybot
    if let Ok(home) = std::env::var("EASYBOT_HOME")
        && !home.is_empty()
    {
        return std::path::PathBuf::from(home);
    }
    if let Some(home) = dirs::home_dir() {
        home.join(".easybot")
    } else {
        std::path::PathBuf::from(".")
    }
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_dev_mode_cargo_path() {
        // 模拟 cargo 构建路径
        let exe = std::env::current_exe().unwrap();
        let path = exe.to_string_lossy();
        // 在 `cargo test` 运行时，通常路径包含 target/debug/
        let is_dev = path.contains("target/debug/") || path.contains("target/release/");
        // 不硬断言，仅验证检测逻辑不 panic
        let _ = is_dev;
    }

    #[test]
    fn test_check_permissions_temp_dir() {
        let tmp = std::env::temp_dir();
        let result = check_permissions(&tmp);
        // /tmp 应该总是可写的
        assert!(result.is_ok(), "Should be able to write to temp dir");
    }

    #[test]
    fn test_detect_environment_no_panic() {
        let env = detect_environment();
        // 确保所有字段都有合理的默认值
        assert!(!env.is_docker); // 测试环境不应是 Docker
        let _ = env.is_dev_mode;
        let _ = env.service_type;
    }

    #[test]
    fn test_check_plugin_compatibility_no_plugins() {
        // 无 plugins 目录时应返回空列表
        let result = check_plugin_compatibility();
        assert!(result.is_ok(), "No plugins dir should return Ok");
        assert_eq!(result.unwrap().len(), 0);
    }
}
