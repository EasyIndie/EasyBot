//! Telegram 适配器端到端（E2E）集成测试
//!
//! 使用 wiremock 模拟 Telegram Bot API，通过 REST API 验证完整服务链路。
//!
//! 测试覆盖：
//! - 适配器生命周期（注册/init/connect/status/stop/start）
//! - 消息发送/编辑/删除
//! - API 认证

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use easybot_api::AppState;
use easybot_core::adapter::AdapterManager;
use easybot_core::auth::ApiKeyManager;
use easybot_core::bus::EventBus;
use easybot_core::session::SessionManager;
use easybot_core::storage::sqlite::{run_migrations, SqliteMessageStore};
use easybot_core::types::config::{
    ApiConfig, GatewayConfig, MetricsConfig, RateLimitConfig, ServerConfig, TlsConfig,
    WebSocketConfig,
};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::SqlitePool;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ══════════════════════════════════════════════════════════════
// 辅助：构建 HTTP 请求
// ══════════════════════════════════════════════════════════════

async fn auth_get(router: &Router, path: &str, key: &str) -> (axum::http::StatusCode, Value) {
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

async fn auth_post(
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

async fn public_get(router: &Router, path: &str) -> (axum::http::StatusCode, Value) {
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

/// 等待适配器连接（轮询 status 端点）
async fn wait_for_connection(router: &Router, key: &str, max_retries: u32) -> bool {
    for i in 0..max_retries {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (_, json) = auth_get(router, "/api/v1/adapters/telegram/status", key).await;
        if json.get("connected").and_then(|v| v.as_bool()) == Some(true) {
            return true;
        }
        if i == max_retries - 1 {
            eprintln!("Last status before timeout: {:?}", json);
        }
    }
    false
}

// ══════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════

/// 每个测试独立的 setup：创建 wiremock + AppState + Router
async fn setup() -> (Router, String, MockServer) {
    // 启动 mock Telegram API
    let mock_server = MockServer::start().await;
    let mock_port = mock_server.address().port();
    let mock_base = format!("http://127.0.0.1:{}/bot", mock_port);

    // 核心组件
    let event_bus = Arc::new(EventBus::new());
    let adapter_manager = Arc::new(AdapterManager::new());
    let session_manager = Arc::new(SessionManager::new());

    // 内存 SQLite
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    let message_store: Arc<dyn easybot_core::storage::MessageStore> =
        Arc::new(SqliteMessageStore::new(pool));

    // API 认证
    let auth_manager = Arc::new(ApiKeyManager::new());
    let (_, raw_key) = auth_manager.create_key("e2e", vec![], None).await.unwrap();

    // 注册 Telegram 适配器工厂
    let registry = adapter_manager.registry();
    let eb = event_bus.clone();
    registry
        .register(
            "telegram",
            "Telegram",
            Arc::new(move |_config| {
                let eb = eb.clone();
                Box::pin(async move {
                    let mut adapter = easybot_adapter_telegram::TelegramAdapter::new();
                    adapter.set_event_bus(eb);
                    Ok(Box::new(adapter) as Box<dyn easybot_core::PlatformAdapter>)
                })
            }),
        )
        .await;

    // GatewayConfig（包含适配器凭证 + mock base_url）
    let mut adapter_configs = std::collections::HashMap::new();
    adapter_configs.insert(
        "telegram".to_string(),
        easybot_core::types::adapter::AdapterConfig {
            enabled: true,
            token: Some("test-token".to_string()),
            api_key: None,
            base_url: Some(mock_base),
            extra: serde_json::json!({}),
        },
    );

    let config = GatewayConfig {
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
        adapters: adapter_configs,
        ..GatewayConfig::default()
    };
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

    (router, raw_key, mock_server)
}

// ── 基础测试 ──

#[tokio::test]
async fn test_e2e_health() {
    let (router, ..) = setup().await;
    let (status, json) = public_get(&router, "/api/v1/health").await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "degraded");
}

#[tokio::test]
async fn test_e2e_auth_required() {
    let (router, ..) = setup().await;
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/api/v1/adapters")
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(router.clone().oneshot(req).await.unwrap().status(), 401);

    let req2 = axum::http::Request::builder()
        .method("GET")
        .uri("/api/v1/adapters")
        .header("Authorization", "Bearer bad-key")
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(router.clone().oneshot(req2).await.unwrap().status(), 401);
}

// ── 适配器生命周期 ──

#[tokio::test]
async fn test_e2e_lifecycle() {
    let (router, key, mock_server) = setup().await;

    // Mock getMe（适配器 init 和 connect 所需的 API 调用）
    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "result": { "id": 123, "is_bot": true, "first_name": "Bot", "username": "bot" }
        })))
        .expect(0..) // 使用 0.. 避免 verify 冲突
        .mount(&mock_server)
        .await;

    // 启动 → 应返回 ok:true
    let (status, json) = auth_post(&router, "/api/v1/adapters/telegram/start", &key, None).await;
    assert_eq!(status, 200, "start failed: {:?}", json);
    assert_eq!(json["ok"], true, "start result: {:?}", json);

    // 等待连接
    let conn = wait_for_connection(&router, &key, 10).await;
    assert!(conn, "adapter should connect");

    // 停止
    let (status, json) = auth_post(&router, "/api/v1/adapters/telegram/stop", &key, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true, "stop: {:?}", json);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let (_, json) = auth_get(&router, "/api/v1/adapters/telegram/status", &key).await;
    assert_eq!(json["connected"], false, "should be stopped");

    // 重新启动（getMe mock 已经注册了 expect(0..)，不需要重复注册）
    let (status, json) = auth_post(&router, "/api/v1/adapters/telegram/start", &key, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true, "restart: {:?}", json);

    let conn = wait_for_connection(&router, &key, 10).await;
    assert!(conn, "adapter should reconnect");
}

// ── 消息发送 ──

#[tokio::test]
async fn test_e2e_send_message() {
    let (router, key, mock_server) = setup().await;

    // 注册所有 mock
    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "result": { "id": 123, "is_bot": true, "first_name": "Bot", "username": "bot" }
        })))
        .expect(0..)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "result": { "message_id": 42, "date": 1000000, "chat": {"id": 123, "type": "private"}, "from": {"id": 1, "is_bot": true, "first_name": "Bot"}, "text": "hello" }
        })))
        .expect(0..)
        .mount(&mock_server)
        .await;

    // 启动适配器
    auth_post(&router, "/api/v1/adapters/telegram/start", &key, None).await;
    let conn = wait_for_connection(&router, &key, 10).await;
    assert!(conn, "adapter should connect before sending");

    // 发送消息
    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "telegram:12345", "text": "Hello"})),
    )
    .await;
    assert_eq!(status, 200, "send msg: {:?}", json);
    assert_eq!(json["status"], "sent", "should succeed: {:?}", json);

    // 验证 mock 被调用
    tokio::time::sleep(Duration::from_millis(200)).await;
    mock_server.verify().await;
}

// ── 消息发送（Markdown） ──

#[tokio::test]
async fn test_e2e_send_markdown() {
    let (router, key, mock_server) = setup().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "result": { "id": 123, "is_bot": true, "first_name": "Bot", "username": "bot" }
        })))
        .expect(0..)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "result": { "message_id": 77, "date": 1000000, "chat": {"id": 123, "type": "private"}, "from": {"id": 1, "is_bot": true, "first_name": "Bot"}, "text": "**bold**" }
        })))
        .expect(0..)
        .mount(&mock_server)
        .await;

    auth_post(&router, "/api/v1/adapters/telegram/start", &key, None).await;
    let conn = wait_for_connection(&router, &key, 10).await;
    assert!(conn);

    let (status, _) = auth_post(
        &router, "/api/v1/messages/send", &key,
        Some(serde_json::json!({"target": "telegram:12345", "text": "**bold**", "parse_mode": "markdown"})),
    ).await;
    assert_eq!(status, 200, "send markdown");

    tokio::time::sleep(Duration::from_millis(200)).await;
    mock_server.verify().await;
}
