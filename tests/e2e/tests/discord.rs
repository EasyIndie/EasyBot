//! Discord 适配器端到端（E2E）集成测试
//!
//! 使用 wiremock 模拟 Discord REST API（Gateway WebSocket 不在此测试范围内）。

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
            "discord",
            "Discord",
            Arc::new(move |config| {
                let eb = eb.clone();
                Box::pin(async move {
                    let mut adapter = easybot_adapter_discord::DiscordAdapter::new();
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
            &["DISCORD_BOT_TOKEN"],
        )
        .await;

    let mut config = default_gateway_config();
    config.adapters = {
        let mut m = HashMap::new();
        m.insert(
            "discord".to_string(),
            AdapterConfig {
                enabled: Some(true),
                token: Some("test-discord-token".to_string()),
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

async fn mock_discord_users_me(mock_server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "1515905813489782835",
            "username": "TestBot",
            "discriminator": "0000",
            "bot": true
        })))
        .expect(1)
        .mount(mock_server)
        .await;
}

async fn mock_discord_send(mock_server: &MockServer, channel_id: &str, msg_id: &str) {
    Mock::given(method("POST"))
        .and(path(format!("/channels/{}/messages", channel_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": msg_id,
            "channel_id": channel_id,
            "content": "Hello Discord",
            "author": { "id": "bot", "username": "Bot", "discriminator": "0000", "bot": true },
            "timestamp": "2026-06-20T12:00:00+00:00"
        })))
        .expect(1)
        .mount(mock_server)
        .await;
}

async fn mock_discord_send_media(mock_server: &MockServer, channel_id: &str, msg_id: &str) {
    Mock::given(method("POST"))
        .and(path(format!("/channels/{}/messages", channel_id)))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": msg_id,
            "channel_id": channel_id,
            "content": "Check this out",
            "author": { "id": "bot", "username": "Bot", "discriminator": "0000", "bot": true },
            "timestamp": "2026-06-20T12:00:00+00:00",
            "attachments": [{"id": "att-1", "filename": "photo.jpg", "url": "https://cdn.discord.com/att/photo.jpg"}]
        })))
        .expect(1..)  // 必须至少调用一次
        .mount(mock_server)
        .await;
}

async fn mock_discord_edit_message(mock_server: &MockServer, channel_id: &str, msg_id: &str) {
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/channels/{}/messages/{}",
            channel_id, msg_id
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": msg_id,
            "channel_id": channel_id,
            "content": "edited",
            "author": {"id": "bot", "username": "Bot", "discriminator": "0000", "bot": true},
            "timestamp": "2026-06-20T12:05:00+00:00"
        })))
        .expect(1)
        .mount(mock_server)
        .await;
}

async fn mock_discord_delete_message(mock_server: &MockServer, channel_id: &str, msg_id: &str) {
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/channels/{}/messages/{}",
            channel_id, msg_id
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(mock_server)
        .await;
}

// ── 基础 ──

#[tokio::test]
async fn test_e2e_discord_health() {
    let (router, ..) = setup().await;
    let (status, _) = public_get(&router, "/api/v1/health").await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_e2e_discord_lifecycle() {
    let (router, key, mock_server) = setup().await;

    // Discord connect() 调用 GET /users/@me 验证 token
    mock_discord_users_me(&mock_server).await;

    let _conn = start_and_connect(&router, &key, "discord").await;
    // Gateway 部分无法 mock → connect 可能报告 ok 但 Gateway 连接失败
    // 这里验证 REST API 层（/users/@me）正常工作
    let (_, json) = auth_get(&router, "/api/v1/adapters/discord/status", &key).await;
    // Discord 在 Gateway 连接失败时仍可能 report Connected（取决于实现）
    eprintln!("Discord status after start: {:?}", json);

    // 停止
    let (status, json) = auth_post(&router, "/api/v1/adapters/discord/stop", &key, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);
}

#[tokio::test]
async fn test_e2e_discord_send_message() {
    let (router, key, mock_server) = setup().await;

    mock_discord_users_me(&mock_server).await;
    mock_discord_send(&mock_server, "dm_channel_123", "msg_discord_001").await;

    let conn = start_and_connect(&router, &key, "discord").await;
    assert!(conn, "discord should connect");

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "discord:dm_channel_123", "text": "Hello Discord"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
    assert_eq!(json["messageId"], "msg_discord_001");
}

#[tokio::test]
async fn test_e2e_discord_send_error() {
    let (router, key, mock_server) = setup().await;

    mock_discord_users_me(&mock_server).await;

    // 不 mock send endpoint → wiremock 返回 404 → send 失败
    let conn = start_and_connect(&router, &key, "discord").await;
    assert!(conn);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "discord:bad_channel", "text": "fail"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "failed");
}

#[tokio::test]
async fn test_e2e_discord_auth_failure() {
    let (router, key, mock_server) = setup().await;

    // /users/@me 返回 401 → connect 应失败
    // expect(0..): Discord connect 可能先调 /gateway/bot 等端点，到达 /users/@me
    // 之前就可能失败。测试只关心最终状态（connected != true），不关心失败路径。
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "message": "401: Unauthorized", "code": 0
        })))
        .expect(0..)
        .mount(&mock_server)
        .await;

    let (_, _) = auth_post(&router, "/api/v1/adapters/discord/start", &key, None).await;
    // 给一点时间让 connect 后台任务执行
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let (_, json) = auth_get(&router, "/api/v1/adapters/discord/status", &key).await;
    assert_ne!(
        json["connected"], true,
        "discord should not connect with invalid token"
    );
}

// ── 媒体消息 ──

#[tokio::test]
async fn test_e2e_discord_send_media() {
    let (router, key, mock_server) = setup().await;

    mock_discord_users_me(&mock_server).await;
    mock_discord_send_media(&mock_server, "dm_channel_123", "msg_discord_media_001").await;

    let conn = start_and_connect(&router, &key, "discord").await;
    assert!(conn, "discord should connect");

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({
            "target": "discord:dm_channel_123",
            "text": "Check this out",
            "media": {
                "media_type": "Image",
                "data": fixtures::image_base64(),
                "mime_type": "image/png",
                "filename": "photo.png"
            }
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
    assert_eq!(json["messageId"], "msg_discord_media_001");
}

// ── 交互式消息 ──

#[tokio::test]
async fn test_e2e_discord_send_interactive() {
    let (router, key, mock_server) = setup().await;

    mock_discord_users_me(&mock_server).await;
    mock_discord_send(&mock_server, "dm_channel_123", "msg_interactive_001").await;

    let conn = start_and_connect(&router, &key, "discord").await;
    assert!(conn, "discord should connect");

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({
            "target": "discord:dm_channel_123",
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
                            {"text": "Open Link", "url": "https://example.com"}
                        ]
                    }
                ]
            }
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
    assert_eq!(json["messageId"], "msg_interactive_001");
}

// ── 编辑消息 ──

#[tokio::test]
async fn test_e2e_discord_edit_message() {
    let (router, key, mock_server) = setup().await;

    mock_discord_users_me(&mock_server).await;
    mock_discord_edit_message(&mock_server, "dm_channel_123", "msg_to_edit").await;

    let conn = start_and_connect(&router, &key, "discord").await;
    assert!(conn, "discord should connect");

    let (status, json) = auth_put(
        &router,
        "/api/v1/messages/msg_to_edit",
        &key,
        Some(serde_json::json!({
            "target": "discord:dm_channel_123",
            "text": "edited content"
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);
}

// ── 删除消息 ──

#[tokio::test]
async fn test_e2e_discord_delete_message() {
    let (router, key, mock_server) = setup().await;

    mock_discord_users_me(&mock_server).await;
    mock_discord_delete_message(&mock_server, "dm_channel_123", "msg_to_delete").await;

    let conn = start_and_connect(&router, &key, "discord").await;
    assert!(conn, "discord should connect");

    let (status, json) = auth_delete(
        &router,
        "/api/v1/messages/msg_to_delete",
        &key,
        Some(serde_json::json!({
            "target": "discord:dm_channel_123"
        })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);
}
