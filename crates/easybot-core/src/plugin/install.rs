//! 插件安装流水线（纯逻辑部分）
//!
//! 编排（异步下载/信任确认/互斥）在 [`super::manager::PluginManager`]，
//! 本文件提供可独立测试的构建块：
//!
//! - [`validate_name`]：插件名白名单（市场元数据不可信）
//! - [`resolve_source`]：合并多注册源目录，按 `publisher/name` 解析
//! - [`pick_version`]：按渠道选择最新版本
//! - [`synthesize_manifest`]：由 `PluginVersionMeta` 合成 `plugin.yaml`
//! - [`build_signature`]：组装 `plugin.sig.json` 内容
//! - [`place_installed`]：原子落位（校验过的临时目录改名进 plugins 目录）

use super::error::PluginManagerError;
use super::loader::EASYBOT_PLUGIN_ABI_VERSION;
use super::manifest::PluginManifest;
use super::registry::PluginRegistry;
use super::registry::types::{
    PluginArtifact, PluginChannel, PluginSource, PluginVersionMeta, parse_qualified_name,
};
use super::signing::{PluginSignature, SIGNATURE_SCHEMA_VERSION};
use std::path::Path;

/// 插件名白名单：字母数字下划线连字符（拒绝 `..` / 分隔符 / 绝对路径）
///
/// 市场元数据来自不可信网络来源，落位前必须校验——防止
/// `plugins/{name}` 目录穿越或覆盖宿主目录。
pub fn validate_name(name: &str) -> Result<(), PluginManagerError> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(PluginManagerError::InvalidName(name.to_string()))
    }
}

/// 解析 `publisher/name` 限定名，返回 `(publisher: Option, name)`
///
/// 无 `/` 时按裸名在全部注册源中查找（名称唯一）。
pub fn split_qualified(input: &str) -> (Option<String>, String) {
    match parse_qualified_name(input) {
        Some((publisher, name)) => (Some(publisher), name),
        None => (None, input.trim().to_string()),
    }
}

/// 在多个注册源目录中解析插件源码条目（Homebrew Taps 模型）
///
/// 按注册源顺序查找：任一源命中即返回该源与条目；全部失败时返回
/// 最后一个目录错误（网络不可达优先于 NotFound，便于诊断）。
pub async fn resolve_source(
    registries: &[std::sync::Arc<dyn PluginRegistry>],
    publisher: Option<&str>,
    name: &str,
) -> Result<(std::sync::Arc<dyn PluginRegistry>, PluginSource), PluginManagerError> {
    let mut last_err: Option<PluginManagerError> = None;
    for registry in registries {
        match registry.catalog().await {
            Ok(catalog) => {
                if let Some(source) = catalog.find(publisher, name) {
                    return Ok((registry.clone(), source.clone()));
                }
            }
            Err(e) => last_err = Some(PluginManagerError::Registry(e)),
        }
    }
    Err(last_err.unwrap_or_else(|| PluginManagerError::NotFound(name.to_string())))
}

/// 从版本列表选择指定渠道的最新版本（版本号语义化比较）
pub fn pick_version(
    versions: &[PluginVersionMeta],
    channel: PluginChannel,
) -> Option<&PluginVersionMeta> {
    versions
        .iter()
        .filter(|v| v.channel == channel)
        .max_by(|a, b| {
            semver::Version::parse(&a.version)
                .ok()
                .cmp(&semver::Version::parse(&b.version).ok())
        })
}

/// 由 `PluginVersionMeta` 合成插件清单（落位用）
pub fn synthesize_manifest(
    source: &PluginSource,
    meta: &PluginVersionMeta,
    artifact: &PluginArtifact,
) -> PluginManifest {
    PluginManifest {
        name: source.name.clone(),
        display_name: source.display_name.clone(),
        description: source.description.clone(),
        version: meta.version.clone(),
        sdk_version: meta.sdk_version,
        author: Some(source.publisher.clone()),
        library: artifact.library.clone(),
        enabled: Some(true),
    }
}

/// 组装 `plugin.sig.json` 内容
///
/// `signature`/`public_key` 来自 `PluginArtifact`（随 `easybot-plugin.json`
/// 经 HTTPS 分发）；落位后加载器对磁盘库文件重新验签。
pub fn build_signature(
    source: &PluginSource,
    meta: &PluginVersionMeta,
    library: &str,
    signature: &str,
    public_key: &str,
) -> PluginSignature {
    PluginSignature {
        schema_version: SIGNATURE_SCHEMA_VERSION,
        name: source.name.clone(),
        version: meta.version.clone(),
        publisher: source.publisher.clone(),
        artifact: library.to_string(),
        signature: signature.to_string(),
        public_key: public_key.to_string(),
    }
}

/// 原子落位：把已校验的临时目录改名为 `plugins_dir/{name}`
///
/// 在 staging 目录内写入 `plugin.yaml`（+ `plugin.sig.json`）后整体 `rename`——
/// 同文件系统内 rename 是原子的。`replace=false` 时目标已存在直接失败（安装防覆盖）；
/// `replace=true`（更新/刷新）时先移除旧目录再落位（下载+验签已完成，rename 本地
/// 操作极难失败；v1 无回滚，坏更新由 `plugin update` 手动重装修复）。
/// 失败方由调用方清理 staging。
pub fn place_installed(
    plugins_dir: &Path,
    name: &str,
    staging: &Path,
    manifest: &PluginManifest,
    signature: Option<&PluginSignature>,
    replace: bool,
) -> Result<(), PluginManagerError> {
    validate_name(name)?;

    let target = plugins_dir.join(name);
    if target.exists() {
        if !replace {
            return Err(PluginManagerError::AlreadyInstalled(name.to_string()));
        }
        std::fs::remove_dir_all(&target)?;
    }
    if !staging.is_dir() {
        return Err(PluginManagerError::Other(format!(
            "staging dir missing: {}",
            staging.display()
        )));
    }

    // 写入清单与签名
    let yaml = serde_yaml::to_string(manifest)?;
    std::fs::write(staging.join("plugin.yaml"), yaml)?;
    if let Some(sig) = signature {
        sig.write_to(&staging.join("plugin.sig.json"))?;
    }

    // 原子改名进目标目录
    std::fs::rename(staging, &target)?;
    Ok(())
}

/// 按平台规则推断缺省动态库文件名
///
/// 与 `manifest.library_path()` 一致：cargo cdylib 产物用下划线 crate 名
/// （kebab-case 包名 → 下划线），推导名必须对齐，否则手动安装 / `--file`
/// 落位的库文件找不到。
pub fn default_library_name(name: &str, triple: &str) -> String {
    let crate_name = name.replace('-', "_");
    if triple.contains("windows") {
        format!("{crate_name}.dll")
    } else if triple.contains("apple") {
        format!("lib{crate_name}.dylib")
    } else {
        format!("lib{crate_name}.so")
    }
}

/// ABI 兼容预检（与 SDK 常量一致）
pub fn check_abi(name: &str, sdk_version: u32) -> Result<(), PluginManagerError> {
    if sdk_version == EASYBOT_PLUGIN_ABI_VERSION {
        Ok(())
    } else {
        Err(PluginManagerError::AbiMismatch {
            name: name.to_string(),
            expected: EASYBOT_PLUGIN_ABI_VERSION,
            got: sdk_version,
        })
    }
}

/// 插件清单反序列化（统一包装 YAML 错误）
pub fn parse_manifest_yaml(content: &str) -> Result<PluginManifest, PluginManagerError> {
    serde_yaml::from_str(content).map_err(PluginManagerError::Yaml)
}

/// 宿主 EasyBot 版本是否满足插件的 semver range 要求
pub fn check_easybot_range(range: &str, current: &str) -> bool {
    semver::VersionReq::parse(range)
        .map(|req| {
            semver::Version::parse(current)
                .map(|v| req.matches(&v))
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_name_accepts_valid() {
        assert!(validate_name("my-plugin").is_ok());
        assert!(validate_name("My_Plugin_2").is_ok());
    }

    #[test]
    fn test_validate_name_rejects_traversal() {
        assert!(validate_name("..").is_err());
        assert!(validate_name("../evil").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\\b").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("a b").is_err());
    }

    #[test]
    fn test_split_qualified() {
        assert_eq!(
            split_qualified("easybot/slack"),
            (Some("easybot".into()), "slack".into())
        );
        assert_eq!(split_qualified("slack"), (None, "slack".into()));
        assert_eq!(split_qualified("  slack  "), (None, "slack".into()));
    }

    #[test]
    fn test_pick_version_prefers_channel_and_newest() {
        fn meta(version: &str, channel: PluginChannel) -> PluginVersionMeta {
            PluginVersionMeta {
                schema_version: 1,
                name: "p".into(),
                version: version.into(),
                sdk_version: 1,
                publisher: "easybot".into(),
                tag: format!("v{version}"),
                channel,
                requires: None,
                deprecated: false,
                artifacts: Default::default(),
            }
        }
        let versions = vec![
            meta("1.0.0", PluginChannel::Stable),
            meta("1.2.0", PluginChannel::Beta),
            meta("1.1.0", PluginChannel::Stable),
            meta("2.0.0", PluginChannel::Beta),
        ];
        let stable = pick_version(&versions, PluginChannel::Stable).unwrap();
        assert_eq!(stable.version, "1.1.0");
        let beta = pick_version(&versions, PluginChannel::Beta).unwrap();
        assert_eq!(beta.version, "2.0.0");
    }

    #[test]
    fn test_check_easybot_range() {
        assert!(check_easybot_range(">=0.0.28", "0.0.33"));
        // caret 语义（Cargo）：^0.0.28 = >=0.0.28, <0.0.29，0.0.x 不允许补丁级提升
        assert!(check_easybot_range("^0.0.28", "0.0.28"));
        assert!(!check_easybot_range("^0.0.28", "0.0.33"));
        assert!(!check_easybot_range(">=0.1.0", "0.0.33"));
        assert!(!check_easybot_range("garbage", "0.0.33"));
    }

    #[test]
    fn test_place_installed_writes_manifest_and_renames() {
        let root = std::env::temp_dir().join(format!("install-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plugins = root.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("libx.so"), b"x").unwrap();

        let manifest = PluginManifest {
            name: "demo".into(),
            display_name: Some("Demo".into()),
            description: None,
            version: "1.0.0".into(),
            sdk_version: 1,
            author: Some("easybot".into()),
            library: Some("libx.so".into()),
            enabled: Some(true),
        };
        place_installed(&plugins, "demo", &staging, &manifest, None, false).unwrap();

        let installed_dir = plugins.join("demo");
        assert!(installed_dir.join("libx.so").exists());
        let yaml = std::fs::read_to_string(installed_dir.join("plugin.yaml")).unwrap();
        let parsed = PluginManifest::from_yaml(&yaml).unwrap();
        assert_eq!(parsed.name, "demo");
        assert_eq!(parsed.version, "1.0.0");
        assert_eq!(parsed.sdk_version, 1);
        assert!(parsed.is_enabled());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_place_installed_rejects_existing_target() {
        let root = std::env::temp_dir().join(format!("install-test-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let plugins = root.join("plugins");
        std::fs::create_dir_all(plugins.join("demo")).unwrap();
        let staging = root.join("staging");
        std::fs::create_dir_all(&staging).unwrap();

        let manifest = PluginManifest {
            name: "demo".into(),
            display_name: None,
            description: None,
            version: "1.0.0".into(),
            sdk_version: 1,
            author: None,
            library: None,
            enabled: None,
        };
        let err = place_installed(&plugins, "demo", &staging, &manifest, None, false).unwrap_err();
        assert!(matches!(err, PluginManagerError::AlreadyInstalled(n) if n == "demo"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_default_library_name_kebab_to_underscore() {
        // cargo cdylib 产物用下划线 crate 名（kebab-case 包名转下划线），
        // 推导名必须对齐，否则手动安装 / `--file` 落位的库文件找不到。
        assert_eq!(
            default_library_name("hello-adapter", "x86_64-apple-darwin"),
            "libhello_adapter.dylib"
        );
        assert_eq!(
            default_library_name("hello-adapter", "x86_64-unknown-linux-musl"),
            "libhello_adapter.so"
        );
        assert_eq!(
            default_library_name("hello-adapter", "x86_64-pc-windows-msvc"),
            "hello_adapter.dll"
        );
        // 无连字符的插件名不受影响
        assert_eq!(
            default_library_name("myplugin", "x86_64-apple-darwin"),
            "libmyplugin.dylib"
        );
    }
}
