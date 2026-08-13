//! 插件管理器
//!
//! 编排插件生命周期：安装/更新/卸载/启停，市场目录查询，发布者信任。
//!
//! # 信任语义（对齐 VS Code 1.97）
//!
//! - 首次安装**未受信任**发布者的插件需确认（`install` 返回 `needs_trust`，
//!   CLI/UI 弹确认后带 `trust: true` 重试）
//! - `--yes` / `trust: true` 跳过确认，但**不自动**加入 `.trust`
//!   （显式 `plugin trust <publisher>` 才写入 `{plugins_dir}/.trust`）
//! - 信任 = 配置 `trusted_publishers`（官方内置 + 覆盖） ∪ 用户 `.trust`，
//!   按**发布者**粒度，公钥指纹匹配
//!
//! # 事务性
//!
//! 安装：解析 → ABI/requires 预检 → 下载（sha256）→ 验签 → 全部通过后
//! 原子 `rename` 进 `plugins/{name}/`（临时目录与目标同文件系统）。
//! 安装/卸载/启停经内部 `Mutex` 串行化，与启动 `load_all` 互斥。

use super::error::PluginManagerError;
use super::install::{
    build_signature, check_abi, check_easybot_range, default_library_name, parse_manifest_yaml,
    pick_version, place_installed, resolve_source, split_qualified, synthesize_manifest,
    validate_artifact_url, validate_library_name, validate_name,
};
use super::loader::{PluginError, PluginLoadPolicy, PluginLoadResult, PluginLoader};
use super::manifest::PluginManifest;
use super::registry::PluginRegistry;
use super::registry::github::GitHubRegistry;
use super::registry::types::{PluginChannel, PluginSource, PluginVersionMeta};
use super::signing::trust::{CompositePublisherTrust, TrustStore};
use super::signing::{PluginSignature, SigningError, parse_public_key, verify_artifact};
use crate::adapter::AdapterManager;
use crate::bus::EventBus;
use crate::types::config::PluginConfig;
use crate::updater::types::{current_target_triple, is_newer_than};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

/// 已安装插件（API/UI 展示用）
#[derive(Debug, Clone)]
pub struct InstalledPlugin {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub sdk_version: u32,
    pub enabled: bool,
    /// 是否存在 `plugin.sig.json`
    pub signed: bool,
    /// 签名是否通过校验（`None` = 未签名无法校验）
    pub signature_valid: Option<bool>,
    /// 发布者标识（签名文件优先，其次 manifest.author）
    pub publisher: Option<String>,
    /// 已加载时的平台名（未加载为 `None`）
    pub platform: Option<String>,
    /// 加载失败原因（未加载且非禁用时）
    pub load_error: Option<String>,
}

/// 安装请求
#[derive(Debug, Clone)]
pub struct InstallRequest {
    /// 插件限定名（`publisher/name` 或裸 `name`）
    pub qualified: String,
    /// 发布渠道（默认 stable）
    pub channel: PluginChannel,
    /// 接受发布者信任确认（跳过 `needs_trust`；**不自动**写入 `.trust`）
    pub trust: bool,
    /// 允许降级安装（默认拒绝）
    pub allow_downgrade: bool,
    /// 离线安装源目录（含 `plugin.yaml` + 库文件 + 可选 `plugin.sig.json`）
    pub file: Option<PathBuf>,
}

impl Default for InstallRequest {
    fn default() -> Self {
        Self {
            qualified: String::new(),
            channel: PluginChannel::Stable,
            trust: false,
            allow_downgrade: false,
            file: None,
        }
    }
}

/// 安装结果
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub name: String,
    pub publisher: String,
    pub version: String,
    /// 需要用户确认发布者信任（未受信任且未给 `trust`）——非错误，UI 弹确认后重试
    pub needs_trust: bool,
    /// 是否为升级（覆盖已安装版本）
    pub upgraded: bool,
}

/// 更新选项
#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    /// 跨到最新版本（默认 pin 当前版本）
    pub latest: bool,
    /// 渠道（默认 stable；`latest` 时按此渠道选最新）
    pub channel: Option<PluginChannel>,
}

/// 市场插件详情
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub source: PluginSource,
    pub versions: Vec<PluginVersionMeta>,
    pub installed_version: Option<String>,
}

/// 插件管理器
pub struct PluginManager {
    plugins_dir: PathBuf,
    /// 注册源（多源合并，Taps 模型）；热重载时经 `set_registries` 重建
    registries: RwLock<Vec<Arc<dyn PluginRegistry>>>,
    trust_store: Arc<std::sync::RwLock<TrustStore>>,
    trust_path: PathBuf,
    config: Arc<RwLock<PluginConfig>>,
    loader: Arc<PluginLoader>,
    adapter_manager: Arc<AdapterManager>,
    event_bus: Arc<EventBus>,
    /// 插件名 → 平台名（`load_all` 时记录，供 disable/uninstall 停止适配器）
    platforms: RwLock<HashMap<String, String>>,
    /// 插件名 → 加载失败原因
    load_failures: RwLock<HashMap<String, String>>,
    /// 串行化安装/卸载/启停/更新（与启动 `load_all` 互斥）
    op_lock: Arc<Mutex<()>>,
}

impl PluginManager {
    /// 创建插件管理器
    ///
    /// 加载策略：dev（`production=false`）用 lenient——有签名验签、无签名仅告警；
    /// prod 由配置决定：`allow_untrusted=false` 时 strict（强制签名 + 发布者信任），
    /// `allow_untrusted=true` 时退化为"有签名验、无签名放行"。`verify_signatures`
    /// 配置关闭时跳过签名校验（死字段接线）。
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        plugins_dir: PathBuf,
        config: Arc<RwLock<PluginConfig>>,
        adapter_manager: Arc<AdapterManager>,
        event_bus: Arc<EventBus>,
        production: bool,
    ) -> Self {
        let trust_path = plugins_dir.join(".trust");
        let trust_store = Arc::new(std::sync::RwLock::new(TrustStore::load(&trust_path)));
        let cfg = config.read().await;

        let policy = if production && !cfg.allow_untrusted {
            // prod 且不允许未受信任：强制签名 + 发布者信任（配置 trusted_publishers ∪ .trust）
            PluginLoadPolicy::strict(Some(Arc::new(CompositePublisherTrust::new(
                trust_store.clone(),
                cfg.trusted_publishers.clone(),
            ))))
        } else {
            PluginLoadPolicy {
                verify_signatures: cfg.verify_signatures,
                require_signatures: false,
                trust: None,
            }
        };
        let loader = Arc::new(PluginLoader::with_policy(plugins_dir.clone(), policy));
        let registries = build_registries(&cfg);
        drop(cfg);
        Self {
            plugins_dir,
            registries: RwLock::new(registries),
            trust_store,
            trust_path,
            config,
            loader,
            adapter_manager,
            event_bus,
            platforms: RwLock::new(HashMap::new()),
            load_failures: RwLock::new(HashMap::new()),
            op_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 按配置重建注册源（热重载 `plugins.registries` 后调用）
    pub async fn set_registries(&self, config: &PluginConfig) {
        *self.registries.write().await = build_registries(config);
    }

    /// 直接替换注册源列表（测试注入 / 运行时自定义）
    pub async fn set_registry_sources(&self, regs: Vec<Arc<dyn PluginRegistry>>) {
        *self.registries.write().await = regs;
    }

    pub fn plugins_dir(&self) -> &Path {
        &self.plugins_dir
    }

    pub fn loader(&self) -> &PluginLoader {
        &self.loader
    }

    pub fn trust_store(&self) -> &Arc<std::sync::RwLock<TrustStore>> {
        &self.trust_store
    }

    /// 将已加载插件注册为适配器工厂（启动时 `load_all` 后调用）
    pub async fn register_loaded(&self) {
        self.loader
            .register_all(self.adapter_manager.registry(), self.event_bus.clone())
            .await;
    }

    /// 扫描并加载全部插件，记录 插件名 → 平台名 映射与加载失败原因
    pub async fn load_all(&self) -> (Vec<PluginLoadResult>, Vec<(PathBuf, PluginError)>) {
        let (named, failed) = self.loader.load_all_with_names().await;

        let mut platforms = HashMap::new();
        for (dir_name, result) in &named {
            platforms.insert(dir_name.clone(), result.platform_name.clone());
        }
        let mut failures = HashMap::new();
        for (path, err) in &failed {
            if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned())
                && !name.starts_with('.')
            {
                failures.insert(name, err.to_string());
            }
        }

        *self.platforms.write().await = platforms;
        *self.load_failures.write().await = failures;
        (
            named.into_iter().map(|(_, result)| result).collect(),
            failed,
        )
    }

    /// 列出已安装插件（含签名状态与加载失败原因，无需 dlopen）
    pub async fn list_installed(&self) -> Vec<InstalledPlugin> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.plugins_dir) {
            Ok(entries) => entries,
            Err(_) => return out,
        };
        let platforms = self.platforms.read().await;
        let failures = self.load_failures.read().await;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().map(|n| n.to_string_lossy().into_owned()) {
                Some(n) => n,
                None => continue,
            };
            if name.starts_with('.') {
                continue;
            }
            let manifest_path = path.join("plugin.yaml");
            if !manifest_path.exists() {
                continue;
            }

            // 清单解析（失败仍展示，标 load_error）
            let manifest = match parse_manifest_yaml(
                &std::fs::read_to_string(&manifest_path).unwrap_or_default(),
            ) {
                Ok(m) => m,
                Err(e) => {
                    out.push(InstalledPlugin {
                        name,
                        display_name: None,
                        description: None,
                        version: String::new(),
                        sdk_version: 0,
                        enabled: true,
                        signed: path.join("plugin.sig.json").exists(),
                        signature_valid: None,
                        publisher: None,
                        platform: None,
                        load_error: Some(format!("manifest parse error: {e}")),
                    });
                    continue;
                }
            };

            // 签名状态（读文件验签，不 dlopen）
            let (signed, signature_valid, publisher) = inspect_signature(&path, &manifest);

            // 平台/加载失败映射：`load_all` 按**目录名**记录，市场安装目录名 ==
            // manifest.name；手动放置插件目录名可能与 manifest.name 不一致 →
            // 先按 manifest.name 查，未命中回退目录名。
            let lookup = [manifest.name.as_str(), name.as_str()];
            let platform = lookup.iter().find_map(|n| platforms.get(*n)).cloned();
            let load_error = lookup.iter().find_map(|n| failures.get(*n)).cloned();

            out.push(InstalledPlugin {
                name: manifest.name.clone(),
                display_name: manifest.display_name.clone(),
                description: manifest.description.clone(),
                version: manifest.version.clone(),
                sdk_version: manifest.sdk_version,
                enabled: manifest.is_enabled(),
                signed,
                signature_valid,
                publisher: publisher.or_else(|| manifest.author.clone()),
                platform,
                load_error,
            });
        }
        out
    }

    /// 搜索市场目录（多源合并；`query` 过滤名称/描述/标签）
    pub async fn search_catalog(&self, query: Option<&str>) -> Vec<PluginSource> {
        let regs = self.registries.read().await.clone();
        let mut all = Vec::new();
        for reg in &regs {
            match reg.catalog().await {
                Ok(catalog) => all.extend(catalog.plugins),
                Err(e) => tracing::warn!("plugin catalog fetch failed: {e}"),
            }
        }
        // 按 publisher/name 去重（首源优先）
        let mut seen = HashSet::new();
        all.retain(|p| seen.insert(format!("{}/{}", p.publisher, p.name)));

        if let Some(q) = query.map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let q = q.to_lowercase();
            all.retain(|p| {
                p.name.to_lowercase().contains(&q)
                    || p.display_name
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q)
                    || p.description
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&q)
                    || p.tags.iter().any(|t| t.to_lowercase().contains(&q))
            });
        }
        all
    }

    /// 市场插件详情（源码条目 + 版本列表 + 已装版本）
    pub async fn plugin_info(&self, qualified: &str) -> Result<PluginInfo, PluginManagerError> {
        let (publisher, name) = split_qualified(qualified);
        validate_name(&name)?;
        let regs = self.registries.read().await.clone();
        let (registry, source) = resolve_source(&regs, publisher.as_deref(), &name).await?;
        let versions = registry.versions_for(&source, 50).await?;
        let installed = self.read_installed_manifest(&name).await?;
        Ok(PluginInfo {
            source,
            versions,
            installed_version: installed.map(|m| m.version),
        })
    }

    /// 安装插件（市场下载 或 离线 `--file`）
    pub async fn install(&self, req: InstallRequest) -> Result<InstallOutcome, PluginManagerError> {
        let _guard = self.op_lock.lock().await;

        if let Some(file) = &req.file {
            return self.install_from_file(file, &req).await;
        }

        let (publisher, name) = split_qualified(&req.qualified);
        validate_name(&name)?;

        let regs = self.registries.read().await.clone();
        let (registry, source) = resolve_source(&regs, publisher.as_deref(), &name).await?;
        let versions = registry.versions_for(&source, 50).await?;
        let meta = pick_version(&versions, req.channel).ok_or_else(|| {
            PluginManagerError::Other(format!(
                "no '{}' release of plugin '{}' found",
                channel_label(req.channel),
                name
            ))
        })?;

        self.install_meta(
            registry,
            &source,
            meta,
            req.trust,
            req.allow_downgrade,
            false,
        )
        .await
    }

    /// 更新插件（默认 pin 当前版本；`--latest`/`--channel` 跨版本）
    pub async fn update(
        &self,
        name: &str,
        opts: UpdateOptions,
    ) -> Result<InstallOutcome, PluginManagerError> {
        let _guard = self.op_lock.lock().await;
        let installed = self
            .read_installed_manifest(name)
            .await?
            .ok_or_else(|| PluginManagerError::NotFound(name.to_string()))?;

        let regs = self.registries.read().await.clone();
        let (registry, source) = resolve_source(&regs, None, name).await?;
        let versions = registry.versions_for(&source, 50).await?;
        let channel = opts.channel.unwrap_or(PluginChannel::Stable);

        let meta = if opts.latest {
            let m = pick_version(&versions, channel).ok_or_else(|| {
                PluginManagerError::Other(format!(
                    "no '{}' release of plugin '{}' found",
                    channel_label(channel),
                    name
                ))
            })?;
            if m.version == installed.version {
                return Err(PluginManagerError::AlreadyInstalled(name.to_string()));
            }
            m
        } else {
            // 默认 pin 当前版本：重新拉取同版本产物（重建/修复刷新）
            versions
                .iter()
                .find(|v| v.version == installed.version)
                .ok_or_else(|| PluginManagerError::VersionNotFound {
                    name: name.to_string(),
                    version: installed.version.clone(),
                })?
        };

        // 已装插件更新：同密钥信任视为已授予；发布者换了公钥（密钥轮换/泄露）→
        // `is_publisher_trusted` 判定未受信任 → 返回 `needs_trust` 由 CLI/UI 显式确认。
        // `trust=false`：更新**不自动**写入 `.trust`（对齐"一次性确认"信任语义）。
        self.install_meta(registry, &source, meta, false, false, true)
            .await
    }

    /// 卸载插件（停止+注销运行中的适配器后删除目录）
    pub async fn uninstall(&self, name: &str) -> Result<(), PluginManagerError> {
        let _guard = self.op_lock.lock().await;
        validate_name(name)?;
        let dir = self.plugins_dir.join(name);
        if !dir.exists() {
            return Err(PluginManagerError::NotFound(name.to_string()));
        }

        if let Some(platform) = self.platforms.read().await.get(name).cloned() {
            self.stop_adapter(name, &platform).await?;
        }
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    /// 启用/禁用插件
    ///
    /// 禁用：写 `enabled:false` 并立即 stop+unregister（适配器停止）。
    /// 启用：写回 `true`，**下次启动生效**（v1 避免热 dlopen）。
    pub async fn set_enabled(&self, name: &str, enabled: bool) -> Result<bool, PluginManagerError> {
        let _guard = self.op_lock.lock().await;
        let dir = self.plugins_dir.join(name);
        let manifest_path = dir.join("plugin.yaml");
        if !manifest_path.exists() {
            return Err(PluginManagerError::NotFound(name.to_string()));
        }
        let content = std::fs::read_to_string(&manifest_path)?;
        let mut manifest = parse_manifest_yaml(&content)?;
        manifest.enabled = Some(enabled);

        // temp + rename 原子改写（防半写损坏清单）
        let yaml = serde_yaml::to_string(&manifest)?;
        let tmp = dir.join("plugin.yaml.tmp");
        std::fs::write(&tmp, yaml)?;
        std::fs::rename(&tmp, &manifest_path)?;

        if !enabled && let Some(platform) = self.platforms.read().await.get(name).cloned() {
            self.stop_adapter(name, &platform).await?;
        }
        Ok(enabled)
    }

    /// 显式信任发布者（写入 `{plugins_dir}/.trust`）
    ///
    /// 先校验公钥格式（ed25519 base64），防止坏数据进 `.trust`。
    pub async fn trust_publisher(
        &self,
        publisher: &str,
        public_key_b64: &str,
    ) -> Result<(), SigningError> {
        parse_public_key(public_key_b64)?;
        let mut trust = self
            .trust_store
            .write()
            .map_err(|_| SigningError::Other("trust store lock poisoned".to_string()))?;
        trust.add(publisher, public_key_b64);
        // 确保 plugins 目录存在（市场安装前目录可能尚未创建）
        std::fs::create_dir_all(&self.plugins_dir)?;
        trust.save(&self.trust_path)
    }

    /// 读取已安装插件的清单（未安装返回 `None`）
    pub async fn read_installed_manifest(
        &self,
        name: &str,
    ) -> Result<Option<PluginManifest>, PluginManagerError> {
        let dir = self.plugins_dir.join(name);
        let manifest_path = dir.join("plugin.yaml");
        if !manifest_path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&manifest_path)?;
        Ok(Some(parse_manifest_yaml(&content)?))
    }

    /// 离线安装：本地目录（`plugin.yaml` + 库 + 可选签名）走同一验签/ABI 流水线，仅跳过下载
    async fn install_from_file(
        &self,
        dir: &Path,
        req: &InstallRequest,
    ) -> Result<InstallOutcome, PluginManagerError> {
        let manifest_path = dir.join("plugin.yaml");
        if !manifest_path.exists() {
            return Err(PluginManagerError::NotFound(format!(
                "manifest not found: {}",
                manifest_path.display()
            )));
        }
        let content = std::fs::read_to_string(&manifest_path)?;
        let manifest = parse_manifest_yaml(&content)?;
        validate_name(&manifest.name)?;
        // 库名只允许裸文件名（与市场安装一致；比 `library_path` 的绝对路径/`..` 检查更早、更严）
        if let Some(lib) = &manifest.library {
            validate_library_name(lib)?;
        }
        check_abi(&manifest.name, manifest.sdk_version)?;

        // requires.easybot 兼容范围校验（离线 `--file` 读 plugin.yaml 的 requires）
        if let Some(req) = &manifest.requires
            && let Some(range) = &req.easybot
            && !check_easybot_range(range, env!("CARGO_PKG_VERSION"))
        {
            return Err(PluginManagerError::EasyBotVersionRequirement {
                name: manifest.name.clone(),
                range: range.clone(),
                current: env!("CARGO_PKG_VERSION").to_string(),
            });
        }

        let lib_path = manifest
            .library_path(dir)
            .map_err(PluginManagerError::Other)?;
        if !lib_path.exists() {
            return Err(PluginManagerError::Loader(PluginError::LibraryNotFound(
                lib_path,
            )));
        }

        // 验签 + 信任（有 plugin.sig.json 时）
        let mut signature: Option<PluginSignature> = None;
        let sig_path = dir.join("plugin.sig.json");
        if sig_path.exists() {
            let sig = PluginSignature::from_file(&sig_path)?;
            sig.verify_library(&lib_path)?;

            let trusted = self
                .is_publisher_trusted(&sig.publisher, &sig.public_key)
                .await;
            if !trusted && !req.trust {
                return Ok(InstallOutcome {
                    name: manifest.name.clone(),
                    publisher: sig.publisher.clone(),
                    version: manifest.version.clone(),
                    needs_trust: true,
                    upgraded: false,
                });
            }
            // 一次性确认：不写入 `.trust`（显式 `plugin trust <publisher>` 才写）
            signature = Some(sig);
        } else if !self.config.read().await.allow_untrusted {
            return Err(PluginManagerError::SignatureRequired(manifest.name));
        }

        // 已安装 → 降级/同版本保护
        let installed = self.read_installed_manifest(&manifest.name).await?;
        if let Some(inst) = &installed
            && !req.allow_downgrade
            && is_newer_than(&inst.version, &manifest.version)
        {
            return Err(PluginManagerError::DowngradeNotAllowed {
                name: manifest.name.clone(),
                installed: inst.version.clone(),
                available: manifest.version.clone(),
            });
        }

        // 复制到 staging 后原子落位
        //
        // 缺省库名必须按**宿主 triple** 推导：`library_path()`（manifest.rs）已用
        // `cfg!(target_os)` 解析出 `.dylib`/`.dll`/`.so`，落位名须与之一致，
        // 否则加载期（libloading + 磁盘库验签）找不到正确扩展名的库。
        // 曾传字面量 "host"——既不含 "windows" 也不含 "apple"，恒落入 `.so`
        // 分支，macOS/Windows 上离线 `install --file` 落位错误、验签失效。
        let library_file = manifest.library.clone().unwrap_or_else(|| {
            let triple = current_target_triple()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "unknown".to_string());
            default_library_name(&manifest.name, &triple)
        });
        validate_library_name(&library_file)?;
        let staging = self.marketplace_tmp().await?;
        let staging_lib = staging.join(&library_file);
        if let Err(e) = std::fs::copy(&lib_path, &staging_lib) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(PluginManagerError::Io(e));
        }

        let result = place_installed(
            &self.plugins_dir,
            &manifest.name,
            &staging,
            &manifest,
            signature.as_ref(),
            installed.is_some(),
        );
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result?;

        Ok(InstallOutcome {
            name: manifest.name.clone(),
            publisher: manifest.author.clone().unwrap_or_default(),
            version: manifest.version.clone(),
            needs_trust: false,
            upgraded: installed.is_some(),
        })
    }

    /// 市场安装核心流水线（下载 + 验签 + 信任确认 + 原子落位）
    #[allow(clippy::too_many_arguments)]
    async fn install_meta(
        &self,
        registry: Arc<dyn PluginRegistry>,
        source: &PluginSource,
        meta: &PluginVersionMeta,
        trust: bool,
        allow_downgrade: bool,
        allow_same_version: bool,
    ) -> Result<InstallOutcome, PluginManagerError> {
        let name = source.name.clone();

        // 预检：ABI / requires.easybot / 平台产物
        check_abi(&name, meta.sdk_version)?;
        if let Some(req) = &meta.requires
            && let Some(range) = &req.easybot
            && !check_easybot_range(range, env!("CARGO_PKG_VERSION"))
        {
            return Err(PluginManagerError::EasyBotVersionRequirement {
                name: name.clone(),
                range: range.clone(),
                current: env!("CARGO_PKG_VERSION").to_string(),
            });
        }
        let triple = current_target_triple()?;
        let artifact =
            meta.artifacts
                .get(triple)
                .ok_or_else(|| PluginManagerError::UnsupportedPlatform {
                    name: name.clone(),
                    triple: triple.to_string(),
                })?;

        // 已安装 → 同版本/降级保护
        let installed = self.read_installed_manifest(&name).await?;
        if let Some(inst) = &installed {
            if inst.version == meta.version && !allow_same_version {
                return Err(PluginManagerError::AlreadyInstalled(name.clone()));
            }
            if !allow_downgrade && is_newer_than(&inst.version, &meta.version) {
                return Err(PluginManagerError::DowngradeNotAllowed {
                    name: name.clone(),
                    installed: inst.version.clone(),
                    available: meta.version.clone(),
                });
            }
        }

        // 下载 + 验签 + 信任确认（临时目录与 plugins_dir 同文件系统，rename 原子）
        //
        // 元数据（URL/库文件名）来自不可信市场端 → 下载前先做防御性校验：
        // - `artifact.url` 只允许 https + GitHub 主机（防 SSRF 与跨源下载）
        // - `library` 只允许裸文件名（防路径穿越；下载前就拒绝，避免把文件写进
        //    staging 之外的路径）
        validate_artifact_url(&artifact.url)?;
        let library_file = artifact
            .library
            .clone()
            .unwrap_or_else(|| default_library_name(&name, triple));
        validate_library_name(&library_file)?;
        let staging = self.marketplace_tmp().await?;
        let lib_path = staging.join(&library_file);

        let mut signature: Option<PluginSignature> = None;
        if let (Some(sig_b64), Some(pk_b64)) = (
            artifact.signature.as_deref(),
            artifact.public_key.as_deref(),
        ) {
            if let Err(e) = registry.download(artifact, &lib_path).await {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(PluginManagerError::Registry(e));
            }
            let data = match std::fs::read(&lib_path) {
                Ok(d) => d,
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(PluginManagerError::Io(e));
                }
            };
            verify_artifact(&data, sig_b64, pk_b64).map_err(PluginManagerError::Signing)?;

            // 信任确认（验签通过后判定；`trust=true` 仅一次性放行，**不写入** `.trust`）
            let trusted = self.is_publisher_trusted(&source.publisher, pk_b64).await;
            if !trusted && !trust {
                let _ = std::fs::remove_dir_all(&staging);
                return Ok(InstallOutcome {
                    name,
                    publisher: source.publisher.clone(),
                    version: meta.version.clone(),
                    needs_trust: true,
                    upgraded: installed.is_some(),
                });
            }
            signature = Some(build_signature(
                source,
                meta,
                &library_file,
                sig_b64,
                pk_b64,
            ));
        } else {
            // 无签名/公钥：无法验签 → 按 allow_untrusted 决定
            if let Err(e) = registry.download(artifact, &lib_path).await {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(PluginManagerError::Registry(e));
            }
            if !self.config.read().await.allow_untrusted {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(PluginManagerError::SignatureRequired(name));
            }
            tracing::warn!(
                publisher = %source.publisher,
                name = %name,
                "installing unsigned plugin (plugins.allowUntrusted)"
            );
        }

        let manifest = synthesize_manifest(source, meta, artifact);
        let result = place_installed(
            &self.plugins_dir,
            &source.name,
            &staging,
            &manifest,
            signature.as_ref(),
            installed.is_some(),
        );
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result?;

        Ok(InstallOutcome {
            name,
            publisher: source.publisher.clone(),
            version: meta.version.clone(),
            needs_trust: false,
            upgraded: installed.is_some(),
        })
    }

    /// 发布者是否受信任：配置 `trusted_publishers`（公钥精确匹配） ∪ 用户 `.trust`
    async fn is_publisher_trusted(&self, publisher: &str, public_key_b64: &str) -> bool {
        let cfg = self.config.read().await;
        if cfg
            .trusted_publishers
            .get(publisher)
            .is_some_and(|pk| pk == public_key_b64)
        {
            return true;
        }
        drop(cfg);
        self.trust_store
            .read()
            .map(|t| t.is_trusted(publisher, public_key_b64))
            .unwrap_or(false)
    }

    /// 停止并注销插件适配器（卸载/禁用时）
    async fn stop_adapter(&self, name: &str, platform: &str) -> Result<(), PluginManagerError> {
        self.adapter_manager
            .stop(platform)
            .await
            .map_err(|e| PluginManagerError::StopFailed {
                platform: platform.to_string(),
                detail: e.to_string(),
            })?;
        self.adapter_manager.registry().unregister(platform).await;
        self.loader.unload(platform).await;
        self.platforms.write().await.remove(name);
        Ok(())
    }

    /// 创建/清空安装暂存目录（plugins 目录内隐藏目录，rename 与目标同文件系统）
    async fn marketplace_tmp(&self) -> Result<PathBuf, PluginManagerError> {
        std::fs::create_dir_all(&self.plugins_dir)?;
        let dir = self.plugins_dir.join(".marketplace");
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

/// 由配置构建注册源列表
fn build_registries(config: &PluginConfig) -> Vec<Arc<dyn PluginRegistry>> {
    let mut out: Vec<Arc<dyn PluginRegistry>> = Vec::new();
    for rc in &config.registries {
        match rc.kind.as_str() {
            "github" => {
                out.push(Arc::new(GitHubRegistry::with_catalog(&rc.owner, &rc.repo)));
            }
            other => {
                tracing::warn!(kind = other, "unsupported plugin registry kind, skipping");
            }
        }
    }
    out
}

fn channel_label(channel: PluginChannel) -> &'static str {
    match channel {
        PluginChannel::Stable => "stable",
        PluginChannel::Beta => "beta",
    }
}

/// 读取插件目录的签名状态（读文件验签，不 dlopen）
fn inspect_signature(
    path: &Path,
    manifest: &PluginManifest,
) -> (bool, Option<bool>, Option<String>) {
    let sig_path = path.join("plugin.sig.json");
    if !sig_path.exists() {
        return (false, None, None);
    }
    let sig = match PluginSignature::from_file(&sig_path) {
        Ok(s) => s,
        Err(_) => return (true, Some(false), None),
    };
    let lib_path = manifest.library_path(path).ok();
    let valid = lib_path.map(|lp| match sig.verify_library(&lp) {
        Ok(()) => true,
        Err(_) => false,
    });
    (true, valid, Some(sig.publisher))
}

/// 生产模式签名扫描：返回所有"未验签"插件 `(name, reason)`
///
/// 判定为已验证（不加入结果）当且仅当：
/// 1. 有 `plugin.yaml` + `plugin.sig.json`
/// 2. 库文件存在且验签通过
/// 3. 签名发布者的公钥受信任（配置 `trusted_publishers` 精确匹配 ∪ 用户 `.trust`）
///
/// 跳过点目录与非插件目录（松散文件如误放的 `.so` 不算插件）。缺清单/缺库/缺签名/
/// 验签失败/发布者未信任均计为未验证。供 `bin/main.rs` 生产门禁调用——dev 恒
/// lenient，不调用此函数。同步于 [`PluginManager::install`] 的验签+信任语义。
pub fn scan_unverified(
    plugins_dir: &Path,
    config: &PluginConfig,
    trust_store: &TrustStore,
) -> Vec<(String, String)> {
    let mut unverified: Vec<(String, String)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return unverified; // 目录不存在 → 无插件
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') {
            continue; // .marketplace / .trust 等
        }

        let manifest = match std::fs::read_to_string(dir.join("plugin.yaml"))
            .map_err(|_| "missing plugin.yaml".to_string())
            .and_then(|content| {
                parse_manifest_yaml(&content).map_err(|e| format!("invalid plugin.yaml: {e}"))
            }) {
            Ok(m) => m,
            Err(reason) => {
                unverified.push((name, reason));
                continue;
            }
        };
        // 禁用插件不加载 → 无需验签（否则生产启动会因故意禁用的未签名插件误拒）
        if !manifest.is_enabled() {
            continue;
        }
        let lib_path = match manifest.library_path(&dir) {
            Ok(lp) => lp,
            Err(e) => {
                unverified.push((name, format!("invalid library path: {e}")));
                continue;
            }
        };
        if !lib_path.exists() {
            unverified.push((name, format!("missing library {}", lib_path.display())));
            continue;
        }

        let sig = match PluginSignature::from_file(&dir.join("plugin.sig.json"))
            .map_err(|_| "missing or invalid plugin.sig.json".to_string())
        {
            Ok(s) => s,
            Err(reason) => {
                unverified.push((name, reason));
                continue;
            }
        };
        if let Err(e) = sig.verify_library(&lib_path) {
            unverified.push((name, format!("signature verification failed: {e}")));
            continue;
        }

        // 发布者公钥受信任（配置 `trusted_publishers` 精确匹配 ∪ 用户 `.trust`）
        let trusted = config
            .trusted_publishers
            .get(&sig.publisher)
            .is_some_and(|pk| pk == &sig.public_key)
            || trust_store.is_trusted(&sig.publisher, &sig.public_key);
        if !trusted {
            unverified.push((
                name,
                format!("publisher '{}' is not trusted", sig.publisher),
            ));
        }
    }
    unverified
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::registry::types::{
        PluginArtifact, PluginCatalog, PluginRegistryError, PluginRequirements,
    };
    use crate::plugin::signing::{
        SIGNATURE_SCHEMA_VERSION, encode_public_key, generate_keypair, sign_artifact,
    };
    use async_trait::async_trait;

    /// 内存注册表（无网络）：返回固定目录/版本/产物字节
    struct TestRegistry {
        catalog: PluginCatalog,
        versions: Vec<PluginVersionMeta>,
        artifact_bytes: Vec<u8>,
    }

    #[async_trait]
    impl PluginRegistry for TestRegistry {
        async fn catalog(&self) -> Result<PluginCatalog, PluginRegistryError> {
            Ok(self.catalog.clone())
        }
        async fn versions_for(
            &self,
            _source: &PluginSource,
            _limit: usize,
        ) -> Result<Vec<PluginVersionMeta>, PluginRegistryError> {
            Ok(self.versions.clone())
        }
        async fn download(
            &self,
            _artifact: &PluginArtifact,
            dest: &Path,
        ) -> Result<(), PluginRegistryError> {
            std::fs::write(dest, &self.artifact_bytes)?;
            Ok(())
        }
    }

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("plugin-manager-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    async fn manager_with(plugins_dir: PathBuf, registry: TestRegistry) -> Arc<PluginManager> {
        let config = Arc::new(RwLock::new(PluginConfig::default()));
        let manager = Arc::new(
            PluginManager::new(
                plugins_dir,
                config,
                Arc::new(AdapterManager::new()),
                Arc::new(EventBus::new()),
                false,
            )
            .await,
        );
        manager.set_registry_sources(vec![Arc::new(registry)]).await;
        manager
    }

    /// 构造一个已签名的插件版本元数据（产物字节 = b"plugin-bytes"）
    fn signed_meta(
        name: &str,
        version: &str,
        publisher: &str,
        trusted: bool,
    ) -> (PluginVersionMeta, TestRegistry, String) {
        let (signing, verifying) = generate_keypair();
        let pk_b64 = encode_public_key(&verifying);
        let data = b"plugin-bytes".to_vec();
        let sig = sign_artifact(&data, &signing);
        let sha = crate::updater::github::sha256_hex_bytes(&data);
        let triple = current_target_triple().unwrap().to_string();

        let source = PluginSource {
            name: name.into(),
            publisher: publisher.into(),
            owner: "EasyIndie".into(),
            repo: format!("easybot-plugin-{name}"),
            display_name: Some(name.to_string()),
            description: Some("test".into()),
            tags: vec![],
            verified: trusted,
        };
        let artifact = PluginArtifact {
            url: format!(
                "https://github.com/EasyIndie/easybot-plugin-{name}/releases/download/v{version}/lib{name}.so"
            ),
            size: data.len() as u64,
            sha256: sha.clone(),
            signature: Some(sig),
            public_key: Some(pk_b64.clone()),
            library: Some(format!("lib{name}.so")),
        };
        let mut artifacts = HashMap::new();
        artifacts.insert(triple, artifact);

        let meta = PluginVersionMeta {
            schema_version: 1,
            name: name.into(),
            version: version.into(),
            sdk_version: 1,
            publisher: publisher.into(),
            tag: format!("v{version}"),
            channel: PluginChannel::Stable,
            requires: None,
            deprecated: false,
            artifacts,
        };
        let catalog = PluginCatalog {
            schema_version: 1,
            plugins: vec![source],
        };
        let registry = TestRegistry {
            catalog,
            versions: vec![meta.clone()],
            artifact_bytes: data,
        };
        (meta, registry, pk_b64)
    }

    #[tokio::test]
    async fn test_install_requires_trust_then_succeeds() {
        let home = temp_home("install");
        let plugins = home.join("plugins");
        let (_, registry, pk_b64) = signed_meta("slack", "1.0.0", "easybot", false);
        let manager = manager_with(plugins.clone(), registry).await;

        // 未受信任发布者 → needs_trust（非错误）
        let outcome = manager
            .install(InstallRequest {
                qualified: "easybot/slack".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(outcome.needs_trust);
        assert!(
            !plugins.join("slack").exists(),
            "trust not granted → no install"
        );

        // 用户显式 trust 后安装成功
        let outcome = manager
            .install(InstallRequest {
                qualified: "easybot/slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!outcome.needs_trust);
        assert!(!outcome.upgraded);

        let dir = plugins.join("slack");
        assert!(dir.join("plugin.yaml").exists());
        assert!(dir.join("plugin.sig.json").exists());
        assert!(dir.join("libslack.so").exists());
        assert_eq!(
            std::fs::read(dir.join("libslack.so")).unwrap(),
            b"plugin-bytes"
        );

        // trust: true 是**一次性确认**，不写入 `.trust`（显式 plugin trust 才写）
        assert!(
            !manager
                .trust_store()
                .read()
                .unwrap()
                .is_trusted("easybot", &pk_b64),
            "install --yes must NOT auto-write .trust (explicit plugin trust required)"
        );

        // list_installed 展示签名有效
        let listed = manager.list_installed().await;
        assert_eq!(listed.len(), 1);
        let p = &listed[0];
        assert_eq!(p.name, "slack");
        assert!(p.signed);
        assert_eq!(p.signature_valid, Some(true));
        assert_eq!(p.publisher.as_deref(), Some("easybot"));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn test_install_downgrade_rejected_and_uninstall() {
        let home = temp_home("uninstall");
        let plugins = home.join("plugins");
        let (_, registry, _pk) = signed_meta("slack", "1.0.0", "easybot", true);
        let manager = manager_with(plugins.clone(), registry).await;
        manager
            .install(InstallRequest {
                qualified: "easybot/slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(plugins.join("slack").exists());

        // 再次安装同版本 → AlreadyInstalled
        let err = manager
            .install(InstallRequest {
                qualified: "slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(err, PluginManagerError::AlreadyInstalled(_)));

        // 降级到 0.9.0 → 拒绝
        let (_, reg_old, _) = signed_meta("slack", "0.9.0", "easybot", true);
        let manager_old = manager_with(plugins.clone(), reg_old).await;
        let err = manager_old
            .install(InstallRequest {
                qualified: "slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            PluginManagerError::DowngradeNotAllowed { .. }
        ));

        // 卸载
        manager_old.uninstall("slack").await.unwrap();
        assert!(!plugins.join("slack").exists());

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn test_update_pins_current_version_by_default() {
        let home = temp_home("update");
        let plugins = home.join("plugins");
        let (_, registry, pk_b64) = signed_meta("slack", "1.0.0", "easybot", true);
        let manager = manager_with(plugins.clone(), registry).await;
        manager
            .install(InstallRequest {
                qualified: "slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap();
        // 显式信任发布者（更新不自动写 .trust，且同密钥才通过）
        manager.trust_publisher("easybot", &pk_b64).await.unwrap();

        // 默认 pin 当前版本：同版本重拉（重建刷新），不跨版本
        let outcome = manager
            .update("slack", UpdateOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.version, "1.0.0");
        assert!(!outcome.needs_trust);

        // 注册表只有 1.0.0 时 --latest 视为已最新
        let err = manager
            .update(
                "slack",
                UpdateOptions {
                    latest: true,
                    channel: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PluginManagerError::AlreadyInstalled(_)));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn test_update_key_rotation_requires_re_trust() {
        let home = temp_home("update-key-rotation");
        let plugins = home.join("plugins");
        let (_, registry, pk_a) = signed_meta("slack", "1.0.0", "easybot", true);
        let manager = manager_with(plugins.clone(), registry).await;
        manager
            .install(InstallRequest {
                qualified: "slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap();
        // 显式信任密钥 A
        manager.trust_publisher("easybot", &pk_a).await.unwrap();

        // 发布者换了签名密钥（密钥轮换/泄露）：signed_meta 每次生成独立密钥对，
        // v2.0.0 用的是另一把密钥 B → 同发布者信任不再成立 → 更新需重新信任
        let (_, reg_v2, _pk_b) = signed_meta("slack", "2.0.0", "easybot", true);
        manager.set_registry_sources(vec![Arc::new(reg_v2)]).await;
        let outcome = manager
            .update(
                "slack",
                UpdateOptions {
                    latest: true,
                    channel: None,
                },
            )
            .await
            .unwrap();
        assert!(
            outcome.needs_trust,
            "changed signing key must require explicit re-trust"
        );
        // needs_trust 在落位前返回 → 已装的 v1.0.0 目录保持原样（未被 v2 覆盖）
        assert_eq!(
            manager
                .read_installed_manifest("slack")
                .await
                .unwrap()
                .unwrap()
                .version,
            "1.0.0"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn test_set_enabled_and_requires_easybot_gate() {
        let home = temp_home("enabled");
        let plugins = home.join("plugins");
        let (mut meta, registry, _pk) = signed_meta("slack", "1.0.0", "easybot", true);
        // requires 不满足 → 拒绝安装
        meta.requires = Some(PluginRequirements {
            easybot: Some(">=99.0.0".into()),
        });
        let manager = manager_with(
            plugins.clone(),
            TestRegistry {
                catalog: registry.catalog,
                versions: vec![meta],
                artifact_bytes: registry.artifact_bytes,
            },
        )
        .await;
        let err = manager
            .install(InstallRequest {
                qualified: "slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            PluginManagerError::EasyBotVersionRequirement { .. }
        ));

        // 满足 requires 后安装，然后禁用/启用
        let (_, registry, _) = signed_meta("slack", "1.0.0", "easybot", true);
        let manager = manager_with(plugins.clone(), registry).await;
        manager
            .install(InstallRequest {
                qualified: "slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap();

        manager.set_enabled("slack", false).await.unwrap();
        let listed = manager.list_installed().await;
        assert!(!listed[0].enabled);

        manager.set_enabled("slack", true).await.unwrap();
        let listed = manager.list_installed().await;
        assert!(listed[0].enabled);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn test_scan_unverified_detects_unsigned_tampered_and_untrusted() {
        let home = temp_home("scan");
        let plugins = home.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();

        // 空目录 + 松散文件 + 点目录 → 不报告
        std::fs::write(plugins.join("stray.so"), b"junk").unwrap();
        std::fs::create_dir_all(plugins.join(".marketplace")).unwrap();
        let empty = scan_unverified(&plugins, &PluginConfig::default(), &TrustStore::default());
        assert!(empty.is_empty(), "{empty:?}");

        // 已签名 + 配置 trusted_publishers 受信任 → 通过
        let (signing, verifying) = generate_keypair();
        let pk = encode_public_key(&verifying);
        let mut cfg = PluginConfig::default();
        cfg.trusted_publishers.insert("pub-a".into(), pk.clone());
        let trusted_dir = plugins.join("trusted-plugin");
        std::fs::create_dir_all(&trusted_dir).unwrap();
        std::fs::write(
            trusted_dir.join("plugin.yaml"),
            "name: trusted-plugin\nsdk_version: 1\nlibrary: libtrusted.so\n",
        )
        .unwrap();
        std::fs::write(trusted_dir.join("libtrusted.so"), b"plugin-bytes").unwrap();
        let sig = PluginSignature {
            schema_version: SIGNATURE_SCHEMA_VERSION,
            name: "trusted-plugin".into(),
            version: "1.0.0".into(),
            publisher: "pub-a".into(),
            artifact: "libtrusted.so".into(),
            signature: sign_artifact(b"plugin-bytes", &signing),
            public_key: pk.clone(),
        };
        sig.write_to(&trusted_dir.join("plugin.sig.json")).unwrap();
        assert!(scan_unverified(&plugins, &cfg, &TrustStore::default()).is_empty());

        // 未签名插件 → 报告缺签名
        let unsigned_dir = plugins.join("unsigned-plugin");
        std::fs::create_dir_all(&unsigned_dir).unwrap();
        std::fs::write(
            unsigned_dir.join("plugin.yaml"),
            "name: unsigned-plugin\nsdk_version: 1\n",
        )
        .unwrap();
        let out = scan_unverified(&plugins, &cfg, &TrustStore::default());
        let (name, reason) = out.iter().find(|(n, _)| n == "unsigned-plugin").unwrap();
        assert_eq!(name, "unsigned-plugin");
        assert!(reason.contains("sig"), "{reason}");

        // 已签名但发布者未受信任 → 报告（用独立密钥对，签名本身有效）
        let (other_signing, other_verifying) = generate_keypair();
        let untrusted_dir = plugins.join("untrusted-plugin");
        std::fs::create_dir_all(&untrusted_dir).unwrap();
        std::fs::write(
            untrusted_dir.join("plugin.yaml"),
            "name: untrusted-plugin\nsdk_version: 1\nlibrary: libu.so\n",
        )
        .unwrap();
        std::fs::write(untrusted_dir.join("libu.so"), b"plugin-bytes").unwrap();
        let sig = PluginSignature {
            schema_version: SIGNATURE_SCHEMA_VERSION,
            name: "untrusted-plugin".into(),
            version: "1.0.0".into(),
            publisher: "pub-b".into(),
            artifact: "libu.so".into(),
            signature: sign_artifact(b"plugin-bytes", &other_signing),
            public_key: encode_public_key(&other_verifying),
        };
        sig.write_to(&untrusted_dir.join("plugin.sig.json"))
            .unwrap();
        let out = scan_unverified(&plugins, &cfg, &TrustStore::default());
        let (_, reason) = out.iter().find(|(n, _)| n == "untrusted-plugin").unwrap();
        assert!(reason.contains("not trusted"), "{reason}");

        // 篡改库文件 → 验签失败
        std::fs::write(untrusted_dir.join("libu.so"), b"tampered-bytes").unwrap();
        let out = scan_unverified(&plugins, &cfg, &TrustStore::default());
        let (_, reason) = out.iter().find(|(n, _)| n == "untrusted-plugin").unwrap();
        assert!(reason.contains("verification failed"), "{reason}");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn test_scan_unverified_skips_disabled_plugins() {
        let home = temp_home("scan-disabled");
        let plugins = home.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();

        // 故意禁用的未签名插件 → 不报告（不加载 → 无需验签）
        let disabled_dir = plugins.join("disabled-plugin");
        std::fs::create_dir_all(&disabled_dir).unwrap();
        std::fs::write(
            disabled_dir.join("plugin.yaml"),
            "name: disabled-plugin\nsdk_version: 1\nenabled: false\n",
        )
        .unwrap();

        let out = scan_unverified(&plugins, &PluginConfig::default(), &TrustStore::default());
        assert!(out.is_empty(), "{out:?}");

        // 同目录启用 → 缺签名被报告
        std::fs::write(
            disabled_dir.join("plugin.yaml"),
            "name: disabled-plugin\nsdk_version: 1\n",
        )
        .unwrap();
        let out = scan_unverified(&plugins, &PluginConfig::default(), &TrustStore::default());
        assert!(out.iter().any(|(n, _)| n == "disabled-plugin"), "{out:?}");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn test_explicit_trust_writes_trust_store() {
        let home = temp_home("trust-explicit");
        let plugins = home.join("plugins");
        let (_, registry, pk_b64) = signed_meta("slack", "1.0.0", "easybot", false);
        let manager = manager_with(plugins.clone(), registry).await;

        // 安装不写 .trust
        manager
            .install(InstallRequest {
                qualified: "easybot/slack".into(),
                trust: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            !manager
                .trust_store()
                .read()
                .unwrap()
                .is_trusted("easybot", &pk_b64)
        );

        // 显式 plugin trust → 写入 .trust 且落盘
        manager.trust_publisher("easybot", &pk_b64).await.unwrap();
        assert!(
            manager
                .trust_store()
                .read()
                .unwrap()
                .is_trusted("easybot", &pk_b64)
        );
        let on_disk = TrustStore::load(&plugins.join(".trust"));
        assert!(on_disk.is_trusted("easybot", &pk_b64));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn test_install_from_file_validates_library_and_requires() {
        let home = temp_home("install-file");
        let plugins = home.join("plugins");
        let (_, registry, _) = signed_meta("slack", "1.0.0", "easybot", true);
        let manager = manager_with(plugins.clone(), registry).await;

        let src = home.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("libslack.so"), b"plugin-bytes").unwrap();
        std::fs::write(
            src.join("plugin.yaml"),
            "name: slack\nsdk_version: 1\nlibrary: libslack.so\nrequires:\n  easybot: \">=99.0.0\"\n",
        )
        .unwrap();

        // requires 不满足 → 拒绝
        let err = manager
            .install(InstallRequest {
                qualified: "slack".into(),
                file: Some(src.clone()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            PluginManagerError::EasyBotVersionRequirement { .. }
        ));

        // library 路径穿越 → 拒绝
        std::fs::write(
            src.join("plugin.yaml"),
            "name: slack\nsdk_version: 1\nlibrary: ../../etc/passwd\n",
        )
        .unwrap();
        let err = manager
            .install(InstallRequest {
                qualified: "slack".into(),
                file: Some(src.clone()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, PluginManagerError::InvalidLibrary(_)),
            "{err:?}"
        );

        // 合法输入（签名 + 已信任发布者）→ 安装成功
        let (signing, verifying) = generate_keypair();
        let pk_b64 = encode_public_key(&verifying);
        let sig = PluginSignature {
            schema_version: SIGNATURE_SCHEMA_VERSION,
            name: "slack".into(),
            version: "0.1.0".into(),
            publisher: "easybot".into(),
            artifact: "libslack.so".into(),
            signature: sign_artifact(b"plugin-bytes", &signing),
            public_key: pk_b64.clone(),
        };
        sig.write_to(&src.join("plugin.sig.json")).unwrap();
        manager.trust_publisher("easybot", &pk_b64).await.unwrap();
        std::fs::write(
            src.join("plugin.yaml"),
            "name: slack\nsdk_version: 1\nlibrary: libslack.so\n",
        )
        .unwrap();
        let outcome = manager
            .install(InstallRequest {
                qualified: "slack".into(),
                file: Some(src),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!outcome.needs_trust);
        assert!(plugins.join("slack").join("libslack.so").exists());

        let _ = std::fs::remove_dir_all(&home);
    }
}
