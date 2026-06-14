//! QQ 频道机器人适配器
//!
//! 使用 QQ 频道机器人 API（WebSocket Gateway + HTTP API）实现消息收发。
//! 架构类似 Discord 适配器：
//! - Gateway WebSocket 用于接收消息（AT_MESSAGE_CREATE 等事件）
//! - HTTP REST API 用于发送消息

mod types;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::message::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::GatewayEvent;
use types::*;

/// QQ API 基础 URL（正式环境）
const QQ_API: &str = "https://api.sgroup.qq.com";

/// QQ 频道机器人适配器
pub struct QqAdapter {
    platform_name: String,
    display_name: String,
    config: Option<AdapterConfig>,
    state: AdapterState,
    bot_info: Option<BotInfo>,
    capabilities: Vec<Capability>,
    messages_in: AtomicU64,
    messages_out: AtomicU64,
    errors: AtomicU64,
    event_bus: Option<Arc<EventBus>>,
    cancel_tx: Option<broadcast::Sender<()>>,
    http_client: Option<reqwest::Client>,
    bot_user_id: Option<String>,
}

impl QqAdapter {
    pub fn new() -> Self {
        Self {
            platform_name: "qq".to_string(),
            display_name: "QQ".to_string(),
            config: None,
            state: AdapterState::Created,
            bot_info: None,
            capabilities: vec![
                Capability { name: CapabilityName::Text, supported: true, limits: None },
                Capability { name: CapabilityName::Image, supported: true, limits: None },
                Capability { name: CapabilityName::Markdown, supported: true, limits: None },
                Capability { name: CapabilityName::Group, supported: true, limits: None },
                Capability { name: CapabilityName::Thread, supported: true, limits: None },
                Capability { name: CapabilityName::MessageEdit, supported: true, limits: None },
                Capability { name: CapabilityName::MessageDelete, supported: true, limits: None },
            ],
            messages_in: AtomicU64::new(0),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            cancel_tx: None,
            http_client: None,
            bot_user_id: None,
        }
    }

    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    fn client(&self) -> Result<&reqwest::Client, GatewayError> {
        self.http_client.as_ref().ok_or_else(|| {
            GatewayError::Internal("HTTP client not initialized".to_string())
        })
    }

    /// 构造 Bearer token: "Bot {appid}.{app_token}"
    fn bot_token(&self) -> Result<String, GatewayError> {
        let config = self.config.as_ref()
            .ok_or_else(|| GatewayError::ConfigError("Adapter not initialized".to_string()))?;
        let app_id = config.extra.get("app_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::ConfigError("Missing 'app_id' in qq config.extra".to_string()))?;
        let token = config.token.as_deref()
            .ok_or_else(|| GatewayError::ConfigError("Missing 'token' for qq".to_string()))?;
        Ok(format!("Bot {}.{}", app_id, token))
    }

    /// QQ API GET
    async fn api_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, GatewayError> {
        let token = self.bot_token()?;
        let client = self.client()?;
        let url = format!("{}{}", QQ_API, path);
        let resp = client.get(&url)
            .header("Authorization", &token)
            .send().await
            .map_err(|e| GatewayError::Internal(format!("QQ GET {} failed: {}", path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!("QQ API error (GET {}): {} - {}", path, s, b)));
        }
        resp.json().await
            .map_err(|e| GatewayError::Internal(format!("QQ GET {} parse failed: {}", path, e)))
    }

    /// QQ API POST
    async fn api_post<T: serde::de::DeserializeOwned>(
        &self, path: &str, body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        let token = self.bot_token()?;
        let client = self.client()?;
        let url = format!("{}{}", QQ_API, path);
        let resp = client.post(&url)
            .header("Authorization", &token)
            .json(body)
            .send().await
            .map_err(|e| GatewayError::Internal(format!("QQ POST {} failed: {}", path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!("QQ API error (POST {}): {} - {}", path, s, b)));
        }
        resp.json().await
            .map_err(|e| GatewayError::Internal(format!("QQ POST {} parse failed: {}", path, e)))
    }

    /// QQ API PATCH
    async fn api_patch<T: serde::de::DeserializeOwned>(
        &self, path: &str, body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        let token = self.bot_token()?;
        let client = self.client()?;
        let url = format!("{}{}", QQ_API, path);
        let resp = client.patch(&url)
            .header("Authorization", &token)
            .json(body)
            .send().await
            .map_err(|e| GatewayError::Internal(format!("QQ PATCH {} failed: {}", path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!("QQ API error (PATCH {}): {} - {}", path, s, b)));
        }
        resp.json().await
            .map_err(|e| GatewayError::Internal(format!("QQ PATCH {} parse failed: {}", path, e)))
    }

    /// QQ API DELETE
    async fn api_delete(&self, path: &str) -> Result<(), GatewayError> {
        let token = self.bot_token()?;
        let client = self.client()?;
        let url = format!("{}{}", QQ_API, path);
        let resp = client.delete(&url)
            .header("Authorization", &token)
            .send().await
            .map_err(|e| GatewayError::Internal(format!("QQ DELETE {} failed: {}", path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!("QQ API error (DELETE {}): {} - {}", path, s, b)));
        }
        Ok(())
    }

}

// ── Gateway WebSocket 事件循环 ──

impl QqAdapter {
    #[allow(clippy::too_many_lines)]
    async fn gateway_loop(
        token: String,
        event_bus: Arc<EventBus>,
        bot_id: String,
        mut cancel_rx: broadcast::Receiver<()>,
    ) {
        loop {
            // 获取 Gateway URL
            let gw_url = match Self::fetch_gateway_url(&token).await {
                Some(url) => url,
                None => {
                    tracing::error!("QQ Gateway: failed to get gateway URL, retry 30s");
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
            };

            tracing::info!("QQ Gateway: connecting to {}", gw_url);

            let (ws_stream, _) = match tokio::select! {
                _ = cancel_rx.recv() => { tracing::info!("QQ cancelled"); return; }
                r = connect_async(&gw_url) => r,
            } {
                Ok(r) => r,
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
                        let p: GatewayPayload<serde_json::Value> = match serde_json::from_str(&t) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        if p.op == 10 {
                            if let Some(d) = p.d {
                                if let Ok(h) = serde_json::from_value::<HelloData>(d) {
                                    break h;
                                }
                            }
                        }
                    }
                    _ => { tokio::time::sleep(Duration::from_secs(1)).await; continue; }
                }
            };

            let hb_interval = Duration::from_millis(
                (hello.heartbeat_interval as f64 * 0.75) as u64
            );
            tracing::info!("QQ Gateway connected");

            // 发送 Identify
            let identify = serde_json::json!({
                "op": 2,
                "d": {
                    "token": token,
                    "intents": types::intents::AT_MESSAGE | types::intents::C2C_MESSAGE | types::intents::GROUP_AT_MESSAGE,
                    "shard": [0, 1],
                }
            });
            if write.send(Message::Text(identify.to_string())).await.is_err() {
                tracing::error!("QQ identify send failed");
                continue;
            }

            // 事件循环
            let mut seq: u64 = 0;
            let mut identified = false;
            let mut hb_timer = tokio::time::interval(hb_interval);
            hb_timer.tick().await; // 跳过第一次立即触发

            loop {
                tokio::select! {
                    _ = cancel_rx.recv() => {
                        tracing::info!("QQ Gateway cancelled");
                        return;
                    }
                    // 定时心跳
                    _ = hb_timer.tick() => {
                        let hb = serde_json::json!({"op": 1, "d": seq});
                        if write.send(Message::Text(hb.to_string())).await.is_err() {
                            tracing::warn!("QQ heartbeat failed");
                            break;
                        }
                    }
                    // 接收消息
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                let payload: GatewayPayload<serde_json::Value> =
                                    match serde_json::from_str(&text) {
                                        Ok(p) => p,
                                        Err(_) => continue,
                                    };

                                if let Some(s) = payload.s { seq = s; }

                                match payload.op {
                                    0 => {
                                        if !identified {
                                            if payload.t.as_deref() == Some("READY") {
                                                identified = true;
                                                tracing::info!("QQ Gateway ready");
                                                continue;
                                            }
                                        }
                                        if let Some(ref et) = payload.t {
                                            Self::handle_dispatch(et, &payload, &event_bus, &bot_id).await;
                                        }
                                    }
                                    7 => { tracing::info!("QQ reconnect requested"); break; }
                                    9 => { tracing::error!("QQ invalid session"); break; }
                                    11 => {} // Heartbeat ACK
                                    _ => {}
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

    async fn fetch_gateway_url(token: &str) -> Option<String> {
        let client = reqwest::Client::new();
        let resp = client.get(format!("{}/gateway/bot", QQ_API))
            .header("Authorization", token)
            .send().await.ok()?;
        let data: GatewayResponse = resp.json().await.ok()?;
        Some(data.url)
    }

    async fn handle_dispatch(
        event_type: &str,
        payload: &GatewayPayload<serde_json::Value>,
        event_bus: &EventBus,
        bot_id: &str,
    ) {
        let data = match payload.d.as_ref() {
            Some(d) => d,
            None => return,
        };

        match event_type {
            "AT_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE" | "GROUP_AT_MESSAGE_CREATE" => {
                let msg_event: QqMessageEvent = match serde_json::from_value(data.clone()) {
                    Ok(m) => m,
                    Err(_) => return,
                };

                if msg_event.author.id == *bot_id { return; }

                let chat_id = msg_event.channel_id;
                let chat_type = match event_type {
                    "C2C_MESSAGE_CREATE" => ChatType::Dm,
                    _ => ChatType::Group,
                };

                let inbound = InboundMessage {
                    id: msg_event.id,
                    platform: "qq".to_string(),
                    chat_id,
                    chat_type: chat_type.clone(),
                    chat_name: None,
                    text: msg_event.content,
                    author: MessageAuthor {
                        id: msg_event.author.id,
                        name: msg_event.author.username,
                        is_bot: msg_event.author.bot,
                    },
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    thread_id: None,
                    is_group: chat_type == ChatType::Group,
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

#[async_trait]
impl PlatformAdapter for QqAdapter {
    fn platform_name(&self) -> &str { &self.platform_name }
    fn display_name(&self) -> &str { &self.display_name }
    fn capabilities(&self) -> &[Capability] { &self.capabilities }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        let has_app_id = config.extra.get("app_id").and_then(|v| v.as_str()).is_some();
        let has_token = config.token.is_some();

        if !has_app_id || !has_token {
            return Ok(InitResult {
                ok: false,
                error: Some("QQ 适配器需要配置 extra.app_id 和 token".to_string()),
            });
        }

        self.config = Some(config);
        self.http_client = Some(reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| GatewayError::Internal(format!("Failed to create HTTP client: {}", e)))?);
        self.state = AdapterState::Starting;
        Ok(InitResult { ok: true, error: None })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        let token = self.bot_token()?;

        let bot_user: QqUser = match self.api_get("/users/@me").await {
            Ok(u) => u,
            Err(e) => return Ok(ConnectResult {
                ok: false,
                error: Some(format!("QQ auth failed: {}", e)),
                bot_info: None,
            }),
        };

        let bot_id = bot_user.id.clone();
        let bot_info = BotInfo {
            name: bot_user.username.clone(),
            username: Some(bot_user.username),
            id: bot_id.clone(),
        };

        self.state = AdapterState::Connected;
        self.bot_info = Some(bot_info.clone());
        self.bot_user_id = Some(bot_id.clone());
        tracing::info!("QQ adapter connected: {} (id={})", bot_info.name, bot_info.id);

        if let Some(ref event_bus) = self.event_bus {
            let (cancel_tx, cancel_rx) = broadcast::channel(1);
            self.cancel_tx = Some(cancel_tx);
            let eb = event_bus.clone();
            tokio::spawn(async move {
                Self::gateway_loop(token, eb, bot_id, cancel_rx).await;
            });
        }

        Ok(ConnectResult { ok: true, error: None, bot_info: Some(bot_info) })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        if let Some(cancel_tx) = &self.cancel_tx { let _ = cancel_tx.send(()); }
        self.cancel_tx = None;
        self.state = AdapterState::Stopped;
        tracing::info!("QQ adapter disconnected");
        Ok(())
    }

    fn state(&self) -> AdapterState { self.state.clone() }

    async fn health(&self) -> HealthReport {
        HealthReport {
            status: if self.state == AdapterState::Connected { HealthStatus::Healthy } else { HealthStatus::Down },
            connected: self.state == AdapterState::Connected,
            last_connected_at: None, last_error_at: None, last_error: None,
            messages_in: self.messages_in.load(Ordering::Relaxed),
            messages_out: self.messages_out.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            uptime: None,
        }
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: self.config.as_ref().map(|c| c.enabled).unwrap_or(false),
            token_configured: self.config.as_ref().and_then(|c| c.token.as_ref()).is_some(),
            extra: self.config.as_ref().map(|c| c.extra.clone()).unwrap_or_default(),
        }
    }

    fn status_summary(&self) -> AdapterStatusSummary {
        AdapterStatusSummary {
            platform: self.platform_name.clone(),
            display_name: self.display_name.clone(),
            state: self.state.clone(),
            connected: self.state == AdapterState::Connected,
            health: None, last_error: None, uptime: None,
            messages_in: self.messages_in.load(Ordering::Relaxed),
            messages_out: self.messages_out.load(Ordering::Relaxed),
        }
    }

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        let path = format!("/channels/{}/messages", params.chat_id);
        let body = serde_json::json!({ "content": params.message.text, "msg_type": 0 });
        match self.api_post::<QqSendMessageResponse>(&path, &body).await {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult {
                    success: true,
                    message_id: Some(resp.id),
                    timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
                    error: None, error_code: None, retryable: false,
                })
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult::fail(e.to_string(), true))
            }
        }
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        let path = format!("/channels/{}/messages", params.chat_id);
        let image_url = params.media.url.unwrap_or_default();
        let body = serde_json::json!({
            "content": params.text.unwrap_or_default(),
            "image": image_url,
            "msg_type": 2,
        });
        match self.api_post::<QqSendMessageResponse>(&path, &body).await {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult {
                    success: true,
                    message_id: Some(resp.id),
                    timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
                    error: None, error_code: None, retryable: false,
                })
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult::fail(e.to_string(), true))
            }
        }
    }

    async fn edit_message(&self, params: EditMessageParams) -> Result<EditResult, GatewayError> {
        let path = format!("/channels/{}/messages/{}", params.chat_id, params.message_id);
        let body = serde_json::json!({ "content": params.message.text });
        match self.api_patch::<QqSendMessageResponse>(&path, &body).await {
            Ok(_) => Ok(EditResult { success: true, updated_at: Some(chrono::Utc::now().timestamp_millis()), error: None }),
            Err(e) => Ok(EditResult { success: false, updated_at: None, error: Some(e.to_string()) }),
        }
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<DeleteResult, GatewayError> {
        let path = format!("/channels/{}/messages/{}", chat_id, message_id);
        match self.api_delete(&path).await {
            Ok(_) => Ok(DeleteResult { success: true, error: None }),
            Err(e) => Ok(DeleteResult { success: false, error: Some(e.to_string()) }),
        }
    }

    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError> {
        let info: QqChannelInfo = self.api_get(&format!("/channels/{}", chat_id)).await?;
        Ok(ChatInfo {
            chat_id: info.id,
            name: Some(info.name),
            chat_type: ChatType::Group,
            member_count: None,
        })
    }

    async fn list_chats(&self, _filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        Ok(Vec::new())
    }
}

impl Default for QqAdapter {
    fn default() -> Self { Self::new() }
}

// ── 单元测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_adapter() {
        let adapter = QqAdapter::new();
        assert_eq!(adapter.platform_name(), "qq");
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[test]
    fn test_capabilities() {
        let adapter = QqAdapter::new();
        assert!(adapter.capabilities().iter().any(|c| c.name == CapabilityName::Text && c.supported));
    }

    #[tokio::test]
    async fn test_init_missing_config() {
        let mut adapter = QqAdapter::new();
        let r = adapter.init(AdapterConfig {
            enabled: true, token: None, api_key: None, extra: serde_json::json!({}),
        }).await.unwrap();
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn test_init_valid_config() {
        let mut adapter = QqAdapter::new();
        let r = adapter.init(AdapterConfig {
            enabled: true, token: Some("tk".into()), api_key: None,
            extra: serde_json::json!({"app_id": "123"}),
        }).await.unwrap();
        assert!(r.ok);
    }

    #[test]
    fn test_bot_token_format() {
        let adapter = QqAdapter {
            config: Some(AdapterConfig {
                enabled: true, token: Some("mytoken".into()), api_key: None,
                extra: serde_json::json!({"app_id": "123"}),
            }),
            ..QqAdapter::new()
        };
        assert_eq!(adapter.bot_token().unwrap(), "Bot 123.mytoken");
    }

    #[test]
    fn test_status_summary() {
        let adapter = QqAdapter::new();
        let s = adapter.status_summary();
        assert_eq!(s.platform, "qq");
        assert_eq!(s.display_name, "QQ");
    }
}
