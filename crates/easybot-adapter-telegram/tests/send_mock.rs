//! Telegram 适配器 send() 的 HTTP mock 测试
//!
//! 使用 wiremock 模拟 Telegram Bot API，验证 send() 方法正确构造请求并解析响应。

use std::sync::Arc;
use std::time::Duration;

use easybot_core::types::adapter::{AdapterConfig, AdapterState, PlatformAdapter};
use easybot_core::types::message::{
    Button, InlineKeyboard, KeyboardRow, MediaAttachment, MediaType, OutboundMessage, ParseMode,
    SendInteractiveParams, SendMediaParams, SendTextParams,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 构建测试用的 Telegram 适配器
async fn make_adapter(mock_port: u16) -> impl PlatformAdapter {
    let base_url = format!("http://127.0.0.1:{}/bot", mock_port);
    let config = AdapterConfig {
        enabled: Some(true),
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
    assert_eq!(
        result.message_id,
        Some("12345".to_string()),
        "message_id should match"
    );
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
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false,
            "description": "Forbidden: bot was blocked by the user",
            "error_code": 403
        })))
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
        enabled: Some(true),
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

// ── connect() 测试 ──

fn bot_info_response() -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "result": {
            "id": 12345,
            "is_bot": true,
            "first_name": "TestBot",
            "username": "my_test_bot"
        }
    })
}

#[tokio::test]
async fn test_connect_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(bot_info_response()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.connect().await.unwrap();

    assert!(result.ok, "connect should succeed");
    assert!(result.error.is_none(), "no error expected");
    let bot = result.bot_info.expect("bot_info should be present");
    assert_eq!(bot.name, "TestBot");
    assert_eq!(bot.id, "12345");
    assert_eq!(adapter.state(), AdapterState::Connected);
    assert!(adapter.is_connected());

    mock_server.verify().await;
}

#[tokio::test]
async fn test_connect_api_error() {
    let mock_server = MockServer::start().await;

    // Telegram API 返回 ok:false（如 token 不合法）
    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false,
            "description": "Unauthorized: bot token is invalid",
            "error_code": 401
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.connect().await.unwrap();

    assert!(!result.ok, "connect should fail");
    assert!(result.error.is_some(), "should contain error message");
    // 状态应为 Created（未切换）
    assert_eq!(adapter.state(), AdapterState::Created);

    mock_server.verify().await;
}

#[tokio::test]
async fn test_connect_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.connect().await;

    assert!(result.is_err(), "HTTP error should return Err");

    mock_server.verify().await;
}

#[tokio::test]
async fn test_connect_malformed_response() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/bottest-token/getMe"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("not-json")
                .insert_header("Content-Type", "text/plain"),
        )
        .expect(1..)
        .mount(&mock_server)
        .await;

    let mut adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.connect().await;

    assert!(result.is_err(), "malformed response should return Err");

    mock_server.verify().await;
}

#[tokio::test]
async fn test_connect_no_token_returns_config_error() {
    let config = AdapterConfig {
        enabled: Some(true),
        token: None,
        api_key: None,
        base_url: None,
        extra: serde_json::json!({}),
    };
    let mut adapter = easybot_adapter_telegram::TelegramAdapter::new();
    // init with empty token returns ok:false, config NOT stored
    adapter.init(config).await.unwrap();

    // connect should fail because config is None
    let result = adapter.connect().await;
    assert!(result.is_err(), "connect without token should error");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("token") || err.contains("Config"),
        "error should mention token/config, got: {}",
        err
    );
}

// ── send_media() 测试 ──

fn send_media_params() -> SendMediaParams {
    SendMediaParams {
        chat_id: "12345".to_string(),
        text: None,
        media: MediaAttachment {
            media_type: MediaType::Image,
            url: Some("https://example.com/photo.jpg".to_string()),
            data: None,
            mime_type: "image/jpeg".to_string(),
            filename: Some("photo.jpg".to_string()),
            caption: Some("See this photo".to_string()),
            thumbnail_url: None,
            file_size: None,
            duration: None,
        },
        reply_to: None,
    }
}

fn send_media_success_body() -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "result": {
            "message_id": 67890,
            "date": 1000001,
            "chat": {"id": 12345, "type": "private"},
            "from": {"id": 1, "is_bot": true, "first_name": "TestBot"},
            "text": "See this photo"
        }
    })
}

#[tokio::test]
async fn test_send_media_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendPhoto"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_media_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send_media(send_media_params()).await.unwrap();

    assert!(result.success, "send_media should succeed");
    assert_eq!(result.message_id, Some("67890".to_string()));

    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_media_sends_correct_fields() {
    let mock_server = MockServer::start().await;

    let captured_body = Arc::new(std::sync::Mutex::new(None::<serde_json::Value>));
    let captured = captured_body.clone();

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendPhoto"))
        .and(move |req: &wiremock::Request| {
            if let Ok(body) = serde_json::from_slice::<serde_json::Value>(&req.body) {
                *captured.lock().unwrap() = Some(body);
            }
            true
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(send_media_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send_media(send_media_params()).await.unwrap();
    assert!(result.success);

    tokio::time::sleep(Duration::from_millis(100)).await;
    let body = captured_body.lock().unwrap().take().unwrap();
    assert_eq!(body["chat_id"], "12345");
    assert_eq!(body["photo"], "https://example.com/photo.jpg");
    assert_eq!(body["caption"], "See this photo");
}

#[tokio::test]
async fn test_send_media_api_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendPhoto"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false,
            "description": "Bad Request: wrong file identifier/HTTP URL specified",
            "error_code": 400
        })))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send_media(send_media_params()).await.unwrap();

    assert!(!result.success, "send_media should fail with API error");
}

#[tokio::test]
async fn test_send_media_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendPhoto"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter.send_media(send_media_params()).await.unwrap();

    assert!(!result.success, "send_media should fail with HTTP 500");
}

#[tokio::test]
async fn test_send_media_no_url_or_data() {
    let adapter = make_adapter(1).await; // port doesn't matter
    let params = SendMediaParams {
        chat_id: "12345".to_string(),
        text: None,
        media: MediaAttachment {
            media_type: MediaType::Image,
            url: None,
            data: None,
            mime_type: "image/jpeg".to_string(),
            filename: None,
            caption: None,
            thumbnail_url: None,
            file_size: None,
            duration: None,
        },
        reply_to: None,
    };
    let result = adapter.send_media(params).await.unwrap();
    assert!(!result.success, "should fail when no URL or data");
}

#[tokio::test]
async fn test_send_media_audio_type() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendAudio"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_media_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let params = SendMediaParams {
        chat_id: "12345".to_string(),
        text: None,
        media: MediaAttachment {
            media_type: MediaType::Audio,
            url: Some("https://example.com/audio.mp3".to_string()),
            data: None,
            mime_type: "audio/mpeg".to_string(),
            filename: Some("audio.mp3".to_string()),
            caption: None,
            thumbnail_url: None,
            file_size: None,
            duration: None,
        },
        reply_to: None,
    };
    let result = adapter.send_media(params).await.unwrap();
    assert!(result.success, "audio send_media should succeed");
    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_media_video_type() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendVideo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_media_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let params = SendMediaParams {
        chat_id: "12345".to_string(),
        text: None,
        media: MediaAttachment {
            media_type: MediaType::Video,
            url: Some("https://example.com/video.mp4".to_string()),
            data: None,
            mime_type: "video/mp4".to_string(),
            filename: Some("video.mp4".to_string()),
            caption: None,
            thumbnail_url: None,
            file_size: None,
            duration: None,
        },
        reply_to: None,
    };
    let result = adapter.send_media(params).await.unwrap();
    assert!(result.success, "video send_media should succeed");
    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_media_document_type() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendDocument"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_media_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let params = SendMediaParams {
        chat_id: "12345".to_string(),
        text: None,
        media: MediaAttachment {
            media_type: MediaType::Document,
            url: Some("https://example.com/doc.pdf".to_string()),
            data: None,
            mime_type: "application/pdf".to_string(),
            filename: Some("doc.pdf".to_string()),
            caption: None,
            thumbnail_url: None,
            file_size: None,
            duration: None,
        },
        reply_to: None,
    };
    let result = adapter.send_media(params).await.unwrap();
    assert!(result.success, "document send_media should succeed");
    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_media_sticker_type() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendSticker"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_media_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let params = SendMediaParams {
        chat_id: "12345".to_string(),
        text: None,
        media: MediaAttachment {
            media_type: MediaType::Sticker,
            url: Some("https://example.com/sticker.webp".to_string()),
            data: None,
            mime_type: fixtures::sticker_attachment().mime_type,
            filename: fixtures::sticker_attachment().filename,
            caption: None,
            thumbnail_url: None,
            file_size: None,
            duration: None,
        },
        reply_to: None,
    };
    let result = adapter.send_media(params).await.unwrap();
    assert!(result.success, "sticker send_media should succeed");
    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_media_animation_type() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendAnimation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(send_media_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let params = SendMediaParams {
        chat_id: "12345".to_string(),
        text: None,
        media: MediaAttachment {
            media_type: MediaType::Animation,
            url: Some("https://example.com/animation.gif".to_string()),
            data: None,
            mime_type: fixtures::animation_attachment().mime_type,
            filename: fixtures::animation_attachment().filename,
            caption: None,
            thumbnail_url: None,
            file_size: None,
            duration: None,
        },
        reply_to: None,
    };
    let result = adapter.send_media(params).await.unwrap();
    assert!(result.success, "animation send_media should succeed");
    mock_server.verify().await;
}

// ── send_interactive() 测试 ──

fn interactive_params() -> SendInteractiveParams {
    SendInteractiveParams {
        chat_id: "12345".to_string(),
        text: "Choose an option:".to_string(),
        keyboard: InlineKeyboard {
            rows: vec![
                KeyboardRow {
                    buttons: vec![
                        Button {
                            text: "Yes".to_string(),
                            callback_data: Some("yes".to_string()),
                            url: None,
                        },
                        Button {
                            text: "No".to_string(),
                            callback_data: Some("no".to_string()),
                            url: None,
                        },
                    ],
                },
                KeyboardRow {
                    buttons: vec![Button {
                        text: "Cancel".to_string(),
                        callback_data: Some("cancel".to_string()),
                        url: None,
                    }],
                },
            ],
        },
        reply_to: None,
    }
}

fn interactive_success_body() -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "result": {
            "message_id": 999,
            "date": 1000002,
            "chat": {"id": 12345, "type": "private"},
            "from": {"id": 1, "is_bot": true, "first_name": "TestBot"},
            "text": "Choose an option:"
        }
    })
}

#[tokio::test]
async fn test_send_interactive_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(interactive_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter
        .send_interactive(interactive_params())
        .await
        .unwrap();

    assert!(result.success, "send_interactive should succeed");
    assert_eq!(result.message_id, Some("999".to_string()));

    mock_server.verify().await;
}

#[tokio::test]
async fn test_send_interactive_inline_keyboard_format() {
    let mock_server = MockServer::start().await;

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
        .respond_with(ResponseTemplate::new(200).set_body_json(interactive_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let result = adapter
        .send_interactive(interactive_params())
        .await
        .unwrap();
    assert!(result.success);

    tokio::time::sleep(Duration::from_millis(100)).await;
    let body = captured_body.lock().unwrap().take().unwrap();

    // 验证消息内容
    assert_eq!(body["chat_id"], "12345");
    assert_eq!(body["text"], "Choose an option:");

    // 验证键盘结构
    let markup = &body["reply_markup"];
    let keyboard = &markup["inline_keyboard"];
    assert!(keyboard.is_array(), "inline_keyboard should be an array");
    assert_eq!(keyboard.as_array().unwrap().len(), 2, "should have 2 rows");

    // 第一行：Yes | No
    assert_eq!(keyboard[0][0]["text"], "Yes");
    assert_eq!(keyboard[0][0]["callback_data"], "yes");
    assert_eq!(keyboard[0][1]["text"], "No");
    assert_eq!(keyboard[0][1]["callback_data"], "no");

    // 第二行：Cancel
    assert_eq!(keyboard[1][0]["text"], "Cancel");
    assert_eq!(keyboard[1][0]["callback_data"], "cancel");
}

#[tokio::test]
async fn test_send_interactive_with_url_button() {
    let mock_server = MockServer::start().await;

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
        .respond_with(ResponseTemplate::new(200).set_body_json(interactive_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let params = SendInteractiveParams {
        chat_id: "12345".to_string(),
        text: "Visit our site:".to_string(),
        keyboard: InlineKeyboard {
            rows: vec![KeyboardRow {
                buttons: vec![Button {
                    text: "Open Website".to_string(),
                    callback_data: None,
                    url: Some("https://example.com".to_string()),
                }],
            }],
        },
        reply_to: None,
    };
    let result = adapter.send_interactive(params).await.unwrap();
    assert!(result.success);

    tokio::time::sleep(Duration::from_millis(100)).await;
    let body = captured_body.lock().unwrap().take().unwrap();
    let keyboard = &body["reply_markup"]["inline_keyboard"];
    assert_eq!(keyboard[0][0]["text"], "Open Website");
    assert_eq!(keyboard[0][0]["url"], "https://example.com");
    assert!(keyboard[0][0].get("callback_data").is_none());
}

#[tokio::test]
async fn test_send_interactive_api_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": false,
            "description": "Bad Request: can't parse reply keyboard markup",
            "error_code": 400
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

#[tokio::test]
async fn test_send_interactive_http_error() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/bottest-token/sendMessage"))
        .respond_with(ResponseTemplate::new(403))
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
        "send_interactive should fail with HTTP 403"
    );
}

#[tokio::test]
async fn test_send_interactive_with_reply_to() {
    let mock_server = MockServer::start().await;

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
        .respond_with(ResponseTemplate::new(200).set_body_json(interactive_success_body()))
        .expect(1..)
        .mount(&mock_server)
        .await;

    let adapter = make_adapter(mock_server.address().port()).await;
    let params = SendInteractiveParams {
        chat_id: "12345".to_string(),
        text: "Reply".to_string(),
        keyboard: InlineKeyboard { rows: vec![] },
        reply_to: Some("42".to_string()),
    };
    let result = adapter.send_interactive(params).await.unwrap();
    assert!(result.success);

    tokio::time::sleep(Duration::from_millis(100)).await;
    let body = captured_body.lock().unwrap().take().unwrap();
    assert_eq!(body["reply_to_message_id"], "42");
}
