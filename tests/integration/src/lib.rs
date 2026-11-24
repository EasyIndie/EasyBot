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

    /// `plugin enable` 必须**立即生效**（不再「下次启动生效」），
    /// 与 `disable` 对称；两者都不需要重启网关。
    #[tokio::test]
    async fn test_plugin_enable_and_disable_are_hot() {
        use easybot_core::adapter::AdapterManager;
        use easybot_core::plugin::manager::PluginManager;
        use easybot_core::types::config::PluginConfig;

        let plugin_path = find_mock_plugin().expect(
            "ipc-mock-plugin not found. Build it first with: cargo build -p ipc-mock-plugin",
        );
        let temp_dir = create_temp_plugin_dir(&plugin_path);
        let plugins_dir = temp_dir.path().to_path_buf();

        // 模拟「已安装但被禁用」：清单写 enabled: false
        let manifest_path = plugins_dir.join("ipc-mock").join("plugin.yaml");
        let manifest = std::fs::read_to_string(&manifest_path).unwrap();
        std::fs::write(&manifest_path, format!("{manifest}enabled: false\n")).unwrap();

        let event_bus = Arc::new(EventBus::new());
        let adapter_manager = Arc::new(AdapterManager::new().with_event_bus(event_bus.clone()));
        adapter_manager.init_self_ref().await;

        let config = Arc::new(tokio::sync::RwLock::new(PluginConfig::default()));
        let manager = PluginManager::new(
            plugins_dir.clone(),
            config,
            adapter_manager.clone(),
            event_bus.clone(),
            false,
        )
        .await;

        // 禁用状态：未注册、不可用
        assert!(
            adapter_manager.get_status("ipc-mock").await.is_none(),
            "disabled plugin must not be registered"
        );

        // 启用 → 立即注册并启动
        manager
            .set_enabled("ipc-mock", true)
            .await
            .expect("hot enable should succeed");
        let status = adapter_manager
            .get_status("ipc-mock")
            .await
            .expect("enabled plugin must be registered immediately");
        assert_eq!(status.platform, "ipc-mock");

        // 禁用 → 立即停止（未连接则标记 Stopped）并注销工厂
        manager
            .set_enabled("ipc-mock", false)
            .await
            .expect("hot disable must not hang or fail");
        let after = adapter_manager
            .get_status("ipc-mock")
            .await
            .expect("status entry is kept for the API, but must be stopped");
        assert_eq!(after.state, AdapterState::Stopped);
        assert!(!after.connected, "disabled plugin must not stay connected");
        assert!(
            !adapter_manager.registry().has_platform("ipc-mock").await,
            "disabled plugin factory must be unregistered"
        );

        // 再次启用 → 仍然可用（工厂已注销后重新注册）
        manager
            .set_enabled("ipc-mock", true)
            .await
            .expect("re-enable after disable should succeed");
        assert!(adapter_manager.registry().has_platform("ipc-mock").await);
    }
}
