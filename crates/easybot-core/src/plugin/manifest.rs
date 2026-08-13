//! 插件清单
//!
//! 每个插件目录下包含一个 plugin.yaml 清单文件，描述插件元数据和库路径。
//! 加载器通过清单定位动态库文件。

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
    /// 动态库路径（相对于插件目录）。
    /// 不指定时按平台规则推断：lib{name}.so / lib{name}.dylib / {name}.dll
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    /// 是否启用。缺省启用（向后兼容：旧清单无此字段默认 true）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

impl PluginManifest {
    /// 解析 YAML 字符串为清单
    pub fn from_yaml(yaml: &str) -> Result<Self, String> {
        serde_yaml::from_str(yaml).map_err(|e| format!("Failed to parse plugin manifest: {}", e))
    }

    /// 计算动态库的完整路径
    ///
    /// 安全检查：拒绝绝对路径和含 `..` 的路径穿越。
    pub fn library_path(&self, plugin_dir: &Path) -> Result<std::path::PathBuf, String> {
        let lib = match self.library {
            Some(ref lib) => lib.clone(),
            None => {
                // 按平台规则推断默认库文件名。
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

        // 安全检查：绝对路径可绕过插件目录
        if Path::new(&lib).is_absolute() {
            return Err(format!("插件 library 路径不允许使用绝对路径: {}", lib));
        }

        // 安全检查：拒绝含 .. 的目录穿越
        if Path::new(&lib)
            .components()
            .any(|c| c == std::path::Component::ParentDir)
        {
            return Err(format!("插件 library 路径包含非法 '..' 组件: {}", lib));
        }

        Ok(plugin_dir.join(&lib))
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
}
