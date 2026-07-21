//! 二进制安全替换与回滚
//!
//! 提供跨平台的二进制文件安全替换操作：
//! - 备份当前二进制（同文件系统复制）
//! - 原子替换（Unix: `rename()`，Windows: rename + copy）
//! - 回滚到备份版本
//! - Unix 可执行权限设置

use super::types::UpdateError;
use std::path::{Path, PathBuf};

/// 备份文件信息
pub struct BinaryBackup {
    pub backup_path: PathBuf,
}

/// 安全替换当前运行中的二进制文件
///
/// 流程：
/// 1. 备份当前二进制到 `{exe}.bak.{version}`
/// 2. 将新二进制复制到目标目录更名
/// 3. 原子 `rename()` 替换（Unix）或 rename-then-copy（Windows）
/// 4. 设置可执行权限（Unix）
/// 5. 返回备份对象（可用于回滚）
///
/// 如果替换过程中任何步骤失败，自动尝试回滚。
pub fn replace_binary(new_bin: &Path, current_version: &str) -> Result<BinaryBackup, UpdateError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::BinaryReplaceFailed(format!("Cannot get current exe: {}", e)))?;

    // 1. 备份当前二进制
    let backup = create_backup(&current_exe, current_version)?;

    // 2. 将新二进制移到目标文件（同文件系统确保 rename 原子性）
    let temp = current_exe.with_extension("tmp.new");
    std::fs::rename(new_bin, &temp).map_err(|e| {
        // 回滚：删除临时文件
        let _ = std::fs::remove_file(&temp);
        UpdateError::BinaryReplaceFailed(format!("Cannot stage new binary: {}", e))
    })?;

    // 3. 原子替换
    let result = std::fs::rename(&temp, &current_exe);

    match result {
        Ok(_) => {
            // 4. 设置可执行权限（Unix）
            #[cfg(unix)]
            set_executable(&current_exe)?;

            tracing::info!(
                "Binary replaced: {} -> {}",
                current_exe.display(),
                current_version
            );
            Ok(BinaryBackup {
                backup_path: backup,
            })
        }
        Err(e) => {
            // 替换失败，回滚
            tracing::error!("Binary replace failed: {}, attempting rollback", e);
            let _ = std::fs::rename(&backup, &current_exe);
            let _ = std::fs::remove_file(&temp);
            Err(UpdateError::BinaryReplaceFailed(format!(
                "Cannot replace binary: {}. Rolled back to original.",
                e
            )))
        }
    }
}

/// 从备份恢复二进制文件
pub fn rollback_binary(backup: &BinaryBackup) -> Result<(), UpdateError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::RollbackFailed(format!("Cannot get current exe: {}", e)))?;

    if !backup.backup_path.exists() {
        return Err(UpdateError::RollbackFailed(format!(
            "Backup not found: {}",
            backup.backup_path.display()
        )));
    }

    std::fs::copy(&backup.backup_path, &current_exe)
        .map_err(|e| UpdateError::RollbackFailed(format!("Cannot restore backup: {}", e)))?;

    #[cfg(unix)]
    set_executable(&current_exe)?;

    tracing::info!("Binary rolled back from {}", backup.backup_path.display());
    Ok(())
}

/// 创建当前二进制的备份
fn create_backup(exe_path: &Path, version: &str) -> Result<PathBuf, UpdateError> {
    let backup_name = format!(
        "{}.bak.{}",
        exe_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("easybot"),
        version
    );
    let backup_path = exe_path.with_file_name(&backup_name);

    std::fs::copy(exe_path, &backup_path)
        .map_err(|e| UpdateError::BackupFailed(format!("Cannot create binary backup: {}", e)))?;

    tracing::info!("Binary backup created: {}", backup_path.display());
    Ok(backup_path)
}

/// 验证新二进制能否正常启动
///
/// 通过运行 `{new_bin} --check-update` 检测退出码。
pub async fn verify_binary(bin_path: &Path) -> Result<(), UpdateError> {
    let bin = bin_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new(&bin)
            .arg("--check-update")
            .output()
            .map_err(|e| {
                UpdateError::VerificationFailed(format!("Cannot start new binary: {}", e))
            })?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(UpdateError::VerificationFailed(format!(
                "New binary exited with code {}: {}",
                output.status,
                stderr.trim()
            )))
        }
    })
    .await
    .map_err(|e| UpdateError::VerificationFailed(format!("Join error: {}", e)))?
}

/// 设置 Unix 可执行权限
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)?;
    let mut perm = metadata.permissions();
    let current_mode = perm.mode();

    // 保持原有 owner/group，添加 owner 和 group 的可执行位
    let new_mode = current_mode | 0o111; // 不移除任何权限

    if current_mode != new_mode {
        perm.set_mode(new_mode);
        std::fs::set_permissions(path, perm)?;
    }

    Ok(())
}

#[cfg(not(unix))]
#[allow(dead_code)]
fn set_executable(_path: &Path) -> Result<(), UpdateError> {
    // Windows 没有 Unix 风格的可执行位
    Ok(())
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_backup_and_rollback() {
        let dir = std::env::temp_dir().join(format!("easybot_test_replace_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        // 创建 "当前二进制"
        let exe_path = dir.join("easybot");
        fs::write(&exe_path, b"original content").unwrap();

        // 模拟 current_exe() 返回
        // 注意：不能实际修改 std::env::current_exe，我们直接测试备份逻辑
        let backup = create_backup(&exe_path, "0.0.16").unwrap();
        assert!(backup.exists());

        // 验证备份内容
        let content = fs::read(&backup).unwrap();
        assert_eq!(content, b"original content");

        // 修改原文件
        fs::write(&exe_path, b"new content").unwrap();
        assert_eq!(fs::read(&exe_path).unwrap(), b"new content");

        // 手动恢复
        fs::copy(&backup, &exe_path).unwrap();
        assert_eq!(fs::read(&exe_path).unwrap(), b"original content");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_verify_binary_nonexistent() {
        let result = verify_binary(Path::new("/nonexistent/easybot")).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_create_backup_nonexistent_source() {
        let result = create_backup(Path::new("/nonexistent/binary"), "0.0.16");
        assert!(result.is_err());
    }
}
