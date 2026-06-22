//! E2E 测试共享工具
//!
//! 提供所有平台 E2E 测试共用的辅助函数。

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use easybot_api::AppState;
use easybot_core::adapter::AdapterManager;
use easybot_core::auth::ApiKeyManager;
use easybot_core::bus::EventBus;
use easybot_core::session::SessionManager;
use easybot_core::storage::sqlite::{SqliteMessageStore, run_migrations};
use easybot_core::types::config::{
    ApiConfig, GatewayConfig, MetricsConfig, RateLimitConfig, ServerConfig, TlsConfig,
    WebSocketConfig,
};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::SqlitePool;
use tower::ServiceExt;

// ══════════════════════════════════════════════════════════════
// HTTP 请求辅助
// ══════════════════════════════════════════════════════════════

pub async fn auth_get(router: &Router, path: &str, key: &str) -> (axum::http::StatusCode, Value) {
    let req = axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

pub async fn auth_post(
    router: &Router,
    path: &str,
    key: &str,
    body: Option<Value>,
) -> (axum::http::StatusCode, Value) {
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(path)
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string(&body.unwrap_or(serde_json::json!({}))).unwrap(),
        ))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

pub async fn public_get(router: &Router, path: &str) -> (axum::http::StatusCode, Value) {
    let req = axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or_default())
}

// ══════════════════════════════════════════════════════════════
// 环境构建
// ══════════════════════════════════════════════════════════════

/// 创建内存 SQLite + 核心组件
pub async fn create_core() -> (
    Arc<EventBus>,
    Arc<AdapterManager>,
    Arc<SessionManager>,
    Arc<dyn easybot_core::storage::MessageStore>,
) {
    let event_bus = Arc::new(EventBus::new());
    let adapter_manager = Arc::new(AdapterManager::new());
    let session_manager = Arc::new(SessionManager::new());
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let message_store: Arc<dyn easybot_core::storage::MessageStore> =
        Arc::new(SqliteMessageStore::new(pool));
    (event_bus, adapter_manager, session_manager, message_store)
}

/// 构建默认 GatewayConfig（仅 server + api 段）
pub fn default_gateway_config() -> GatewayConfig {
    GatewayConfig {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            tls: TlsConfig::default(),
        },
        api: ApiConfig {
            base_path: "/api/v1".into(),
            websocket: WebSocketConfig::default(),
            rate_limit: RateLimitConfig {
                enabled: false,
                requests_per_minute: 60,
                burst_size: 10,
            },
            metrics: MetricsConfig {
                enabled: false,
                path: "/metrics".into(),
            },
        },
        ..GatewayConfig::default()
    }
}

/// 创建 AppState + Router + API Key
pub async fn build_router(
    event_bus: Arc<EventBus>,
    adapter_manager: Arc<AdapterManager>,
    session_manager: Arc<SessionManager>,
    message_store: Arc<dyn easybot_core::storage::MessageStore>,
    config: GatewayConfig,
) -> (Router, String) {
    let auth_manager = Arc::new(ApiKeyManager::new());
    let (_, raw_key) = auth_manager.create_key("e2e", vec![], None).await.unwrap();

    let config_manager =
        easybot_api::config_manager::ConfigManager::new_shared(Arc::new(config.clone()));

    let state = AppState::new(
        event_bus,
        adapter_manager,
        session_manager,
        message_store,
        auth_manager,
        config,
        config_manager,
        None,
    );
    let router = easybot_api::server::create_router(state);
    (router, raw_key)
}

// ══════════════════════════════════════════════════════════════
// 断言辅助
// ══════════════════════════════════════════════════════════════

/// 轮询适配器状态直到 Connected 或超时
pub async fn wait_for_connection(
    router: &Router,
    key: &str,
    platform: &str,
    max_retries: u32,
) -> bool {
    let status_path = format!("/api/v1/adapters/{}/status", platform);
    for i in 0..max_retries {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (_, json) = auth_get(router, &status_path, key).await;
        if json.get("connected").and_then(|v| v.as_bool()) == Some(true) {
            return true;
        }
        if i == max_retries - 1 {
            eprintln!("Last status before timeout ({}): {:?}", platform, json);
        }
    }
    false
}

/// 通过 adapter/start API 启动适配器并等待连接就绪
pub async fn start_and_connect(router: &Router, key: &str, platform: &str) -> bool {
    let (status, json) = auth_post(
        router,
        &format!("/api/v1/adapters/{}/start", platform),
        key,
        None,
    )
    .await;
    if status != 200 || json.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        eprintln!("start {} failed: {:?}", platform, json);
        return false;
    }
    wait_for_connection(router, key, platform, 15).await
}
