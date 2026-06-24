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
use futures::{SinkExt, StreamExt};
use std::time::Instant;
use tokio::sync::OwnedSemaphorePermit;
use tracing::{info, warn};

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
    ws.on_upgrade(move |socket| handle_ws(socket, state, permit))
        .into_response()
}

async fn handle_ws(socket: WebSocket, state: AppState, _permit: OwnedSemaphorePermit) {
    let (mut sender, mut receiver) = socket.split();

    // 订阅网关事件
    let mut event_rx = state.event_bus.subscribe_many(&[
        "message.inbound",
        "message.sent",
        "message.failed",
        "adapter.connected",
        "adapter.disconnected",
        "adapter.error",
        "callback.received",
    ]);

    let mut authenticated = false;
    let mut event_seq: u64 = 0;
    let mut dropped_events: u32 = 0;
    const MAX_DROPPED_EVENTS: u32 = 50; // 连续丢弃超过 N 个事件则断开

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
                        if !authenticated {
                            // 通过 ApiKeyManager 进行真实认证
                            let token = serde_json::from_str::<serde_json::Value>(&text)
                                .ok()
                                .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string()));

                            match token {
                                Some(ref key) => {
                                    match state.auth_manager.authenticate(key).await {
                                        Ok(_auth_info) => {
                                            authenticated = true;
                                            let _ = sender.send(Message::Text(
                                                r#"{"type":"auth_ok"}"#.into()
                                            )).await;
                                        }
                                        Err(_) => {
                                            let _ = sender.send(Message::Text(
                                                r#"{"type":"auth_failed","message":"Invalid API key"}"#.into()
                                            )).await;
                                            break;
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
                            // 处理客户端帧 (Phase 2+ 实现)
                            handle_client_frame(&text, &state).await;
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
                if sender.send(Message::Ping(axum::body::Bytes::new())).await.is_err() {
                    break;
                }
            }

            // 推送网关事件到客户端（带背压处理）
            event = event_rx.recv() => {
                match event {
                    Ok(event) => {
                        event_seq += 1;
                        let frame = serde_json::json!({
                            "type": "event",
                            "event": event.event_type,
                            "data": event.data,
                            "seq": event_seq,
                            "timestamp": event.timestamp,
                        });

                        // 超时发送：100ms 内发不出去则丢弃，防止慢客户端反压
                        let msg = Message::Text(frame.to_string().into());
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
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        let _ = sender.send(Message::Text(
                            serde_json::json!({"type":"lagged","dropped":n}).to_string().into()
                        )).await;
                    }
                    Err(_) => break,
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
    // 当前仅做 debug 日志
    tracing::debug!("WS client frame: {}", text);
}
