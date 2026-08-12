//! 插件加载器
//!
//! 从 `plugins/` 目录发现并加载动态库插件。
//! 所有 `unsafe` 代码隔离在此文件中。
//!
//! 本文件是唯一需要 `unsafe` 代码的模块（FFI / 动态库加载），
//! 因此显式允许 unsafe——workspace lint 规则 `unsafe_code = "deny"` 对此文件豁免。
#![allow(unsafe_code)]
//!
//! # 安全性
//!
//! - `PluginLibrary` 通过 `Arc<Library>` 管理动态库生命周期
//! - 工厂闭包捕获 `Arc<Library>`，确保适配器存活期间库不被卸载
//! - 所有裸指针操作限制在 `create_adapter()` 方法内
//! - ABI 版本号在创建适配器前校验
//!
//! # 沙箱限制
//!
//! **Warning**: 原生动态库插件（`.so`/`.dylib`/`.dll`）在宿主进程内运行，
//! **不受沙箱保护**。插件代码享有与 EasyBot 进程完全相同的权限：
//!
//! - 文件系统访问（包括数据库文件和凭证文件）
//! - 网络访问（可绕过 EasyBot 的 HTTP 客户端）
//! - 内存访问（可读取进程内所有数据）
//!
//! **当前实现的防护措施：**
//!
//! 1. **路径校验** ([`PluginManifest::library_path()`]):
//!    - 拒绝绝对路径（防止加载任意位置的文件）
//!    - 拒绝 `..` 目录穿越（防止离开插件目录）
//!
//! 2. **Lint 规则**（workspace）:
//!    - `unsafe_code = "deny"` — 禁止插件使用 unsafe 代码
//!
//! **建议的安全实践：**
//! - 仅从可信来源安装插件
//! - 在容器化环境中运行 EasyBot
//! - 生产部署前审计插件源码
//! - 参见 [SECURITY.md] 了解更多

use libloading::{Library, Symbol};
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

    #[error("Library not found: {0}")]
    LibraryNotFound(PathBuf),

    #[error("Failed to load library {path}: {detail}")]
    LibraryLoadError { path: PathBuf, detail: String },

    #[error("Required symbol '{symbol}' not found in {path}: {detail}")]
    SymbolNotFound {
        path: PathBuf,
        symbol: String,
        detail: String,
    },

    #[error("ABI version mismatch: plugin uses v{got}, host expects v{expected}")]
    AbiVersionMismatch { expected: u32, got: u32 },

    #[error("Plugin returned null adapter pointer")]
    NullAdapter,

    #[error("Plugin platform '{0}' conflicts with already registered platform")]
    PlatformConflict(String),

    #[error("Plugin '{0}' is disabled in its manifest")]
    DisabledPlugin(String),

    #[error("Plugin signature verification failed for {path}: {detail}")]
    SignatureVerificationFailed { path: PathBuf, detail: String },

    #[error("Plugin publisher '{0}' is not trusted")]
    UntrustedPublisher(String),
}

/// 已加载的插件库包装
///
/// 使用 `Arc<Library>` 允许多个工厂闭包共享同一个动态库句柄。
/// 当所有引用释放时，库自动卸载。
pub struct PluginLibrary {
    inner: Arc<Library>,
}

// SAFETY: Library 自身不是 Send/Sync，但 Arc<Library> 通过引用计数管理，
// 且所有实际内存访问发生在工厂闭包内部（通过 `unsafe` 方法）。
// PluginLibrary 提供安全的封装，外部代码通过安全接口访问。
unsafe impl Send for PluginLibrary {}
unsafe impl Sync for PluginLibrary {}

impl PluginLibrary {
    /// 包装一个已加载的 Library
    ///
    /// # Safety
    ///
    /// `lib` 必须保持有效，直到所有从它创建的适配器都被销毁。
    pub unsafe fn new(lib: Library) -> Self {
        Self {
            inner: Arc::new(lib),
        }
    }

    /// 从插件创建适配器实例
    ///
    /// # Safety
    ///
    /// 返回的 `Box<dyn PlatformAdapter>` 包含指向本库代码段的函数指针。
    /// 本 `PluginLibrary` 实例必须比所有适配器存活得更久。
    pub unsafe fn create_adapter(&self) -> Result<Box<dyn PlatformAdapter>, PluginError> {
        unsafe {
            let create: Symbol<unsafe extern "C" fn() -> *mut std::ffi::c_void> = self
                .inner
                .get(b"easybot_plugin_create")
                .map_err(|e| PluginError::SymbolNotFound {
                    path: PathBuf::from("<plugin>"),
                    symbol: "easybot_plugin_create".into(),
                    detail: e.to_string(),
                })?;

            let ptr = create();
            if ptr.is_null() {
                return Err(PluginError::NullAdapter);
            }

            // `Box<dyn PlatformAdapter>` 是胖指针（128 bits），不能直接存为 `*mut c_void`
            // 插件方通过 `Box<Box<dyn PlatformAdapter>>` 做了一层包装（瘦指针）
            // 这里解一层 Box 即可
            let inner: Box<Box<dyn PlatformAdapter>> =
                Box::from_raw(ptr as *mut Box<dyn PlatformAdapter>);
            let adapter: Box<dyn PlatformAdapter> = *inner;
            Ok(adapter)
        }
    }

    /// 验证插件 ABI 版本与主机匹配
    fn check_abi_version(&self) -> Result<(), PluginError> {
        unsafe {
            let abi_version: Symbol<unsafe extern "C" fn() -> u32> = self
                .inner
                .get(b"easybot_abi_version")
                .map_err(|e| PluginError::SymbolNotFound {
                    path: PathBuf::from("<plugin>"),
                    symbol: "easybot_abi_version".into(),
                    detail: e.to_string(),
                })?;

            let version = abi_version();
            let expected = EASYBOT_PLUGIN_ABI_VERSION;
            if version != expected {
                return Err(PluginError::AbiVersionMismatch {
                    expected,
                    got: version,
                });
            }
            Ok(())
        }
    }
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

/// 插件加载器
///
/// 扫描指定目录，加载所有有效插件。
pub struct PluginLoader {
    plugins_dir: PathBuf,
    policy: PluginLoadPolicy,
    /// platform_name → (library, display_name)
    loaded: RwLock<HashMap<String, (Arc<PluginLibrary>, String)>>,
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
            loaded: RwLock::new(HashMap::new()),
        }
    }

    /// 扫描并加载所有有效插件
    ///
    /// 返回成功列表和失败列表。单插件失败不影响其他插件。
    pub async fn load_all(&self) -> (Vec<PluginLoadResult>, Vec<(PathBuf, PluginError)>) {
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
            if !path.is_dir() {
                continue;
            }

            match self.load_single(&path).await {
                Ok(result) => {
                    info!(
                        "Loaded plugin '{}' ({}) from {}",
                        result.platform_name,
                        result.display_name,
                        path.display()
                    );
                    succeeded.push(result);
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

        // 2. 定位动态库（含路径穿越安全检查）
        let lib_path = manifest
            .library_path(dir)
            .map_err(|e| PluginError::ManifestParseError {
                path: manifest_path.clone(),
                detail: e,
            })?;
        if !lib_path.exists() {
            return Err(PluginError::LibraryNotFound(lib_path));
        }

        // 2.5 签名校验（在 dlopen 之前执行，避免加载未经验证的代码）
        if self.policy.verify_signatures {
            self.verify_signature(dir, &lib_path, &manifest)?;
        }

        // 3. 加载动态库
        // SAFETY: dlopen/dlsym 是 unsafe 操作，因为动态库中的代码
        // 在执行构造函数时立即运行。我们已经验证了文件存在性与签名。
        let library = unsafe {
            Library::new(&lib_path).map_err(|e| PluginError::LibraryLoadError {
                path: lib_path.clone(),
                detail: e.to_string(),
            })?
        };

        let plugin_lib = unsafe { PluginLibrary::new(library) };

        // 4. 验证 ABI 版本
        plugin_lib.check_abi_version()?;

        // 5. 创建临时适配器提取元信息
        // SAFETY: 暂存适配器后立即释放，PluginLibrary 在此期间保持存活
        let (platform_name, display_name) = unsafe {
            let adapter = plugin_lib.create_adapter()?;
            let name = adapter.platform_name().to_string();
            let display = manifest
                .display_name
                .clone()
                .unwrap_or_else(|| adapter.display_name().to_string());
            // drop adapter 会通过 vtable 调用 plugin 的析构函数
            // 此时 Library 仍然加载，所以是安全的
            drop(adapter);
            (name, display)
        };

        // 6. 检查平台名冲突
        {
            let loaded = self.loaded.read().await;
            if loaded.contains_key(&platform_name) {
                return Err(PluginError::PlatformConflict(platform_name));
            }
        }

        // 7. 存储库引用和显示名
        let arc_lib = Arc::new(plugin_lib);
        {
            let mut loaded = self.loaded.write().await;
            loaded.insert(
                platform_name.clone(),
                (arc_lib.clone(), display_name.clone()),
            );
        }

        Ok(PluginLoadResult {
            platform_name,
            display_name,
        })
    }

    /// 校验插件签名（`plugin.sig.json` 覆盖动态库字节）
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
    /// 工厂闭包捕获 `Arc<Library>`，确保适配器存活期间库不被卸载。
    pub async fn get_factory(
        &self,
        platform_name: &str,
        event_bus: Arc<EventBus>,
    ) -> Option<AdapterFactory> {
        let loaded = self.loaded.read().await;
        let (lib, _display_name) = loaded.get(platform_name)?.clone();
        let platform = platform_name.to_string();
        drop(loaded);

        Some(Arc::new(move |config| {
            let lib = lib.clone();
            let eb = event_bus.clone();
            let p = platform.clone();
            Box::pin(async move {
                // SAFETY: 适配器创建涉及从动态库加载函数指针
                // Arc<Library> 确保库在闭包执行期间保持存活
                unsafe {
                    let mut adapter = lib
                        .create_adapter()
                        .map_err(|e| format!("plugin create failed: {}", e))?;

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
                    Ok(adapter)
                }
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
}

/// SDK ABI 版本常量（与 easybot-plugin-sdk 中的值同步）
pub const EASYBOT_PLUGIN_ABI_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建临时插件目录，包含一个指定内容的子目录（代表一个插件）
    fn create_plugin_subdir(
        parent: &Path,
        name: &str,
        manifest_content: &str,
        lib_exists: bool,
    ) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.yaml"), manifest_content).unwrap();
        if lib_exists {
            // 写入一个占位文件充当 "库文件"
            std::fs::write(dir.join("libtest.so"), b"dummy").unwrap();
        }
        dir
    }

    #[test]
    fn test_plugin_error_messages() {
        let err = PluginError::AbiVersionMismatch {
            expected: 1,
            got: 2,
        };
        let msg = err.to_string();
        assert!(msg.contains("v1"), "expected 'v1' in '{}'", msg);
        assert!(msg.contains("v2"), "expected 'v2' in '{}'", msg);

        let err = PluginError::NullAdapter;
        assert!(err.to_string().contains("null"));

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
library: "libnonexistent.so"
"#,
            false, // lib does NOT exist
        );

        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        assert!(matches!(result, Err(PluginError::LibraryNotFound(_))));

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

        // 子目录3：缺失库文件（但 manifest 有效）
        let missing_lib = base.join("missing-lib");
        std::fs::create_dir_all(&missing_lib).unwrap();
        std::fs::write(
            missing_lib.join("plugin.yaml"),
            r#"name: "missing-lib"
display_name: "Missing Lib"
version: "1.0"
sdk_version: 1
library: "libmissing.so"
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
library: "libtest.so"
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
library: "libtest.so"
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
library: "libtest.so"
"#,
            true,
        );

        // lenient：无签名仅告警，继续走到 dlopen（假库 → LibraryLoadError，而非签名错误）
        let loader = PluginLoader::new(dir.parent().unwrap().to_path_buf());
        let result = loader.load_single(&dir).await;
        assert!(
            matches!(result, Err(PluginError::LibraryLoadError { .. })),
            "lenient should proceed past signature check, got: {:?}",
            result.map(|_| ())
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
library: "libtest.so"
"#,
            true,
        );

        // 签名内容与实际库文件（b"dummy"）不符 → 验签失败（dlopen 之前即拒）
        write_sig_file(&dir, "libtest.so", b"different-content", "pub-a");

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
library: "libtest.so"
"#,
            true,
        );

        // 签名有效（覆盖 b"dummy"），但发布者未加入信任 → UntrustedPublisher
        write_sig_file(&dir, "libtest.so", b"dummy", "pub-a");
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
library: "libtest.so"
"#,
            true,
        );

        let (_sig, pk_b64) = write_sig_file(&dir, "libtest.so", b"dummy", "pub-a");
        let mut trust = crate::plugin::signing::trust::TrustStore::default();
        trust.add("pub-a", &pk_b64);

        let loader = PluginLoader::with_policy(
            dir.parent().unwrap().to_path_buf(),
            PluginLoadPolicy::strict(Some(Arc::new(trust))),
        );
        // 签名 + 信任都通过 → 走到 dlopen（假库 → LibraryLoadError，而非签名/信任错误）
        let result = loader.load_single(&dir).await;
        assert!(
            matches!(result, Err(PluginError::LibraryLoadError { .. })),
            "should pass signature+trust and reach dlopen, got: {:?}",
            result.map(|_| ())
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
