//! WebSocket 路由集成测试
//!
//! 测试 WebSocket 认证流程、事件推送和客户端断连处理。
//! 由于 WebSocket 需要真实端口，使用 tokio::net::TcpListener 绑定 127.0.0.1:0。

use std::net::SocketAddr;
use std::time::Duration;

use axum::http;
use easybot_api::AppState;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

mod common;

static WS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// WebSocket 测试客户端封装
struct WsClient {
    write: futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    read: futures::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
}

impl WsClient {
    /// 发送文本帧
    async fn send_text(&mut self, text: &str) {
        self.write
            .send(Message::Text(text.to_string().into()))
            .await
            .expect("send_text failed");
    }

    /// 接收文本帧（带 3 秒超时）
    ///
    /// 非文本帧（如 Binary/Ping/Pong）会被自动忽略。
    async fn recv_text(&mut self) -> Option<String> {
        loop {
            match tokio::time::timeout(Duration::from_secs(3), self.read.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => return Some(text.to_string()),
                Ok(Some(Ok(Message::Close(_)))) => return None,
                Ok(Some(Ok(_))) => continue,     // 非文本帧，忽略
                Ok(Some(Err(_))) => return None, // 连接错误/中断 = 已断开
                Ok(None) => return None,
                Err(_) => return None, // 超时
            }
        }
    }

    /// 接收并解析 JSON 帧
    async fn recv_json(&mut self) -> Option<Value> {
        self.recv_text()
            .await
            .and_then(|t| serde_json::from_str(&t).ok())
    }

    /// 关闭连接
    async fn close(&mut self) {
        let _ = self.write.send(Message::Close(None)).await;
    }
}

/// 启动 WebSocket 测试服务器
///
/// 绑定随机端口，启动 axum 服务，返回 (AppState, api_key, SocketAddr)。
/// 启动后通过重试 TCP 连接确保服务器 Accept 循环已运行，避免并发下的竞态条件。
async fn ws_server() -> (
    tokio::sync::MutexGuard<'static, ()>,
    AppState,
    String,
    SocketAddr,
) {
    let guard = WS_TEST_LOCK.lock().await;
    let (state, key) = common::test_app_state().await;
    let router = easybot_api::server::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    // 主动轮询直到服务端可连接，替代固定 sleep
    let start = std::time::Instant::now();
    loop {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "WS test server failed to start within 5s"
        );
    }

    (guard, state, key, addr)
}

/// 连接 WebSocket。认证只通过连接后的 auth 文本帧完成。
async fn connect_ws(addr: SocketAddr, _http_key: &str) -> WsClient {
    let uri_str = format!("ws://{}/api/v1/ws", addr);
    let request: http::Request<()> = uri_str
        .into_client_request()
        .expect("Failed to build WS upgrade request");
    let (ws_stream, _) = connect_async(request)
        .await
        .expect("WebSocket connect failed — HTTP-level auth may have failed");
    let (write, read) = ws_stream.split();
    WsClient { write, read }
}

/// 收到 auth_ok 响应后，客户端已完全认证
async fn auth_ws(client: &mut WsClient, api_key: &str) {
    // 发送 WS-level 认证帧
    client
        .send_text(&format!(r#"{{"token":"{}"}}"#, api_key))
        .await;
    let resp = client.recv_json().await;
    assert!(
        resp.is_some(),
        "Expected auth_ok response, got None (timeout or disconnect)"
    );
    let resp = resp.unwrap();
    assert_eq!(resp["type"], "auth_ok", "Expected auth_ok, got: {:?}", resp);
}

// ── 测试用例 ──

#[tokio::test]
async fn test_ws_auth_ok_with_valid_token() {
    let (_guard, state, key, addr) = ws_server().await;
    let key_id = state.auth_manager.list_keys().await[0].id.clone();
    let mut client = connect_ws(addr, &key).await;

    // 发送有效 token → 应收到 auth_ok
    client.send_text(&format!(r#"{{"token":"{}"}}"#, key)).await;
    let resp = client.recv_json().await;
    assert!(resp.is_some(), "Expected auth_ok, got timeout");
    let resp = resp.unwrap();
    assert_eq!(resp["type"], "auth_ok");

    let now = chrono::Utc::now().timestamp_millis();
    let usage = state
        .auth_manager
        .usage_records(
            now - 3_600_000,
            now + 3_600_000,
            Some(&key_id),
            None,
            100,
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        usage.iter().map(|record| record.request_count).sum::<i64>(),
        1
    );
    assert_eq!(usage[0].status_class, 1);

    client.close().await;
}

#[tokio::test]
async fn test_ws_global_admin_key_receives_targets_without_grants() {
    let (_guard, state, _default_key, addr) = ws_server().await;
    let (_, admin_key) = state
        .auth_manager
        .create_key("admin-session", vec!["*".into()], None)
        .await
        .unwrap();
    let auth = state.auth_manager.authenticate(&admin_key).await.unwrap();
    assert!(
        state
            .auth_manager
            .list_target_grants(&auth.subject_id)
            .await
            .unwrap()
            .is_empty()
    );

    let mut client = connect_ws(addr, &admin_key).await;
    auth_ws(&mut client, &admin_key).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    state
        .event_bus
        .publish(easybot_core::types::event::GatewayEvent {
            event_type: "message.inbound".into(),
            source: "qq".into(),
            timestamp: 43,
            data: json!({"platform":"qq","chat_id":"unlisted-group","text":"admin-visible"}),
            metadata: None,
        });

    let event = client
        .recv_json()
        .await
        .expect("global admin receives target event without a grant");
    assert_eq!(event["data"]["text"], "admin-visible");
    client.close().await;
}

#[tokio::test]
async fn test_ws_auth_required_when_no_token() {
    let (_guard, _state, key, addr) = ws_server().await;
    let mut client = connect_ws(addr, &key).await;

    // 发送不带 token 的 JSON → 应收到 auth_required
    client.send_text(r#"{"hello":"world"}"#).await;
    let resp = client.recv_json().await;
    assert!(resp.is_some(), "Expected auth_required, got timeout");
    let resp = resp.unwrap();
    assert_eq!(resp["type"], "auth_required");
    assert!(resp["message"].is_string());

    // 然后发送有效 token → 可以继续认证
    auth_ws(&mut client, &key).await;

    client.close().await;
}

#[tokio::test]
async fn test_ws_auth_failed_with_invalid_token() {
    let (_guard, _state, key, addr) = ws_server().await;
    let mut client = connect_ws(addr, &key).await;

    // 发送无效 token → 应收到 auth_failed 后断开
    client.send_text(r#"{"token":"wrong-key"}"#).await;
    let resp = client.recv_json().await;
    assert!(resp.is_some(), "Expected auth_failed, got timeout");
    let resp = resp.unwrap();
    assert_eq!(resp["type"], "auth_failed");

    // 连接应已关闭，再次接收应返回 None
    let next = client.recv_json().await;
    assert!(
        next.is_none(),
        "Connection should be closed after auth_failed"
    );
}

#[tokio::test]
async fn test_ws_event_streaming_after_auth() {
    let (_guard, state, key, addr) = ws_server().await;
    let mut client = connect_ws(addr, &key).await;

    // 认证
    auth_ws(&mut client, &key).await;

    // 发布事件到 EventBus（等待一小段时间确保 WS handler 已订阅在 select! 中）
    tokio::time::sleep(Duration::from_millis(100)).await;

    state
        .event_bus
        .publish(easybot_core::types::event::GatewayEvent {
            event_type: "message.inbound".to_string(),
            source: "test".to_string(),
            timestamp: 1234567890,
            data: json!({
                "platform": "telegram",
                "chat_id": "42",
                "text": "hello from ws test",
            }),
            metadata: None,
        });

    // 客户端应收到事件帧
    let event = client.recv_json().await;
    assert!(event.is_some(), "Expected event frame, got timeout");
    let event = event.unwrap();
    assert_eq!(event["type"], "event", "Frame should be type 'event'");
    assert_eq!(event["event"], "message.inbound");
    assert_eq!(event["data"]["text"], "hello from ws test");
    assert_eq!(event["seq"], 1);
    assert_eq!(event["timestamp"], 1234567890);

    client.close().await;
}

#[tokio::test]
async fn test_ws_multiple_events_sequential() {
    let (_guard, state, key, addr) = ws_server().await;
    let mut client = connect_ws(addr, &key).await;
    auth_ws(&mut client, &key).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 发布两个事件
    state
        .event_bus
        .publish(easybot_core::types::event::GatewayEvent {
            event_type: "message.inbound".to_string(),
            source: "test".to_string(),
            timestamp: 1000,
            data: json!({"platform":"telegram","chat_id":"42","text": "first"}),
            metadata: None,
        });

    tokio::time::sleep(Duration::from_millis(50)).await;

    state
        .event_bus
        .publish(easybot_core::types::event::GatewayEvent {
            event_type: "adapter.connected".to_string(),
            source: "test".to_string(),
            timestamp: 2000,
            data: json!({"platform": "telegram"}),
            metadata: None,
        });

    // 收集两个事件（不假定跨类型事件顺序，EventBus 使用多 channel 轮询）
    let mut events: Vec<Value> = Vec::new();
    for _ in 0..2 {
        let ev = client.recv_json().await;
        assert!(ev.is_some(), "Expected event");
        events.push(ev.unwrap());
    }

    // 验证 seq 递增
    assert_eq!(events[0]["seq"], 1);
    assert_eq!(events[1]["seq"], 2);

    // 验证两种事件类型都收到了
    let types: Vec<&str> = events
        .iter()
        .map(|e| e["event"].as_str().unwrap())
        .collect();
    assert!(
        types.contains(&"message.inbound"),
        "Expected message.inbound event"
    );
    assert!(
        types.contains(&"adapter.connected"),
        "Expected adapter.connected event"
    );

    client.close().await;
}

#[tokio::test]
async fn test_ws_multiple_clients_receive_events() {
    let (_guard, state, key, addr) = ws_server().await;

    // 两个独立客户端连接并认证
    let mut client1 = connect_ws(addr, &key).await;
    auth_ws(&mut client1, &key).await;

    let mut client2 = connect_ws(addr, &key).await;
    auth_ws(&mut client2, &key).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 发布事件
    state
        .event_bus
        .publish(easybot_core::types::event::GatewayEvent {
            event_type: "message.inbound".to_string(),
            source: "test".to_string(),
            timestamp: 9999,
            data: json!({"platform":"telegram","chat_id":"42","text": "broadcast test"}),
            metadata: None,
        });

    // 两个客户端都应收到
    let e1 = client1.recv_json().await;
    assert!(e1.is_some(), "Client 1 should receive event");
    assert_eq!(e1.unwrap()["data"]["text"], "broadcast test");

    let e2 = client2.recv_json().await;
    assert!(e2.is_some(), "Client 2 should receive event");
    assert_eq!(e2.unwrap()["data"]["text"], "broadcast test");

    client1.close().await;
    client2.close().await;
}

#[tokio::test]
async fn test_ws_subjects_are_isolated_by_target_grants() {
    let (_guard, state, _admin_key, addr) = ws_server().await;
    let (_, key_a) = state
        .auth_manager
        .create_key("subject-a", vec!["websocketconnect".into()], None)
        .await
        .unwrap();
    let (_, key_b) = state
        .auth_manager
        .create_key("subject-b", vec!["websocketconnect".into()], None)
        .await
        .unwrap();
    let auth_a = state.auth_manager.authenticate(&key_a).await.unwrap();
    let auth_b = state.auth_manager.authenticate(&key_b).await.unwrap();
    state
        .auth_manager
        .create_target_grant(
            &auth_a.subject_id,
            "qq",
            "group-a",
            vec![easybot_core::auth::target_actions::INBOUND_READ.into()],
            "test",
        )
        .await
        .unwrap();
    state
        .auth_manager
        .create_target_grant(
            &auth_b.subject_id,
            "qq",
            "group-b",
            vec![easybot_core::auth::target_actions::INBOUND_READ.into()],
            "test",
        )
        .await
        .unwrap();

    let mut client_a = connect_ws(addr, &key_a).await;
    auth_ws(&mut client_a, &key_a).await;
    let mut client_b = connect_ws(addr, &key_b).await;
    auth_ws(&mut client_b, &key_b).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    state
        .event_bus
        .publish(easybot_core::types::event::GatewayEvent {
            event_type: "message.inbound".into(),
            source: "qq".into(),
            timestamp: 42,
            data: json!({"platform":"qq","chat_id":"group-a","text":"secret-a"}),
            metadata: None,
        });

    let event_a = client_a
        .recv_json()
        .await
        .expect("subject A receives group A");
    assert_eq!(event_a["data"]["text"], "secret-a");
    assert!(
        client_b.recv_json().await.is_none(),
        "subject B must not receive group A"
    );
    client_a.close().await;
    client_b.close().await;
}

#[tokio::test]
async fn test_ws_http_auth_failed() {
    let (_guard, _state, _key, addr) = ws_server().await;

    // WS upgrade 不再在 HTTP 层鉴权（new WebSocket() 无法设自定义头）。
    // 不带 HTTP Authorization 头 → 连接应成功建立（旧行为会返回 401）
    let uri_str = format!("ws://{}/api/v1/ws", addr);
    let request: http::Request<()> = uri_str
        .into_client_request()
        .expect("Failed to build WS upgrade request");
    let (ws_stream, _) = connect_async(request)
        .await
        .expect("WS upgrade should succeed without auth header");
    // 连接成功即通过测试——鉴权已下放到连接后的 frame 层
    let (_write, _read) = ws_stream.split();
}

#[tokio::test]
async fn test_ws_client_clean_disconnect() {
    let (_guard, state, key, addr) = ws_server().await;
    let mut client = connect_ws(addr, &key).await;
    auth_ws(&mut client, &key).await;

    // 客户端主动发送 Close 帧
    client.close().await;

    // 等待服务器处理关闭帧并释放资源
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 服务器应仍可接受新连接
    let mut client2 = connect_ws(addr, &key).await;
    auth_ws(&mut client2, &key).await;

    // 等待 WS handler 订阅就绪
    tokio::time::sleep(Duration::from_millis(100)).await;

    state
        .event_bus
        .publish(easybot_core::types::event::GatewayEvent {
            event_type: "message.inbound".to_string(),
            source: "test".to_string(),
            timestamp: 12345,
            data: json!({"platform":"telegram","chat_id":"42","text": "after disconnect"}),
            metadata: None,
        });

    let event = client2.recv_json().await;
    assert!(event.is_some(), "New client should still receive events");
    assert_eq!(event.unwrap()["data"]["text"], "after disconnect");

    client2.close().await;
}
