//! 个人微信适配器端到端（E2E）集成测试
//!
//! 使用 wiremock 模拟 iLink Bot API。

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
            "wechat",
            "个人微信",
            Arc::new(move |config| {
                let eb = eb.clone();
                Box::pin(async move {
                    let mut adapter = easybot_adapter_wechat::WeChatAdapter::new();
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
            &[], // WeChat 无必需凭证环境变量
        )
        .await;

    let mut config = default_gateway_config();
    config.adapters = {
        let mut m = HashMap::new();
        m.insert(
            "wechat".to_string(),
            AdapterConfig {
                enabled: Some(true),
                token: None,
                api_key: None,
                base_url: Some(mock_base),
                extra: serde_json::json!({
                    "bot_token": "test-wechat-token",
                    "ilink_bot_id": "347c4943280a@im.bot",
                    "ilink_user_id": "test_user@im.wechat"
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

// ── 基础 ──

#[tokio::test]
async fn test_e2e_wechat_health() {
    let (router, ..) = setup().await;
    let (status, _) = public_get(&router, "/api/v1/health").await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_e2e_wechat_lifecycle() {
    let (router, key, _mock_server) = setup().await;

    // WeChat init 不需要网络调用（仅校验凭证文件或 config.extra）
    let conn = start_and_connect(&router, &key, "wechat").await;
    assert!(conn, "wechat should connect without network calls");

    let (_, json) = auth_get(&router, "/api/v1/adapters/wechat/status", &key).await;
    assert_eq!(json["connected"], true);
    assert_eq!(json["state"], "Connected");

    // 停止
    let (status, json) = auth_post(&router, "/api/v1/adapters/wechat/stop", &key, None).await;
    assert_eq!(status, 200);
    assert_eq!(json["ok"], true);

    let (_, json) = auth_get(&router, "/api/v1/adapters/wechat/status", &key).await;
    assert_eq!(json["connected"], false);
}

#[tokio::test]
async fn test_e2e_wechat_send_message() {
    let (router, key, mock_server) = setup().await;

    // WeChat send: POST /ilink/bot/sendmessage（扁平结构，无 msg 包装）
    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message_id": "msg_wechat_001",
            "seq": 100
        })))
        .expect(0..)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "wechat").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "wechat:test_user@im.wechat", "text": "Hello WeChat"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "sent");
}

#[tokio::test]
async fn test_e2e_wechat_send_error() {
    let (router, key, mock_server) = setup().await;

    // WeChat API 返回非 JSON → send() 解析失败
    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("not-valid-json")
                .insert_header("Content-Type", "text/plain"),
        )
        .expect(0..)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "wechat").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "wechat:bad_user", "text": "fail"})),
    )
    .await;
    // WeChat send 解析失败时 API 层返回 500
    assert!(status.is_server_error() || json["status"] == "failed");
}

#[tokio::test]
async fn test_e2e_wechat_send_api_error() {
    let (router, key, mock_server) = setup().await;

    // WeChat API 返回 ret != 0 → 业务错误，send() 返回 status=failed
    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ret": 1001,
            "errmsg": "invalid token"
        })))
        .expect(0..)
        .mount(&mock_server)
        .await;

    assert!(start_and_connect(&router, &key, "wechat").await);

    let (status, json) = auth_post(
        &router,
        "/api/v1/messages/send",
        &key,
        Some(serde_json::json!({"target": "wechat:test_user@im.wechat", "text": "Hello"})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["status"], "failed");
    assert!(
        json["error"].as_str().unwrap_or("").contains("1001"),
        "error should contain ret code 1001, got: {:?}",
        json["error"]
    );
}
