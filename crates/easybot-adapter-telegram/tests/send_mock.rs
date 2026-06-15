//! Telegram 适配器 send() 的 HTTP mock 测试
//!
//! 使用 wiremock 模拟 Telegram Bot API，验证 send() 方法正确构造请求并解析响应。

use std::sync::Arc;
use std::time::Duration;

use easybot_core::types::adapter::{AdapterConfig, PlatformAdapter};
use easybot_core::types::message::{SendTextParams, OutboundMessage, ParseMode};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

/// 构建测试用的 Telegram 适配器
async fn make_adapter(mock_port: u16) -> impl PlatformAdapter {
    let base_url = format!("http://127.0.0.1:{}/bot", mock_port);
    let config = AdapterConfig {
        enabled: true,
        token: Some("test-token".into()),
        api_key: None,
        base_url: Some(base_url),
        extra: serde_json::json!({}),
    };

    let mut adapter = easybot_adapter_telegram::TelegramAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(result.ok, "init should succeed");
    adapter
}

fn send_text_params() -> SendTextParams {
    SendTextParams {
        chat_id: "12345".to_string(),
        message: OutboundMessage {
            text: "Hello from test".to_string(),
            parse_mode: ParseMode::None,
        },
        reply_to: None,
        metadata: None,
    }
}

// ── 成功路径 ──

fn success_response() -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "result": {
            "message_id": 12345,
            "date": 1000000,
            "chat": {"id": 12345, "type": "private"},
            "from": {"id": 1, "is_bot": true, "first_name": "TestBot"},
            "text": "Hello from test"
        }
    })
}

#[tokio::test]
async fn test_send_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(success_response()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(result.success, "send should succeed");
    assert_eq!(result.message_id, Some("12345".to_string()), "message_id should match");
    assert!(result.timestamp.is_some(), "timestamp should be present");

    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_uses_correct_chat_id() {
    let mock_server = MockServer::start().await;

    // 捕获请求体以验证 chat_id
    let captured_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let captured = captured_body.clone();

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .and(move |req: &wiremock::Request| {
            if let Ok(body) = serde_json::from_slice::<serde_json::Value>(&req.body) {
                *captured.lock().unwrap() = Some(body);
            }
            true
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(success_response()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    adapter.send(send_text_params()).await.unwrap();

    // 验证请求体包含正确的参数
    tokio::time::sleep(Duration::from_millis(100)).await;
    let body = captured_body.lock().unwrap().take().unwrap();
    assert_eq!(body["chat_id"], "12345");
    assert_eq!(body["text"], "Hello from test");
}

// ── 错误路径 ──

#[tokio::test]
async fn test_send_telegram_api_error() {
    let mock_server = MockServer::start().await;

    // Telegram API 返回 ok: false（如 bot 被 stop）
    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false,
                "description": "Forbidden: bot was blocked by the user",
                "error_code": 403
            }))
        )
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with API error");
}

#[tokio::test]
async fn test_send_http_error_401() {
    let mock_server = MockServer::start().await;

    // Telegram API 返回 401 Unauthorized
    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with HTTP 401");
}

#[tokio::test]
async fn test_send_http_error_429_rate_limit() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(429))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with HTTP 429");
    assert!(result.retryable, "rate limit errors should be retryable");
}

#[tokio::test]
async fn test_send_http_error_500() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with HTTP 500");
}

#[tokio::test]
async fn test_send_with_malformed_json_response() {
    let mock_server = MockServer::start().await;

    // 返回非 JSON 响应
    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("not json at all")
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
async fn test_init_rejects_empty_token() {
    let config = AdapterConfig {
        enabled: true,
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    let mut adapter = easybot_adapter_telegram::TelegramAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(!result.ok, "init should fail with empty token");
    assert!(result.error.is_some(), "should provide error message");
}
