//! WebSocket 实时推送路由
//!
//! 外部客户端通过 WebSocket 连接接收实时事件推送。

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use tracing::{info, warn};
use crate::AppState;

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
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: WebSocket, state: AppState) {
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

    info!("WebSocket client connected");

    loop {
        tokio::select! {
            // 接收客户端消息
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if !authenticated {
                            // 尝试认证
                            if text.contains("\"token\"") {
                                // 简化认证：假设任何带 token 的消息都通过
                                // Phase 4 接入真实 API Key 验证
                                authenticated = true;
                                let _ = sender.send(Message::Text(
                                    r#"{"type":"auth_ok"}"#.into()
                                )).await;
                            } else {
                                let _ = sender.send(Message::Text(
                                    r#"{"type":"auth_required","message":"Please send auth frame with token"}"#.into()
                                )).await;
                            }
                        } else {
                            // 处理客户端帧 (Phase 2+ 实现)
                            handle_client_frame(&text, &state).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }

            // 推送网关事件到客户端
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

                        if sender.send(Message::Text(frame.to_string().into())).await.is_err() {
                            warn!("WebSocket client disconnected (send failed)");
                            break;
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

    info!("WebSocket client disconnected");
}

/// 处理客户端发来的业务帧
async fn handle_client_frame(text: &str, _state: &AppState) {
    // Phase 2: 支持客户端发送消息、订阅过滤等
    // 当前仅做 debug 日志
    tracing::debug!("WS client frame: {}", text);
}
