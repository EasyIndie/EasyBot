//! API 测试辅助工具
//!
//! 提供创建测试用 AppState 的通用函数。

use easybot_api::{AppState, config_manager::ConfigManager};
use easybot_core::{
    adapter::AdapterManager,
    auth::ApiKeyManager,
    bus::EventBus,
    session::SessionManager,
    storage::sqlite::{SqliteMessageStore, run_migrations},
    types::config::{
        ApiConfig, GatewayConfig, MetricsConfig, RateLimitConfig, ServerConfig, TlsConfig,
        WebSocketConfig,
    },
};
use sqlx::SqlitePool;
use std::sync::Arc;

/// 构建测试用的完整 AppState
///
/// 包含：
/// - 真实 EventBus
/// - 空 AdapterManager（无注册适配器）
/// - 内存 SessionManager
/// - 内存 SQLite MessageStore
/// - ApiKeyManager + 一个预先创建的测试 key
/// - 限流禁用、指标禁用的配置
///
/// 返回 (AppState, test_api_key)，test_api_key 可用于认证请求。
pub async fn test_app_state() -> (AppState, String) {
    let event_bus = Arc::new(EventBus::new());
    let adapter_manager = Arc::new(AdapterManager::new());
    let session_manager = Arc::new(SessionManager::new());

    // 内存 SQLite 作为消息存储
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let message_store: Arc<dyn easybot_core::storage::MessageStore> =
        Arc::new(SqliteMessageStore::new(pool));

    // API Key 管理
    let auth_manager = Arc::new(ApiKeyManager::new());
    let (_, raw_key) = auth_manager
        .create_key("test-key", vec![], None)
        .await
        .unwrap();

    // 测试配置（限流禁用，指标禁用）
    let config = GatewayConfig {
        server: ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0, // 不绑定
            tls: TlsConfig::default(),
            ..Default::default()
        },
        api: ApiConfig {
            base_path: "/api/v1".to_string(),
            websocket: WebSocketConfig::default(),
            rate_limit: RateLimitConfig {
                enabled: false,
                requests_per_minute: 60,
                burst_size: 10,
            },
            metrics: MetricsConfig {
                enabled: false,
                path: "/metrics".to_string(),
            },
        },
        ..GatewayConfig::default()
    };

    let config_manager = ConfigManager::new_shared(Arc::new(config.clone()));

    let state = AppState::new(
        event_bus,
        adapter_manager,
        session_manager,
        message_store,
        auth_manager,
        config,
        config_manager,
        None, // 无 metrics
    );

    (state, raw_key)
}
