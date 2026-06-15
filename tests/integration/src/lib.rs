//! EasyBot 集成测试
//!
//! 覆盖 CLI、插件系统等端到端场景。

#[cfg(test)]
mod cli;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use easybot_core::plugin::*;
    use easybot_core::bus::EventBus;
    use easybot_core::AdapterConfig;
    use easybot_core::AdapterState;

    /// 查找 mock-adapter 编译产物的路径
    fn find_mock_lib() -> Option<PathBuf> {
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                p.pop();
                p.pop();
                p.join("target")
            });

        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };

        let candidates = [
            target_dir.join(profile).join("libmock_adapter.dylib"),
            target_dir.join(profile).join("libmock_adapter.so"),
            target_dir.join(profile).join("mock_adapter.dll"),
            target_dir.join(profile).join("deps").join("libmock_adapter.dylib"),
            target_dir.join(profile).join("deps").join("libmock_adapter.so"),
        ];

        for c in &candidates {
            if c.exists() {
                return Some(c.clone());
            }
        }

        // Fallback: search deps/
        if let Ok(entries) = std::fs::read_dir(target_dir.join(profile).join("deps")) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("mock_adapter")
                    && (name.ends_with(".so")
                        || name.ends_with(".dylib")
                        || name.ends_with(".dll"))
                {
                    return Some(entry.path());
                }
            }
        }

        None
    }

    fn create_temp_plugin_dir(lib_path: &std::path::Path) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let plugin_dir = dir.path().join("mock-test");

        std::fs::create_dir_all(&plugin_dir).expect("failed to create plugin subdir");

        let lib_name = lib_path.file_name().unwrap().to_str().unwrap();
        let manifest = format!(
            r#"name: "mock-test"
display_name: "Mock Test Adapter"
version: "1.0.0"
sdk_version: 1
library: "{}"
"#,
            lib_name
        );
        std::fs::write(plugin_dir.join("plugin.yaml"), &manifest)
            .expect("failed to write plugin.yaml");

        let dest = plugin_dir.join(lib_name);
        std::fs::copy(lib_path, &dest).expect("failed to copy plugin library");

        dir
    }

    fn make_adapter_config() -> AdapterConfig {
        AdapterConfig {
            enabled: true,
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

        let lib_path = find_mock_lib().expect(
            "mock-adapter library not found. Build it first with: cargo build -p mock-adapter"
        );
        eprintln!("Found mock adapter at: {}", lib_path.display());

        let temp_dir = create_temp_plugin_dir(&lib_path);
        let loader = PluginLoader::new(temp_dir.path().to_path_buf());
        let (succeeded, failed) = loader.load_all().await;

        assert!(failed.is_empty(), "plugin loading failed: {:?}", failed);
        assert_eq!(succeeded.len(), 1, "should load exactly 1 plugin");

        let result = &succeeded[0];
        assert_eq!(result.platform_name, "mock-test");
        assert_eq!(result.display_name, "Mock Test Adapter");

        // Create adapter through factory
        let event_bus = Arc::new(EventBus::new());
        let factory = loader
            .get_factory("mock-test", event_bus.clone())
            .await
            .expect("factory not found");

        let mut adapter = factory(make_adapter_config())
            .await
            .expect("factory create failed");

        // Verify adapter metadata (factory already calls init, so state is Starting)
        assert_eq!(adapter.platform_name(), "mock-test");
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
