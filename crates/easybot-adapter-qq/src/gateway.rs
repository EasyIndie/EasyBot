//! QQ Gateway WebSocket 事件循环
//!
//! 管理与 QQ Bot Gateway 的 WebSocket 连接、心跳、事件分发。
//! 处理频道消息（AT_MESSAGE_CREATE）、群消息（GROUP_AT_MESSAGE_CREATE、
//! GROUP_MESSAGE_CREATE）和私聊消息（C2C_MESSAGE_CREATE）。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use easybot_core::bus::EventBus;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::message::*;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::tungstenite::Message;

use crate::auth::QqTokenStore;

impl crate::QqAdapter {
    /// 建立到 QQ Gateway 的 WebSocket 连接（使用 rustls）
    pub(crate) async fn connect_gateway(
        ws_url: &str,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url).await?;
        Ok(ws_stream)
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) async fn gateway_loop(
        token_store: QqTokenStore,
        base_url: String,
        event_bus: Arc<EventBus>,
        bot_id: String,
        mut cancel_rx: broadcast::Receiver<()>,
        messages_in: Arc<AtomicU64>,
        heartbeat: easybot_core::types::adapter::Heartbeat,
    ) {
        loop {
            // 每次重连前刷新 access token
            if token_store.needs_refresh()
                && let Err(e) = token_store.refresh().await
            {
                tracing::error!("QQ token refresh failed: {}, retry 30s", e);
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }

            // 获取 Gateway URL
            let gw_url = match Self::fetch_gateway_url(&token_store, &base_url).await {
                Some(url) => url,
                None => {
                    tracing::error!("QQ Gateway: failed to get gateway URL, retry 30s");
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
            };

            tracing::info!("QQ Gateway: connecting to {}", gw_url);

            let ws_stream = match tokio::select! {
                _ = cancel_rx.recv() => { tracing::info!("QQ cancelled"); return; }
                r = Self::connect_gateway(&gw_url) => r,
            } {
                Ok(ws) => ws,
                Err(e) => {
                    tracing::error!("QQ connect failed: {}", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            let (mut write, mut read) = ws_stream.split();

            // 等待 Hello
            let hello = loop {
                match read.next().await {
                    Some(Ok(Message::Text(t))) => {
                        let p: crate::types::GatewayPayload<serde_json::Value> =
                            match serde_json::from_str(&t) {
                                Ok(p) => p,
                                Err(_) => continue,
                            };
                        if p.op == 10
                            && let Some(d) = p.d
                            && let Ok(h) = serde_json::from_value::<crate::types::HelloData>(d)
                        {
                            break h;
                        }
                    }
                    _ => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            };

            let hb_interval =
                Duration::from_millis((hello.heartbeat_interval as f64 * 0.75) as u64);
            tracing::info!("QQ Gateway connected");

            // 发送 Identify（使用 QQBot {access_token} 格式）
            let token_str = match token_store.get() {
                Ok(t) => t,
                Err(e) => {
                    tracing::error!("QQ failed to get token: {}", e);
                    continue;
                }
            };
            let identify = serde_json::json!({
                "op": 2,
                "d": {
                    "token": token_str,
                    "intents": crate::types::intents::AT_MESSAGE
                        | crate::types::intents::C2C_MESSAGE
                        | crate::types::intents::GROUP_AT_MESSAGE,
                    "shard": [0, 1],
                }
            });
            if write
                .send(Message::Text(identify.to_string().into()))
                .await
                .is_err()
            {
                tracing::error!("QQ identify send failed");
                continue;
            }

            // 事件循环
            let mut seq: u64 = 0;
            let mut identified = false;
            let mut hb_timer = tokio::time::interval(hb_interval);
            hb_timer.tick().await; // 跳过第一次立即触发

            // 定期刷新 token（~3500s ≈ 7200s 的一半）
            let mut token_refresh_timer = tokio::time::interval(Duration::from_secs(3500));
            token_refresh_timer.tick().await; // 跳过第一次

            loop {
                tokio::select! {
                    _ = cancel_rx.recv() => {
                        tracing::info!("QQ Gateway cancelled");
                        return;
                    }
                    // 定时心跳
                    _ = hb_timer.tick() => {
                        let hb = serde_json::json!({"op": 1, "d": seq});
                        if write.send(Message::Text(hb.to_string().into())).await.is_err() {
                            tracing::warn!("QQ heartbeat failed");
                            break;
                        }
                    }
                    // 定时刷新 token
                    _ = token_refresh_timer.tick() => {
                        if let Err(e) = token_store.refresh().await {
                            tracing::warn!("QQ token refresh failed: {}", e);
                        }
                    }
                    // 接收消息
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                let payload: crate::types::GatewayPayload<serde_json::Value> =
                                    match serde_json::from_str(&text) {
                                        Ok(p) => p,
                                        Err(_) => continue,
                                    };

                                if let Some(s) = payload.s { seq = s; }

                                match payload.op {
                                    0 => {
                                        heartbeat.beat(); // liveness: Gateway event received
                                        if !identified
                                            && payload.t.as_deref() == Some("READY") {
                                                identified = true;
                                                tracing::info!("QQ Gateway ready");
                                                continue;
                                            }
                                        if let Some(ref et) = payload.t {
                                            tracing::debug!("QQ dispatch event: {}", et);
                                            Self::handle_dispatch(
                                                et, &payload, &event_bus, &bot_id, &messages_in,
                                            ).await;
                                        } else {
                                            tracing::debug!("QQ dispatch with no t field");
                                        }
                                    }
                                    7 => { tracing::info!("QQ reconnect requested"); break; }
                                    9 => { tracing::error!("QQ invalid session"); break; }
                                    11 => {
                                        heartbeat.beat(); // liveness: heartbeat ack received
                                        tracing::trace!("QQ heartbeat ack");
                                    }
                                    _ => { tracing::debug!("QQ unknown op: {}", payload.op); }
                                }
                            }
                            Some(Ok(Message::Close(_))) | None => { break; }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    pub(crate) async fn fetch_gateway_url(
        token_store: &QqTokenStore,
        base_url: &str,
    ) -> Option<String> {
        let token = token_store.get().ok()?;
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/gateway/bot", base_url))
            .header("Authorization", &token)
            .send()
            .await
            .ok()?;
        let data: crate::types::GatewayResponse = resp.json().await.ok()?;
        Some(data.url)
    }

    /// 解析 QQ 时间戳字符串（ISO 8601）为毫秒时间戳
    pub(crate) fn parse_timestamp(ts: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(ts)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis())
    }

    pub(crate) async fn handle_dispatch(
        event_type: &str,
        payload: &crate::types::GatewayPayload<serde_json::Value>,
        event_bus: &EventBus,
        bot_id: &str,
        messages_in: &AtomicU64,
    ) {
        let data = match payload.d.as_ref() {
            Some(d) => d,
            None => return,
        };

        match event_type {
            "AT_MESSAGE_CREATE" => {
                let msg_event: crate::types::QqChannelMessageEvent =
                    match serde_json::from_value(data.clone()) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("QQ failed to parse {}: {}", event_type, e);
                            return;
                        }
                    };
                tracing::info!(
                    "QQ {} from user={} id={} channel={}",
                    event_type,
                    msg_event.author.id,
                    msg_event.id,
                    msg_event.channel_id
                );

                if msg_event.author.id == *bot_id {
                    return;
                }

                messages_in.fetch_add(1, Ordering::Relaxed);
                let ts = Self::parse_timestamp(&msg_event.timestamp);

                let inbound = InboundMessage {
                    id: msg_event.id,
                    platform: "qq".to_string(),
                    msg_type: MessageType::Text,
                    text: msg_event.content,
                    sender: MessageSender {
                        id: msg_event.author.id,
                        name: msg_event.author.username.clone(),
                        username: msg_event.author.username.clone(),
                        avatar_url: None,
                        is_bot: msg_event.author.bot,
                        role: None,
                        language_code: None,
                    },
                    recipient: Some(bot_id.to_string()),
                    chat_id: msg_event.channel_id,
                    chat_name: None,
                    chat_type: ChatType::Group,
                    guild_id: msg_event.guild_id.clone(),
                    thread_id: None,
                    root_id: None,
                    timestamp: ts,
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    mentions: None,
                    mentioned: Some(true),
                    metadata: None,
                };

                let event = GatewayEvent::new(
                    easybot_core::types::event::event_types::MESSAGE_INBOUND,
                    "qq",
                    serde_json::to_value(&inbound).unwrap_or_default(),
                );
                event_bus.publish(event);
            }
            "GROUP_AT_MESSAGE_CREATE" => {
                let msg_event: crate::types::QqGroupMessageEvent =
                    match serde_json::from_value(data.clone()) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("QQ failed to parse {}: {}", event_type, e);
                            return;
                        }
                    };
                tracing::info!(
                    "QQ {} from member={} id={} group={}",
                    event_type,
                    msg_event.author.member_openid,
                    msg_event.id,
                    msg_event.group_openid
                );

                messages_in.fetch_add(1, Ordering::Relaxed);
                let ts = Self::parse_timestamp(&msg_event.timestamp);
                let openid = msg_event.group_openid.clone();
                let member_id = msg_event.author.member_openid.clone();
                let inbound = InboundMessage {
                    id: msg_event.id,
                    platform: "qq".to_string(),
                    msg_type: MessageType::Text,
                    text: msg_event.content,
                    sender: MessageSender {
                        id: member_id.clone(),
                        name: Some(member_id.clone()),
                        username: None,
                        avatar_url: None,
                        is_bot: false,
                        role: None,
                        language_code: None,
                    },
                    recipient: Some(bot_id.to_string()),
                    chat_id: openid,
                    chat_name: None,
                    chat_type: ChatType::Group,
                    guild_id: None,
                    thread_id: None,
                    root_id: None,
                    timestamp: ts,
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    mentions: None,
                    mentioned: Some(true),
                    metadata: None,
                };

                let event = GatewayEvent::new(
                    easybot_core::types::event::event_types::MESSAGE_INBOUND,
                    "qq",
                    serde_json::to_value(&inbound).unwrap_or_default(),
                );
                event_bus.publish(event);
            }
            "GROUP_MESSAGE_CREATE" => {
                let msg_event: crate::types::QqGroupMessageCreateEvent =
                    match serde_json::from_value(data.clone()) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("QQ failed to parse {}: {}", event_type, e);
                            return;
                        }
                    };
                let is_mentioned = msg_event.mentions.iter().any(|m| m.is_you);
                tracing::info!(
                    "QQ {} from member={} id={} group={} mentioned={}",
                    event_type,
                    msg_event.author.member_openid,
                    msg_event.id,
                    msg_event.group_openid,
                    is_mentioned
                );

                messages_in.fetch_add(1, Ordering::Relaxed);
                let ts = Self::parse_timestamp(&msg_event.timestamp);
                let openid = msg_event.group_openid.clone();
                let member_id = msg_event.author.member_openid.clone();
                let mentions: Option<Vec<MentionInfo>> = if msg_event.mentions.is_empty() {
                    None
                } else {
                    Some(
                        msg_event
                            .mentions
                            .iter()
                            .map(|m| MentionInfo {
                                user_id: None,
                                username: m.username.clone(),
                                scope: m.scope.clone(),
                            })
                            .collect(),
                    )
                };
                let inbound = InboundMessage {
                    id: msg_event.id,
                    platform: "qq".to_string(),
                    msg_type: MessageType::Text,
                    text: msg_event.content,
                    sender: MessageSender {
                        id: member_id.clone(),
                        name: Some(member_id.clone()),
                        username: None,
                        avatar_url: None,
                        is_bot: false,
                        role: None,
                        language_code: None,
                    },
                    recipient: Some(bot_id.to_string()),
                    chat_id: openid,
                    chat_name: None,
                    chat_type: ChatType::Group,
                    guild_id: None,
                    thread_id: None,
                    root_id: None,
                    timestamp: ts,
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    mentions,
                    mentioned: Some(is_mentioned),
                    metadata: msg_event.message_scene.as_ref().map(|s| {
                        serde_json::json!({
                            "message_scene": s,
                        })
                    }),
                };

                let event = GatewayEvent::new(
                    easybot_core::types::event::event_types::MESSAGE_INBOUND,
                    "qq",
                    serde_json::to_value(&inbound).unwrap_or_default(),
                );
                event_bus.publish(event);
            }
            "C2C_MESSAGE_CREATE" => {
                let msg_event: crate::types::QqC2cMessageEvent =
                    match serde_json::from_value(data.clone()) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("QQ failed to parse {}: {}", event_type, e);
                            return;
                        }
                    };
                tracing::info!(
                    "QQ {} from user={} id={}",
                    event_type,
                    msg_event.author.user_openid,
                    msg_event.id
                );

                messages_in.fetch_add(1, Ordering::Relaxed);
                let ts = Self::parse_timestamp(&msg_event.timestamp);
                let user_openid = msg_event.author.user_openid.clone();
                let inbound = InboundMessage {
                    id: msg_event.id,
                    platform: "qq".to_string(),
                    msg_type: MessageType::Text,
                    text: msg_event.content,
                    sender: MessageSender {
                        id: user_openid.clone(),
                        name: None,
                        username: None,
                        avatar_url: None,
                        is_bot: false,
                        role: None,
                        language_code: None,
                    },
                    recipient: Some(bot_id.to_string()),
                    chat_id: user_openid.clone(),
                    chat_name: None,
                    chat_type: ChatType::Dm,
                    guild_id: None,
                    thread_id: None,
                    root_id: None,
                    timestamp: ts,
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    mentions: None,
                    mentioned: None,
                    metadata: None,
                };

                let event = GatewayEvent::new(
                    easybot_core::types::event::event_types::MESSAGE_INBOUND,
                    "qq",
                    serde_json::to_value(&inbound).unwrap_or_default(),
                );
                event_bus.publish(event);
            }
            _ => {}
        }
    }
}
