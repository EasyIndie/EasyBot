//! Discord 适配器 send() 的 HTTP mock 测试
//!
//! 使用 wiremock 模拟 Discord REST API，验证 send() 方法正确构造请求并解析响应。

use easybot_core::types::adapter::{AdapterConfig, PlatformAdapter};
use easybot_core::types::message::{SendTextParams, OutboundMessage, ParseMode};
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
