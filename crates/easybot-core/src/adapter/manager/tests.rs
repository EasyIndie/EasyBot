use super::*;
use crate::adapter::AdapterFactory;
use crate::types::message::{ChatInfo, SendResult, SendTextParams};
use async_trait::async_trait;

/// 创建一个测试用的 Arc<AdapterManager>（自动调用 init_self_ref）
async fn new_manager() -> Arc<AdapterManager> {
    let mgr = Arc::new(AdapterManager::new());
    mgr.init_self_ref().await;
    mgr
}

/// 注册 MockTestAdapter 到 manager
async fn register_mock_adapter(manager: &AdapterManager) {
    let registry = manager.registry();
    let factory: AdapterFactory = Arc::new(|config| {
        Box::pin(async move {
            let mut adapter = MockTestAdapter::new();
            let result = adapter.init(config).await.map_err(|e| e.to_string())?;
            if !result.ok {
                return Err(result.error.unwrap_or_default());
            }
            let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
            Ok(boxed)
        })
    });
    registry
        .register("test-mock", "Test Mock", factory, &[])
        .await;
}

/// 注册一个 connect 会失败的 mock 适配器（用于测试健康监控的重连分类）。
async fn register_failing_mock_adapter(manager: &AdapterManager, failure: MockConnectFailure) {
    let registry = manager.registry();
    let factory: AdapterFactory = Arc::new(move |config| {
        let failure = failure.clone();
        Box::pin(async move {
            let mut adapter = MockTestAdapter::new().failing(failure);
            let result = adapter.init(config).await.map_err(|e| e.to_string())?;
            if !result.ok {
                return Err(result.error.unwrap_or_default());
            }
            let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
            Ok(boxed)
        })
    });
    registry
        .register("test-mock", "Test Mock", factory, &[])
        .await;
}

/// 等待适配器从 Connecting 变为 Connected（最长 2 秒）
async fn wait_connected(manager: &AdapterManager, platform: &str) {
    for _ in 0..100 {
        if let Some(status) = manager.get_status(platform).await
            && status.state == AdapterState::Connected
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Adapter '{}' did not connect within timeout", platform);
}

/// 等待适配器进入 Failed 状态（最长 2 秒）
async fn wait_failed(manager: &AdapterManager, platform: &str) {
    for _ in 0..100 {
        if let Some(status) = manager.get_status(platform).await
            && status.state == AdapterState::Failed
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Adapter '{}' did not fail within timeout", platform);
}

// ── Mock 适配器 ──────────────────────────────────────────

/// 可配置的 connect 失败模式（用于测试健康监控的重连分类）。
#[derive(Clone)]
enum MockConnectFailure {
    /// connect() 返回 `Ok(ConnectResult{ok:false, ...})`，带显式分类（或 None 回退启发式）。
    Result {
        error: &'static str,
        kind: Option<ConnectErrorKind>,
    },
    /// connect() 返回 `Err(GatewayError::Transient(...))`。
    Transient,
    /// connect() 返回 `Err(GatewayError::Unauthorized(...))`。
    Unauthorized,
}

struct MockTestAdapter {
    platform: String,
    display: String,
    state: AdapterState,
    connect_failure: Option<MockConnectFailure>,
}

impl MockTestAdapter {
    fn new() -> Self {
        Self {
            platform: "test-mock".into(),
            display: "Test Mock".into(),
            state: AdapterState::Created,
            connect_failure: None,
        }
    }

    fn failing(mut self, failure: MockConnectFailure) -> Self {
        self.connect_failure = Some(failure);
        self
    }
}

#[async_trait]
impl PlatformAdapter for MockTestAdapter {
    fn platform_name(&self) -> &str {
        &self.platform
    }
    fn display_name(&self) -> &str {
        &self.display
    }
    fn capabilities(&self) -> &[Capability] {
        &[]
    }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        let _ = config;
        self.state = AdapterState::Starting;
        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        if let Some(failure) = &self.connect_failure {
            self.state = AdapterState::Failed;
            return match failure {
                MockConnectFailure::Result { error, kind } => {
                    Ok(ConnectResult::failed((*error).to_string(), *kind))
                }
                MockConnectFailure::Transient => Err(GatewayError::Transient(
                    "mock transient network failure".into(),
                )),
                MockConnectFailure::Unauthorized => Err(GatewayError::Unauthorized(
                    "mock invalid credentials".into(),
                )),
            };
        }
        self.state = AdapterState::Connected;
        Ok(ConnectResult::ok(None))
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state.clone()
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            status: if self.state == AdapterState::Connected {
                HealthStatus::Healthy
            } else {
                HealthStatus::Down
            },
            connected: self.state == AdapterState::Connected,
            last_connected_at: None,
            last_error_at: None,
            last_error: None,
            messages_in: 0,
            messages_out: 0,
            errors: 0,
            uptime: None,
        }
    }

    async fn send(&self, _p: SendTextParams) -> Result<SendResult, GatewayError> {
        Ok(SendResult {
            success: true,
            message_id: None,
            timestamp: None,
            error: None,
            error_code: None,
            retryable: false,
        })
    }

    async fn get_chat_info(&self, _id: &str) -> Result<ChatInfo, GatewayError> {
        Err(GatewayError::capability_not_supported("get_chat_info"))
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: true,
            token_configured: false,
            extra: serde_json::json!({}),
        }
    }

    fn status_summary(&self) -> AdapterStatusSummary {
        AdapterStatusSummary {
            platform: self.platform.clone(),
            display_name: self.display.clone(),
            state: self.state.clone(),
            connected: self.state == AdapterState::Connected,
            health: Some(self.health_status()),
            last_error: None,
            uptime: None,
            messages_in: 0,
            messages_out: 0,
        }
    }
}

// ── 测试: config（含 token）被正确传递到适配器 ───────────

#[tokio::test]
async fn test_start_passes_config_to_adapter() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("my-secret-token".into()),
        api_key: Some("my-api-key".into()),
        base_url: None,
        extra: serde_json::json!({"custom": "value"}),
    };

    // 启动后，config 应该被传递到 init()，工厂创建 adapter 时使用
    let start_result = manager.start("test-mock", config.clone()).await.unwrap();
    assert!(start_result.ok);
    assert!(start_result.pending);

    // 等待后台连接完成
    wait_connected(&manager, "test-mock").await;

    // get_status 验证状态
    let status = manager.get_status("test-mock").await.unwrap();
    assert_eq!(status.state, AdapterState::Connected);
}

#[tokio::test]
async fn test_has_connected_after_start() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    assert!(!manager.has_connected().await);

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    assert!(manager.has_connected().await);
}

#[tokio::test]
async fn test_send_message_delegation() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    let params = SendTextParams {
        chat_id: "1".to_string(),
        message: crate::types::message::OutboundMessage {
            text: "hello".to_string(),
            parse_mode: crate::types::message::ParseMode::None,
        },
        reply_to: None,
        metadata: None,
    };
    let result = manager.send_message("test-mock", params).await.unwrap();
    assert!(result.success);
}

#[tokio::test]
async fn test_stop_all_cleans_up() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;
    assert!(manager.has_connected().await);

    manager.stop_all().await;
    // stop_all 清空 adapters map, 取消 pending_connections
    assert!(!manager.has_connected().await);
}

#[tokio::test]
async fn test_start_all_skips_disabled() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let mut configs = HashMap::new();
    configs.insert(
        "test-mock".to_string(),
        AdapterConfig {
            enabled: Some(false),
            token: None,
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        },
    );

    let result = manager.start_all(configs).await;
    assert!(
        result.succeeded.is_empty(),
        "disabled adapter should not start"
    );
    assert!(
        result.failed.is_empty(),
        "disabled adapter should not fail either"
    );
}

#[tokio::test]
async fn test_start_all_disabled_by_config_overrides_env_vars() {
    // 模拟真实场景：.env 中已设置凭据（候选 auto-enable），
    // 但 gateway.local.yaml 中 enabled: false → 应跳过
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let registry = manager.registry();
    registry
        .register(
            "cred-mock",
            "Cred Mock",
            Arc::new(|config| {
                Box::pin(async move {
                    let mut adapter = MockTestAdapter::new();
                    adapter.platform = "cred-mock".into();
                    let result = adapter.init(config).await.map_err(|e| e.to_string())?;
                    if !result.ok {
                        return Err(result.error.unwrap_or_default());
                    }
                    let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
                    Ok(boxed)
                })
            }),
            &["CRED_MOCK_TOKEN"],
        )
        .await;

    // 设置凭据 — auto-enable 会因此尝试启动，但 enabled: false 优先
    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::set_var("CRED_MOCK_TOKEN", "some-token") };

    let mut configs = HashMap::new();
    configs.insert(
        "cred-mock".to_string(),
        AdapterConfig {
            enabled: Some(false),
            token: None,
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        },
    );

    let result = manager.start_all(configs).await;
    assert!(
        !result.succeeded.contains(&"cred-mock".to_string()),
        "cred-mock should NOT start when enabled:false in config \
             even though env vars are set for auto-enable; succeeded: {:?}",
        result.succeeded
    );

    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::remove_var("CRED_MOCK_TOKEN") };
}

#[tokio::test]
async fn test_start_publishes_adapter_connected() {
    let event_bus = Arc::new(EventBus::new());
    let mut rx = event_bus.subscribe(event_types::ADAPTER_CONNECTED);
    let manager = Arc::new(AdapterManager::new().with_event_bus(event_bus));
    AdapterManager::init_self_ref(&manager).await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();

    // ADAPTER_CONNECTED 现在由后台任务发布
    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive ADAPTER_CONNECTED")
        .expect("event should be valid");

    assert_eq!(event.event_type, event_types::ADAPTER_CONNECTED);
    assert_eq!(event.source, "adapter_manager");
}

#[tokio::test]
async fn test_stop_publishes_adapter_disconnected() {
    let event_bus = Arc::new(EventBus::new());
    let mut rx = event_bus.subscribe(event_types::ADAPTER_DISCONNECTED);
    let manager = Arc::new(AdapterManager::new().with_event_bus(event_bus));
    AdapterManager::init_self_ref(&manager).await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    manager.stop("test-mock").await.unwrap();

    let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("should receive ADAPTER_DISCONNECTED")
        .expect("event should be valid");

    assert_eq!(event.event_type, event_types::ADAPTER_DISCONNECTED);
    assert_eq!(event.source, "adapter_manager");
}

// ── inject_credentials 测试 ───────────────────────────────

#[tokio::test]
async fn test_inject_credentials_populates_token_and_extra() {
    let manager = AdapterManager::new();
    let registry = manager.registry();
    let factory: AdapterFactory = Arc::new(|_config| {
        Box::pin(async move {
            let adapter = MockTestAdapter::new();
            let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
            Ok(boxed)
        })
    });
    // platform "test" → prefix "TEST_" → env vars must start with "TEST_"
    registry
        .register("test", "Test", factory, &["TEST_APP_ID", "TEST_APP_SECRET"])
        .await;

    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::set_var("TEST_APP_ID", "app-id-123") };
    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::set_var("TEST_APP_SECRET", "secret-456") };

    let mut config = AdapterConfig {
        enabled: None,
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };

    manager.inject_credentials("test", &mut config).await;

    // token 应为最后一个凭据变量（SECRET）
    assert_eq!(config.token.as_deref(), Some("secret-456"));
    // extra: 去掉平台前缀 TEST_ 后的小写 key
    assert_eq!(config.extra["app_id"], "app-id-123");
    assert_eq!(config.extra["app_secret"], "secret-456");

    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::remove_var("TEST_APP_ID") };
    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::remove_var("TEST_APP_SECRET") };
}

#[tokio::test]
async fn test_inject_credentials_does_not_overwrite_existing_token() {
    let manager = AdapterManager::new();
    let registry = manager.registry();
    let factory: AdapterFactory = Arc::new(|_config| {
        Box::pin(async move {
            let adapter = MockTestAdapter::new();
            let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
            Ok(boxed)
        })
    });
    registry
        .register("toktest", "TokTest", factory, &["TOKTEST_TOKEN"])
        .await;

    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::set_var("TOKTEST_TOKEN", "from-env") };

    let mut config = AdapterConfig {
        enabled: None,
        token: Some("explicit-token".to_string()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };

    manager.inject_credentials("toktest", &mut config).await;

    // 显式设置的 token 保持不变
    assert_eq!(config.token.as_deref(), Some("explicit-token"));
    // extra 仍会被填充，key 为去前缀后的小写: TOKTEST_TOKEN → strip TOKTEST_ → TOKEN → lowercase → token
    assert_eq!(config.extra["token"], "from-env");

    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::remove_var("TOKTEST_TOKEN") };
}

#[tokio::test]
async fn test_start_all_auto_enables_with_injected_credentials() {
    let manager = new_manager().await;
    let registry = manager.registry();
    let factory: AdapterFactory = Arc::new(|config| {
        Box::pin(async move {
            let mut adapter = MockTestAdapter::new();
            // 验证 config 已被注入凭据
            let init_result = adapter.init(config).await.map_err(|e| e.to_string())?;
            if !init_result.ok {
                return Err(init_result.error.unwrap_or_default());
            }
            let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
            Ok(boxed)
        })
    });
    registry
        .register("autotest", "AutoTest", factory, &["AUTOTEST_TOKEN"])
        .await;

    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::set_var("AUTOTEST_TOKEN", "my-token") };

    // 不传入任何 config — start_all 应自动检测并注入凭据
    let result = manager.start_all(HashMap::new()).await;
    assert!(
        result.succeeded.contains(&"autotest".to_string()),
        "autotest should be auto-enabled: {:?}",
        result
    );

    // SAFETY: 测试环境，单线程执行
    unsafe { std::env::remove_var("AUTOTEST_TOKEN") };
}

#[tokio::test]
async fn test_start_all_skips_opt_in_adapter_without_explicit_enable() {
    // 回归：无凭据 + 注册策略为「不自动启用」（个人微信语义）时，默认不应启动。
    let manager = new_manager().await;
    let factory: AdapterFactory = Arc::new(|config| {
        Box::pin(async move {
            let mut adapter = MockTestAdapter::new();
            let result = adapter.init(config).await.map_err(|e| e.to_string())?;
            if !result.ok {
                return Err(result.error.unwrap_or_default());
            }
            let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
            Ok(boxed)
        })
    });
    manager
        .registry()
        .register_with_policy("optin", "OptIn", factory, &[], false)
        .await;

    let result = manager.start_all(HashMap::new()).await;
    assert!(
        !result.succeeded.contains(&"optin".to_string()),
        "opt-in 适配器未显式 enabled: true 时不得自动启动: {:?}",
        result
    );
    assert!(
        manager.get_status("optin").await.is_none(),
        "opt-in 适配器不应被创建"
    );
}

#[tokio::test]
async fn test_start_all_starts_opt_in_adapter_with_explicit_enable() {
    let manager = new_manager().await;
    let factory: AdapterFactory = Arc::new(|config| {
        Box::pin(async move {
            let mut adapter = MockTestAdapter::new();
            let result = adapter.init(config).await.map_err(|e| e.to_string())?;
            if !result.ok {
                return Err(result.error.unwrap_or_default());
            }
            let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
            Ok(boxed)
        })
    });
    manager
        .registry()
        .register_with_policy("optin", "OptIn", factory, &[], false)
        .await;

    let mut configs = HashMap::new();
    configs.insert(
        "optin".to_string(),
        AdapterConfig {
            enabled: Some(true),
            token: None,
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        },
    );

    let result = manager.start_all(configs).await;
    assert!(
        result.succeeded.contains(&"optin".to_string()),
        "opt-in 适配器在显式 enabled: true 时应启动: {:?}",
        result
    );
}

// ── compute_backoff 测试 ───────────────────────────────────

#[test]
fn test_compute_backoff_sequence() {
    assert_eq!(compute_backoff(0), Duration::from_secs(5));
    assert_eq!(compute_backoff(1), Duration::from_secs(10));
    assert_eq!(compute_backoff(2), Duration::from_secs(30));
    assert_eq!(compute_backoff(3), Duration::from_secs(60));
    assert_eq!(compute_backoff(4), Duration::from_secs(120));
}

#[test]
fn test_compute_backoff_capped_at_300s() {
    assert_eq!(compute_backoff(5), Duration::from_secs(300));
    assert_eq!(compute_backoff(10), Duration::from_secs(300));
    assert_eq!(compute_backoff(100), Duration::from_secs(300));
}

// ── config storage 测试 ────────────────────────────────────

#[tokio::test]
async fn test_config_stored_on_start_cleared_on_stop() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("test-token".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({"key": "val"}),
    };

    // Start — config should be stored immediately by start()
    manager.start("test-mock", config.clone()).await.unwrap();
    wait_connected(&manager, "test-mock").await;
    {
        let configs = manager.configs.read().await;
        assert!(configs.contains_key("test-mock"));
        assert_eq!(
            configs.get("test-mock").unwrap().token.as_deref(),
            Some("test-token")
        );
        assert_eq!(configs.get("test-mock").unwrap().extra["key"], "val");
    }

    // Stop — config should be cleared
    manager.stop("test-mock").await.unwrap();
    {
        let configs = manager.configs.read().await;
        assert!(!configs.contains_key("test-mock"));
    }
}

#[tokio::test]
async fn test_stop_all_clears_configs() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    // Verify config was stored
    assert!(!manager.configs.read().await.is_empty());

    manager.stop_all().await;

    // stop_all now clears configs as well
    assert!(manager.configs.read().await.is_empty());
}

#[tokio::test]
async fn test_list_statuses_after_start() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    // Before start: statuses should be empty
    let statuses = manager.list_statuses().await;
    assert!(statuses.is_empty());

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    // After start: statuses should include the adapter
    let statuses = manager.list_statuses().await;
    assert!(!statuses.is_empty());
    let status = statuses.iter().find(|s| s.platform == "test-mock");
    assert!(status.is_some());
    assert!(status.unwrap().connected);
}

#[tokio::test]
async fn test_list_statuses_after_stop() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    let _ = manager.stop("test-mock").await;

    // After stop: statuses should reflect disconnected state
    let statuses = manager.list_statuses().await;
    let status = statuses.iter().find(|s| s.platform == "test-mock");
    assert!(status.is_some());
    assert!(!status.unwrap().connected);
}

// ── health / status_summary 相关测试 ─────────────────────

#[tokio::test]
async fn test_list_statuses_preserves_terminal_failed_state() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    // Start adapter and wait for connection
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    // Simulate the race condition: health monitor calls set_status_failed
    // during a transport retry while the adapter is still in the adapters
    // map (Tier 1 permanent failure path). The adapter instance hasn't
    // been removed yet, but the cache has the Failed state.
    manager
        .set_status_failed("test-mock", "permanent auth failure")
        .await;

    // list_statuses should preserve the Failed state from cache,
    // NOT overwrite it with the live adapter's self-reported Connected.
    let statuses = manager.list_statuses().await;
    let test_status = statuses.iter().find(|s| s.platform == "test-mock").unwrap();
    assert_eq!(
        test_status.state,
        AdapterState::Failed,
        "terminal Failed state should be preserved"
    );
    assert_eq!(
        test_status.last_error.as_deref(),
        Some("permanent auth failure")
    );
}

#[tokio::test]
async fn test_set_status_failed_preserves_display_name() {
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    // First, populate the cache with a real display name via start
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    // Verify the display name is correct
    {
        let statuses = manager.statuses.read().await;
        let s = statuses.get("test-mock").unwrap();
        assert_eq!(s.display_name, "Test Mock");
    }

    // Simulate set_status_failed
    manager
        .set_status_failed("test-mock", "permanent auth error")
        .await;

    // Verify display_name is preserved, not overwritten with "test-mock"
    let statuses = manager.statuses.read().await;
    let s = statuses.get("test-mock").unwrap();
    assert_eq!(s.state, AdapterState::Failed);
    assert_eq!(s.display_name, "Test Mock");
    assert_eq!(s.last_error.as_deref(), Some("permanent auth error"));
}

#[tokio::test]
async fn test_health_status_computation() {
    // Test the default health_status() implementation on the trait.
    // MockTestAdapter uses the default health_status() which checks:
    //   - Connected + no heartbeat mechanism → Healthy
    //   - Connected + fresh heartbeat → Healthy
    //   - Connected + stale heartbeat → Degraded
    //   - Anything else → Down
    //
    // MockTestAdapter does NOT override heartbeat_age_ms() (returns None),
    // so when Connected, health_status() returns Healthy.
    // When Created, it returns Down.

    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    // Before start: state is Created → health_status() → Down
    let s = manager.get_status("test-mock").await;
    // No status yet (adapter never started)
    assert!(s.is_none());

    // Start and connect
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    // After connect: state is Connected, no heartbeat → Healthy
    let adapters = manager.adapters.read().await;
    let adapter = adapters.get("test-mock").unwrap();
    let health = adapter.health().await;
    assert_eq!(health.status, HealthStatus::Healthy);
    assert!(health.connected);
}

#[tokio::test]
async fn test_list_statuses_reflects_live_adapter_health() {
    // When an adapter is connected and healthy, list_statuses should
    // return the live status with correct health (not None).
    let manager = new_manager().await;
    register_mock_adapter(&manager).await;

    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("t".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    manager.start("test-mock", config).await.unwrap();
    wait_connected(&manager, "test-mock").await;

    let statuses = manager.list_statuses().await;
    let s = statuses.iter().find(|s| s.platform == "test-mock").unwrap();
    assert_eq!(s.state, AdapterState::Connected);
    assert!(s.connected);
    // For the mock adapter (no heartbeat mechanism), Connected → Healthy
    assert_eq!(s.health, Some(HealthStatus::Healthy));
}

// ── classify_error 分类测试 ──────────────────────────────

#[test]
fn classify_error_transient_for_qq_network_auth_failure() {
    // 回归：QQ connect 网络失败被 "auth failed" 前缀误判为永久。
    let e = GatewayError::Internal(
        "QQ auth failed (getAppAccessToken): QQ getAppAccessToken request failed: \
             error sending request for url (https://...): connection timed out"
            .into(),
    );
    assert!(
        matches!(classify_error(&e), ReconnectFailure::Transient(_)),
        "network failure during auth must be Transient, got {:?}",
        classify_error(&e)
    );
}

#[test]
fn classify_error_transient_for_connection_reset() {
    let e = GatewayError::Internal(
        "QQ auth failed: QQ GET /users/@me failed: \
             error sending request: connection reset by peer"
            .into(),
    );
    assert!(matches!(classify_error(&e), ReconnectFailure::Transient(_)));
}

#[test]
fn classify_error_transient_for_rate_limit() {
    // 回归：Discord 限流被 "auth failed" 前缀误判为永久。
    let e = GatewayError::Internal(
        "Discord auth failed: Discord API rate limited on /users/@me after retry".into(),
    );
    assert!(matches!(classify_error(&e), ReconnectFailure::Transient(_)));
}

#[test]
fn classify_error_permanent_for_genuine_auth_rejection() {
    let e = GatewayError::Internal("Telegram auth failed: Unauthorized".into());
    assert!(matches!(classify_error(&e), ReconnectFailure::Permanent(_)));
}

#[test]
fn classify_error_permanent_for_discord_401() {
    let e = GatewayError::Internal(
        "Discord auth failed: Discord API 401 /users/@me: Unauthorized".into(),
    );
    assert!(matches!(classify_error(&e), ReconnectFailure::Permanent(_)));
}

#[test]
fn classify_error_structured_variants() {
    assert!(matches!(
        classify_error(&GatewayError::Unauthorized("x".into())),
        ReconnectFailure::Permanent(_)
    ));
    assert!(matches!(
        classify_error(&GatewayError::AuthFailed("x".into())),
        ReconnectFailure::Permanent(_)
    ));
    assert!(matches!(
        classify_error(&GatewayError::Forbidden("x".into())),
        ReconnectFailure::Permanent(_)
    ));
    assert!(matches!(
        classify_error(&GatewayError::Transient("x".into())),
        ReconnectFailure::Transient(_)
    ));
    assert!(matches!(
        classify_error(&GatewayError::RequestTimeout("x".into())),
        ReconnectFailure::Transient(_)
    ));
    assert!(matches!(
        classify_error(&GatewayError::RateLimited { retry_after_ms: 1 }),
        ReconnectFailure::Transient(_)
    ));
}

#[test]
fn classify_error_unknown_defaults_to_transient() {
    let e = GatewayError::Internal("some unexpected internal condition".into());
    assert!(matches!(classify_error(&e), ReconnectFailure::Transient(_)));
}

// ── 健康监控重连分类测试 ─────────────────────────────────

#[tokio::test]
async fn health_monitor_retries_transient_connect_failure() {
    // 瞬态 connect 失败不得触发永久禁用 → 健康监控继续退避重试。
    let manager = new_manager().await;
    register_failing_mock_adapter(&manager, MockConnectFailure::Transient).await;

    manager
        .start("test-mock", AdapterConfig::with_enabled(true))
        .await
        .unwrap();
    wait_failed(&manager, "test-mock").await;

    let mut reconnect_state = HashMap::new();
    manager.run_health_check(&mut reconnect_state).await;

    let state = reconnect_state.get("test-mock").expect("reconnect state");
    assert!(
        !state.permanent_failure,
        "transient connect failure must not disable the adapter"
    );
    assert_eq!(
        state.total_failures, 1,
        "should count one retryable failure"
    );
    assert!(
        state.backoff_until.is_some(),
        "should schedule a retry backoff"
    );

    // 同步到 API 可见层：非永久 + 重试次数 + 下次重试已排定
    let info = manager.get_reconnect_info("test-mock").await;
    assert!(!info.permanent_failure, "frontend must see non-permanent");
    assert_eq!(info.retry_attempt, 1, "frontend must see retry count");
    assert!(
        info.next_retry_in_ms.is_some(),
        "frontend must see scheduled retry"
    );
}

#[tokio::test]
async fn health_monitor_disables_on_permanent_connect_failure() {
    // 显式标记 Permanent 的 connect 失败 → 适配器被永久停用（不重试）。
    let manager = new_manager().await;
    register_failing_mock_adapter(
        &manager,
        MockConnectFailure::Result {
            error: "mock invalid credentials",
            kind: Some(ConnectErrorKind::Permanent),
        },
    )
    .await;

    manager
        .start("test-mock", AdapterConfig::with_enabled(true))
        .await
        .unwrap();
    wait_failed(&manager, "test-mock").await;

    let mut reconnect_state = HashMap::new();
    manager.run_health_check(&mut reconnect_state).await;

    let state = reconnect_state.get("test-mock").expect("reconnect state");
    assert!(
        state.permanent_failure,
        "permanent connect failure must disable the adapter"
    );

    // 同步到 API 可见层：永久停用标记透出，且无排定的重试
    let info = manager.get_reconnect_info("test-mock").await;
    assert!(
        info.permanent_failure,
        "frontend must see the permanent failure"
    );
    assert_eq!(
        info.retry_attempt, 0,
        "permanent failure does not count as a retryable attempt"
    );
}

#[tokio::test]
async fn health_monitor_disables_on_unauthorized_connect_error() {
    // Err(GatewayError::Unauthorized) 路径（无 error_kind，走启发式）→ 永久。
    let manager = new_manager().await;
    register_failing_mock_adapter(&manager, MockConnectFailure::Unauthorized).await;

    manager
        .start("test-mock", AdapterConfig::with_enabled(true))
        .await
        .unwrap();
    wait_failed(&manager, "test-mock").await;

    let mut reconnect_state = HashMap::new();
    manager.run_health_check(&mut reconnect_state).await;

    let state = reconnect_state.get("test-mock").expect("reconnect state");
    assert!(
        state.permanent_failure,
        "Unauthorized connect error must disable the adapter"
    );
}
