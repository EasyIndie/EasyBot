//! EasyBot 自动更新模块
//!
//! 提供完整的版本升级生命周期管理：
//!
//! - `Updater::check_update()` — 检查新版本并生成更新计划
//! - `Updater::perform_update()` — 执行完整更新流程（预检→备份→下载→替换→迁移→验证）
//! - `Updater::rollback()` — 回滚到上一个版本

mod compact;
mod download;
pub mod github;
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

        // 4. 下载 + SHA256 校验（失败时清理已建的备份与清单，避免 `.bak`/manifest 残留）
        tracing::info!("Phase 2/5: Downloading new binary...");
        let release = self.github.latest_release().await?;
        let (temp_path, _checksum, _size) = match download::download_and_verify(
            &mut self.github,
            &self.home,
            &tag,
            &release.assets,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                let _ = BackupManager::cleanup_artifacts(&self.home, &manifest).await;
                let _ = tokio::fs::remove_file(&self.home.join(".update_manifest.json")).await;
                return Err(e);
            }
        };

        // 5. 替换二进制（平台自适应：Unix 原地替换；Windows 暂存待交换）。
        //    失败时清理已建的备份与清单（与 U3 清理契约一致）。
        tracing::info!("Phase 3/5: Replacing binary...");
        let replace = match replace::replace_binary(&temp_path, &self.current_version) {
            Ok(r) => r,
            Err(e) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                let _ = BackupManager::cleanup_artifacts(&self.home, &manifest).await;
                let _ = tokio::fs::remove_file(&self.home.join(".update_manifest.json")).await;
                return Err(e);
            }
        };

        // 6. 更新服务路径（如需要；Windows 服务路径 install 时写入，无需更新）
        if precheck.service_type != ServiceType::None {
            let _ = compact::update_service_bin_path(precheck.service_type.clone());
        }

        // 7. 运行数据库迁移（由启动时的新二进制执行，这里仅记录数量）
        let mut migrations_applied = 0;
        if plan.requires_db_migration && plan.target_schema_version > self.current_schema_version {
            tracing::info!("Phase 4/5: Running database migrations...");
            migrations_applied = plan.db_migrations.len();
        }

        // 8. 验证新二进制（校验在提交点执行：Unix = 替换后的 target；Windows = 暂存文件）
        tracing::info!("Phase 5/5: Verifying new binary...");
        match replace::verify_binary(&replace.verify_path, &self.home).await {
            Ok(_) => {
                tracing::info!("New binary verification passed");

                // Windows：校验通过后安排分离辅助脚本在主进程退出后完成交换
                // （Unix 分支 swap 恒为 None，此处不会执行，仅需通过编译）
                #[allow(unused_mut)] // Unix 分支下不变量化，Windows 下被写入
                let mut swap_scheduled = false;
                #[allow(unused_mut)]
                let mut swap_marker = None;
                if let Some(pending) = &replace.swap {
                    #[cfg(windows)]
                    match replace::schedule_swap(&self.home, pending, &plan.target_version, &[]) {
                        Ok(marker) => {
                            swap_scheduled = true;
                            // 先记录日志再 move 进 swap_marker（PathBuf 非 Copy）
                            tracing::warn!(
                                "Binary swap scheduled; will complete after this process exits. \
                                 Marker: {}",
                                marker.display()
                            );
                            swap_marker = Some(marker);
                        }
                        Err(e) => {
                            // 安排失败：清理暂存 + 备份 + 清单后上报
                            let _ = std::fs::remove_file(&replace.verify_path);
                            let _ = tokio::fs::remove_file(&temp_path).await;
                            let _ = BackupManager::cleanup_artifacts(&self.home, &manifest).await;
                            let _ =
                                tokio::fs::remove_file(&self.home.join(".update_manifest.json"))
                                    .await;
                            return Err(e);
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        // 逻辑上不可达（Unix 分支 swap = None），此处仅为类型检查
                        let _ = pending;
                    }
                }

                // Keep backups and the manifest so `easybot rollback` remains available
                // after a successful update.

                // 清理临时下载文件
                let _ = tokio::fs::remove_file(&temp_path).await;

                Ok(UpdateResult {
                    old_version: self.current_version.clone(),
                    new_version: plan.target_version,
                    backup_path: replace.backup.backup_path,
                    db_backup_path: None,
                    migrations_applied,
                    swap_scheduled,
                    swap_marker,
                })
            }
            Err(e) => {
                // 验证失败：回滚 + 清理残留。
                // 回滚是否成功决定是否清理备份：失败时必须保留备份供人工恢复，
                // 否则 cleanup_artifacts 会删掉唯一一份旧二进制，导致无法恢复。
                tracing::error!("New binary verification failed: {} — rolling back", e);
                // Windows 分支不写入（cfg(not(windows)) 块被编译掉），此处仅在 Unix 下变更
                #[allow(unused_mut)]
                let mut rollback_ok = true;

                #[cfg(not(windows))]
                {
                    // Unix：target 已被替换，从备份恢复旧二进制 + 恢复 DB/配置。
                    // restore_all 传 false：二进制已由 rollback_binary 同步恢复，避免重复复制。
                    rollback_ok &= replace::rollback_binary(&replace.backup).is_ok();
                    rollback_ok &= BackupManager::restore_all(&manifest, false).await.is_ok();
                }
                #[cfg(windows)]
                {
                    // Windows：target 未被触碰，仅删除暂存文件
                    let _ = std::fs::remove_file(&replace.verify_path);
                }

                // 统一清理：临时下载 + 备份 + 清单（仅回滚成功时）
                let _ = tokio::fs::remove_file(&temp_path).await;
                if rollback_ok {
                    let _ = BackupManager::cleanup_artifacts(&self.home, &manifest).await;
                    let _ = tokio::fs::remove_file(&self.home.join(".update_manifest.json")).await;
                } else {
                    tracing::error!(
                        "Rollback during verification FAILED — backups retained for manual recovery: \
                         manifest={}, binary_backup={}",
                        self.home.join(".update_manifest.json").display(),
                        manifest.binary_backup.as_deref().unwrap_or("<none>")
                    );
                }

                Err(UpdateError::VerificationFailed(format!(
                    "New binary verification failed (rolled back): {}",
                    e
                )))
            }
        }
    }

    /// 回滚到上一个版本
    ///
    /// 从备份清单恢复：二进制 → 数据库 → 配置。Windows 上运行中的 exe 无法原地
    /// 覆盖，二进制恢复走分离辅助脚本延迟交换（`schedule_swap`），DB/配置可立即恢复；
    /// **前提是服务已停止**——服务运行中回滚会拒绝（旧 DB 覆盖活动库 + exe 被锁）。
    /// 完成后清理备份文件与临时产物。
    pub async fn rollback(&self) -> Result<(), UpdateError> {
        let manifest = BackupManager::read_manifest(&self.home).await?;
        tracing::warn!(
            "Rolling back from v{} to v{}...",
            manifest.to_version,
            manifest.from_version
        );

        // 0. Windows：拒绝在服务运行时回滚——exe 被服务锁定（分离交换会 TIMEOUT），
        //    且旧 DB/config 恢复会覆盖仍被服务打开的活动数据库（SQLite 并发写 → 损坏）。
        //    必须先从服务侧停止（NSSM: `nssm stop EasyBot` / PowerShell: `Stop-Service EasyBot`）。
        #[cfg(windows)]
        if precheck::is_windows_service_running("EasyBot") {
            return Err(UpdateError::RollbackFailed(
                "EasyBot Windows 服务仍在运行，无法安全回滚：运行中的 exe 被服务锁定，且旧数据库会覆盖活动库。请先停止服务再重试（`nssm stop EasyBot`）。".into(),
            ));
        }

        // 1. 恢复二进制
        if let Some(ref backup) = manifest.binary_backup {
            let backup_path = std::path::Path::new(backup);
            if !backup_path.exists() {
                return Err(UpdateError::RollbackFailed(format!(
                    "Backup not found: {}",
                    backup
                )));
            }

            #[cfg(windows)]
            {
                // Windows：当前 exe 被进程映射锁定，安排分离脚本在本进程退出后
                // 执行 `move /y backup → exe`。用户需先停止服务（否则 15 次重试后
                // marker 写 TIMEOUT，目标保持新版本、可安全重试）。
                // 交换成功后由脚本顺带删除已恢复完毕的陈旧 DB/config 备份。
                let exe = std::env::current_exe()
                    .map_err(|e| UpdateError::RollbackFailed(format!("Cannot get exe: {}", e)))?;
                let swap = replace::PendingSwap {
                    staged: backup_path.to_path_buf(),
                    target: exe,
                };
                let cleanup: Vec<std::path::PathBuf> =
                    [manifest.db_backup.as_ref(), manifest.config_backup.as_ref()]
                        .into_iter()
                        .flatten()
                        .map(std::path::PathBuf::from)
                        .collect();
                let marker =
                    replace::schedule_swap(&self.home, &swap, &manifest.from_version, &cleanup)?;
                tracing::warn!(
                    "二进制回滚已安排在后台交换（marker: {}）；请先停止服务，等待 marker 出现 OK",
                    marker.display()
                );
            }
            #[cfg(not(windows))]
            {
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
        }

        // 2. 恢复数据库/配置
        //    Windows：二进制走延迟交换，DB/config 不锁定、可立即恢复
        //    Unix：二进制已在上一步同步恢复，这里仅恢复 DB/config（传 false 避免二次复制）
        #[cfg(not(windows))]
        {
            BackupManager::restore_all(&manifest, false).await?;
        }
        #[cfg(windows)]
        {
            BackupManager::restore_all(&manifest, false).await?;
        }

        // 3. 清理备份清单
        let manifest_path = self.home.join(".update_manifest.json");
        let _ = tokio::fs::remove_file(&manifest_path).await;

        // 4. 清理陈旧备份与临时产物
        //    Windows：二进制备份是延迟交换的源文件，交换脚本 `move` 消耗后自然消失，
        //    此处不能删除；其余备份（DB/config）删除。
        #[cfg(not(windows))]
        {
            BackupManager::cleanup_artifacts(&self.home, &manifest).await?;
        }

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
