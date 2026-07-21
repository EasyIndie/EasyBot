//! 备份管理与服务单元更新
//!
//! 管理更新过程中的备份生命周期（创建、读取、恢复、清理），
//! 以及更新后更新系统服务（systemd/launchd）的二进制路径。

use super::types::{ServiceType, UpdateError, UpdateManifest};
use std::path::{Path, PathBuf};

// ══════════════════════════════════════════════════════════════════
// 备份管理
// ══════════════════════════════════════════════════════════════════

/// 备份管理器：创建和恢复更新前的备份快照
pub struct BackupManager;

impl BackupManager {
    /// 创建完整备份
    ///
    /// 备份以下内容：
    /// 1. 当前二进制文件
    /// 2. SQLite 数据库（如果存在）
    /// 3. 配置文件（gateway.yaml）
    ///
    /// 所有备份路径记录到 `.update_manifest.json`。
    pub async fn create_backup(
        home: &Path,
        from_version: &str,
        to_version: &str,
        from_schema_version: i64,
        to_schema_version: i64,
    ) -> Result<UpdateManifest, UpdateError> {
        let timestamp = chrono::Utc::now().timestamp_millis();

        let mut manifest = UpdateManifest {
            timestamp,
            from_version: from_version.to_string(),
            to_version: to_version.to_string(),
            from_schema_version,
            to_schema_version,
            binary_backup: None,
            db_backup: None,
            config_backup: None,
            migrations_applied: Vec::new(),
        };

        // 1. 备份二进制
        if let Ok(exe) = std::env::current_exe() {
            let backup_name = format!(
                "{}.bak.{}",
                exe.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("easybot"),
                from_version
            );
            let backup_path = exe.with_file_name(&backup_name);

            // 同文件系统复制
            match tokio::fs::copy(&exe, &backup_path).await {
                Ok(_) => {
                    manifest.binary_backup = Some(backup_path.to_string_lossy().to_string());
                    tracing::info!("Binary backup created: {}", backup_path.display());
                }
                Err(e) => {
                    tracing::warn!("Failed to backup binary: {}", e);
                    return Err(UpdateError::BackupFailed(format!(
                        "Binary backup failed: {}",
                        e
                    )));
                }
            }
        }

        // 2. 备份数据库（SQLite）
        let db_path = home.join("data").join("gateway.db");
        if db_path.exists() {
            // 先执行 WAL checkpoint 确保数据一致性
            if let Some(pool) = try_get_sqlite_pool(&db_path).await {
                let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                    .execute(&pool)
                    .await;
                drop(pool);
            }

            let db_backup = home
                .join("data")
                .join(format!("gateway.db.bak.{}", from_version));

            match tokio::fs::copy(&db_path, &db_backup).await {
                Ok(_) => {
                    manifest.db_backup = Some(db_backup.to_string_lossy().to_string());
                    tracing::info!("Database backup created: {}", db_backup.display());
                }
                Err(e) => {
                    // DB 备份失败不中止更新，仅记录警告
                    tracing::warn!("Failed to backup database: {}", e);
                }
            }
        }

        // 3. 备份配置文件
        let config_path = home.join("gateway.yaml");
        if config_path.exists() {
            let config_backup = home.join(format!("gateway.yaml.bak.{}", from_version));
            match tokio::fs::copy(&config_path, &config_backup).await {
                Ok(_) => {
                    manifest.config_backup = Some(config_backup.to_string_lossy().to_string());
                    tracing::info!("Config backup created: {}", config_backup.display());
                }
                Err(e) => {
                    tracing::warn!("Failed to backup config: {}", e);
                }
            }
        }

        // 4. 写入备份清单
        Self::write_manifest(home, &manifest).await?;

        Ok(manifest)
    }

    /// 读取备份清单
    pub async fn read_manifest(home: &Path) -> Result<UpdateManifest, UpdateError> {
        let manifest_path = home.join(".update_manifest.json");
        if !manifest_path.exists() {
            return Err(UpdateError::Other(
                "No update manifest found. Nothing to rollback.".into(),
            ));
        }

        let content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(UpdateError::IoError)?;
        let manifest: UpdateManifest = serde_json::from_str(&content)?;
        Ok(manifest)
    }

    /// 写入备份清单
    async fn write_manifest(home: &Path, manifest: &UpdateManifest) -> Result<(), UpdateError> {
        let manifest_path = home.join(".update_manifest.json");
        let content = serde_json::to_string_pretty(manifest)?;
        tokio::fs::write(&manifest_path, &content)
            .await
            .map_err(UpdateError::IoError)?;
        Ok(())
    }

    /// 恢复所有备份
    pub async fn restore_all(manifest: &UpdateManifest) -> Result<(), UpdateError> {
        // 1. 恢复二进制
        if let Some(ref backup) = manifest.binary_backup {
            let backup_path = Path::new(backup);
            if backup_path.exists() {
                let exe = std::env::current_exe().map_err(|e| {
                    UpdateError::BackupFailed(format!("Cannot get exe path: {}", e))
                })?;
                tokio::fs::copy(backup_path, &exe).await.map_err(|e| {
                    UpdateError::BackupFailed(format!("Failed to restore binary: {}", e))
                })?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(perm) = std::fs::metadata(&exe).map(|m| m.permissions()) {
                        let mut perm = perm;
                        perm.set_mode(0o755);
                        let _ = std::fs::set_permissions(&exe, perm);
                    }
                }

                tracing::info!("Binary restored from backup: {}", backup);
            }
        }

        // 2. 恢复数据库
        if let Some(ref backup) = manifest.db_backup {
            let backup_path = Path::new(backup);
            if backup_path.exists() {
                // 目标路径：从备份名推断原始路径
                let db_path = backup_path
                    .parent()
                    .map(|p| p.join("gateway.db"))
                    .unwrap_or_else(|| PathBuf::from("gateway.db"));

                tokio::fs::copy(backup_path, &db_path).await.map_err(|e| {
                    UpdateError::BackupFailed(format!("Failed to restore DB: {}", e))
                })?;
                tracing::info!("Database restored from backup: {}", backup);
            }
        }

        // 3. 恢复配置
        if let Some(ref backup) = manifest.config_backup {
            let backup_path = Path::new(backup);
            if backup_path.exists() {
                let config_path = backup_path
                    .parent()
                    .map(|p| p.join("gateway.yaml"))
                    .unwrap_or_else(|| PathBuf::from("gateway.yaml"));

                tokio::fs::copy(backup_path, &config_path)
                    .await
                    .map_err(|e| {
                        UpdateError::BackupFailed(format!("Failed to restore config: {}", e))
                    })?;
                tracing::info!("Config restored from backup: {}", backup);
            }
        }

        Ok(())
    }

    /// 删除所有备份文件
    pub async fn cleanup(manifest: &UpdateManifest) -> Result<(), UpdateError> {
        let paths = [
            manifest.binary_backup.as_deref(),
            manifest.db_backup.as_deref(),
            manifest.config_backup.as_deref(),
        ];

        for path in paths.iter().flatten() {
            let p = Path::new(path);
            if p.exists()
                && let Err(e) = tokio::fs::remove_file(p).await
            {
                tracing::warn!("Failed to remove backup {}: {}", path, e);
            }
        }

        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════════
// 服务管理器路径更新
// ══════════════════════════════════════════════════════════════════

/// 更新服务管理器配置中的二进制路径
///
/// 当二进制路径在更新后发生变化时（如从 cargo 安装迁移到 /usr/local/bin），
/// 自动更新 systemd/launchd 的配置。
pub fn update_service_bin_path(service_type: ServiceType) -> Result<(), UpdateError> {
    let exe = std::env::current_exe()
        .map_err(|e| UpdateError::ServiceUpdateFailed(format!("Cannot get exe path: {}", e)))?;
    let exe_path = exe.to_string_lossy();

    match service_type {
        ServiceType::Systemd => update_systemd_exec_start(&exe_path),
        ServiceType::Launchd => update_launchd_program_args(&exe_path),
        ServiceType::Windows => Ok(()), // Windows 服务路径在 install 时写入，不需要更新
        ServiceType::None => Ok(()),
    }
}

/// 更新 systemd unit 中的 `ExecStart` 行
fn update_systemd_exec_start(new_bin: &str) -> Result<(), UpdateError> {
    let unit_path = Path::new("/etc/systemd/system/easybot.service");
    if !unit_path.exists() {
        tracing::debug!("No systemd unit found at {}", unit_path.display());
        return Ok(());
    }

    let content = std::fs::read_to_string(unit_path)
        .map_err(|e| UpdateError::ServiceUpdateFailed(format!("Cannot read unit file: {}", e)))?;

    // 检查当前 ExecStart 是否已指向新路径
    if content.contains(&format!("ExecStart={}", new_bin)) {
        return Ok(()); // 无需更新
    }

    let updated = content
        .lines()
        .map(|line| {
            if line.starts_with("ExecStart=") {
                // 保留 --config 参数
                if let Some(args) = line.strip_prefix("ExecStart=") {
                    // 提取原来的 --config 参数
                    let config_arg = args
                        .split_whitespace()
                        .skip(1)
                        .collect::<Vec<_>>()
                        .join(" ");
                    if !config_arg.is_empty() {
                        format!("ExecStart={} {}", new_bin, config_arg)
                    } else {
                        format!("ExecStart={}", new_bin)
                    }
                } else {
                    format!("ExecStart={}", new_bin)
                }
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // 需要 root 权限写入
    match std::fs::write(unit_path, &updated) {
        Ok(_) => {
            tracing::info!("Updated systemd ExecStart to {}", new_bin);
            // 通知 systemd 重载配置
            let _ = std::process::Command::new("systemctl")
                .arg("daemon-reload")
                .output();
            Ok(())
        }
        Err(e) => Err(UpdateError::ServiceUpdateFailed(format!(
            "Cannot write unit file (need sudo?): {}",
            e
        ))),
    }
}

/// 更新 launchd plist 中的 ProgramArguments
fn update_launchd_program_args(new_bin: &str) -> Result<(), UpdateError> {
    let home = std::env::var("HOME").unwrap_or_default();
    let plist_path = Path::new(&home).join("Library/LaunchAgents/com.easybot.gateway.plist");

    if !plist_path.exists() {
        tracing::debug!("No launchd plist found at {}", plist_path.display());
        return Ok(());
    }

    // 简单实现：读取 plist XML，替换 ProgramArguments 的字符串
    let content = std::fs::read_to_string(&plist_path)
        .map_err(|e| UpdateError::ServiceUpdateFailed(format!("Cannot read plist: {}", e)))?;

    // 检查是否已是最新
    if content.contains(&format!("<string>{}</string>", new_bin)) {
        return Ok(());
    }

    // 替换第一段二进制路径（ProgramArguments 的第一个元素）
    let mut replaced = false;
    let updated = content
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if !replaced && trimmed.starts_with("<string>") && trimmed.ends_with("</string>") {
                // 跳过程序名后的 --config 参数行
                let inner = trimmed
                    .trim_start_matches("<string>")
                    .trim_end_matches("</string>");
                // 只替换看起来是路径的行（包含 / 或 \）
                if inner.contains('/') || inner.contains('\\') {
                    replaced = true;
                    let indent = &line[..line.len() - line.trim_start().len()];
                    return format!("{}<string>{}</string>", indent, new_bin);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    if replaced {
        match std::fs::write(&plist_path, &updated) {
            Ok(_) => {
                tracing::info!("Updated launchd ProgramArguments to {}", new_bin);
                // 通知 launchd 重新加载
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", plist_path.to_str().unwrap_or("")])
                    .output();
                let _ = std::process::Command::new("launchctl")
                    .args(["load", "-w", plist_path.to_str().unwrap_or("")])
                    .output();
                Ok(())
            }
            Err(e) => Err(UpdateError::ServiceUpdateFailed(format!(
                "Cannot write plist: {}",
                e
            ))),
        }
    } else {
        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════════
// 辅助函数
// ══════════════════════════════════════════════════════════════════

/// 尝试获取 SQLite 连接池（用于备份前 WAL checkpoint）
///
/// 这里创建临时连接，因为更新时可能没有已初始化的池。
async fn try_get_sqlite_pool(db_path: &Path) -> Option<sqlx::SqlitePool> {
    use sqlx::sqlite::SqliteConnectOptions;
    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .read_only(true)
        .create_if_missing(false);

    sqlx::SqlitePool::connect_with(opts).await.ok()
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_manifest_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("easybot_test_manifest_{}", std::process::id()));
        let _ = tokio::fs::create_dir_all(&dir).await;

        let manifest = UpdateManifest {
            timestamp: 1234567890,
            from_version: "0.0.16".into(),
            to_version: "0.1.0".into(),
            from_schema_version: 1,
            to_schema_version: 2,
            binary_backup: Some("/tmp/easybot.bak.0.0.16".into()),
            db_backup: None,
            config_backup: Some("/tmp/gateway.yaml.bak.0.0.16".into()),
            migrations_applied: vec![2],
        };

        // 写入
        BackupManager::write_manifest(&dir, &manifest)
            .await
            .unwrap();

        // 读取
        let read = BackupManager::read_manifest(&dir).await.unwrap();
        assert_eq!(read.from_version, "0.0.16");
        assert_eq!(read.to_version, "0.1.0");
        assert_eq!(read.migrations_applied, vec![2]);
        assert_eq!(read.binary_backup, Some("/tmp/easybot.bak.0.0.16".into()));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_read_manifest_nonexistent() {
        let dir = std::env::temp_dir().join(format!(
            "easybot_test_manifest_missing_{}",
            std::process::id()
        ));
        let result = BackupManager::read_manifest(&dir).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("No update manifest"));
    }

    #[test]
    fn test_update_systemd_exec_start_no_unit() {
        // 没有 systemd unit 文件时应静默返回 Ok
        let result = update_systemd_exec_start("/usr/local/bin/easybot");
        assert!(result.is_ok());
    }
}
