#![allow(missing_docs)]

//! EasyBot 集成测试
//!
//! 覆盖 CLI、插件系统等端到端场景。

#[cfg(test)]
mod cli;

#[cfg(test)]
mod updater;

#[cfg(test)]
mod tests {
    use easybot_core::AdapterConfig;
    use easybot_core::AdapterState;
    use easybot_core::bus::EventBus;
    use easybot_core::plugin::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// 查找 `ipc-mock-plugin` 进程外插件可执行文件的路径
    ///
    /// （旧式 cdylib 路径已在 v0.0.42 移除：静态宿主无法 dlopen。）
    fn find_mock_plugin() -> Option<PathBuf> {
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                p.pop();
                p.pop();
                p.join("target")
            });
        let profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        let exe = if cfg!(windows) {
            "ipc-mock-plugin.exe"
        } else {
            "ipc-mock-plugin"
        };
        let path = target_dir.join(profile).join(exe);
        if path.exists() {
            return Some(path);
        }
        None
    }

    fn create_temp_plugin_dir(plugin_path: &std::path::Path) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let plugin_dir = dir.path().join("ipc-mock");

        std::fs::create_dir_all(&plugin_dir).expect("failed to create plugin subdir");

        let bin_name = plugin_path.file_name().unwrap().to_str().unwrap();
        let manifest = format!(
            r#"name: "ipc-mock"
display_name: "IPC Mock"
version: "1.0.0"
sdk_version: 1
command: "{bin_name}"
"#
        );
        std::fs::write(plugin_dir.join("plugin.yaml"), &manifest)
            .expect("failed to write plugin.yaml");

        let dest = plugin_dir.join(bin_name);
        std::fs::copy(plugin_path, &dest).expect("failed to copy plugin binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&dest).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&dest, perms).expect("chmod plugin binary");
        }

        dir
    }

    fn make_adapter_config() -> AdapterConfig {
        AdapterConfig {
            enabled: Some(true),
            token: None,
            api_key: None,
            base_url: None,
            extra: serde_json::Value::Null,
        }
    }

    #[tokio::test]
    async fn test_load_mock_plugin() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("easybot=debug")
            .try_init();

        let plugin_path = find_mock_plugin().expect(
            "ipc-mock-plugin not found. Build it first with: cargo build -p ipc-mock-plugin",
        );
        eprintln!("Found ipc mock plugin at: {}", plugin_path.display());

        let temp_dir = create_temp_plugin_dir(&plugin_path);
        let loader = PluginLoader::new(temp_dir.path().to_path_buf());
        let (succeeded, failed) = loader.load_all().await;

        assert!(failed.is_empty(), "plugin loading failed: {:?}", failed);
        assert_eq!(succeeded.len(), 1, "should load exactly 1 plugin");

        let result = &succeeded[0];
        assert_eq!(result.platform_name, "ipc-mock");
        assert_eq!(result.display_name, "IPC Mock");

        // Create adapter through factory（工厂会 spawn 插件进程并完成 init）
        let event_bus = Arc::new(EventBus::new());
        let factory = loader
            .get_factory("ipc-mock", event_bus.clone())
            .await
            .expect("factory not found");

        let mut adapter = factory(make_adapter_config())
            .await
            .expect("factory create failed");

        assert_eq!(adapter.platform_name(), "ipc-mock");
        assert_eq!(adapter.state(), AdapterState::Starting);

        let conn_result = adapter.connect().await.expect("connect failed");
        assert!(conn_result.ok);
        assert_eq!(adapter.state(), AdapterState::Connected);

        let send_result = adapter
            .send(easybot_core::SendTextParams {
                chat_id: "test-chat".into(),
                message: easybot_core::OutboundMessage {
                    text: "hello".to_string(),
                    parse_mode: easybot_core::ParseMode::default(),
                },
                reply_to: None,
                metadata: None,
            })
            .await
            .expect("send failed");
        assert!(send_result.success);

        adapter.disconnect().await.expect("disconnect failed");
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }
}
