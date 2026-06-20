//! Telegram 适配器端到端（E2E）集成测试
//!
//! 使用 wiremock 模拟 Telegram Bot API，通过 REST API 验证完整服务链路。

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use e2e_tests::{
    auth_get, auth_post, build_router, create_core, default_gateway_config, public_get,
    start_and_connect,
};
use easybot_core::types::adapter::AdapterConfig;
use serde_json::Value;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 每个测试独立的 setup：创建 wiremock + AppState + Router
async fn setup() -> (Router, String, MockServer) {
    let mock_server = MockServer::start().await;
    let mock_port = mock_server.address().port();
    let mock_base = format!("http://127.0.0.1:{}/bot", mock_port);

    let (event_bus, adapter_manager, session_manager, message_store) = create_core().await;

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
            &["TELEGRAM_BOT_TOKEN"],
        )
        .await;

    let mut config = default_gateway_config();
    config.adapters = {
        let mut m = HashMap::new();
        m.insert(
            "telegram".to_string(),
            AdapterConfig {
                enabled: Some(true),
                token: Some("test-token".to_string()),
                api_key: None,
                base_url: Some(mock_base),
                extra: serde_json::json!({}),
            },
        );
        m
    };

    let (router, key) = build_router(
        event_bus,
        adapter_manager,
        session_manager,
        message_store,
        config,
    )
    .await;

    (router, key, mock_server)
}

fn get_me_response() -> Value {
    serde_json::json!({"ok": true, "result": {"id": 123, "is_bot": true, "first_name": "Bot", "username": "bot"}})
}

fn send_message_response() -> Value {
    serde_json::json!({"ok": true, "result": {"message_id": 42, "date": 1000000, "chat": {"id": 123, "type": "private"}, "from": {"id": 1, "is_bot": true, "first_name": "Bot"}, "text": "hello"}})
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

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(get_me_response()))
        .expect(0..)
        .mount(&mock_server)
        .await;

    // 启动 + 连接
    let conn = start_and_connect(&router, &key, "telegram").await;
    assert!(conn, "adapter should connect");

    // 停止
    let (status, json) = auth_post(&router, "/api/v1/adapters/telegram/stop", &key, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);

    let (_, json) = auth_get(&router, "/api/v1/adapters/telegram/status", &key).await;
    assert_eq!(json["connected"], false);

    // 重新启动
    let _conn = start_and_connect(&router, &key, "telegram").await;
    assert!(conn, "adapter should reconnect");
}

// ── 消息发送 ──

#[tokio::test]
async fn test_e2e_send_message() {
    let (router, key, mock_server) = setup().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(get_me_response()))
        .expect(0..)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_message_response()))
        .expect(0..)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "telegram").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "telegram:12345", "text": "Hello"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
}

#[tokio::test]
async fn test_e2e_send_markdown() {
    let (router, key, mock_server) = setup().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(get_me_response()))
        .expect(0..)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_message_response()))
        .expect(0..)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "telegram").await);

    let (status, _) = auth_post(
        &router, "/api/v1/messages/send", &key,
        Some(serde_json::json!({"target": "telegram:12345", "text": "**bold**", "parse_mode": "markdown"})),
    ).await;
    assert_eq!(status, 200);
}
