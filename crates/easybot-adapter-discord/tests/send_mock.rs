//! Discord 适配器 send() 的 HTTP mock 测试
//!
//! 使用 wiremock 模拟 Discord REST API，验证 send() 方法正确构造请求并解析响应。

use easybot_core::types::adapter::{AdapterConfig, AdapterState, PlatformAdapter};
use easybot_core::types::message::{
    EditMessageParams, OutboundMessage, ParseMode, SendTextParams,
};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

/// 构建测试用的 Discord 适配器
async fn make_adapter(mock_port: u16) -> impl PlatformAdapter {
    let base_url = format!("http://127.0.0.1:{}", mock_port);
    let config = AdapterConfig {
        enabled: true,
        token: Some("test-token".into()),
        api_key: None,
        base_url: Some(base_url),
        extra: serde_json::json!({}),
    };

    let mut adapter = easybot_adapter_discord::DiscordAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(result.ok, "init should succeed");
    adapter
}

fn send_text_params() -> SendTextParams {
    SendTextParams {
        chat_id: "98765".to_string(),
        message: OutboundMessage {
            text: "Hello Discord".to_string(),
            parse_mode: ParseMode::None,
        },
        reply_to: None,
        metadata: None,
    }
}

fn success_response() -> serde_json::Value {
    serde_json::json!({
        "id": "12345",
        "channel_id": "98765",
        "content": "Hello Discord",
        "timestamp": "2024-01-15T10:30:00Z",
        "author": {
            "id": "1",
            "username": "TestBot",
            "global_name": "Test Bot",
            "bot": true
        }
    })
}

// ── 成功路径 ──

#[tokio::test]
async fn test_send_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/channels/98765/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(success_response()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(result.success, "send should succeed");
    assert_eq!(result.message_id, Some("12345".to_string()));

    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_uses_correct_channel() {
    let mock_server = MockServer::start().await;

    // 验证请求发送到正确的频道
    Mock::given(method("POST"))
        .and(path("/channels/98765/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(success_response()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();
    assert!(result.success);

    mock_server.verify().await;
}

// ── 错误路径 ──

#[tokio::test]
async fn test_send_http_401() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/channels/98765/messages"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with 401");
}

#[tokio::test]
async fn test_send_http_429_rate_limit() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/channels/98765/messages"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with 429");
    assert!(result.retryable, "rate limit should be retryable");
}

#[tokio::test]
async fn test_send_http_500() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/channels/98765/messages"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with 500");
}

#[tokio::test]
async fn test_send_malformed_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/channels/98765/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("not-json")
                .insert_header("Content-Type", "text/plain"),
        )
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with malformed response");
}

#[tokio::test]
async fn test_send_wrong_channel_returns_error() {
    let mock_server = MockServer::start().await;

    // 不匹配的 channel ID → mock 不会接收请求
    Mock::given(method("POST"))
        .and(path("/channels/99999/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(success_response()))
        .expect(0)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    // 发送到频道 98765，但 mock 只响应 99999
    let result = adapter.send(send_text_params()).await.unwrap();

    // 请求被发送到 /channels/98765/messages，但 mock 不匹配
    // Discord adapter 的 api_call 会收到 HTTP 404（wiremock 默认响应 404）
    assert!(!result.success, "send should fail when endpoint not found");
}

// ── connect() 测试 ──

#[tokio::test]
async fn test_connect_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "12345",
            "username": "TestBot",
            "global_name": "Test Bot",
            "bot": true
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.connect().await.unwrap();

    assert!(result.ok, "connect should succeed");
    assert_eq!(adapter.state(), AdapterState::Connected);
    assert!(adapter.is_connected());
    let bot = result.bot_info.expect("bot_info expected");
    assert_eq!(bot.name, "Test Bot", "global_name should be used as name");

    mock_server.verify().await;
}

#[tokio::test]
async fn test_connect_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.connect().await.unwrap();

    assert!(!result.ok, "connect should fail with HTTP error");
    assert!(result.error.is_some(), "should contain error message");
    assert_eq!(adapter.state(), AdapterState::Created, "state should remain Created");

    mock_server.verify().await;
}

#[tokio::test]
async fn test_connect_no_token_returns_config_error() {
    let config = AdapterConfig {
        enabled: true,
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    let mut adapter = easybot_adapter_discord::DiscordAdapter::new();
    // init without token should fail
    let init = adapter.init(config).await.unwrap();
    assert!(!init.ok, "init should fail without token");

    let result = adapter.connect().await;
    assert!(result.is_err(), "connect without init or token should error");
}

// ── edit_message() 测试 ──

#[tokio::test]
async fn test_edit_message_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/channels/98765/messages/msg-001"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "msg-001",
            "channel_id": "98765",
            "content": "edited",
            "timestamp": "2024-01-15T10:30:00Z",
            "author": {"id": "1", "username": "Bot", "bot": true}
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.edit_message(EditMessageParams {
        chat_id: "98765".to_string(),
        message_id: "msg-001".to_string(),
        message: OutboundMessage {
            text: "edited content".to_string(),
            parse_mode: ParseMode::None,
        },
        keyboard: None,
    }).await.unwrap();

    assert!(result.success, "edit should succeed");

    mock_server.verify().await;
}

#[tokio::test]
async fn test_edit_message_not_found() {
    let mock_server = MockServer::start().await;

    Mock::given(method("PATCH"))
        .and(path("/channels/98765/messages/nonexistent"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.edit_message(EditMessageParams {
        chat_id: "98765".to_string(),
        message_id: "nonexistent".to_string(),
        message: OutboundMessage {
            text: "edited".to_string(),
            parse_mode: ParseMode::None,
        },
        keyboard: None,
    }).await.unwrap();

    assert!(!result.success, "edit should fail for nonexistent message");

    mock_server.verify().await;
}

// ── delete_message() 测试 ──

#[tokio::test]
async fn test_delete_message_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/channels/98765/messages/msg-001"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.delete_message("98765", "msg-001").await.unwrap();

    assert!(result.success, "delete should succeed");

    mock_server.verify().await;
}

#[tokio::test]
async fn test_delete_message_not_found() {
    let mock_server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path("/channels/98765/messages/nonexistent"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.delete_message("98765", "nonexistent").await.unwrap();

    assert!(!result.success, "delete should fail for nonexistent message");

    mock_server.verify().await;
}
