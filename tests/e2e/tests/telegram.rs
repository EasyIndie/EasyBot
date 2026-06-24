//! Telegram 适配器端到端（E2E）集成测试
//!
//! 使用 wiremock 模拟 Telegram Bot API，通过 REST API 验证完整服务链路。

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use e2e_tests::{
    auth_delete, auth_get, auth_post, auth_put, build_router, create_core, default_gateway_config,
    public_get, start_and_connect,
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
                    let boxed: Box<dyn easybot_core::PlatformAdapter> = Box::new(adapter);
                    Ok(boxed)
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

fn send_photo_response() -> Value {
    serde_json::json!({"ok": true, "result": {"message_id": 100, "date": 1000003, "chat": {"id": 123, "type": "private"}, "from": {"id": 1, "is_bot": true, "first_name": "Bot"}, "photo": [{"file_id": "abc123", "width": 100, "height": 100}]}})
}

fn edit_message_response() -> Value {
    serde_json::json!({"ok": true, "result": {"message_id": 42, "date": 1000004, "chat": {"id": 123, "type": "private"}, "from": {"id": 1, "is_bot": true, "first_name": "Bot"}, "text": "Updated content"}})
}

fn delete_message_response() -> Value {
    serde_json::json!({"ok": true, "result": true})
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
        .expect(2) // lifecycle 测试: 启动 + 停止后重新启动，共两次握手
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
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_message_response()))
        .expect(1)
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
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_message_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "telegram").await);

    let (status, _) = auth_post(
        &router, "/api/v1/messages/send", &key,
        Some(serde_json::json!({"target": "telegram:12345", "text": "**bold**", "parse_mode": "markdown"})),
    ).await;
    assert_eq!(status, 200);
}

// ── 媒体消息 ──

#[tokio::test]
async fn test_e2e_send_media() {
    let (router, key, mock_server) = setup().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(get_me_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendPhoto"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_photo_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "telegram").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({
            "target": "telegram:12345",
            "text": "Check this out",
            "media": {
                "media_type": "Image",
                "url": "https://example.com/photo.jpg",
                "mime_type": "image/jpeg",
                "filename": "photo.jpg"
            }
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
    assert_eq!(json["messageId"], "100");
}

// ── 交互式消息 ──

#[tokio::test]
async fn test_e2e_send_interactive() {
    let (router, key, mock_server) = setup().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(get_me_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_message_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "telegram").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({
            "target": "telegram:12345",
            "text": "Choose an option:",
            "keyboard": {
                "rows": [
                    {
                        "buttons": [
                            {"text": "Yes", "callback_data": "yes"},
                            {"text": "No", "callback_data": "no"}
                        ]
                    },
                    {
                        "buttons": [
                            {"text": "Website", "url": "https://example.com"}
                        ]
                    }
                ]
            }
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
    assert_eq!(json["messageId"], "42");
}

// ── 编辑消息 ──

#[tokio::test]
async fn test_e2e_edit_message() {
    let (router, key, mock_server) = setup().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(get_me_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/editMessageText"))
        .respond_with(ResponseTemplate::new(200).set_body_json(edit_message_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "telegram").await);

    let (status, json) = auth_put(
        &router,
        "/api/v1/messages/msg-to-edit",
        &key,
        Some(serde_json::json!({
            "target": "telegram:12345",
            "text": "Updated content"
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);
    assert!(json["updated_at"].is_number());
}

// ── 删除消息 ──

#[tokio::test]
async fn test_e2e_delete_message() {
    let (router, key, mock_server) = setup().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(get_me_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/deleteMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(delete_message_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "telegram").await);

    let (status, json) = auth_delete(
        &router,
        "/api/v1/messages/msg-to-delete",
        &key,
        Some(serde_json::json!({
            "target": "telegram:12345"
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);
}

// ── 批量发送 ──

#[tokio::test]
async fn test_e2e_batch_send() {
    let (router, key, mock_server) = setup().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(get_me_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_message_response()))
        .expect(2) // batch send 到 2 个 target，各一次 sendMessage
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "telegram").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/batch-send",
        &key,
        Some(serde_json::json!({
            "targets": ["telegram:12345", "telegram:67890"],
            "text": "Broadcast message"
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["total"], 2);
    assert_eq!(json["results"]["telegram:12345"]["status"], "sent");
    assert_eq!(json["results"]["telegram:67890"]["status"], "sent");
}
