//! 插件清单
//!
//! 每个插件目录下包含一个 plugin.yaml 清单文件，描述插件元数据和库路径。
//! 加载器通过清单定位动态库文件。

use std::path::Path;

/// 插件清单（plugin.yaml）
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginManifest {
    /// 平台标识符，如 "my-custom-im"
    pub name: String,
    /// 人类可读的显示名称
    #[serde(default)]
    pub display_name: Option<String>,
    /// 功能描述
    #[serde(default)]
    pub description: Option<String>,
    /// 插件版本
    #[serde(default = "default_version")]
    pub version: String,
    /// 所需 easybot-plugin-sdk ABI 版本（可选，默认当前版本）
    #[serde(default)]
    pub sdk_version: Option<u32>,
    /// 作者信息
    #[serde(default)]
    pub author: Option<String>,
    /// 动态库路径（相对于插件目录）。
    /// 不指定时按平台规则推断：lib{name}.so / lib{name}.dylib / {name}.dll
    #[serde(default)]
    pub library: Option<String>,
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
    pub fn library_path(&self, plugin_dir: &Path) -> std::path::PathBuf {
        if let Some(ref lib) = self.library {
            plugin_dir.join(lib)
        } else {
            // 按平台规则推断默认库文件名
            let lib_name = format!("lib{}", self.name);
            #[cfg(target_os = "linux")]
            {
                plugin_dir.join(format!("{}.so", lib_name))
            }
            #[cfg(target_os = "macos")]
            {
                plugin_dir.join(format!("{}.dylib", lib_name))
            }
            #[cfg(target_os = "windows")]
            {
                plugin_dir.join(format!("{}.dll", self.name))
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            {
                plugin_dir.join(format!("{}.so", lib_name))
            }
        }
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
        assert_eq!(manifest.sdk_version, Some(1));
    }

    #[test]
    fn test_manifest_minimal() {
        let yaml = r#"name: "test-plugin""#;
        let manifest = PluginManifest::from_yaml(yaml).unwrap();
        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.version, "0.1.0");
        assert!(manifest.library.is_none());
    }

    #[test]
    fn test_default_library_path_linux() {
        let manifest = PluginManifest {
            name: "my-adapter".into(),
            display_name: None,
            description: None,
            version: "1.0.0".into(),
            sdk_version: None,
            author: None,
            library: None,
        };
        let dir = Path::new("/plugins/my-adapter");
        let path = manifest.library_path(dir);
        // Platform-dependent, but the name should contain "lib" prefix
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(
            filename.starts_with("lib"),
            "filename should start with 'lib', got: {}",
            filename
        );
        assert!(
            filename.contains("my-adapter"),
            "filename should contain plugin name"
        );
    }

    #[test]
    fn test_custom_library_path() {
        let manifest = PluginManifest {
            name: "my-adapter".into(),
            display_name: None,
            description: None,
            version: "1.0.0".into(),
            sdk_version: None,
            author: None,
            library: Some("custom.so".into()),
        };
        let dir = Path::new("/plugins/my-adapter");
        let path = manifest.library_path(dir);
        assert_eq!(path, Path::new("/plugins/my-adapter/custom.so"));
    }

    #[test]
    fn test_invalid_yaml() {
        let result = PluginManifest::from_yaml("invalid: [yaml: broken");
        assert!(result.is_err());
    }
}
