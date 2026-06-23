//! 飞书适配器 send() 的 HTTP mock 测试
//!
//! 使用 wiremock 模拟飞书 API，验证 send() 方法正确构造请求并解析响应。
//! send() 首先通过 POST /auth/v3/tenant_access_token/internal 获取 token，
//! 然后通过 POST /im/v1/messages... 发送消息。

use easybot_core::types::adapter::{AdapterConfig, AdapterState, PlatformAdapter};
use easybot_core::types::message::{
    Button, EditMessageParams, InlineKeyboard, KeyboardRow, OutboundMessage, ParseMode,
    SendInteractiveParams, SendTextParams,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 构建测试用的飞书适配器
async fn make_adapter(mock_port: u16) -> impl PlatformAdapter {
    let base_url = format!("http://127.0.0.1:{}", mock_port);
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("test-app-secret".into()),
        api_key: None,
        base_url: Some(base_url),
        extra: serde_json::json!({
            "app_id": "test-app-id"
        }),
    };

    let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(result.ok, "init should succeed");
    adapter
}

fn send_text_params() -> SendTextParams {
    SendTextParams {
        chat_id: "oc_abc123".to_string(),
        message: OutboundMessage {
            text: "Hello Feishu".to_string(),
            parse_mode: ParseMode::None,
        },
        reply_to: None,
        metadata: None,
    }
}

/// mock 飞书的 token 端点
async fn mock_token_endpoint(mock_server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok",
            "tenant_access_token": "test-access-token-12345",
            "expire": 7200
        })))
        .expect(1..)
        .mount(mock_server)
        .await;
}

fn send_success_response() -> serde_json::Value {
    serde_json::json!({
        "code": 0,
        "msg": "ok",
        "data": {
            "message_id": "om_abc123xyz",
            "root_id": "",
            "parent_id": "",
            "create_time": "1712345678"
        }
    })
}

// ── 成功路径 ──

#[tokio::test]
async fn test_send_success() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_success_response()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(result.success, "send should succeed");
    assert_eq!(result.message_id, Some("om_abc123xyz".to_string()));

    mock_server.verify().await;
}

// ── 错误路径 ──

#[tokio::test]
async fn test_send_apicode_error() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    // 飞书 API 返回 code != 0
    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 10003,
            "msg": "invalid receive_id",
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with API error code");
}

#[tokio::test]
async fn test_send_http_error() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail with HTTP 500");
}

#[tokio::test]
async fn test_send_malformed_response() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
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
async fn test_token_refresh_failure() {
    let mock_server = MockServer::start().await;

    // token 端点返回错误 → send 应失败
    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 99999,
            "msg": "invalid app_id or app_secret",
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();

    assert!(!result.success, "send should fail when token refresh fails");
}

// ── init 验证 ──

#[tokio::test]
async fn test_init_requires_app_id_and_secret() {
    // 缺少 app_id
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("secret".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
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
    let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
    let result = adapter.init(config).await.unwrap();
    assert!(!result.ok, "init should fail without secret");
}

// ── 状态转换测试 ──

#[tokio::test]
async fn test_new_state_created() {
    let adapter = easybot_adapter_feishu::FeishuAdapter::new();
    assert_eq!(adapter.state(), AdapterState::Created);
}

#[tokio::test]
async fn test_init_sets_starting() {
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("app-secret".into()),
        api_key: None,
        base_url: None,
        extra: serde_json::json!({"app_id": "test-app"}),
    };
    let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
    adapter.init(config).await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Starting);
}

#[tokio::test]
async fn test_connect_success_state() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok",
            "tenant_access_token": "test-access-token-12345",
            "expire": 7200
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mock_port = mock_server.address().port();
    let base_url = format!("http://127.0.0.1:{}", mock_port);
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("app-secret".into()),
        api_key: None,
        base_url: Some(base_url),
        extra: serde_json::json!({"app_id": "test-app"}),
    };
    let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
    adapter.init(config).await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Starting);

    let result = adapter.connect().await.unwrap();
    assert!(result.ok, "connect should succeed with mocked token");
    assert_eq!(adapter.state(), AdapterState::Connected);
}

#[tokio::test]
async fn test_connect_fails_without_mock() {
    // 没有 mock server，token refresh 应失败
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("app-secret".into()),
        api_key: None,
        base_url: Some("http://127.0.0.1:1".into()),
        extra: serde_json::json!({"app_id": "test-app"}),
    };
    let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
    adapter.init(config).await.unwrap();

    let result = adapter.connect().await;
    assert!(
        result.is_err(),
        "connect should fail without reachable auth endpoint"
    );
}

#[tokio::test]
async fn test_disconnect_sets_stopped() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/auth/v3/tenant_access_token/internal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok",
            "tenant_access_token": "test-access-token",
            "expire": 7200
        })))
        .expect(0..)
        .mount(&mock_server)
        .await;

    let base_url = format!("http://127.0.0.1:{}", mock_server.address().port());
    let config = AdapterConfig {
        enabled: Some(true),
        token: Some("secret".into()),
        api_key: None,
        base_url: Some(base_url),
        extra: serde_json::json!({"app_id": "test-app"}),
    };
    let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
    adapter.init(config).await.unwrap();

    // 直接断开连接（不经过 connect）
    adapter.disconnect().await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Stopped);

    // 重复断开应幂等
    adapter.disconnect().await.unwrap();
}

// ── send_interactive() 测试 ──

fn interactive_params() -> SendInteractiveParams {
    SendInteractiveParams {
        chat_id: "oc_abc123".to_string(),
        text: "Choose an option:".to_string(),
        keyboard: InlineKeyboard {
            rows: vec![KeyboardRow {
                buttons: vec![
                    Button {
                        text: "Confirm".to_string(),
                        callback_data: Some("confirm".to_string()),
                        url: None,
                    },
                    Button {
                        text: "Cancel".to_string(),
                        callback_data: Some("cancel".to_string()),
                        url: None,
                    },
                ],
            }],
        },
        reply_to: None,
    }
}

#[tokio::test]
async fn test_send_interactive_success() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_success_response()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter
        .send_interactive(interactive_params())
        .await
        .unwrap();

    assert!(result.success, "send_interactive should succeed");
    assert_eq!(result.message_id, Some("om_abc123xyz".to_string()));

    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_interactive_http_error() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter
        .send_interactive(interactive_params())
        .await
        .unwrap();

    assert!(
        !result.success,
        "send_interactive should fail with HTTP 500"
    );
}

#[tokio::test]
async fn test_send_interactive_api_error() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("POST"))
        .and(path("/im/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 10003,
            "msg": "invalid receive_id",
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter
        .send_interactive(interactive_params())
        .await
        .unwrap();

    assert!(
        !result.success,
        "send_interactive should fail with API error"
    );
}

// ── edit_message() 测试 ──

#[tokio::test]
async fn test_edit_message_success() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("PUT"))
        .and(path("/im/v1/messages/om_test_msg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 0,
            "msg": "ok",
            "data": {}
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter
        .edit_message(EditMessageParams {
            chat_id: "oc_abc123".to_string(),
            message_id: "om_test_msg".to_string(),
            message: OutboundMessage {
                text: "edited content".to_string(),
                parse_mode: ParseMode::None,
            },
            keyboard: None,
        })
        .await;

    assert!(
        result.is_ok(),
        "edit_message should succeed, got err: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_edit_message_api_error() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("PUT"))
        .and(path("/im/v1/messages/om_nonexistent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 123456,
            "msg": "message not found",
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter
        .edit_message(EditMessageParams {
            chat_id: "oc_abc123".to_string(),
            message_id: "om_nonexistent".to_string(),
            message: OutboundMessage {
                text: "edited".to_string(),
                parse_mode: ParseMode::None,
            },
            keyboard: None,
        })
        .await;

    assert!(
        result.is_err(),
        "edit_message should return error for nonexistent message"
    );
}

#[tokio::test]
async fn test_edit_message_http_error() {
    let mock_server = MockServer::start().await;
    mock_token_endpoint(&mock_server).await;

    Mock::given(method("PUT"))
        .and(path("/im/v1/messages/om_test_msg"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter
        .edit_message(EditMessageParams {
            chat_id: "oc_abc123".to_string(),
            message_id: "om_test_msg".to_string(),
            message: OutboundMessage {
                text: "edited".to_string(),
                parse_mode: ParseMode::None,
            },
            keyboard: None,
        })
        .await;

    assert!(
        result.is_err(),
        "edit_message should return error with HTTP 500"
    );
}
