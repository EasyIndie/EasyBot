//! QQ 适配器端到端（E2E）集成测试
//!
//! 使用 wiremock 模拟 QQ Bot API，验证 adapter lifecycle + message send。

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use e2e_tests::{
    auth_get, auth_post, build_router, create_core, default_gateway_config, public_get,
    start_and_connect,
};
use easybot_core::PlatformAdapter;
use easybot_core::types::adapter::AdapterConfig;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn setup() -> (Router, String, MockServer) {
    let mock_server = MockServer::start().await;
    let mock_port = mock_server.address().port();
    let mock_base = format!("http://127.0.0.1:{}", mock_port);

    let (event_bus, adapter_manager, session_manager, message_store) = create_core().await;

    let registry = adapter_manager.registry();
    let eb = event_bus.clone();
    registry
        .register(
            "qq",
            "QQ",
            Arc::new(move |config| {
                let eb = eb.clone();
                Box::pin(async move {
                    let mut adapter = easybot_adapter_qq::QqAdapter::new();
                    adapter.set_event_bus(eb);
                    let result = adapter
                        .init(config)
                        .await
                        .map_err(|e| format!("init: {}", e))?;
                    if !result.ok {
                        return Err(result.error.unwrap_or_default());
                    }
                    let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
                    Ok(boxed)
                })
            }),
            &["QQ_APP_ID", "QQ_CLIENT_SECRET"],
        )
        .await;

    let mut config = default_gateway_config();
    config.adapters = {
        let mut m = HashMap::new();
        m.insert(
            "qq".to_string(),
            AdapterConfig {
                enabled: Some(true),
                token: Some("test-client-secret".to_string()),
                api_key: None,
                base_url: Some(mock_base.clone()),
                extra: serde_json::json!({
                    "app_id": "test-app-id",
                    "auth_base_url": mock_base,
                }),
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

async fn mock_qq_token(mock_server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/app/getAppAccessToken"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "mock-qq-token",
            "expires_in": 7200
        })))
        .expect(1)
        .mount(mock_server)
        .await;
}

async fn mock_qq_bot_info(mock_server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "bot_001",
            "username": "QQBot",
            "bot": true
        })))
        .expect(1)
        .mount(mock_server)
        .await;
}

// ── 基础 ──

#[tokio::test]
async fn test_e2e_qq_health() {
    let (router, ..) = setup().await;
    let (status, _) = public_get(&router, "/api/v1/health").await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_e2e_qq_lifecycle() {
    let (router, key, mock_server) = setup().await;

    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    // QQ 适配器不通过 start_all 自动启动（需显式调用 adapter/qq/start）
    // connect 时：刷新 token → GET /users/@me → Connected
    let conn = start_and_connect(&router, &key, "qq").await;
    assert!(conn, "QQ adapter should connect with mocked API");

    // 验证状态
    let (_, json) = auth_get(&router, "/api/v1/adapters/qq/status", &key).await;
    assert_eq!(json["connected"], true);
    assert_eq!(json["state"], "Connected");

    // 停止
    let (status, json) = auth_post(&router, "/api/v1/adapters/qq/stop", &key, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);

    let (_, json) = auth_get(&router, "/api/v1/adapters/qq/status", &key).await;
    assert_eq!(json["connected"], false);
}

#[tokio::test]
async fn test_e2e_qq_send_message() {
    let (router, key, mock_server) = setup().await;

    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/channels/qq-test-123/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_qq_001",
            "timestamp": "2026-06-20T12:00:00+00:00"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "qq").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "qq:qq-test-123", "text": "Hello QQ"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
    assert_eq!(json["messageId"], "msg_qq_001");
}

#[tokio::test]
async fn test_e2e_qq_send_error() {
    let (router, key, mock_server) = setup().await;

    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    // 不 mock /channels 端点 → wiremock 默认返回 404 → try_send 依次尝试所有端点均失败
    assert!(start_and_connect(&router, &key, "qq").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "qq:bad-chat", "text": "should fail"})),
    )
    .await;
    // send 本身返回 200（内部错误编码在 status 字段）
    assert_eq!(status, 200);
    assert_eq!(json["status"], "failed");
}

#[tokio::test]
async fn test_e2e_qq_auth_failure() {
    let (router, key, mock_server) = setup().await;

    // Token 端点返回无效响应（无 access_token）
    Mock::given(method("POST"))
        .and(path("/app/getAppAccessToken"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error": "invalid_secret"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let (_status, json) = auth_post(&router, "/api/v1/adapters/qq/start", &key, None).await;
    // start 可能返回 200 但 ok=false，或 500
    let ok = json["ok"].as_bool().unwrap_or(true);
    if ok {
        // connect 应该失败 → 状态不是 Connected
        let (_, status_json) = auth_get(&router, "/api/v1/adapters/qq/status", &key).await;
        assert_ne!(status_json["connected"], true);
    }
}

// ── 媒体消息 ──

#[tokio::test]
async fn test_e2e_qq_send_media() {
    let (router, key, mock_server) = setup().await;

    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/channels/qq-test-123/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_qq_media_001",
            "timestamp": "2026-06-20T12:00:00+00:00"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "qq").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({
            "target": "qq:qq-test-123",
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
    assert_eq!(json["messageId"], "msg_qq_media_001");
}
