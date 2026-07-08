//! WebSocket 实时推送路由
//!
//! 外部客户端通过 WebSocket 连接接收实时事件推送。
//! 连接数受 config.api.websocket.max_clients 限制，超出返回 503。

use crate::AppState;
use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::{IntoResponse, Response},
};
use easybot_core::types::event::event_types;
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use std::time::Instant;
use tokio::sync::OwnedSemaphorePermit;
use tracing::{info, warn};

/// 直接序列化的 WebSocket 事件帧（避免 serde_json::json!() 宏创建中间 Value 树）
#[derive(Serialize)]
struct WsEventFrame<'a> {
    #[serde(rename = "type")]
    type_: &'a str,
    event: &'a str,
    data: &'a serde_json::Value,
    seq: u64,
    timestamp: i64,
}

/// WebSocket 实时事件流
///
/// 通过 WebSocket 连接订阅网关事件的实时推送。连接后需要先发送认证帧。
/// 认证帧示例: { "token": "your-api-key" }
/// 认证成功后会自动推送 message.inbound, message.sent, adapter.connected 等事件。
#[utoipa::path(
    get,
    path = "/api/v1/ws",
    tag = "WebSocket",
    responses(
        (status = 101, description = "WebSocket upgrade successful"),
        (status = 400, description = "WebSocket upgrade failed"),
    )
)]
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    // 检查并发连接数限制
    let permit = match state.ws_semaphore.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "too many WebSocket connections",
            )
                .into_response();
        }
    };
    ws.max_frame_size(64 * 1024) // 64KB 帧限制
        .max_message_size(256 * 1024) // 256KB 消息限制
        .on_upgrade(move |socket| handle_ws(socket, state, permit))
        .into_response()
}

async fn handle_ws(socket: WebSocket, state: AppState, _permit: OwnedSemaphorePermit) {
    let (mut sender, mut receiver) = socket.split();

    // 认证前使用空流（pending() 永远不产生事件），认证后才创建真实订阅
    let mut event_stream: std::pin::Pin<
        Box<dyn futures::Stream<Item = easybot_core::types::event::GatewayEvent> + Send>,
    > = Box::pin(futures::stream::pending());

    let mut authenticated = false;
    let mut event_seq: u64 = 0;
    let mut dropped_events: u32 = 0;
    const MAX_DROPPED_EVENTS: u32 = 50; // 连续丢弃超过 N 个事件则断开

    // SECURITY: Limit authentication attempts to prevent brute-force
    let mut auth_attempts: u32 = 0;
    const MAX_AUTH_ATTEMPTS: u32 = 5;
    // SECURITY: Require auth within N seconds after connection
    let auth_deadline = Instant::now() + std::time::Duration::from_secs(10);
    // SECURITY: Per-connection frame rate limit (max 10 frames/sec)
    let mut frame_count: u32 = 0;
    let mut frame_window_start = Instant::now();
    const MAX_FRAMES_PER_SEC: u32 = 10;

    // 心跳配置
    let heartbeat_secs = state.config.api.websocket.heartbeat_interval_secs.max(5);
    let hb_duration = std::time::Duration::from_secs(heartbeat_secs);
    let mut heartbeat_timer = tokio::time::interval(hb_duration);
    heartbeat_timer.tick().await; // 跳过第一次立即触发
    let mut last_pong = Instant::now();
    let pong_timeout = hb_duration * 2; // 2 倍心跳间隔内无 pong 则断开

    // 追踪活跃连接数
    if let Some(ref metrics) = state.metrics {
        metrics.inc_websocket_connections();
    }

    info!("WebSocket client connected");

    loop {
        tokio::select! {
            // 接收客户端消息
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // SECURITY: Per-connection frame rate limiting
                        frame_count += 1;
                        let elapsed = frame_window_start.elapsed();
                        if elapsed >= std::time::Duration::from_secs(1) {
                            frame_count = 1;
                            frame_window_start = Instant::now();
                        } else if frame_count > MAX_FRAMES_PER_SEC {
                            warn!("WS client exceeded frame rate limit, disconnecting");
                            let _ = sender.send(Message::Text(
                                r#"{"type":"error","message":"Rate limit exceeded"}"#.into()
                            )).await;
                            break;
                        }

                        if !authenticated {
                            // SECURITY: Check auth deadline
                            if Instant::now() > auth_deadline {
                                warn!("WS client auth deadline exceeded");
                                let _ = sender.send(Message::Text(
                                    r#"{"type":"auth_failed","message":"Authentication timeout"}"#.into()
                                )).await;
                                break;
                            }

                            // SECURITY: Limit auth attempts
                            auth_attempts += 1;
                            if auth_attempts > MAX_AUTH_ATTEMPTS {
                                warn!("WS client exceeded max auth attempts");
                                let _ = sender.send(Message::Text(
                                    r#"{"type":"auth_failed","message":"Too many authentication attempts"}"#.into()
                                )).await;
                                break;
                            }

                            // 通过 ApiKeyManager 进行真实认证
                            let token = serde_json::from_str::<serde_json::Value>(&text)
                                .ok()
                                .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string()));

                            match token {
                                Some(ref key) => {
                                    match state.auth_manager.authenticate(key).await {
                                        Ok(auth_info) => {
                                            // 根据 API Key 的 event_filters 订阅事件
                                            let event_refs: Vec<&str> =
                                                if auth_info.event_filters.is_empty() {
                                                    // 空数组 = 全部事件（向后兼容）
                                                    event_types::all().to_vec()
                                                } else {
                                                    auth_info
                                                        .event_filters
                                                        .iter()
                                                        .map(|s| s.as_str())
                                                        .collect()
                                                };
                                            event_stream = Box::pin(
                                                state.event_bus.subscribe_many(&event_refs),
                                            );
                                            authenticated = true;
                                            let _ = sender.send(Message::Text(
                                                r#"{"type":"auth_ok"}"#.into()
                                            )).await;
                                        }
                                        Err(_) => {
                                            // SECURITY: Add delay on failed auth to slow brute-force
                                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                            let _ = sender.send(Message::Text(
                                                r#"{"type":"auth_failed","message":"Invalid API key"}"#.into()
                                            )).await;
                                            // Don't break — allow retry up to MAX_AUTH_ATTEMPTS
                                        }
                                    }
                                }
                                None => {
                                    let _ = sender.send(Message::Text(
                                        r#"{"type":"auth_required","message":"Send auth frame: {\"token\":\"your-api-key\"}"}"#.into()
                                    )).await;
                                }
                            }
                        } else {
                            // 检查应用层 pong 心跳回复 — 客户端通过 Text 帧回复 {"type":"pong"}
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                                if val.get("type").and_then(|t| t.as_str()) == Some("pong") {
                                    last_pong = Instant::now();
                                } else {
                                    // 非心跳帧 → 处理业务帧
                                    handle_client_frame(&text, &state).await;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // RFC 6455: 回复 Pong（底层 tungstenite 通常自动处理，显式处理更安全）
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                        last_pong = Instant::now();
                    }
                    Some(Ok(Message::Pong(_))) => {
                        // 浏览器自动回复的 Pong（控制帧级别），也更新心跳防超时
                        last_pong = Instant::now();
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        warn!("WebSocket protocol error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // 心跳定时器：定期发送 Ping，检测客户端存活
            _ = heartbeat_timer.tick() => {
                if last_pong.elapsed() > pong_timeout {
                    warn!(
                        "WebSocket client timed out (no pong for {:?})",
                        pong_timeout
                    );
                    break;
                }
                if sender.send(Message::Text(r#"{"type":"ping"}"#.into())).await.is_err() {
                    break;
                }
            }

            // 推送网关事件到客户端（带背压处理）
            event = event_stream.next() => {
                match event {
                    Some(event) => {
                        event_seq += 1;
                        // 透传原始 payload 的控制（生产环境隐藏 metadata）
                        // metadata 现在是预序列化的 JSON 字符串（优化：避免 Value 树分配）
                        let mut event_data = event.data;
                        if !state.config.api.raw_payload_enabled {
                            if let Some(obj) = event_data.as_object_mut() {
                                obj.remove("metadata");
                            }
                        } else if let Some(obj) = event_data.as_object_mut() {
                            // raw_payload_enabled: 将字符串 metadata 解析回 Value 保持兼容
                            if let Some(serde_json::Value::String(s)) = obj.get("metadata")
                                && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
                            {
                                obj["metadata"] = parsed;
                            }
                        }

                        // 直接序列化帧（避免 serde_json::json!() 宏创建中间 Value 树）
                        let frame = serde_json::to_string(&WsEventFrame {
                            type_: "event",
                            event: &event.event_type,
                            data: &event_data,
                            seq: event_seq,
                            timestamp: event.timestamp,
                        }).unwrap_or_default();

                        // 超时发送：100ms 内发不出去则丢弃，防止慢客户端反压
                        let msg = Message::Text(frame.into());
                        let send_fut = sender.send(msg);
                        tokio::select! {
                            result = send_fut => {
                                if result.is_err() {
                                    warn!("WebSocket client disconnected (send failed)");
                                    break;
                                }
                                dropped_events = 0;
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                                dropped_events += 1;
                                warn!("WS client too slow, dropped event #{}", event_seq);
                                if dropped_events > MAX_DROPPED_EVENTS {
                                    info!("WS client disconnected (too many dropped events)");
                                    break;
                                }
                            }
                        }
                    }
                    None => break,
                }
            }
        }
    }

    if let Some(ref metrics) = state.metrics {
        metrics.dec_websocket_connections();
    }

    info!("WebSocket client disconnected");
}

/// 处理客户端发来的业务帧
async fn handle_client_frame(text: &str, _state: &AppState) {
    // Phase 2: 支持客户端发送消息、订阅过滤等
    // SECURITY: Only log frame length, not content (may contain sensitive data)
    tracing::trace!("WS client frame received ({} bytes)", text.len());
}
