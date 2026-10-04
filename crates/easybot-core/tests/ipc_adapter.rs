//! P1-2/P1-3：`IpcPluginAdapter` 端到端 + 异常兜底测试。
//!
//! 需先构建插件二进制：`cargo build -p ipc-mock-plugin`
//! （`cargo test --workspace` 会自动构建）。

#![allow(missing_docs)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use easybot_core::bus::EventBus;
use easybot_core::plugin::IpcPluginAdapter;
use easybot_core::{
    AdapterConfig, AdapterState, OutboundMessage, PlatformAdapter, SendTextParams, event_types,
};

/// 定位 `ipc-mock-plugin` 二进制（与 `tests/integration/src/lib.rs::find_mock_lib` 同思路）。
fn find_ipc_mock_plugin() -> PathBuf {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // crates/easybot-core
            p.pop(); // crates
            p.pop(); // 仓库根
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

    let direct = target_dir.join(profile).join(exe);
    if direct.exists() {
        return direct;
    }
    if let Ok(entries) = std::fs::read_dir(target_dir.join(profile).join("deps")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("ipc-mock-plugin") && !name.ends_with(".d") {
                return entry.path();
            }
        }
    }
    panic!(
        "未找到 ipc-mock-plugin，请先 `cargo build -p ipc-mock-plugin`（{}）",
        direct.display()
    );
}

fn test_config() -> AdapterConfig {
    AdapterConfig {
        enabled: Some(true),
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    }
}

async fn connected_adapter() -> (IpcPluginAdapter, Arc<EventBus>) {
    let event_bus = Arc::new(EventBus::new());
    let mut adapter = IpcPluginAdapter::new(find_ipc_mock_plugin(), "ipc-mock", "IPC Mock", None);
    adapter.set_event_bus(event_bus.clone());
    adapter.init(test_config()).await.expect("init");
    adapter.connect().await.expect("connect");
    (adapter, event_bus)
}

#[tokio::test]
async fn ipc_adapter_full_lifecycle() {
    let event_bus = Arc::new(EventBus::new());
    let mut events = event_bus.subscribe(event_types::MESSAGE_INBOUND);

    let mut adapter = IpcPluginAdapter::new(find_ipc_mock_plugin(), "ipc-mock", "IPC Mock", None);
    adapter.set_event_bus(event_bus.clone());

    adapter
        .init(test_config())
        .await
        .expect("init should succeed");
    assert_eq!(adapter.state(), AdapterState::Starting);

    adapter.connect().await.expect("connect should succeed");
    assert!(adapter.is_connected());

    // 插件 → 宿主：反向事件流
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("timed out waiting for plugin event")
        .expect("event channel closed");
    assert_eq!(event.event_type, event_types::MESSAGE_INBOUND);
    assert_eq!(event.source, "ipc-mock");
    assert_eq!(event.data["chat_id"], "chat-1");

    // 宿主 → 插件：请求/响应
    let sent = adapter
        .send(SendTextParams {
            chat_id: "chat-1".into(),
            message: OutboundMessage {
                text: "ping".into(),
                parse_mode: Default::default(),
            },
            reply_to: None,
            metadata: None,
        })
        .await
        .expect("send should succeed");
    assert!(sent.success);
    assert_eq!(sent.message_id.as_deref(), Some("ipc-chat-1-4"));

    adapter
        .disconnect()
        .await
        .expect("disconnect should succeed");
    assert_eq!(adapter.state(), AdapterState::Stopped);
}

#[tokio::test]
async fn ipc_adapter_send_after_disconnect_fails_fast() {
    let (mut adapter, _bus) = connected_adapter().await;
    adapter.disconnect().await.expect("disconnect");

    // 进程已回收：send 必须立即返回错误，而不是挂起
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        adapter.send(SendTextParams {
            chat_id: "c".into(),
            message: OutboundMessage {
                text: "x".into(),
                parse_mode: Default::default(),
            },
            reply_to: None,
            metadata: None,
        }),
    )
    .await
    .expect("send 不应挂起");
    assert!(result.is_err(), "断开后发送应失败: {result:?}");
}

#[tokio::test]
async fn ipc_adapter_process_exit_wakes_pending_call() {
    // 用 `/bin/true` 冒充插件：它立刻退出，reader 任务须唤醒未决请求，
    // 使 init 快速失败（而非等满 call_timeout）。
    let exit_cmd = if cfg!(windows) { "cmd" } else { "/bin/true" };
    let mut adapter = IpcPluginAdapter::new(exit_cmd, "ipc-mock", "IPC Mock", None)
        .with_call_timeout(Duration::from_secs(30));

    let started = std::time::Instant::now();
    let result = adapter.init(test_config()).await;
    assert!(result.is_err(), "进程立即退出时 init 应失败");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "应被退出事件唤醒而非等满超时，实际耗时 {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn ipc_adapter_passes_home_env_to_plugin() {
    // P1-4：宿主须把配置根目录以 EASYBOT_HOME 注入子进程（供插件持久化凭据）。
    let event_bus = Arc::new(EventBus::new());
    let mut ready = event_bus.subscribe("plugin.ready");
    let home = std::env::temp_dir().join("easybot-ipc-home-test");

    let mut adapter = IpcPluginAdapter::new(
        find_ipc_mock_plugin(),
        "ipc-mock",
        "IPC Mock",
        Some(home.clone()),
    );
    adapter.set_event_bus(event_bus.clone());
    adapter.init(test_config()).await.expect("init");

    let ev = tokio::time::timeout(Duration::from_secs(5), ready.recv())
        .await
        .expect("timed out waiting for plugin.ready")
        .expect("channel closed");
    assert_eq!(ev.event_type, "plugin.ready");
    assert_eq!(
        ev.data["home"].as_str(),
        Some(home.display().to_string().as_str()),
        "插件应看到注入的 EASYBOT_HOME"
    );

    let _ = adapter.disconnect().await;
}

#[tokio::test]
async fn ipc_adapter_detects_crashed_process() {
    // 插件在 connect 后主动退出（模拟崩溃）：适配器须报告 Down，供健康监测器重连。
    let mut adapter = IpcPluginAdapter::new(find_ipc_mock_plugin(), "ipc-mock", "IPC Mock", None)
        .with_env("IPC_MOCK_EXIT_AFTER_CONNECT", "1");
    adapter.init(test_config()).await.expect("init");
    adapter.connect().await.expect("connect");

    for _ in 0..100 {
        if adapter.health_status() == easybot_core::HealthStatus::Down {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(adapter.child_exited(), "应检测到子进程已退出");
    assert_eq!(
        adapter.health_status(),
        easybot_core::HealthStatus::Down,
        "进程退出后须报告 Down"
    );
    let _ = adapter.disconnect().await;
}

#[tokio::test]
async fn ipc_adapter_missing_binary_reports_clear_error() {
    let mut adapter = IpcPluginAdapter::new(
        "/nonexistent/easybot-plugin-xyz",
        "ipc-mock",
        "IPC Mock",
        None,
    );
    let err = adapter.init(test_config()).await.expect_err("应失败");
    let msg = err.to_string();
    assert!(
        msg.contains("failed to spawn plugin"),
        "错误信息应指出 spawn 失败: {msg}"
    );
}
