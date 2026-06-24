//! QQ 适配器 send() 的 HTTP mock 测试
//!
//! 使用 wiremock 模拟 QQ API，验证 connect() 和 send() 方法正确构造请求并解析响应。
//! QQ 适配器有两个独立的 API 端点：
//!   - 鉴权端点: {auth_base_url}/app/getAppAccessToken（默认 https://bots.qq.com）
//!   - 业务端点: {base_url}/...（默认 https://api.sgroup.qq.com）
//!     测试中通过 config.base_url 和 config.extra["auth_base_url"] 将两者指向 wiremock。

use easybot_core::types::adapter::{AdapterConfig, AdapterState, PlatformAdapter};
use easybot_core::types::message::{
    Button, InlineKeyboard, KeyboardRow, MediaAttachment, MediaType, OutboundMessage, ParseMode,
    SendInteractiveParams, SendMediaParams, SendTextParams,
};
use std::sync::Arc;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn send_text_params() -> SendTextParams {
    SendTextParams {
        chat_id: "qq-chat-123".to_string(),
        message: OutboundMessage {
            text: "Hello QQ".to_string(),
            parse_mode: ParseMode::None,
        },
        reply_to: None,
        metadata: None,
    }
}

/// 构建指向 wiremock 服务器的 QQ 适配器配置。
/// 同时设置 base_url 和 extra.auth_base_url，使鉴权和业务 API 都走 mock。
fn qq_config_with_auth(mock_port: u16) -> AdapterConfig {
    let base_url = format!("http://127.0.0.1:{}", mock_port);
    AdapterConfig {
        enabled: Some(true),
        token: Some("test-client-secret".into()),
        api_key: None,
        base_url: Some(base_url.clone()),
        extra: serde_json::json!({
            "app_id": "test-app-id",
            "auth_base_url": base_url,
        }),
    }
}

/// Mock QQ 鉴权端点：POST /app/getAppAccessToken
async fn mock_qq_token(mock_server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/app/getAppAccessToken"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "mock-access-token-12345",
            "expires_in": 7200
        })))
        .expect(1..)
        .mount(mock_server)
        .await;
}

/// Mock QQ bot 信息端点：GET /users/@me（在 connect() 中调用）
async fn mock_qq_bot_info(mock_server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "bot_001",
            "username": "TestBot",
            "bot": true
        })))
        .expect(1..)
        .mount(mock_server)
        .await;
}

// ── 前置条件测试（send() 在连接前应优雅失败） ──

#[tokio::test]
async fn test_send_before_connect_returns_error() {
    // QQ 的 send() 需要 token_store（connect() 时创建），
    // send() 前未连接应返回错误
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("test-secret".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({"app_id": "test-app"}),
    };

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter.init(config).await.unwrap();

    let result = adapter.send(send_text_params()).await.unwrap();
    assert!(!result.success, "send before connect should fail");
    assert!(
        result.retryable,
        "should be retryable as token store is missing"
    );
    if let Some(ref err) = result.error {
        assert!(
            err.contains("token") || err.contains("init") || err.contains("connect"),
            "error should mention missing token/init/connect, got: {}",
            err
        );
    }
}

#[tokio::test]
async fn test_init_requires_app_id_and_token() {
    // 缺少 app_id
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("secret".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(!result.ok, "init should fail without app_id");

    // 缺少 token
    let config = AdapterConfig {
        enabled: Some(true),
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({"app_id": "test-app"}),
    };
    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(!result.ok, "init should fail without token");
}

// ── connect 成功路径（mock token + bot info） ──

#[tokio::test]
async fn test_connect_success() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;
    // event_bus 为 None，gateway_loop 不会启动，无需 mock gateway

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    let config = qq_config_with_auth(mock_server.address().port());
    adapter.init(config).await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Starting);

    let result = adapter.connect().await.unwrap();
    assert!(
        result.ok,
        "connect should succeed with mocked token and bot info"
    );
    assert_eq!(adapter.state(), AdapterState::Connected);
    assert!(result.bot_info.is_some());
    assert_eq!(result.bot_info.as_ref().unwrap().id, "bot_001");
}

#[tokio::test]
async fn test_token_refresh_failure() {
    let mock_server = MockServer::start().await;

    // Token 端点返回不含 access_token 的响应
    Mock::given(method("POST"))
        .and(path("/app/getAppAccessToken"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error": "invalid_client_secret",
            "error_description": "client_secret is invalid"
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();

    let result = adapter.connect().await.unwrap();
    assert!(!result.ok, "connect should fail when token refresh fails");
    assert!(
        result
            .error
            .unwrap_or_default()
            .contains("missing access_token"),
        "error should mention missing access_token"
    );
    assert_eq!(adapter.state(), AdapterState::Starting);
}

// ── send 成功路径 ──

#[tokio::test]
async fn test_send_message_success() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    // Mock 频道消息发送端点 (try_send 优先尝试 /channels/{id}/messages)
    Mock::given(method("POST"))
        .and(path("/channels/qq-chat-123/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_abc123",
            "timestamp": "2026-06-20T12:00:00+00:00"
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter.send(send_text_params()).await.unwrap();
    assert!(result.success);
    assert_eq!(result.message_id, Some("msg_abc123".to_string()));
    assert_eq!(adapter.state(), AdapterState::Connected);
}

#[tokio::test]
async fn test_send_http_error() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    // API 返回 500
    Mock::given(method("POST"))
        .and(path("/channels/qq-chat-123/messages"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter.send(send_text_params()).await.unwrap();
    assert!(!result.success, "send should fail with HTTP 500");
    assert!(result.retryable, "should be retryable");
}

#[tokio::test]
async fn test_send_malformed_response() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/channels/qq-chat-123/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("not-valid-json{{{")
                .insert_header("Content-Type", "text/plain"),
        )
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter.send(send_text_params()).await.unwrap();
    assert!(!result.success, "send should fail with malformed response");
}

// ── send_media() ──

#[tokio::test]
async fn test_send_media_before_connect_fails() {
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("test-secret".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({"app_id": "test-app"}),
    };
    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter.init(config).await.unwrap();

    let result = adapter
        .send_media(SendMediaParams {
            chat_id: "qq-chat-123".to_string(),
            media: MediaAttachment {
                media_type: MediaType::Image,
                url: Some("https://example.com/image.png".to_string()),
                data: None,
                mime_type: "image/png".to_string(),
                filename: None,
                caption: None,
                thumbnail_url: None,
                file_size: None,
                duration: None,
            },
            text: Some("image caption".to_string()),
            reply_to: None,
        })
        .await
        .unwrap();

    assert!(!result.success, "send_media before connect should fail");
    assert!(result.retryable, "should be retryable");
}

// ── connect() 状态测试 ──

#[tokio::test]
async fn test_connect_failure_state() {
    // QQ connect() 需要 access token refresh（硬编码的 bots.qq.com 端点）
    // 该请求必然失败，connect() 应该返回 ConnectResult{ok:false} 或 Err
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("test-secret".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({"app_id": "test-app"}),
    };
    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter.init(config).await.unwrap();

    // 状态应为 Starting（QQ init 设置 Starting）
    assert_eq!(adapter.state(), AdapterState::Starting);

    let _result = adapter.connect().await;
    // connect 可能返回 Ok(ConnectResult{ok:false}) 或 Err(GatewayError)
    // 但无论如何状态不应为 Connected
    assert_ne!(
        adapter.state(),
        AdapterState::Connected,
        "QQ should not be connected without real token"
    );
}

#[tokio::test]
async fn test_connect_disconnect_state_cycle() {
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("test-secret".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({"app_id": "test-app"}),
    };
    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter.init(config).await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Starting);

    // disconnect from Starting should be valid
    let _ = adapter.disconnect().await;
    assert_eq!(adapter.state(), AdapterState::Stopped);

    // 重复 disconnect 应幂等
    let _ = adapter.disconnect().await;
    assert_eq!(adapter.state(), AdapterState::Stopped);
}

// ── P2-9: send_media 成功 ──

#[tokio::test]
async fn test_send_media_image_success() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    // Mock 频道消息发送端点（send_media 用 msg_type: 2 + image URL）
    Mock::given(method("POST"))
        .and(path("/channels/qq-chat-123/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "media_msg_001",
            "timestamp": "2026-06-20T12:00:00+00:00"
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter
        .send_media(SendMediaParams {
            chat_id: "qq-chat-123".to_string(),
            media: MediaAttachment {
                media_type: MediaType::Image,
                url: Some("https://example.com/photo.jpg".to_string()),
                data: None,
                mime_type: "image/jpeg".to_string(),
                filename: Some("photo.jpg".to_string()),
                caption: None,
                thumbnail_url: None,
                file_size: None,
                duration: None,
            },
            text: Some("Check this out".to_string()),
            reply_to: None,
        })
        .await
        .unwrap();

    assert!(
        result.success,
        "send_media should succeed: {:?}",
        result.error
    );
    assert_eq!(result.message_id, Some("media_msg_001".to_string()));
    assert_eq!(adapter.state(), AdapterState::Connected);
}

// ── P2-9: send_media 错误 ──

#[tokio::test]
async fn test_send_media_error() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    // API 返回 500
    Mock::given(method("POST"))
        .and(path("/channels/qq-chat-123/messages"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter
        .send_media(SendMediaParams {
            chat_id: "qq-chat-123".to_string(),
            media: MediaAttachment {
                media_type: MediaType::Image,
                url: Some("https://example.com/photo.jpg".to_string()),
                data: None,
                mime_type: "image/jpeg".to_string(),
                filename: None,
                caption: None,
                thumbnail_url: None,
                file_size: None,
                duration: None,
            },
            text: None,
            reply_to: None,
        })
        .await
        .unwrap();

    assert!(!result.success, "send_media should fail with HTTP 500");
    assert!(result.retryable, "should be retryable");
}

// ── P2-9: send_interactive 成功 ──

#[tokio::test]
async fn test_send_interactive_success() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/channels/qq-chat-123/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "interactive_001",
            "timestamp": "2026-06-20T12:00:00+00:00"
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter
        .send_interactive(SendInteractiveParams {
            chat_id: "qq-chat-123".to_string(),
            text: "Choose:".to_string(),
            keyboard: InlineKeyboard {
                rows: vec![KeyboardRow {
                    buttons: vec![Button {
                        text: "Click me".to_string(),
                        callback_data: Some("cb_1".to_string()),
                        url: None,
                    }],
                }],
            },
            reply_to: None,
        })
        .await
        .unwrap();

    assert!(
        result.success,
        "send_interactive should succeed: {:?}",
        result.error
    );
    assert_eq!(result.message_id, Some("interactive_001".to_string()));
}

// ── P2-9: edit_message 成功 ──

#[tokio::test]
async fn test_edit_message_success() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    // QQ edit_message uses PATCH /channels/{id}/messages/{msg_id}
    Mock::given(method("PATCH"))
        .and(path("/channels/qq-chat-123/messages/msg_to_edit"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg_to_edit",
            "timestamp": "2026-06-20T12:01:00+00:00"
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter
        .edit_message(easybot_core::types::message::EditMessageParams {
            chat_id: "qq-chat-123".to_string(),
            message_id: "msg_to_edit".to_string(),
            message: OutboundMessage {
                text: "Updated text".to_string(),
                parse_mode: ParseMode::None,
            },
            keyboard: None,
        })
        .await
        .unwrap();

    assert!(
        result.success,
        "edit_message should succeed: {:?}",
        result.error
    );
    assert!(
        result.updated_at.is_some(),
        "should have updated_at timestamp"
    );
}

// ── P2-9: delete_message 成功 ──

#[tokio::test]
async fn test_delete_message_success() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    // QQ delete_message uses DELETE /channels/{id}/messages/{msg_id}
    Mock::given(method("DELETE"))
        .and(path("/channels/qq-chat-123/messages/msg_to_delete"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter
        .delete_message("qq-chat-123", "msg_to_delete")
        .await
        .unwrap();

    assert!(
        result.success,
        "delete_message should succeed: {:?}",
        result.error
    );
}

// ── P2-9: send_media 请求体验证 ──

#[tokio::test]
async fn test_send_media_request_body() {
    let mock_server = MockServer::start().await;
    mock_qq_token(&mock_server).await;
    mock_qq_bot_info(&mock_server).await;

    let captured_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let captured = captured_body.clone();

    Mock::given(method("POST"))
        .and(path("/channels/qq-chat-123/messages"))
        .and(move |req: &wiremock::Request| {
            if let Ok(body) = serde_json::from_slice::<serde_json::Value>(&req.body) {
                *captured.lock().unwrap() = Some(body);
            }
            true
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "media_body_test",
            "timestamp": "2026-06-20T12:00:00+00:00"
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = easybot_adapter_qq::QqAdapter::new();
    adapter
        .init(qq_config_with_auth(mock_server.address().port()))
        .await
        .unwrap();
    adapter.connect().await.unwrap();

    let result = adapter
        .send_media(SendMediaParams {
            chat_id: "qq-chat-123".to_string(),
            media: MediaAttachment {
                media_type: MediaType::Image,
                url: Some("https://example.com/cat.png".to_string()),
                data: None,
                mime_type: "image/png".to_string(),
                filename: Some("cat.png".to_string()),
                caption: None,
                thumbnail_url: None,
                file_size: None,
                duration: None,
            },
            text: Some("Look at this cat".to_string()),
            reply_to: None,
        })
        .await
        .unwrap();

    assert!(result.success);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let body = captured_body.lock().unwrap().take().unwrap();
    assert_eq!(body["msg_type"], 2, "should use msg_type:2 for image+text");
    assert_eq!(body["image"], "https://example.com/cat.png");
    assert_eq!(body["content"], "Look at this cat");
}
