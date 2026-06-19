//! 个人微信适配器 send() 的 HTTP mock 测试
//!
//! 使用 wiremock 模拟 iLink Bot API，验证 send() 方法正确构造请求并解析响应。
//! WeChat send() 从 config.extra 中读取 bot_token，无 token 刷新流程。

use easybot_core::types::adapter::{AdapterConfig, AdapterState, PlatformAdapter};
use easybot_core::types::message::{OutboundMessage, ParseMode, SendTextParams};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 构建测试用的微信适配器
async fn make_adapter(mock_port: u16) -> impl PlatformAdapter {
    let base_url = format!("http://127.0.0.1:{}", mock_port);
    let config = AdapterConfig {
        enabled: Some(true),
        token: None,
        api_key: None,
        base_url: Some(base_url),
        extra: serde_json::json!({
            "bot_token": "test-bot-token-abc",
            "ilink_bot_id": "bot-001",
            "ilink_user_id": "user-001"
        }),
    };

    let mut adapter = easybot_adapter_wechat::WeChatAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(result.ok, "init should succeed");
    adapter
}

fn send_text_params() -> SendTextParams {
    SendTextParams {
        chat_id: "wechat_user_001".to_string(),
        message: OutboundMessage {
            text: "Hello WeChat".to_string(),
            parse_mode: ParseMode::None,
        },
        reply_to: None,
        metadata: None,
    }
}

fn send_success_response() -> serde_json::Value {
    serde_json::json!({
        "ret": 0,
        "errmsg": "ok",
        "msg_id": 12345,
        "local_id": "local-001"
    })
}

// ── 成功路径 ──

#[tokio::test]
async fn test_send_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_success_response()))
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
async fn test_send_uses_msg_id_str_fallback() {
    let mock_server = MockServer::start().await;

    // 返回 msg_id_str 而不是 msg_id
    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ret": 0,
            "errmsg": "ok",
            "msg_id_str": "str-msg-001",
            "local_id": "local-001"
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(result.success);
    assert_eq!(
        result.message_id,
        Some("str-msg-001".to_string()),
        "should prefer msg_id_str when msg_id is absent"
    );

    mock_server.verify().await;
}

// ── 错误路径 ──

#[tokio::test]
async fn test_send_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await;

    assert!(result.is_err(), "HTTP 500 should return Err(GatewayError)");
}

#[tokio::test]
async fn test_send_malformed_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("not-json-at-all")
                .insert_header("Content-Type", "text/plain"),
        )
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await;

    assert!(result.is_err(), "malformed response should return Err");
}

#[tokio::test]
async fn test_send_ret_error_code() {
    let mock_server = MockServer::start().await;

    // iLink API 返回 ret != 0（业务错误）
    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ret": 1001,
            "errmsg": "invalid token"
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    // WeChat adapter 的 send() 对任何 200 返回 success
    // ret!=0 会被忽略（当前实现将 ret 字段仅用于 i64 解析）
    let result = adapter.send(send_text_params()).await.unwrap();
    assert!(result.success, "WeChat adapter treats all 200 as success");
}

// ── 前置条件 ──

#[tokio::test]
async fn test_init_always_succeeds() {
    // WeChat init 不验证凭证，总是成功
    let config = AdapterConfig {
        enabled: Some(true),
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    let mut adapter = easybot_adapter_wechat::WeChatAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(result.ok, "WeChat init should always succeed");
    // send() 的结果取决于是否有磁盘凭据，这里不验证
}

// ── 状态转换测试 ──

#[tokio::test]
async fn test_new_state_created() {
    let adapter = easybot_adapter_wechat::WeChatAdapter::new();
    assert_eq!(adapter.state(), AdapterState::Created);
}

#[tokio::test]
async fn test_init_sets_starting() {
    let config = AdapterConfig {
        enabled: Some(true),
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({
            "bot_token": "test-token",
            "ilink_bot_id": "bot-001",
            "ilink_user_id": "user-001",
        }),
    };
    let mut adapter = easybot_adapter_wechat::WeChatAdapter::new();
    adapter.init(config).await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Starting);
}

#[tokio::test]
async fn test_connect_success_with_credentials() {
    // WeChat connect() 在提供了 bot_token/ilink_bot_id/ilink_user_id 时不发起 HTTP 请求
    let config = AdapterConfig {
        enabled: Some(true),
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({
            "bot_token": "test-bot-token",
            "ilink_bot_id": "bot-001",
            "ilink_user_id": "user-001",
        }),
    };
    let mut adapter = easybot_adapter_wechat::WeChatAdapter::new();
    adapter.init(config).await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Starting);

    let result = adapter.connect().await.unwrap();
    assert!(
        result.ok,
        "connect should succeed with credentials in config"
    );
    assert_eq!(adapter.state(), AdapterState::Connected);
}

#[tokio::test]
async fn test_disconnect_from_created_is_idempotent() {
    // 未 init/connect 状态下直接 disconnect
    let mut adapter = easybot_adapter_wechat::WeChatAdapter::new();
    adapter.disconnect().await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Stopped);

    adapter.disconnect().await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Stopped);
}

#[tokio::test]
async fn test_disconnect_sets_stopped() {
    let config = AdapterConfig {
        enabled: Some(true),
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({
            "bot_token": "test-token",
            "ilink_bot_id": "bot-001",
            "ilink_user_id": "user-001",
        }),
    };
    let mut adapter = easybot_adapter_wechat::WeChatAdapter::new();
    adapter.init(config).await.unwrap();

    adapter.disconnect().await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Stopped);

    // 重复断开应幂等
    adapter.disconnect().await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Stopped);
}
