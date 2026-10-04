//! 个人微信适配器 send() 的 HTTP mock 测试
//!
//! 使用 wiremock 模拟 iLink Bot API，验证 send() 方法正确构造请求并解析响应。
//! WeChat send() 从 config.extra 中读取 bot_token，无 token 刷新流程。

use std::sync::Arc;

use easybot_core::types::adapter::{AdapterConfig, AdapterState, CapabilityName, PlatformAdapter};
use easybot_core::types::error::GatewayError;
use easybot_core::types::message::{
    EditMessageParams, InlineKeyboard, MediaAttachment, MediaType, OutboundMessage, ParseMode,
    SendInteractiveParams, SendMediaGroupParams, SendMediaParams, SendTextParams,
};
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
    // iLink send API 返回扁平结构，message_id 为字符串
    serde_json::json!({
        "message_id": "12345",
        "seq": 100,
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

    // 返回 msg_id_str 而不是 message_id
    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
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

    assert!(result.is_err(), "HTTP 500 should return Err");
    assert!(
        result.unwrap_err().to_string().contains("500"),
        "error should contain HTTP status code"
    );
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
    let result = adapter.send(send_text_params()).await.unwrap();
    assert!(!result.success, "ret!=0 should return success=false");
    assert!(
        result.error.unwrap_or_default().contains("1001"),
        "error should contain the ret code"
    );
}

#[tokio::test]
async fn test_send_empty_response() {
    // 真实 iLink send API 在成功时可能返回 {}（空对象）
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send(send_text_params()).await.unwrap();
    assert!(
        result.success,
        "empty response '{{}}' should be treated as success"
    );
    assert_eq!(result.message_id, None, "empty response has no message ID");
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

// ── 元数据 / 能力声明 ──

#[tokio::test]
async fn test_platform_and_display_name() {
    let adapter = easybot_adapter_wechat::WeChatAdapter::new();
    assert_eq!(adapter.platform_name(), "wechat");
    assert_eq!(adapter.display_name(), "个人微信");
}

#[tokio::test]
async fn test_capabilities_supported_and_absent() {
    let adapter = easybot_adapter_wechat::WeChatAdapter::new();
    let caps = adapter.capabilities();
    let has = |name: CapabilityName| caps.iter().any(|c| c.name == name && c.supported);
    assert!(has(CapabilityName::Text));
    assert!(has(CapabilityName::Image));
    assert!(has(CapabilityName::Audio));
    assert!(has(CapabilityName::Video));
    assert!(has(CapabilityName::Document));
    // iLink Bot API 不提供的能力不应声明为 supported
    for absent in [
        CapabilityName::Interactive,
        CapabilityName::Streaming,
        CapabilityName::MessageEdit,
        CapabilityName::MessageDelete,
        CapabilityName::ChatList,
    ] {
        assert!(
            !caps.iter().any(|c| c.name == absent && c.supported),
            "{absent:?} must not be declared supported"
        );
    }
}

#[tokio::test]
async fn test_is_connected_false_initially() {
    let adapter = easybot_adapter_wechat::WeChatAdapter::new();
    assert!(!adapter.is_connected());
}

#[tokio::test]
async fn test_list_chats_returns_empty() {
    let adapter = make_adapter(1).await;
    let chats = adapter.list_chats(None).await.unwrap();
    assert!(chats.is_empty(), "iLink 无会话列表端点，应返回空");
}

// ── 平台不支持的操作（trait 默认实现）──

#[tokio::test]
async fn test_edit_message_unsupported() {
    let adapter = make_adapter(1).await;
    let err = adapter
        .edit_message(EditMessageParams {
            chat_id: "u".into(),
            message_id: "m".into(),
            message: OutboundMessage {
                text: "x".into(),
                parse_mode: ParseMode::None,
            },
            keyboard: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, GatewayError::CapabilityNotSupported(_)));
}

#[tokio::test]
async fn test_delete_message_unsupported() {
    let adapter = make_adapter(1).await;
    let err = adapter.delete_message("u", "m").await.unwrap_err();
    assert!(matches!(err, GatewayError::CapabilityNotSupported(_)));
}

#[tokio::test]
async fn test_send_interactive_unsupported() {
    let adapter = make_adapter(1).await;
    let err = adapter
        .send_interactive(SendInteractiveParams {
            chat_id: "u".into(),
            text: "x".into(),
            keyboard: InlineKeyboard { rows: vec![] },
            reply_to: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, GatewayError::CapabilityNotSupported(_)));
}

#[tokio::test]
async fn test_send_typing_unsupported() {
    let adapter = make_adapter(1).await;
    let err = adapter.send_typing("u").await.unwrap_err();
    assert!(matches!(err, GatewayError::CapabilityNotSupported(_)));
}

#[tokio::test]
async fn test_send_media_group_unsupported() {
    let adapter = make_adapter(1).await;
    let err = adapter
        .send_media_group(SendMediaGroupParams {
            chat_id: "u".into(),
            media: vec![
                wechat_media(MediaType::Image),
                wechat_media(MediaType::Image),
            ],
            text: None,
            reply_to: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, GatewayError::CapabilityNotSupported(_)));
}

// ── send_media：媒体类型映射与错误路径 ──

/// 构造 base64 编码的媒体附件（内容为 "hello world"）。
fn wechat_media(mt: MediaType) -> MediaAttachment {
    MediaAttachment {
        media_type: mt,
        url: None,
        data: Some("aGVsbG8gd29ybGQ=".to_string()),
        mime_type: "application/octet-stream".to_string(),
        filename: Some("file.bin".to_string()),
        caption: None,
        thumbnail_url: None,
        file_size: Some(11),
        duration: None,
    }
}

fn send_media_params(mt: MediaType) -> SendMediaParams {
    SendMediaParams {
        chat_id: "wechat_user_001".to_string(),
        media: wechat_media(mt),
        text: None,
        reply_to: None,
    }
}

/// 挂载 getuploadurl + CDN 上传 + sendmessage 三个 mock，返回 mock server。
async fn mount_media_mocks(
    capture: Option<Arc<std::sync::Mutex<Option<serde_json::Value>>>>,
) -> MockServer {
    let mock_server = MockServer::start().await;
    let base = format!("http://127.0.0.1:{}", mock_server.address().port());

    let mut getupload = Mock::given(method("POST")).and(path("/ilink/bot/getuploadurl"));
    if let Some(cap) = capture {
        getupload = getupload.and(move |req: &wiremock::Request| {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&req.body) {
                *cap.lock().unwrap() = Some(v);
            }
            true
        });
    }
    getupload
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ret": 0,
            "upload_full_url": format!("{}/cdn/upload", base),
        })))
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/cdn/upload"))
        .respond_with(
            ResponseTemplate::new(200).insert_header("x-encrypted-param", "enc-param-xyz"),
        )
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/ilink/bot/sendmessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "message_id": "m-media-1",
            "seq": 7,
        })))
        .mount(&mock_server)
        .await;

    mock_server
}

#[tokio::test]
async fn test_send_media_image_success() {
    let mock_server = mount_media_mocks(None).await;
    let adapter = make_adapter(mock_server.address().port()).await;

    let result = adapter
        .send_media(send_media_params(MediaType::Image))
        .await
        .unwrap();

    assert!(
        result.success,
        "image send_media should succeed: {:?}",
        result.error
    );
    assert_eq!(result.message_id, Some("m-media-1".to_string()));
    mock_server.verify().await;
}

async fn assert_media_type_mapping(mt: MediaType, expected: i64) {
    let captured = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let mock_server = mount_media_mocks(Some(captured.clone())).await;
    let adapter = make_adapter(mock_server.address().port()).await;

    let result = adapter.send_media(send_media_params(mt)).await.unwrap();
    assert!(
        result.success,
        "{mt:?} send_media should succeed: {:?}",
        result.error
    );

    let body = captured.lock().unwrap().clone().unwrap();
    assert_eq!(
        body["media_type"].as_i64(),
        Some(expected),
        "media_type mapping for {mt:?}"
    );
}

#[tokio::test]
async fn test_send_media_maps_image_type() {
    assert_media_type_mapping(MediaType::Image, 1).await;
}

#[tokio::test]
async fn test_send_media_maps_video_type() {
    assert_media_type_mapping(MediaType::Video, 2).await;
}

#[tokio::test]
async fn test_send_media_maps_audio_type() {
    assert_media_type_mapping(MediaType::Audio, 4).await;
}

#[tokio::test]
async fn test_send_media_maps_document_type() {
    assert_media_type_mapping(MediaType::Document, 3).await;
}

#[tokio::test]
async fn test_send_media_maps_sticker_to_file() {
    assert_media_type_mapping(MediaType::Sticker, 3).await;
}

#[tokio::test]
async fn test_send_media_maps_animation_to_file() {
    assert_media_type_mapping(MediaType::Animation, 3).await;
}

#[tokio::test]
async fn test_send_media_getuploadurl_ret_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/ilink/bot/getuploadurl"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ret": 1003,
            "errmsg": "upload denied",
        })))
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let err = adapter
        .send_media(send_media_params(MediaType::Image))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("getuploadurl") || err.to_string().contains("1003"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn test_send_media_cdn_missing_encrypt_param() {
    let mock_server = MockServer::start().await;
    let base = format!("http://127.0.0.1:{}", mock_server.address().port());

    Mock::given(method("POST"))
        .and(path("/ilink/bot/getuploadurl"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ret": 0,
            "upload_full_url": format!("{}/cdn/upload", base),
        })))
        .mount(&mock_server)
        .await;

    // CDN 返回 200 但缺少 x-encrypted-param 头
    Mock::given(method("POST"))
        .and(path("/cdn/upload"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let err = adapter
        .send_media(send_media_params(MediaType::Image))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("x-encrypted-param"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn test_send_media_cdn_http_error() {
    let mock_server = MockServer::start().await;
    let base = format!("http://127.0.0.1:{}", mock_server.address().port());

    Mock::given(method("POST"))
        .and(path("/ilink/bot/getuploadurl"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ret": 0,
            "upload_full_url": format!("{}/cdn/upload", base),
        })))
        .mount(&mock_server)
        .await;

    Mock::given(method("POST"))
        .and(path("/cdn/upload"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let err = adapter
        .send_media(send_media_params(MediaType::Image))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("CDN upload HTTP 500"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn test_send_media_missing_url_and_data() {
    let adapter = make_adapter(1).await;
    let mut media = wechat_media(MediaType::Image);
    media.data = None;
    let err = adapter
        .send_media(SendMediaParams {
            chat_id: "u".into(),
            media,
            text: None,
            reply_to: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("neither url nor data"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn test_send_media_invalid_base64() {
    let adapter = make_adapter(1).await;
    let mut media = wechat_media(MediaType::Image);
    media.data = Some("!!!not-valid-base64!!!".to_string());
    let err = adapter
        .send_media(SendMediaParams {
            chat_id: "u".into(),
            media,
            text: None,
            reply_to: None,
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("base64"),
        "unexpected error: {err}"
    );
}
