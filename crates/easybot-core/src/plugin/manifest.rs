//! 插件清单
//!
//! 每个插件目录下包含一个 plugin.yaml 清单文件，描述插件元数据和库路径。
//! 加载器通过清单定位入口可执行文件（`command`；旧式 `library` 作为兼容别名）。

use super::registry::types::PluginRequirements;
use std::path::Path;

/// 插件清单（plugin.yaml）
///
/// `Serialize` 用于市场安装时由 `PluginVersionMeta` 合成清单落位
/// （字段保持与 `Deserialize` 相同的 key，保证回读一致）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PluginManifest {
    /// 平台标识符，如 "my-custom-im"
    pub name: String,
    /// 人类可读的显示名称
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 功能描述
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 插件版本
    #[serde(default = "default_version")]
    pub version: String,
    /// 所需 easybot-plugin-sdk ABI 版本（必填）
    pub sdk_version: u32,
    /// 作者信息
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// 动态库路径（相对于插件目录）。**deprecated**：进程外插件改用 `command`。
    /// 不指定时按平台规则推断：lib{name}.so / lib{name}.dylib / {name}.dll
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    /// 进程外插件协议版本（`command` 形态使用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    /// 插件可执行文件路径（相对于插件目录）。进程外插件的入口；
    /// 未指定时回退 `library`（旧式 cdylib）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// 运行时类型：`process`（默认）| `wasm`（预留）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// 是否启用。缺省启用（向后兼容：旧清单无此字段默认 true）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// 宿主 EasyBot 版本兼容范围（semver range，如 `>=0.0.28`）。
    /// 市场安装由 `easybot-plugin.json` 的 `requires` 校验；`--file` 离线安装读此字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<PluginRequirements>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// 安全拼接相对路径：拒绝绝对路径与 `..` 穿越，返回 `plugin_dir.join(rel)`。
fn safe_join(plugin_dir: &Path, rel: &str, what: &str) -> Result<std::path::PathBuf, String> {
    if Path::new(rel).is_absolute() {
        return Err(format!("插件 {what} 路径不允许使用绝对路径: {rel}"));
    }
    if Path::new(rel)
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(format!("插件 {what} 路径包含非法 '..' 组件: {rel}"));
    }
    Ok(plugin_dir.join(rel))
}

impl PluginManifest {
    /// 解析 YAML 字符串为清单
    pub fn from_yaml(yaml: &str) -> Result<Self, String> {
        serde_yaml::from_str(yaml).map_err(|e| format!("Failed to parse plugin manifest: {}", e))
    }

    /// 计算旧式 `library` 字段指向的完整路径（兼容别名；新插件用 `command_path`）
    ///
    /// 安全检查：拒绝绝对路径和含 `..` 的路径穿越。
    pub fn library_path(&self, plugin_dir: &Path) -> Result<std::path::PathBuf, String> {
        let lib = match self.library {
            Some(ref lib) => lib.clone(),
            None => {
                // 按平台规则推断旧式 cdylib 的默认库文件名。
                //
                // cargo 对 cdylib 的输出用 **下划线** crate 名（kebab-case 包名会
                // 转下划线）：包 `hello-adapter` → `libhello_adapter.dylib`。
                // 推导必须同样转下划线，否则 `cp target/release/libhello_adapter.*`
                // 手动安装 / `install --file` 的库文件与推导名不匹配、加载找不到。
                let crate_name = self.name.replace('-', "_");
                let lib_name = format!("lib{}", crate_name);
                if cfg!(target_os = "linux") {
                    format!("{}.so", lib_name)
                } else if cfg!(target_os = "macos") {
                    format!("{}.dylib", lib_name)
                } else if cfg!(target_os = "windows") {
                    format!("{}.dll", crate_name)
                } else {
                    format!("{}.so", lib_name)
                }
            }
        };

        // 安全检查：绝对路径 / `..` 穿越（与 `command_path` 共用 `safe_join`）
        safe_join(plugin_dir, &lib, "library")
    }

    /// 计算插件可执行文件路径（进程外插件入口）。
    ///
    /// 优先级：
    /// 1. `command`（规范字段）
    /// 2. `library`（旧式 cdylib 别名，加载期会被 `is_legacy_cdylib()` 拒绝）
    /// 3. 缺省 `{name}`（Windows `{name}.exe`）——与安装落位名
    ///    （`install::default_command_name`）保持一致
    ///
    /// 安全校验：拒绝绝对路径与 `..` 穿越。
    pub fn command_path(&self, plugin_dir: &Path) -> Result<std::path::PathBuf, String> {
        if let Some(ref cmd) = self.command {
            return safe_join(plugin_dir, cmd, "command");
        }
        if self.library.is_some() {
            return self.library_path(plugin_dir);
        }
        let default = if cfg!(target_os = "windows") {
            format!("{}.exe", self.name)
        } else {
            self.name.clone()
        };
        safe_join(plugin_dir, &default, "command")
    }

    /// 是否为进程外（可执行文件）插件（`runtime` 缺省为 `process`）。
    pub fn is_process_plugin(&self) -> bool {
        self.runtime.as_deref().unwrap_or("process") == "process"
    }

    /// 旧式 cdylib 插件（仅有 `library`、无 `command`）。
    pub fn is_legacy_cdylib(&self) -> bool {
        self.command.is_none() && self.library.is_some()
    }

    /// 插件是否启用（缺省启用）
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_parse_manifest() {
        let yaml = r#"
name: "slack"
display_name: "Slack Plugin"
description: "Slack integration via plugin system"
version: "1.0.0"
sdk_version: 1
author: "EasyBot Contributors"
"#;
        let manifest = PluginManifest::from_yaml(yaml).unwrap();
        assert_eq!(manifest.name, "slack");
        assert_eq!(manifest.display_name.unwrap(), "Slack Plugin");
        assert_eq!(manifest.sdk_version, 1);
    }

    #[test]
    fn test_manifest_minimal() {
        let yaml = "name: \"test-plugin\"\nsdk_version: 1";
        let manifest = PluginManifest::from_yaml(yaml).unwrap();
        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.version, "0.1.0");
        assert!(manifest.library.is_none());
    }

    #[test]
    fn test_default_library_path_kebab_to_underscore() {
        let manifest = PluginManifest {
            name: "my-adapter".into(),
            display_name: None,
            description: None,
            version: "1.0.0".into(),
            sdk_version: 1,
            author: None,
            library: None,
            enabled: None,
            requires: None,
            protocol: None,
            command: None,
            runtime: None,
        };
        let dir = Path::new("/plugins/my-adapter");
        let path = manifest.library_path(dir).unwrap();
        // cargo cdylib 产物用下划线 crate 名（kebab 包名 → 下划线），推导名必须一致，
        // 否则手动安装 / `--file` 落位的库文件找不到。
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(
            filename.starts_with("lib"),
            "filename should start with 'lib', got: {}",
            filename
        );
        assert!(
            filename.contains("my_adapter"),
            "filename should use underscore crate name (cargo convention), got: {}",
            filename
        );
        assert!(
            !filename.contains("my-adapter"),
            "filename must NOT contain kebab-case name, got: {}",
            filename
        );
    }

    #[test]
    fn test_custom_library_path() {
        let manifest = PluginManifest {
            name: "my-adapter".into(),
            display_name: None,
            description: None,
            version: "1.0.0".into(),
            sdk_version: 1,
            author: None,
            library: Some("custom.so".into()),
            enabled: None,
            requires: None,
            protocol: None,
            command: None,
            runtime: None,
        };
        let dir = Path::new("/plugins/my-adapter");
        let path = manifest.library_path(dir).unwrap();
        assert_eq!(path, Path::new("/plugins/my-adapter/custom.so"));
    }

    #[test]
    fn test_library_path_rejects_absolute() {
        let manifest = PluginManifest {
            name: "my-adapter".into(),
            display_name: None,
            description: None,
            version: "1.0.0".into(),
            sdk_version: 1,
            author: None,
            library: Some("/usr/lib/libc.so.6".into()),
            enabled: None,
            requires: None,
            protocol: None,
            command: None,
            runtime: None,
        };
        let dir = Path::new("/plugins/my-adapter");
        assert!(manifest.library_path(dir).is_err());
    }

    #[test]
    fn test_library_path_rejects_parent_dir_traversal() {
        let manifest = PluginManifest {
            name: "my-adapter".into(),
            display_name: None,
            description: None,
            version: "1.0.0".into(),
            sdk_version: 1,
            author: None,
            library: Some("../../../usr/lib/libc.so.6".into()),
            enabled: None,
            requires: None,
            protocol: None,
            command: None,
            runtime: None,
        };
        let dir = Path::new("/plugins/my-adapter");
        let result = manifest.library_path(dir);
        assert!(
            result.is_err(),
            "should reject .. traversal, got: {:?}",
            result
        );
    }

    #[test]
    fn test_invalid_yaml() {
        let result = PluginManifest::from_yaml("invalid: [yaml: broken");
        assert!(result.is_err());
    }

    #[test]
    fn test_enabled_defaults_to_true() {
        // 无 enabled 字段 → 启用（向后兼容）
        let manifest = PluginManifest::from_yaml("name: \"a\"\nsdk_version: 1").unwrap();
        assert!(manifest.is_enabled());

        // enabled: false → 禁用
        let manifest =
            PluginManifest::from_yaml("name: \"a\"\nsdk_version: 1\nenabled: false").unwrap();
        assert!(!manifest.is_enabled());
    }

    #[test]
    fn test_command_path_prefers_command_field() {
        let manifest = PluginManifest::from_yaml(
            "name: \"a\"\nsdk_version: 1\ncommand: \"./a-adapter\"\nlibrary: \"liba.so\"",
        )
        .unwrap();
        let path = manifest.command_path(Path::new("/plugins/a")).unwrap();
        assert!(path.ends_with("a-adapter"), "应优先 command: {path:?}");
        assert!(!manifest.is_legacy_cdylib());
    }

    #[test]
    fn test_command_path_falls_back_to_library() {
        let manifest =
            PluginManifest::from_yaml("name: \"a\"\nsdk_version: 1\nlibrary: \"liba.so\"").unwrap();
        let path = manifest.command_path(Path::new("/plugins/a")).unwrap();
        assert!(
            path.ends_with("liba.so"),
            "无 command 时回退 library: {path:?}"
        );
        assert!(manifest.is_legacy_cdylib());
    }

    #[test]
    fn test_command_path_defaults_to_plugin_name() {
        // 既无 command 也无 library：缺省入口名 = 插件名（Windows 加 .exe），
        // 必须与安装落位名（install::default_command_name）一致。
        let manifest = PluginManifest::from_yaml("name: \"my-plugin\"\nsdk_version: 1").unwrap();
        let path = manifest
            .command_path(Path::new("/plugins/my-plugin"))
            .unwrap();
        let expected = if cfg!(target_os = "windows") {
            "my-plugin.exe"
        } else {
            "my-plugin"
        };
        assert!(path.ends_with(expected), "缺省入口名: {path:?}");
        assert!(!manifest.is_legacy_cdylib());
        assert!(manifest.is_process_plugin());
    }

    #[test]
    fn test_command_path_rejects_absolute_and_traversal() {
        let abs =
            PluginManifest::from_yaml("name: \"a\"\nsdk_version: 1\ncommand: \"/usr/bin/evil\"")
                .unwrap();
        assert!(abs.command_path(Path::new("/plugins/a")).is_err());

        let traversal =
            PluginManifest::from_yaml("name: \"a\"\nsdk_version: 1\ncommand: \"../../evil\"")
                .unwrap();
        assert!(traversal.command_path(Path::new("/plugins/a")).is_err());
    }

    #[test]
    fn test_runtime_defaults_to_process() {
        let manifest = PluginManifest::from_yaml("name: \"a\"\nsdk_version: 1").unwrap();
        assert!(manifest.is_process_plugin());

        let wasm = PluginManifest::from_yaml("name: \"a\"\nsdk_version: 1\nruntime: wasm").unwrap();
        assert!(!wasm.is_process_plugin());
    }
}
