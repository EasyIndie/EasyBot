//! 插件加载器（**进程外**）
//!
//! 从 `plugins/` 目录发现并加载**可执行文件形态**的插件：每个插件是一个独立进程，
//! 通过 stdin/stdout 逐行 JSON 协议（`easybot-plugin-protocol`）与宿主通信。
//!
//! 旧式 cdylib（`dlopen`）路径**已移除**：静态链接的宿主二进制没有动态加载器，
//! `dlopen` 不可能成功（详见 `docs/other/rfc-out-of-process-plugins.md`）。
//!
//! 本模块**不含任何 `unsafe`**（无 FFI）；适配器实现见
//! [`IpcPluginAdapter`](super::ipc::IpcPluginAdapter)。
//!
//! # 安全模型
//!
//! 插件以**独立子进程**运行（崩溃隔离），但**v1 不是安全沙箱**：子进程默认
//! 继承宿主用户权限，可读写文件与网络。防护措施：
//!
//! 1. **路径校验**（[`PluginManifest::command_path`]）: 拒绝绝对路径与 `..` 穿越；
//! 2. **启动验签**（`plugin.sig.json` 覆盖入口可执行文件字节）+ 发布者信任；
//! 3. **协议版本校验**（握手时 `PROTOCOL_VERSION` 不匹配即拒绝）。
//!
//! 生产隔离请用容器化（参见 `docs/18 plugin-security.md`）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::manifest::PluginManifest;
use super::signing::PluginSignature;
use super::signing::trust::PublisherTrust;
use crate::adapter::{AdapterFactory, AdapterRegistry};
use crate::bus::EventBus;
use crate::types::adapter::PlatformAdapter;

/// 插件加载错误
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Plugin directory not found: {0}")]
    DirectoryNotFound(PathBuf),

    #[error("Plugin manifest not found: {0}")]
    ManifestNotFound(PathBuf),

    #[error("Failed to parse manifest {path}: {detail}")]
    ManifestParseError { path: PathBuf, detail: String },

    #[error("Plugin entry not found: {0}")]
    EntryNotFound(PathBuf),

    #[error("Plugin entry is not executable: {path}")]
    EntryNotExecutable { path: PathBuf },

    #[error(
        "Plugin '{name}' at {path} is a legacy cdylib (only `library`, no `command`). \
         In-process plugins are no longer supported (the host is statically linked and \
         cannot dlopen): rebuild it as an executable and set `command` in plugin.yaml. \
         See docs/other/plugin-cdylib-to-process-migration.md"
    )]
    LegacyCdylibPlugin { name: String, path: PathBuf },

    #[error("Plugin platform '{0}' conflicts with already registered platform")]
    PlatformConflict(String),

    #[error("Plugin '{0}' is disabled in its manifest")]
    DisabledPlugin(String),

    #[error("Plugin signature verification failed for {path}: {detail}")]
    SignatureVerificationFailed { path: PathBuf, detail: String },

    #[error("Plugin publisher '{0}' is not trusted")]
    UntrustedPublisher(String),
}

/// 单次插件加载的结果
pub struct PluginLoadResult {
    /// 平台标识符
    pub platform_name: String,
    /// 显示名称
    pub display_name: String,
}

/// 插件加载策略
///
/// 控制签名校验强度：
///
/// - [`lenient`](PluginLoadPolicy::lenient)：dev 默认——有 `plugin.sig.json` 则验签，
///   无签名仅告警（向后兼容现有手动放置的插件）
/// - [`strict`](PluginLoadPolicy::strict)：prod——无签名或验签失败即拒绝；
///   且可选地校验发布者是否受信任（`trust` 非空时）
#[derive(Clone)]
pub struct PluginLoadPolicy {
    /// 是否开启签名校验（有 `plugin.sig.json` 即验签）
    pub verify_signatures: bool,
    /// 是否强制要求签名（strict/prod：无签名或验签失败即拒绝）
    pub require_signatures: bool,
    /// 发布者信任判定（None = 只做密码学校验，不做发布者信任校验）
    pub trust: Option<Arc<dyn PublisherTrust + Send + Sync>>,
}

impl PluginLoadPolicy {
    /// lenient：有签名验、无签名 warn（dev 默认，向后兼容）
    pub fn lenient() -> Self {
        Self {
            verify_signatures: true,
            require_signatures: false,
            trust: None,
        }
    }

    /// strict：无签名或验签失败即拒绝；`trust` 非空时校验发布者信任
    pub fn strict(trust: Option<Arc<dyn PublisherTrust + Send + Sync>>) -> Self {
        Self {
            verify_signatures: true,
            require_signatures: true,
            trust,
        }
    }
}

impl Default for PluginLoadPolicy {
    fn default() -> Self {
        Self::lenient()
    }
}

/// 插件加载器（进程外）
///
/// 扫描指定目录，发现所有有效插件（可执行文件形态）。
/// 注意：**不** 在此阶段 spawn 进程；进程在适配器工厂被调用时启动。
pub struct PluginLoader {
    plugins_dir: PathBuf,
    policy: PluginLoadPolicy,
    /// EasyBot 配置根目录（注入子进程 `EASYBOT_HOME`；`None` 时子进程继承宿主环境）
    home: Option<PathBuf>,
    /// platform_name → (入口可执行文件, display_name)
    loaded: RwLock<HashMap<String, (PathBuf, String)>>,
}

impl PluginLoader {
    /// 创建指向 `plugins/` 目录的加载器（lenient 策略）
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self::with_policy(plugins_dir, PluginLoadPolicy::lenient())
    }

    /// 创建带指定加载策略的加载器
    pub fn with_policy(plugins_dir: PathBuf, policy: PluginLoadPolicy) -> Self {
        Self {
            plugins_dir,
            policy,
            home: None,
            loaded: RwLock::new(HashMap::new()),
        }
    }

    /// 设置 EasyBot 配置根目录（注入子进程 `EASYBOT_HOME`）。
    pub fn with_home(mut self, home: Option<PathBuf>) -> Self {
        self.home = home;
        self
    }

    /// 扫描并加载所有有效插件
    ///
    /// 返回成功列表和失败列表。单插件失败不影响其他插件。
    pub async fn load_all(&self) -> (Vec<PluginLoadResult>, Vec<(PathBuf, PluginError)>) {
        let (succeeded, failed) = self.load_all_with_names().await;
        (
            succeeded.into_iter().map(|(_, result)| result).collect(),
            failed,
        )
    }

    /// 同 [`load_all`](PluginLoader::load_all)，但附带每个插件的目录名
    ///
    /// 目录名与插件 `manifest.name` 一致（市场安装按此命名），
    /// 供 [`PluginManager`](crate::plugin::manager::PluginManager)
    /// 在 load 时建立 插件名 → 平台名 映射（disable/uninstall 时停止对应适配器）。
    pub async fn load_all_with_names(
        &self,
    ) -> (Vec<(String, PluginLoadResult)>, Vec<(PathBuf, PluginError)>) {
        let mut succeeded = Vec::new();
        let mut failed = Vec::new();

        let entries = match std::fs::read_dir(&self.plugins_dir) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(
                    "Plugin directory {} not accessible: {}",
                    self.plugins_dir.display(),
                    e
                );
                return (succeeded, failed);
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            // 跳过非目录与隐藏目录（.marketplace 等内部暂存区不是插件）
            if !path.is_dir()
                || path
                    .file_name()
                    .map(|n| n.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
            {
                continue;
            }
            let dir_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            match self.load_single(&path).await {
                Ok(result) => {
                    info!(
                        "Loaded plugin '{}' ({}) from {}",
                        result.platform_name,
                        result.display_name,
                        path.display()
                    );
                    succeeded.push((dir_name, result));
                }
                Err(e) => {
                    warn!("Failed to load plugin from {}: {}", path.display(), e);
                    failed.push((path, e));
                }
            }
        }

        (succeeded, failed)
    }

    /// 加载单个插件目录
    async fn load_single(&self, dir: &Path) -> Result<PluginLoadResult, PluginError> {
        // 1. 解析 plugin.yaml
        let manifest_path = dir.join("plugin.yaml");
        if !manifest_path.exists() {
            return Err(PluginError::ManifestNotFound(manifest_path));
        }
        let content = std::fs::read_to_string(&manifest_path).map_err(|e| {
            PluginError::ManifestParseError {
                path: manifest_path.clone(),
                detail: e.to_string(),
            }
        })?;
        let manifest: PluginManifest =
            serde_yaml::from_str(&content).map_err(|e| PluginError::ManifestParseError {
                path: manifest_path.clone(),
                detail: e.to_string(),
            })?;

        // 1.5 启用检查（禁用插件跳过加载，不报错）
        if !manifest.is_enabled() {
            return Err(PluginError::DisabledPlugin(manifest.name.clone()));
        }

        // 2. 旧式 cdylib 清单（仅 `library`、无 `command`）：明确告警并给出迁移指引。
        //    v0.0.42 起插件必须是可执行文件；`library` 仅作为元数据别名被读取，
        //    不再有 dlopen 路径（静态宿主没有动态加载器）。
        if manifest.is_legacy_cdylib() {
            return Err(PluginError::LegacyCdylibPlugin {
                name: manifest.name.clone(),
                path: dir.to_path_buf(),
            });
        }

        // 3. 定位入口可执行文件（含路径穿越安全检查）
        let command_path =
            manifest
                .command_path(dir)
                .map_err(|e| PluginError::ManifestParseError {
                    path: manifest_path.clone(),
                    detail: e,
                })?;
        if !command_path.exists() {
            return Err(PluginError::EntryNotFound(command_path));
        }
        if !is_executable(&command_path) {
            return Err(PluginError::EntryNotExecutable {
                path: command_path.clone(),
            });
        }

        // 4. 签名校验（覆盖入口可执行文件字节；在 spawn 之前执行）
        if self.policy.verify_signatures {
            self.verify_signature(dir, &command_path, &manifest)?;
        }

        // 5. 平台标识：以清单 name 为准（协议握手中插件自述会再次校验）
        let platform_name = manifest.name.clone();
        let display_name = manifest
            .display_name
            .clone()
            .unwrap_or_else(|| manifest.name.clone());

        // 6. 检查平台名冲突
        {
            let loaded = self.loaded.read().await;
            if loaded.contains_key(&platform_name) {
                return Err(PluginError::PlatformConflict(platform_name));
            }
        }

        // 7. 记录入口路径与显示名
        {
            let mut loaded = self.loaded.write().await;
            loaded.insert(
                platform_name.clone(),
                (command_path.clone(), display_name.clone()),
            );
        }

        Ok(PluginLoadResult {
            platform_name,
            display_name,
        })
    }

    /// 校验插件签名（`plugin.sig.json` 覆盖入口可执行文件字节）
    ///
    /// - 有签名文件：验签 + （可选）发布者信任校验；失败 → `SignatureVerificationFailed` / `UntrustedPublisher`
    /// - 无签名文件：strict 拒绝；lenient 仅告警（向后兼容手动放置的插件）
    fn verify_signature(
        &self,
        dir: &Path,
        lib_path: &Path,
        manifest: &PluginManifest,
    ) -> Result<(), PluginError> {
        let sig_path = dir.join("plugin.sig.json");

        if sig_path.exists() {
            let sig = PluginSignature::from_file(&sig_path).map_err(|e| {
                PluginError::SignatureVerificationFailed {
                    path: sig_path.clone(),
                    detail: format!("cannot read plugin.sig.json: {e}"),
                }
            })?;

            sig.verify_library(lib_path)
                .map_err(|e| PluginError::SignatureVerificationFailed {
                    path: sig_path.clone(),
                    detail: format!(
                        "signature for '{}' does not match {}: {e}",
                        manifest.name,
                        lib_path.display()
                    ),
                })?;

            if let Some(ref trust) = self.policy.trust
                && !trust.is_trusted(&sig.publisher, &sig.public_key)
            {
                return Err(PluginError::UntrustedPublisher(sig.publisher));
            }
        } else if self.policy.require_signatures {
            return Err(PluginError::SignatureVerificationFailed {
                path: sig_path,
                detail: format!(
                    "plugin '{}' has no plugin.sig.json and strict policy requires signatures",
                    manifest.name
                ),
            });
        } else {
            warn!(
                "Plugin '{}' has no plugin.sig.json — skipping signature verification",
                manifest.name
            );
        }

        Ok(())
    }

    /// 为已加载的插件生成 AdapterFactory
    ///
    /// 工厂闭包捕获入口路径，调用时 spawn 插件进程并完成 `init`。
    pub async fn get_factory(
        &self,
        platform_name: &str,
        event_bus: Arc<EventBus>,
    ) -> Option<AdapterFactory> {
        let loaded = self.loaded.read().await;
        let (command, _display_name) = loaded.get(platform_name)?.clone();
        let platform = platform_name.to_string();
        let home = self.home.clone();
        drop(loaded);

        Some(Arc::new(move |config| {
            let command = command.clone();
            let eb = event_bus.clone();
            let p = platform.clone();
            let home = home.clone();
            Box::pin(async move {
                let mut adapter =
                    super::ipc::IpcPluginAdapter::new(command, p.clone(), p.clone(), home);
                adapter.set_event_bus(eb);
                let init_result = adapter
                    .init(config)
                    .await
                    .map_err(|e| format!("plugin '{}' init failed: {}", p, e))?;
                if !init_result.ok {
                    return Err(init_result
                        .error
                        .unwrap_or_else(|| format!("plugin '{}' init returned error", p)));
                }
                let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
                Ok(boxed)
            })
        }))
    }

    /// 注册所有已加载插件到适配器注册表
    pub async fn register_all(&self, registry: &AdapterRegistry, event_bus: Arc<EventBus>) {
        let platforms: Vec<(String, String)> = {
            let loaded = self.loaded.read().await;
            loaded
                .iter()
                .map(|(name, (_, display))| (name.clone(), display.clone()))
                .collect()
        };

        for (platform, display_name) in platforms {
            if let Some(factory) = self.get_factory(&platform, event_bus.clone()).await {
                registry
                    .register(&platform, &display_name, factory, &[])
                    .await;
            }
        }
    }

    /// 卸载一个已加载的插件（禁用/卸载时调用）
    ///
    /// 从 `loaded` 表中移除平台。运行中的适配器（及其 spawn 的插件进程）
    /// 由 `AdapterManager` 停止。
    ///
    /// 返回该平台此前是否已加载。
    pub async fn unload(&self, platform: &str) -> bool {
        let mut loaded = self.loaded.write().await;
        loaded.remove(platform).is_some()
    }
}

/// 判断文件是否可执行（Unix 检查 owner/group/other 任一执行位；其它平台仅要求存在）。
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.exists()
    }
}

/// SDK ABI 版本常量（与 easybot-plugin-sdk 中的值同步）
pub const EASYBOT_PLUGIN_ABI_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建临时插件目录，包含一个指定内容的子目录（代表一个插件）
    ///
    /// `entry_exists`：是否写入占位入口文件（`test-entry`，并置可执行位——
    /// 进程外插件要求入口可执行）。
    fn create_plugin_subdir(
        parent: &Path,
        name: &str,
        manifest_content: &str,
        entry_exists: bool,
    ) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.yaml"), manifest_content).unwrap();
        if entry_exists {
            // 写入一个占位文件充当插件入口（可执行文件形态）
            let entry = dir.join("test-entry");
            std::fs::write(&entry, b"dummy").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&entry).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&entry, perms).unwrap();
            }
        }
        dir
    }

    #[test]
    fn test_plugin_error_messages() {
        let err = PluginError::EntryNotFound(PathBuf::from("/x/entry"));
        assert!(err.to_string().contains("/x/entry"), "{}", err);

        let err = PluginError::EntryNotExecutable {
            path: PathBuf::from("/x/entry"),
        };
        assert!(err.to_string().contains("/x/entry"), "{}", err);

        let err = PluginError::LegacyCdylibPlugin {
            name: "old-plugin".into(),
            path: PathBuf::from("/plugins/old-plugin"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("old-plugin") && msg.contains("legacy cdylib"),
            "{}",
            msg
        );

        let err = PluginError::PlatformConflict("test".into());
        assert!(err.to_string().contains("test"));
    }

    #[tokio::test]
    async fn test_load_from_nonexistent_dir() {
        let loader = PluginLoader::new(PathBuf::from("/tmp/nonexistent-plugin-dir-12345"));
        let (succeeded, failed) = loader.load_all().await;
        assert!(succeeded.is_empty());
        assert!(failed.is_empty());
    }

    #[tokio::test]
    async fn test_load_all_idempotent() {
        let loader = PluginLoader::new(PathBuf::from("/tmp/nonexistent-plugin-dir-12345"));
        let (s1, f1) = loader.load_all().await;
        let (s2, f2) = loader.load_all().await;
        assert_eq!(s1.len(), s2.len(), "should return same number of succeeded");
        assert_eq!(f1.len(), f2.len(), "should return same number of failed");
    }

    #[tokio::test]
    async fn test_load_all_skips_files() {
        // 顶层有文件而非目录时，应跳过
        let dir =
            std::env::temp_dir().join(format!("plugin-test-skips-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 创建一个文件（非目录）
        std::fs::write(dir.join("not-a-dir.txt"), b"hello").unwrap();

        let loader = PluginLoader::new(dir.clone());
        let (succeeded, failed) = loader.load_all().await;
        assert!(succeeded.is_empty());
        assert!(failed.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_load_single_missing_manifest() {
        let dir = std::env::temp_dir().join(format!(
            "plugin-test-missing-manifest-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        assert!(matches!(result, Err(PluginError::ManifestNotFound(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_load_single_invalid_yaml() {
        let dir =
            std::env::temp_dir().join(format!("plugin-test-invalid-yaml-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            "invalid_yaml: [",
            false,
        );

        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        assert!(matches!(
            result,
            Err(PluginError::ManifestParseError { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_load_single_missing_library() {
        let dir =
            std::env::temp_dir().join(format!("plugin-test-missing-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            r#"name: "test-plugin"
display_name: "Test"
version: "1.0"
sdk_version: 1
command: "nonexistent-entry"
"#,
            false, // lib does NOT exist
        );

        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        assert!(matches!(result, Err(PluginError::EntryNotFound(_))));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_load_all_mixed_results() {
        // 混合场景：一个有效插件目录、一个缺少清单的、一个 YAML 错误的
        let base = std::env::temp_dir().join(format!("plugin-test-mixed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        // 子目录1：缺 manifest
        let no_manifest = base.join("no-manifest");
        std::fs::create_dir_all(&no_manifest).unwrap();

        // 子目录2：YAML 错误
        let bad_yaml = base.join("bad-yaml");
        std::fs::create_dir_all(&bad_yaml).unwrap();
        std::fs::write(bad_yaml.join("plugin.yaml"), "bad: [").unwrap();

        // 子目录3：缺失入口文件（但 manifest 有效）
        let missing_lib = base.join("missing-lib");
        std::fs::create_dir_all(&missing_lib).unwrap();
        std::fs::write(
            missing_lib.join("plugin.yaml"),
            r#"name: "missing-lib"
display_name: "Missing Lib"
version: "1.0"
sdk_version: 1
command: "missing-entry"
"#,
        )
        .unwrap();

        let loader = PluginLoader::new(base.clone());
        let (succeeded, failed) = loader.load_all().await;
        assert!(succeeded.is_empty(), "no plugin should fully succeed");
        assert_eq!(failed.len(), 3, "all 3 plugins should fail");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn test_get_factory_for_unknown_plugin() {
        let dir = std::env::temp_dir().join(format!(
            "plugin-test-unknown-factory-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let loader = PluginLoader::new(dir.clone());
        // 没有加载任何插件时，get_factory 应返回 None
        let factory = loader
            .get_factory("unknown", Arc::new(EventBus::new()))
            .await;
        assert!(
            factory.is_none(),
            "factory for unknown plugin should be None"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_loader_empty_dir() {
        let dir = std::env::temp_dir().join(format!("plugin-test-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let loader = PluginLoader::new(dir.clone());
        let (succeeded, failed) = loader.load_all().await;
        assert!(succeeded.is_empty());
        assert!(failed.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_load_single_disabled_plugin() {
        let dir = std::env::temp_dir().join(format!("plugin-test-disabled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            r#"name: "disabled-plugin"
display_name: "Disabled"
version: "1.0"
sdk_version: 1
enabled: false
command: "test-entry"
"#,
            true, // lib exists but should not be loaded
        );

        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        assert!(
            matches!(result, Err(PluginError::DisabledPlugin(name)) if name == "disabled-plugin")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_legacy_cdylib_manifest_is_rejected_with_migration_hint() {
        // 旧式 cdylib 清单（仅 `library`、无 `command`）：必须明确拒绝并给出迁移指引，
        // 而不是模糊的「入口不存在/不可执行」。
        let dir = std::env::temp_dir().join(format!("plugin-test-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            r#"name: "legacy-plugin"
display_name: "Legacy"
version: "1.0"
sdk_version: 1
library: "liblegacy.so"
"#,
            true, // 即使文件存在且可执行，也只能被迁移指引拒绝
        );

        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        match &result {
            Err(err) => {
                let PluginError::LegacyCdylibPlugin { name, .. } = err else {
                    panic!("expected LegacyCdylibPlugin, got {:?}", result.map(|_| ()));
                };
                assert_eq!(name, "legacy-plugin");
                let msg = err.to_string();
                assert!(msg.contains("legacy cdylib"), "{msg}");
                assert!(msg.contains("plugin-cdylib-to-process-migration"), "{msg}");
            }
            Ok(_) => panic!("expected LegacyCdylibPlugin, got Ok"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_strict_policy_rejects_unsigned() {
        let dir = std::env::temp_dir().join(format!(
            "plugin-test-strict-unsigned-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            r#"name: "unsigned-plugin"
display_name: "Unsigned"
version: "1.0"
sdk_version: 1
command: "test-entry"
"#,
            true, // lib exists but no plugin.sig.json
        );

        let loader = PluginLoader::with_policy(
            dir.parent().unwrap().to_path_buf(),
            PluginLoadPolicy::strict(None),
        );
        let result = loader.load_single(&dir).await;
        assert!(matches!(
            result,
            Err(PluginError::SignatureVerificationFailed { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_lenient_allows_unsigned() {
        let dir = std::env::temp_dir().join(format!(
            "plugin-test-lenient-unsigned-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            r#"name: "unsigned-plugin"
display_name: "Unsigned"
version: "1.0"
sdk_version: 1
command: "test-entry"
"#,
            true,
        );

        // lenient：无签名仅告警，继续完成加载（进程外不再有 dlopen 步骤）
        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        assert!(
            result.is_ok(),
            "lenient should proceed past signature check, got: {:?}",
            result.as_ref().err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 在插件目录写一个对给定字节内容有效的 plugin.sig.json
    fn write_sig_file(
        dir: &Path,
        lib_name: &str,
        signed_content: &[u8],
        publisher: &str,
    ) -> (PluginSignature, String) {
        use crate::plugin::signing::{
            SIGNATURE_SCHEMA_VERSION, encode_public_key, generate_keypair, sign_artifact,
        };
        let (signing, verifying) = generate_keypair();
        let sig = PluginSignature {
            schema_version: SIGNATURE_SCHEMA_VERSION,
            name: "signed-plugin".into(),
            version: "1.0.0".into(),
            publisher: publisher.into(),
            artifact: lib_name.into(),
            signature: sign_artifact(signed_content, &signing),
            public_key: encode_public_key(&verifying),
        };
        sig.write_to(&dir.join("plugin.sig.json")).unwrap();
        (sig, encode_public_key(&verifying))
    }

    #[tokio::test]
    async fn test_signature_mismatch_fails() {
        let dir =
            std::env::temp_dir().join(format!("plugin-test-sig-mismatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            r#"name: "signed-plugin"
display_name: "Signed"
version: "1.0"
sdk_version: 1
command: "test-entry"
"#,
            true,
        );

        // 签名内容与实际入口文件（b"dummy"）不符 → 验签失败（spawn 之前即拒）
        write_sig_file(&dir, "test-entry", b"different-content", "pub-a");

        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        assert!(matches!(
            result,
            Err(PluginError::SignatureVerificationFailed { .. })
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_strict_rejects_untrusted_publisher() {
        let dir =
            std::env::temp_dir().join(format!("plugin-test-untrusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            r#"name: "signed-plugin"
display_name: "Signed"
version: "1.0"
sdk_version: 1
command: "test-entry"
"#,
            true,
        );

        // 签名有效（覆盖 b"dummy"），但发布者未加入信任 → UntrustedPublisher
        write_sig_file(&dir, "test-entry", b"dummy", "pub-a");
        let empty_trust = Arc::new(crate::plugin::signing::trust::TrustStore::default());

        let loader = PluginLoader::with_policy(
            dir.parent().unwrap().to_path_buf(),
            PluginLoadPolicy::strict(Some(empty_trust)),
        );
        let result = loader.load_single(&dir).await;
        assert!(
            matches!(result, Err(PluginError::UntrustedPublisher(ref p)) if p == "pub-a"),
            "got: {:?}",
            result.map(|_| ())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_strict_accepts_trusted_publisher() {
        let dir = std::env::temp_dir().join(format!("plugin-test-trusted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_plugin_subdir(
            dir.parent().unwrap(),
            dir.file_name().unwrap().to_str().unwrap(),
            r#"name: "signed-plugin"
display_name: "Signed"
version: "1.0"
sdk_version: 1
command: "test-entry"
"#,
            true,
        );

        let (_sig, pk_b64) = write_sig_file(&dir, "test-entry", b"dummy", "pub-a");
        let mut trust = crate::plugin::signing::trust::TrustStore::default();
        trust.add("pub-a", &pk_b64);

        let loader = PluginLoader::with_policy(
            dir.parent().unwrap().to_path_buf(),
            PluginLoadPolicy::strict(Some(Arc::new(trust))),
        );
        // 签名 + 信任都通过 → 加载成功（进程外不再有 dlopen 步骤）
        let result = loader.load_single(&dir).await;
        assert!(
            result.is_ok(),
            "should pass signature+trust and load, got: {:?}",
            result.as_ref().err()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_register_all_empty_registry() {
        let dir =
            std::env::temp_dir().join(format!("plugin-test-empty-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let loader = PluginLoader::new(dir.clone());
        loader.load_all().await;

        let registry = AdapterRegistry::new();
        let eb = Arc::new(EventBus::new());
        // 没有加载任何插件时，register_all 不应 panic
        loader.register_all(&registry, eb).await;
        let platforms = registry.list_platforms().await;
        assert!(platforms.is_empty(), "registry should still be empty");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
