//! EasyBot 自动更新模块
//!
//! 提供完整的版本升级生命周期管理：
//!
//! - `Updater::check_update()` — 检查新版本并生成更新计划
//! - `Updater::perform_update()` — 执行完整更新流程（预检→备份→下载→替换→迁移→验证）
//! - `Updater::rollback()` — 回滚到上一个版本

mod compact;
mod download;
mod github;
mod precheck;
mod replace;
pub mod types;

use crate::storage::migration;
use compact::BackupManager;
use types::{PreCheckResult, ServiceType, UpdateError, UpdatePlan, UpdateResult};

/// EasyBot 默认的 GitHub 仓库信息
const DEFAULT_OWNER: &str = "EasyIndie";
const DEFAULT_REPO: &str = "EasyBot";

/// 更新器：管理 EasyBot 版本升级的完整生命周期
pub struct Updater {
    github: github::GitHubClient,
    home: std::path::PathBuf,
    current_version: String,
    current_schema_version: i64,
    precheck: Option<PreCheckResult>,
}

impl Updater {
    /// 创建新的更新器
    pub fn new(home: std::path::PathBuf) -> Self {
        Updater {
            github: github::GitHubClient::new(DEFAULT_OWNER, DEFAULT_REPO),
            home,
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            current_schema_version: migration::SCHEMA_VERSION,
            precheck: None,
        }
    }

    /// 获取当前版本号
    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    /// 检查更新并生成更新计划
    ///
    /// 返回完整的 `UpdatePlan`，包含目标版本、DB 迁移、breaking changes 等信息。
    pub async fn check_update(&mut self) -> Result<UpdatePlan, UpdateError> {
        // 1. 获取最新 release
        let release = self.github.latest_release().await?;
        let tag = release.tag_name.trim_start_matches('v').to_string();

        // 2. 版本比较
        if !types::is_newer_than(&tag, &self.current_version) {
            return Err(UpdateError::AlreadyUpToDate(self.current_version.clone()));
        }

        // 3. 获取版本清单
        let manifest = self.github.version_manifest(&release.tag_name).await?;

        // 4. 检查最低可升级版本
        let min_upgradable = &manifest.min_upgradable_from;
        if types::is_newer_than(min_upgradable, &self.current_version) {
            return Err(UpdateError::Other(format!(
                "Current version {} is too old. Minimum upgradable version is {}. \
                 Please upgrade step by step.",
                self.current_version, min_upgradable
            )));
        }

        // 5. 获取当前平台的 asset 信息
        let asset_name = types::current_asset_name()?;
        let asset = release.assets.iter().find(|a| a.name == asset_name);

        // 6. 构建更新计划
        let plan = UpdatePlan {
            current_version: self.current_version.clone(),
            target_version: tag,
            target_schema_version: manifest.schema_version,
            current_schema_version: self.current_schema_version,
            requires_db_migration: manifest.requires_db_migration,
            db_migrations: manifest.migrations.clone(),
            requires_config_migration: manifest.requires_config_migration,
            config_changes: manifest.config_changes.clone(),
            breaking_changes: manifest.breaking_changes.clone(),
            plugin_incompatible: Vec::new(), // 由预检填充
            binary_size: asset.map(|a| a.size).unwrap_or(0),
            checksum: String::new(),
            requires_service_update: false,
        };

        Ok(plan)
    }

    /// 执行预检
    pub async fn run_precheck(&mut self) -> PreCheckResult {
        let result = precheck::run_precheck().await;
        self.precheck = Some(result.clone());
        result
    }

    /// 执行完整更新
    ///
    /// 完整的更新流程：
    /// 1. 检查更新 → 2. 预检 → 3. 备份 → 4. 下载 + 校验 → 5. 替换 → 6. 迁移 → 7. 验证
    pub async fn perform_update(&mut self) -> Result<UpdateResult, UpdateError> {
        // 1. 检查更新
        let plan = self.check_update().await?;
        let tag = format!("v{}", plan.target_version);

        // 2. 预检（如果尚未执行）
        if self.precheck.is_none() {
            self.run_precheck().await;
        }
        let precheck = self.precheck.as_ref().unwrap();

        // 环境检查
        if precheck.is_docker {
            return Err(UpdateError::Other(
                "Running inside Docker — use `docker compose pull && docker compose up -d` to update"
                    .into(),
            ));
        }
        if precheck.is_dev_mode {
            return Err(UpdateError::Other(
                "Development mode detected — auto-update is not supported in dev mode".into(),
            ));
        }

        // 3. 备份
        tracing::info!("Phase 1/5: Creating backups...");
        let manifest = BackupManager::create_backup(
            &self.home,
            &self.current_version,
            &plan.target_version,
            self.current_schema_version,
            plan.target_schema_version,
        )
        .await?;

        // 4. 下载 + SHA256 校验
        tracing::info!("Phase 2/5: Downloading new binary...");
        let release = self.github.latest_release().await?;
        let (temp_path, _checksum, _size) =
            download::download_and_verify(&mut self.github, &self.home, &tag, &release.assets)
                .await?;

        // 5. 替换二进制
        tracing::info!("Phase 3/5: Replacing binary...");
        let backup = replace::replace_binary(&temp_path, &self.current_version)?;

        // 6. 更新服务路径（如需要）
        if precheck.service_type != ServiceType::None {
            let _ = compact::update_service_bin_path(precheck.service_type.clone());
        }

        // 7. 运行数据库迁移
        let mut migrations_applied = 0;
        if plan.requires_db_migration && plan.target_schema_version > self.current_schema_version {
            tracing::info!("Phase 4/5: Running database migrations...");
            // DB 迁移由启动时的新二进制执行，这里仅记录
            migrations_applied = plan.db_migrations.len();
        }

        // 8. 验证新二进制
        tracing::info!("Phase 5/5: Verifying new binary...");
        let exe = std::env::current_exe()
            .map_err(|e| UpdateError::VerificationFailed(format!("Cannot get exe path: {}", e)))?;
        match replace::verify_binary(&exe).await {
            Ok(_) => {
                tracing::info!("New binary verification passed");

                // 清理备份
                let _ = BackupManager::cleanup(&manifest).await;

                // 清理临时下载文件
                let _ = tokio::fs::remove_file(&temp_path).await;

                Ok(UpdateResult {
                    old_version: self.current_version.clone(),
                    new_version: plan.target_version,
                    backup_path: backup.backup_path,
                    db_backup_path: None,
                    migrations_applied,
                })
            }
            Err(e) => {
                // 验证失败：自动回滚
                tracing::error!("New binary verification failed: {} — rolling back", e);
                let _ = replace::rollback_binary(&backup);
                let _ = BackupManager::restore_all(&manifest).await;
                Err(UpdateError::VerificationFailed(format!(
                    "New binary verification failed (rolled back): {}",
                    e
                )))
            }
        }
    }

    /// 回滚到上一个版本
    ///
    /// 从备份清单恢复：二进制 → 数据库 → 配置
    pub async fn rollback(&self) -> Result<(), UpdateError> {
        let manifest = BackupManager::read_manifest(&self.home).await?;
        tracing::warn!(
            "Rolling back from v{} to v{}...",
            manifest.to_version,
            manifest.from_version
        );

        // 恢复二进制
        if let Some(ref backup) = manifest.binary_backup {
            let backup_path = std::path::Path::new(backup);
            let exe = std::env::current_exe()
                .map_err(|e| UpdateError::RollbackFailed(format!("Cannot get exe: {}", e)))?;

            std::fs::copy(backup_path, &exe)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))?;
            }
            tracing::info!("Binary restored from backup");
        }

        // 恢复数据库
        BackupManager::restore_all(&manifest).await?;

        // 清理备份清单
        let manifest_path = self.home.join(".update_manifest.json");
        let _ = tokio::fs::remove_file(&manifest_path).await;

        tracing::warn!("Rollback completed. Service restart required.");
        Ok(())
    }

    /// 获取预检结果
    pub fn precheck_result(&self) -> Option<&PreCheckResult> {
        self.precheck.as_ref()
    }

    /// GitHub 客户端（对外暴露，允许 mock 测试）
    #[cfg(test)]
    pub fn github_mut(&mut self) -> &mut github::GitHubClient {
        &mut self.github
    }
}
