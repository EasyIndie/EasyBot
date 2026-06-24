//! 飞书适配器端到端（E2E）集成测试
//!
//! 使用 wiremock 模拟飞书开放平台 API。

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use e2e_tests::{
    auth_delete, auth_get, auth_post, auth_put, build_router, create_core, default_gateway_config,
    public_get, start_and_connect,
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
            "feishu",
            "飞书",
            Arc::new(move |config| {
                let eb = eb.clone();
                Box::pin(async move {
                    let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
                    adapter.set_event_bus(eb);
                    let result = adapter
                        .init(config)
                        .await
                        .map_err(|e| format!("init: {}", e))?;
                    if !result.ok {
                        return Err(result.error.unwrap_or_default());
                    }
                    Ok(Box::new(adapter) as Box<dyn easybot_core::PlatformAdapter>)
                })
            }),
            &["FEISHU_APP_ID", "FEISHU_APP_SECRET"],
        )
        .await;

    let mut config = default_gateway_config();
    config.adapters = {
        let mut m = HashMap::new();
        m.insert(
            "feishu".to_string(),
            AdapterConfig {
                enabled: Some(true),
                token: Some("test-app-secret".to_string()),
                api_key: None,
                base_url: Some(mock_base),
                extra: serde_json::json!({"app_id": "test-feishu-app"}),
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

async fn mock_feishu_token(mock_server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok",
            "tenant_access_token": "t-access-token-67890",
            "expire": 7200
        })))
        .expect(1..)
        .mount(mock_server)
        .await;
}

async fn mock_feishu_send(mock_server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": { "message_id": "om_feishu_001" }
        })))
        .expect(1)
        .mount(mock_server)
        .await;
}

async fn mock_feishu_edit_message(mock_server: &MockServer, msg_id: &str) {
    Mock::given(method("PUT"))
        .and(path(format!("/im/v1/messages/{}", msg_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": {}
        })))
        .expect(1)
        .mount(mock_server)
        .await;
}

// ── 基础 ──

#[tokio::test]
async fn test_e2e_feishu_health() {
    let (router, ..) = setup().await;
    let (status, _) = public_get(&router, "/api/v1/health").await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_e2e_feishu_lifecycle() {
    let (router, key, mock_server) = setup().await;

    mock_feishu_token(&mock_server).await;

    let conn = start_and_connect(&router, &key, "feishu").await;
    assert!(conn, "feishu should connect");

    let (_, json) = auth_get(&router, "/api/v1/adapters/feishu/status", &key).await;
    assert_eq!(json["connected"], true);

    // 停止
    let (status, json) = auth_post(&router, "/api/v1/adapters/feishu/stop", &key, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);

    let (_, json) = auth_get(&router, "/api/v1/adapters/feishu/status", &key).await;
    assert_eq!(json["connected"], false);
}

#[tokio::test]
async fn test_e2e_feishu_send_message() {
    let (router, key, mock_server) = setup().await;

    mock_feishu_token(&mock_server).await;

    // 飞书 send 路径: POST /im/v1/messages/{msg_id}/reply 或类似
    // 需要 mock send 端点
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": { "message_id": "om_feishu_001" }
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "feishu").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "feishu:oc_test123", "text": "Hello Feishu"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
}

#[tokio::test]
async fn test_e2e_feishu_send_error() {
    let (router, key, mock_server) = setup().await;

    mock_feishu_token(&mock_server).await;

    // API 返回错误 code
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 10001,
            "msg": "invalid chat_id"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "feishu").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "feishu:bad_chat", "text": "fail"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "failed");
}

#[tokio::test]
async fn test_e2e_feishu_auth_failure() {
    let (router, key, mock_server) = setup().await;

    // Token 端点返回错误
    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 99991663,
            "msg": "invalid app secret"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let (status, _) = auth_post(&router, "/api/v1/adapters/feishu/start", &key, None).await;
    // connect 应该失败，adapter 不应标记为 connected
    let (_, json) = auth_get(&router, "/api/v1/adapters/feishu/status", &key).await;
    assert!(
        !json["connected"].as_bool().unwrap_or(true),
        "auth 失败时 adapter 不应连接: {}",
        json
    );
    // API 应返回有效响应
    assert!(status.is_success() || status.is_server_error());
}

// ── 交互式消息 ──

#[tokio::test]
async fn test_e2e_feishu_send_interactive() {
    let (router, key, mock_server) = setup().await;

    mock_feishu_token(&mock_server).await;
    mock_feishu_send(&mock_server).await;

    assert!(start_and_connect(&router, &key, "feishu").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({
            "target": "feishu:oc_test123",
            "text": "Choose an option:",
            "keyboard": {
                "rows": [
                    {
                        "buttons": [
                            {"text": "Confirm", "callback_data": "confirm"},
                            {"text": "Cancel", "callback_data": "cancel"}
                        ]
                    }
                ]
            }
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
}

// ── 编辑消息 ──

#[tokio::test]
async fn test_e2e_feishu_edit_message() {
    let (router, key, mock_server) = setup().await;

    mock_feishu_token(&mock_server).await;
    mock_feishu_edit_message(&mock_server, "om_test_msg").await;

    assert!(start_and_connect(&router, &key, "feishu").await);

    let (status, json) = auth_put(
        &router,
        "/api/v1/messages/om_test_msg",
        &key,
        Some(serde_json::json!({
            "target": "feishu:oc_test123",
            "text": "edited content"
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);
}

// ── 删除消息 ──

async fn mock_feishu_delete_message(mock_server: &MockServer, msg_id: &str) {
    Mock::given(method("DELETE"))
        .and(path(format!("/im/v1/messages/{}", msg_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok"
        })))
        .expect(1)
        .mount(mock_server)
        .await;
}

#[tokio::test]
async fn test_e2e_feishu_delete_message() {
    let (router, key, mock_server) = setup().await;

    mock_feishu_token(&mock_server).await;
    mock_feishu_delete_message(&mock_server, "om_del_msg").await;

    assert!(start_and_connect(&router, &key, "feishu").await);

    let (status, json) = auth_delete(
        &router,
        "/api/v1/messages/om_del_msg",
        &key,
        Some(serde_json::json!({
            "target": "feishu:oc_test123"
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);
}
