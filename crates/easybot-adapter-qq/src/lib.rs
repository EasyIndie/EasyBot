//! QQ 频道机器人适配器
//!
//! 使用 QQ 频道机器人 API（WebSocket Gateway + HTTP API）实现消息收发。
//! 架构类似 Discord 适配器：
//! - Gateway WebSocket 用于接收消息（AT_MESSAGE_CREATE 等事件）
//! - HTTP REST API 用于发送消息

mod types;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::message::*;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use types::*;

/// QQ API 基础 URL（正式环境）
const QQ_API: &str = "https://api.sgroup.qq.com";
/// QQ 鉴权 API 基础 URL（正式环境）
const QQ_AUTH_API: &str = "https://bots.qq.com";

// ── Access Token 管理 ──

/// QQ 统一机器人平台 Access Token 存储
///
/// 通过 `Arc<Mutex>` 在适配器与 Gateway 事件循环间共享。
/// 按需调用 `refresh()` 从 QQ 鉴权端点获取新 token。
/// Token 有效期 7200 秒，提前 60 秒触发刷新。
#[derive(Clone)]
struct QqTokenStore {
    state: Arc<Mutex<Option<(String, tokio::time::Instant)>>>,
    app_id: String,
    client_secret: String,
    auth_base_url: String,
}

impl QqTokenStore {
    fn new(app_id: String, client_secret: String, auth_base_url: String) -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
            app_id,
            client_secret,
            auth_base_url,
        }
    }

    /// 从 QQ 鉴权端点获取新 token
    async fn refresh(&self) -> Result<(), GatewayError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| GatewayError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        let body = serde_json::json!({
            "appId": self.app_id,
            "clientSecret": self.client_secret,
        });

        let resp = client
            .post(format!("{}/app/getAppAccessToken", self.auth_base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                GatewayError::Internal(format!("QQ getAppAccessToken request failed: {}", e))
            })?;

        let data: serde_json::Value = resp.json().await.map_err(|e| {
            GatewayError::Internal(format!("QQ getAppAccessToken parse failed: {}", e))
        })?;

        let access_token = data["access_token"].as_str().ok_or_else(|| {
            GatewayError::Internal("QQ getAppAccessToken: missing access_token".to_string())
        })?;

        let expires_in = data["expires_in"].as_u64().unwrap_or(7200);
        let expires_at =
            tokio::time::Instant::now() + Duration::from_secs(expires_in) - Duration::from_secs(60);

        let mut guard = self
            .state
            .lock()
            .map_err(|e| GatewayError::Internal(format!("Token mutex poisoned: {}", e)))?;
        *guard = Some((access_token.to_string(), expires_at));

        tracing::info!("QQ access token refreshed, expires in {}s", expires_in);

        Ok(())
    }

    /// 获取 `QQBot {access_token}` 格式的鉴权字符串
    fn get(&self) -> Result<String, GatewayError> {
        let guard = self
            .state
            .lock()
            .map_err(|e| GatewayError::Internal(format!("Token mutex poisoned: {}", e)))?;
        let (token, expires_at) = guard
            .as_ref()
            .ok_or_else(|| GatewayError::Internal("QQ access token not initialized".to_string()))?;

        if tokio::time::Instant::now() >= *expires_at {
            // 过期了但尚未刷新 — 返回当前 token 并记录警告
            // 调用方应确保在首次使用前 refresh()
            tracing::warn!("QQ access token may be expired");
        }

        Ok(format!("QQBot {}", token))
    }

    /// 检查是否需要刷新
    fn needs_refresh(&self) -> bool {
        match self.state.lock() {
            Ok(guard) => match guard.as_ref() {
                Some((_, expires_at)) => tokio::time::Instant::now() >= *expires_at,
                None => true,
            },
            Err(_) => true,
        }
    }
}

/// QQ 频道机器人适配器
pub struct QqAdapter {
    platform_name: String,
    display_name: String,
    config: Option<AdapterConfig>,
    state: AdapterState,
    bot_info: Option<BotInfo>,
    capabilities: Vec<Capability>,
    messages_in: Arc<AtomicU64>,
    messages_out: AtomicU64,
    errors: AtomicU64,
    event_bus: Option<Arc<EventBus>>,
    cancel_tx: Option<broadcast::Sender<()>>,
    heartbeat: Heartbeat,
    http_client: Option<reqwest::Client>,
    bot_user_id: Option<String>,
    token_store: Option<QqTokenStore>,
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
                Capability {
                    name: CapabilityName::Text,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Image,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Markdown,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Group,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Thread,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::MessageEdit,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::MessageDelete,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Interactive,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::ChatList,
                    supported: true,
                    limits: None,
                },
            ],
            messages_in: Arc::new(AtomicU64::new(0)),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            cancel_tx: None,
            heartbeat: Heartbeat::new(),
            http_client: None,
            bot_user_id: None,
            token_store: None,
        }
    }

    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// 返回 API 基础 URL（支持通过 config.base_url 覆盖）
    fn api_base_url(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.base_url.as_deref())
            .unwrap_or(QQ_API)
    }

    /// 返回鉴权 API 基础 URL（支持通过 config.extra["auth_base_url"] 覆盖）
    fn auth_base_url(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.extra.get("auth_base_url").and_then(|v| v.as_str()))
            .unwrap_or(QQ_AUTH_API)
    }

    fn client(&self) -> Result<&reqwest::Client, GatewayError> {
        self.http_client
            .as_ref()
            .ok_or_else(|| GatewayError::Internal("HTTP client not initialized".to_string()))
    }

    /// 获取鉴权头字符串：`QQBot {access_token}`
    fn bot_token(&self) -> Result<String, GatewayError> {
        self.token_store
            .as_ref()
            .ok_or_else(|| {
                GatewayError::ConfigError(
                    "Token store not initialized (call connect() first)".to_string(),
                )
            })?
            .get()
    }

    /// QQ API GET
    async fn api_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, GatewayError> {
        let token = self.bot_token()?;
        let client = self.client()?;
        let url = format!("{}{}", self.api_base_url(), path);
        let resp = client
            .get(&url)
            .header("Authorization", &token)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("QQ GET {} failed: {}", path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "QQ API error (GET {}): {} - {}",
                path, s, b
            )));
        }
        resp.json()
            .await
            .map_err(|e| GatewayError::Internal(format!("QQ GET {} parse failed: {}", path, e)))
    }

    /// QQ API POST
    async fn api_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        let token = self.bot_token()?;
        let client = self.client()?;
        let url = format!("{}{}", self.api_base_url(), path);
        let resp = client
            .post(&url)
            .header("Authorization", &token)
            .json(body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("QQ POST {} failed: {}", path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "QQ API error (POST {}): {} - {}",
                path, s, b
            )));
        }
        resp.json()
            .await
            .map_err(|e| GatewayError::Internal(format!("QQ POST {} parse failed: {}", path, e)))
    }

    /// QQ API PATCH
    async fn api_patch<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        let token = self.bot_token()?;
        let client = self.client()?;
        let url = format!("{}{}", self.api_base_url(), path);
        let resp = client
            .patch(&url)
            .header("Authorization", &token)
            .json(body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("QQ PATCH {} failed: {}", path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "QQ API error (PATCH {}): {} - {}",
                path, s, b
            )));
        }
        resp.json()
            .await
            .map_err(|e| GatewayError::Internal(format!("QQ PATCH {} parse failed: {}", path, e)))
    }

    /// QQ API DELETE
    async fn api_delete(&self, path: &str) -> Result<(), GatewayError> {
        let token = self.bot_token()?;
        let client = self.client()?;
        let url = format!("{}{}", self.api_base_url(), path);
        let resp = client
            .delete(&url)
            .header("Authorization", &token)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("QQ DELETE {} failed: {}", path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "QQ API error (DELETE {}): {} - {}",
                path, s, b
            )));
        }
        Ok(())
    }
}

// ── 消息发送（自动判断频道/群聊） ──

impl QqAdapter {
    /// 尝试发送消息，自动判断是频道消息还是群聊消息
    async fn try_send(
        &self,
        chat_id: &str,
        body: &serde_json::Value,
    ) -> Result<QqSendMessageResponse, GatewayError> {
        // 先尝试频道端点
        let channel_path = format!("/channels/{}/messages", chat_id);
        match self
            .api_post::<QqSendMessageResponse>(&channel_path, body)
            .await
        {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                if e.to_string().contains("频道不存在") || e.to_string().contains("11263") {
                    tracing::debug!(
                        "QQ chat_id {} is not a channel, trying other endpoints",
                        chat_id
                    );
                } else {
                    return Err(e);
                }
            }
        }
        // 尝试群聊端点（v2 API）
        let group_path = format!("/v2/groups/{}/messages", chat_id);
        match self
            .api_post::<QqSendMessageResponse>(&group_path, body)
            .await
        {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                if e.to_string().contains("群")
                    || e.to_string().contains("group")
                    || e.to_string().contains("11263")
                {
                    tracing::debug!("QQ chat_id {} is not a group, trying C2C endpoint", chat_id);
                } else {
                    return Err(e);
                }
            }
        }
        // 尝试 C2C 私聊端点（v2 API）
        let c2c_path = format!("/v2/users/{}/messages", chat_id);
        self.api_post::<QqSendMessageResponse>(&c2c_path, body)
            .await
    }
}

// ── Gateway WebSocket 事件循环 ──

impl QqAdapter {
    /// 建立到 QQ Gateway 的 WebSocket 连接（使用 native-tls / 系统 CA 证书）
    async fn connect_gateway(
        ws_url: &str,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        use native_tls::TlsConnector as NativeTlsBuilder;
        use tokio::net::TcpStream;
        use tokio_native_tls::TlsConnector;

        // 解析 URL 获取 hostname
        let uri = ws_url.parse::<tokio_tungstenite::tungstenite::http::Uri>()?;
        let host = uri.host().ok_or("No host in gateway URL")?.to_string();
        let port = uri.port_u16().unwrap_or(443);

        // DNS 解析
        let addr = tokio::net::lookup_host((host.clone(), port))
            .await?
            .next()
            .ok_or("DNS resolution failed")?;

        // TCP 连接
        let tcp = TcpStream::connect(addr).await?;

        // TLS 配置（使用系统 CA 证书 — macOS SecureTransport / Linux OpenSSL）
        let native_tls = NativeTlsBuilder::new()
            .map_err(|e| format!("Failed to build native-tls connector: {}", e))?;
        let connector = TlsConnector::from(native_tls);
        let tls = connector.connect(&host, tcp).await?;

        // 包装为 MaybeTlsStream
        let stream = MaybeTlsStream::NativeTls(tls);

        // 升级到 WebSocket
        let (ws_stream, _) = tokio_tungstenite::client_async(uri, stream).await?;
        Ok(ws_stream)
    }

    #[allow(clippy::too_many_lines)]
    async fn gateway_loop(
        token_store: QqTokenStore,
        base_url: String,
        event_bus: Arc<EventBus>,
        bot_id: String,
        mut cancel_rx: broadcast::Receiver<()>,
        messages_in: Arc<AtomicU64>,
        heartbeat: Heartbeat,
    ) {
        loop {
            // 每次重连前刷新 access token
            if token_store.needs_refresh() {
                if let Err(e) = token_store.refresh().await {
                    tracing::error!("QQ token refresh failed: {}, retry 30s", e);
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
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
                    "intents": types::intents::AT_MESSAGE | types::intents::C2C_MESSAGE | types::intents::GROUP_AT_MESSAGE,
                    "shard": [0, 1],
                }
            });
            if write
                .send(Message::Text(identify.to_string()))
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
                        if write.send(Message::Text(hb.to_string())).await.is_err() {
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
                                let payload: GatewayPayload<serde_json::Value> =
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
                                            Self::handle_dispatch(et, &payload, &event_bus, &bot_id, &messages_in).await;
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

    async fn fetch_gateway_url(token_store: &QqTokenStore, base_url: &str) -> Option<String> {
        let token = token_store.get().ok()?;
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/gateway/bot", base_url))
            .header("Authorization", &token)
            .send()
            .await
            .ok()?;
        let data: GatewayResponse = resp.json().await.ok()?;
        Some(data.url)
    }

    /// 解析 QQ 时间戳字符串（ISO 8601）为毫秒时间戳
    fn parse_timestamp(ts: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(ts)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis())
    }

    async fn handle_dispatch(
        event_type: &str,
        payload: &GatewayPayload<serde_json::Value>,
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
                let msg_event: QqChannelMessageEvent = match serde_json::from_value(data.clone()) {
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
                    chat_id: msg_event.channel_id,
                    chat_type: ChatType::Group,
                    chat_name: None,
                    text: msg_event.content,
                    author: MessageAuthor {
                        id: msg_event.author.id,
                        name: msg_event.author.username,
                        is_bot: msg_event.author.bot,
                    },
                    timestamp: ts,
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    thread_id: None,
                    mentioned: Some(true), // 频道 @消息
                    is_group: true,
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
                let msg_event: QqGroupMessageEvent = match serde_json::from_value(data.clone()) {
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

                // 群消息没有 bot 字段，无法通过 id 过滤自身消息
                messages_in.fetch_add(1, Ordering::Relaxed);
                let ts = Self::parse_timestamp(&msg_event.timestamp);
                let openid = msg_event.group_openid.clone();
                let member_id = msg_event.author.member_openid.clone();
                let inbound = InboundMessage {
                    id: msg_event.id,
                    platform: "qq".to_string(),
                    chat_id: openid,
                    chat_type: ChatType::Group,
                    chat_name: None,
                    text: msg_event.content,
                    author: MessageAuthor {
                        id: member_id.clone(),
                        name: Some(member_id),
                        is_bot: false,
                    },
                    timestamp: ts,
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    thread_id: None,
                    mentioned: Some(true), // 旧协议仅推送 @消息
                    is_group: true,
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
                let msg_event: QqGroupMessageCreateEvent =
                    match serde_json::from_value(data.clone()) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::warn!("QQ failed to parse {}: {}", event_type, e);
                            return;
                        }
                    };
                // 通过 mentions 数组判断是否 @了机器人
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
                let inbound = InboundMessage {
                    id: msg_event.id,
                    platform: "qq".to_string(),
                    chat_id: openid,
                    chat_type: ChatType::Group,
                    chat_name: None,
                    text: msg_event.content,
                    author: MessageAuthor {
                        id: member_id.clone(),
                        name: Some(member_id),
                        is_bot: false,
                    },
                    timestamp: ts,
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    thread_id: None,
                    mentioned: Some(is_mentioned),
                    is_group: true,
                    metadata: Some(serde_json::json!({
                        "mentions": msg_event.mentions.iter().map(|m| serde_json::json!({
                            "is_you": m.is_you,
                            "scope": m.scope,
                            "username": m.username,
                        })).collect::<Vec<_>>(),
                        "message_scene": msg_event.message_scene.map(|s| serde_json::json!({
                            "source": s.source,
                        })),
                    })),
                };

                let event = GatewayEvent::new(
                    easybot_core::types::event::event_types::MESSAGE_INBOUND,
                    "qq",
                    serde_json::to_value(&inbound).unwrap_or_default(),
                );
                event_bus.publish(event);
            }
            "C2C_MESSAGE_CREATE" => {
                let msg_event: QqC2cMessageEvent = match serde_json::from_value(data.clone()) {
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
                    chat_id: user_openid.clone(),
                    chat_type: ChatType::Dm,
                    chat_name: None,
                    text: msg_event.content,
                    author: MessageAuthor {
                        id: user_openid.clone(),
                        name: None,
                        is_bot: false,
                    },
                    timestamp: ts,
                    media: None,
                    command: None,
                    callback: None,
                    reply_to: None,
                    thread_id: None,
                    mentioned: None, // C2C 私聊无 @概念
                    is_group: false,
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
    fn platform_name(&self) -> &str {
        &self.platform_name
    }
    fn display_name(&self) -> &str {
        &self.display_name
    }
    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        let has_app_id = config
            .extra
            .get("app_id")
            .and_then(|v| v.as_str())
            .is_some();
        let has_token = config.token.is_some();

        if !has_app_id || !has_token {
            return Ok(InitResult {
                ok: false,
                error: Some("QQ 适配器需要配置 extra.app_id 和 token (注意: token 字段需填写 AppSecret, 不再使用旧的静态 Bot Token)".to_string()),
            });
        }

        self.config = Some(config);
        self.http_client = Some(
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|e| {
                    GatewayError::Internal(format!("Failed to create HTTP client: {}", e))
                })?,
        );
        self.state = AdapterState::Starting;
        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| GatewayError::ConfigError("Adapter not initialized".to_string()))?;
        let app_id = config
            .extra
            .get("app_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                GatewayError::ConfigError("Missing 'app_id' in qq config.extra".to_string())
            })?;
        let client_secret = config.token.as_deref().ok_or_else(|| {
            GatewayError::ConfigError("Missing 'token' (client_secret) for qq".to_string())
        })?;

        // auth_base_url 支持通过 config.extra["auth_base_url"] 覆盖（测试/代理场景）
        let auth_base_url = self.auth_base_url().to_string();

        // 创建 TokenStore 并获取 access token
        let token_store =
            QqTokenStore::new(app_id.to_string(), client_secret.to_string(), auth_base_url);
        if let Err(e) = token_store.refresh().await {
            return Ok(ConnectResult {
                ok: false,
                error: Some(format!("QQ auth failed (getAppAccessToken): {}", e)),
                bot_info: None,
            });
        }

        // 先设置 token_store，以便 api_get 等方法使用
        let ts_clone = token_store.clone();
        self.token_store = Some(token_store);

        let bot_user: QqUser = match self.api_get("/users/@me").await {
            Ok(u) => u,
            Err(e) => {
                self.token_store = None;
                return Ok(ConnectResult {
                    ok: false,
                    error: Some(format!("QQ auth failed: {}", e)),
                    bot_info: None,
                });
            }
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
        tracing::info!(
            "QQ adapter connected: {} (id={})",
            bot_info.name,
            bot_info.id
        );

        if let Some(ref event_bus) = self.event_bus {
            let (cancel_tx, cancel_rx) = broadcast::channel(1);
            self.cancel_tx = Some(cancel_tx);
            let eb = event_bus.clone();
            let msg_in = self.messages_in.clone();
            let base_url = self
                .config
                .as_ref()
                .and_then(|c| c.base_url.clone())
                .unwrap_or_else(|| QQ_API.to_string());
            let hb = self.heartbeat.clone();
            tokio::spawn(async move {
                Self::gateway_loop(ts_clone, base_url, eb, bot_id, cancel_rx, msg_in, hb).await;
            });
        }

        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: Some(bot_info),
        })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        if let Some(cancel_tx) = &self.cancel_tx {
            let _ = cancel_tx.send(());
        }
        self.cancel_tx = None;
        self.state = AdapterState::Stopped;
        tracing::info!("QQ adapter disconnected");
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state.clone()
    }

    fn heartbeat_age_ms(&self) -> Option<i64> {
        Some(self.heartbeat.age_ms())
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            status: self.health_status(),
            connected: self.state == AdapterState::Connected,
            last_connected_at: None,
            last_error_at: None,
            last_error: None,
            messages_in: self.messages_in.load(Ordering::Relaxed),
            messages_out: self.messages_out.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            uptime: None,
        }
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: self
                .config
                .as_ref()
                .map(|c| c.enabled != Some(false))
                .unwrap_or(false),
            token_configured: self
                .config
                .as_ref()
                .and_then(|c| c.token.as_ref())
                .is_some(),
            extra: self
                .config
                .as_ref()
                .map(|c| c.extra.clone())
                .unwrap_or_default(),
        }
    }

    fn status_summary(&self) -> AdapterStatusSummary {
        AdapterStatusSummary {
            platform: self.platform_name.clone(),
            display_name: self.display_name.clone(),
            state: self.state.clone(),
            connected: self.state == AdapterState::Connected,
            health: None,
            last_error: None,
            uptime: None,
            messages_in: self.messages_in.load(Ordering::Relaxed),
            messages_out: self.messages_out.load(Ordering::Relaxed),
        }
    }

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        let body = if let Some(ref reply_to) = params.reply_to {
            serde_json::json!({
                "content": params.message.text,
                "msg_type": 0,
                "msg_id": reply_to,
            })
        } else {
            serde_json::json!({ "content": params.message.text, "msg_type": 0 })
        };
        match self.try_send(&params.chat_id, &body).await {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult {
                    success: true,
                    message_id: Some(resp.id),
                    timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
                    error: None,
                    error_code: None,
                    retryable: false,
                })
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult::fail(e.to_string(), true))
            }
        }
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        let image_url = params.media.url.unwrap_or_default();
        let body = serde_json::json!({
            "content": params.text.unwrap_or_default(),
            "image": image_url,
            "msg_type": 2,
        });
        match self.try_send(&params.chat_id, &body).await {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult {
                    success: true,
                    message_id: Some(resp.id),
                    timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
                    error: None,
                    error_code: None,
                    retryable: false,
                })
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult::fail(e.to_string(), true))
            }
        }
    }

    async fn send_interactive(
        &self,
        params: SendInteractiveParams,
    ) -> Result<SendResult, GatewayError> {
        // 构建 QQ 键盘
        let keyboard = QqKeyboard {
            content: QqKeyboardContent {
                rows: params
                    .keyboard
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(row_idx, row)| QqKeyboardRow {
                        buttons: row
                            .buttons
                            .iter()
                            .enumerate()
                            .map(|(btn_idx, btn)| {
                                let id = format!("btn_{}_{}", row_idx, btn_idx);
                                let (action_type, data) = if let Some(ref url) = btn.url {
                                    // URL 跳转
                                    (0u32, url.clone())
                                } else {
                                    // 回调（at 机器人）
                                    (
                                        2u32,
                                        btn.callback_data
                                            .clone()
                                            .unwrap_or_default(),
                                    )
                                };
                                QqKeyboardButton {
                                    id,
                                    render_data: QqButtonRenderData {
                                        label: btn.text.clone(),
                                        visited_label: btn.text.clone(),
                                        style: 1, // 蓝色主按钮
                                    },
                                    action: QqButtonAction {
                                        action_type,
                                        permission: QqButtonPermission {
                                            permission_type: 2, // 所有人可点击
                                        },
                                        data,
                                        enter: false,
                                    },
                                }
                            })
                            .collect(),
                    })
                    .collect(),
            },
        };

        let mut body = serde_json::json!({
            "content": params.text,
            "msg_type": 0,
            "keyboard": serde_json::to_value(&keyboard).unwrap_or_default(),
        });

        if let Some(ref reply_to) = params.reply_to {
            body["msg_id"] = serde_json::Value::String(reply_to.clone());
        }

        match self.try_send(&params.chat_id, &body).await {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult {
                    success: true,
                    message_id: Some(resp.id),
                    timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
                    error: None,
                    error_code: None,
                    retryable: false,
                })
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult::fail(e.to_string(), true))
            }
        }
    }

    async fn edit_message(&self, params: EditMessageParams) -> Result<EditResult, GatewayError> {
        let path = format!(
            "/channels/{}/messages/{}",
            params.chat_id, params.message_id
        );
        let body = serde_json::json!({ "content": params.message.text });
        match self.api_patch::<QqSendMessageResponse>(&path, &body).await {
            Ok(_) => Ok(EditResult {
                success: true,
                updated_at: Some(chrono::Utc::now().timestamp_millis()),
                error: None,
            }),
            Err(e) => Ok(EditResult {
                success: false,
                updated_at: None,
                error: Some(e.to_string()),
            }),
        }
    }

    async fn delete_message(
        &self,
        chat_id: &str,
        message_id: &str,
    ) -> Result<DeleteResult, GatewayError> {
        let path = format!("/channels/{}/messages/{}", chat_id, message_id);
        match self.api_delete(&path).await {
            Ok(_) => Ok(DeleteResult {
                success: true,
                error: None,
            }),
            Err(e) => Ok(DeleteResult {
                success: false,
                error: Some(e.to_string()),
            }),
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

    async fn list_chats(&self, filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        let want_group = filter
            .as_ref()
            .and_then(|f| f.chat_type.as_ref())
            .map(|t| *t == ChatType::Group)
            .unwrap_or(true);

        if !want_group {
            // QQ API 仅支持列出频道服务器，没有 DM/群聊列表端点
            return Ok(Vec::new());
        }

        match self.api_get::<Vec<QqGuild>>("/users/@me/guilds").await {
            Ok(guilds) => {
                let chats = guilds
                    .into_iter()
                    .map(|g| ChatInfo {
                        chat_id: g.id,
                        name: Some(g.name),
                        chat_type: ChatType::Group,
                        member_count: None,
                    })
                    .collect();
                Ok(chats)
            }
            Err(e) => {
                tracing::warn!("QQ list_chats: failed to get guilds: {}", e);
                Ok(Vec::new())
            }
        }
    }
}

impl Default for QqAdapter {
    fn default() -> Self {
        Self::new()
    }
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
    fn test_display_name() {
        let adapter = QqAdapter::new();
        assert_eq!(adapter.display_name(), "QQ");
    }

    #[test]
    fn test_default() {
        let adapter = QqAdapter::default();
        assert_eq!(adapter.platform_name(), "qq");
    }

    #[test]
    fn test_def_state_created() {
        let adapter = QqAdapter::new();
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[tokio::test]
    async fn test_disconnect_idempotent() {
        let mut adapter = QqAdapter::new();
        adapter.disconnect().await.unwrap();
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_double_disconnect() {
        let mut adapter = QqAdapter::new();
        adapter.disconnect().await.unwrap();
        adapter.disconnect().await.unwrap();
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_health_before_init() {
        let adapter = QqAdapter::new();
        let h = adapter.health().await;
        assert_eq!(h.status, HealthStatus::Down);
        assert!(!h.connected);
    }

    #[tokio::test]
    async fn test_runtime_config_before_init() {
        let adapter = QqAdapter::new();
        let r = adapter.runtime_config();
        assert!(!r.enabled);
        assert!(!r.token_configured);
    }

    #[tokio::test]
    async fn test_runtime_config_after_init() {
        let mut adapter = QqAdapter::new();
        adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("secret".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({"app_id": "123"}),
            })
            .await
            .unwrap();
        let r = adapter.runtime_config();
        assert!(r.enabled);
        assert!(r.token_configured);
    }

    #[tokio::test]
    async fn test_send_before_connect() {
        let adapter = QqAdapter::new();
        let result = adapter
            .send(SendTextParams {
                chat_id: "123".to_string(),
                message: OutboundMessage {
                    text: "hello".to_string(),
                    parse_mode: ParseMode::None,
                },
                reply_to: None,
                metadata: None,
            })
            .await
            .unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_get_chat_info_uninitialized() {
        let adapter = QqAdapter::new();
        let result = adapter.get_chat_info("123").await;
        assert!(result.is_err(), "Expected error when not initialized");
    }

    #[test]
    fn test_capabilities() {
        let adapter = QqAdapter::new();
        assert!(adapter
            .capabilities()
            .iter()
            .any(|c| c.name == CapabilityName::Text && c.supported));
    }

    #[tokio::test]
    async fn test_init_missing_config() {
        let mut adapter = QqAdapter::new();
        let r = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: None,
                api_key: None,
                extra: serde_json::json!({}),
                base_url: None,
            })
            .await
            .unwrap();
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn test_init_valid_config() {
        let mut adapter = QqAdapter::new();
        let r = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("tk".into()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({"app_id": "123"}),
            })
            .await
            .unwrap();
        assert!(r.ok);
    }

    #[test]
    fn test_bot_token_uninitialized() {
        let adapter = QqAdapter::new();
        // token_store 未设置时应返回错误
        assert!(adapter.bot_token().is_err());
    }

    #[test]
    fn test_token_store_new_needs_refresh() {
        let store = QqTokenStore::new("app123".into(), "secret".into(), QQ_AUTH_API.to_string());
        // 新创建的 store 还没有 token，需要 refresh
        assert!(store.needs_refresh());
    }

    #[test]
    fn test_token_store_get_uninitialized_returns_err() {
        let store = QqTokenStore::new("app123".into(), "secret".into(), QQ_AUTH_API.to_string());
        // 未 refresh 前 get 应该返回错误
        assert!(store.get().is_err());
    }

    #[test]
    fn test_token_store_clone() {
        let store = QqTokenStore::new("app123".into(), "secret".into(), QQ_AUTH_API.to_string());
        let cloned = store.clone();
        // 两个实例共享同一个 Arc<Mutex> 状态
        assert!(store.needs_refresh());
        assert!(cloned.needs_refresh());
    }

    #[test]
    fn test_status_summary() {
        let adapter = QqAdapter::new();
        let s = adapter.status_summary();
        assert_eq!(s.platform, "qq");
        assert_eq!(s.display_name, "QQ");
    }

    // ── Gateway dispatch 测试 ──

    #[tokio::test]
    async fn test_handle_dispatch_at_message() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        let data = serde_json::json!({
            "id": "msg001",
            "channel_id": "ch123",
            "guild_id": "guild456",
            "content": "Hello from QQ channel",
            "author": {"id": "user_001", "username": "TestUser", "bot": false},
            "timestamp": "2026-06-01T12:00:00+00:00"
        });
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(100),
            t: Some("AT_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "AT_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_id_001",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(
            event.is_some(),
            "Expected MESSAGE_INBOUND event for AT_MESSAGE_CREATE"
        );
        if let Some(e) = event {
            assert_eq!(e.event_type, "message.inbound");
            assert_eq!(e.source, "qq");
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_group_at() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        let data = serde_json::json!({
            "id": "gmsg001",
            "group_openid": "GROUP_OPENID_001",
            "content": "@bot hello group",
            "author": {"member_openid": "MEMBER_001"},
            "timestamp": "2026-06-01T12:00:00+00:00"
        });
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(101),
            t: Some("GROUP_AT_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "GROUP_AT_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_id",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(
            event.is_some(),
            "Expected MESSAGE_INBOUND for group message"
        );
        if let Some(e) = event {
            assert_eq!(e.data["is_group"], true);
            assert_eq!(e.data["chat_type"], "Group");
            assert_eq!(e.data["chat_id"], "GROUP_OPENID_001");
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_group_message_create_mentioned() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx =
            event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        // 2026 新版全量群消息，其中 @了机器人
        let data = serde_json::json!({
            "id": "ROBOT1.0_gmsg001",
            "group_openid": "GROUP_OPENID_NEW001",
            "content": "@bot hello everyone",
            "author": {"member_openid": "MEMBER_002"},
            "timestamp": "2026-06-01T12:00:00+00:00",
            "mentions": [
                {"is_you": true, "scope": "single", "username": "EasyBot"}
            ],
            "message_scene": {"source": "default", "ext": ["msg_idx=REFIDX_001"]},
            "message_type": 0
        });
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(200),
            t: Some("GROUP_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "GROUP_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_id",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(
            event.is_some(),
            "Expected MESSAGE_INBOUND for GROUP_MESSAGE_CREATE"
        );
        if let Some(e) = event {
            assert_eq!(e.data["is_group"], true);
            assert_eq!(e.data["chat_type"], "Group");
            assert_eq!(e.data["chat_id"], "GROUP_OPENID_NEW001");
            assert_eq!(e.data["mentioned"], true);
            // metadata contains mentions info
            assert!(e.data["metadata"]["mentions"].is_array());
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_group_message_create_not_mentioned() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx =
            event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        // 2026 新版全量群消息，没有 @机器人
        let data = serde_json::json!({
            "id": "ROBOT1.0_gmsg002",
            "group_openid": "GROUP_OPENID_NEW002",
            "content": "just a regular message",
            "author": {"member_openid": "MEMBER_003"},
            "timestamp": "2026-06-01T12:01:00+00:00",
            "mentions": [],
            "message_type": 0
        });
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(201),
            t: Some("GROUP_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "GROUP_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_id",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(
            event.is_some(),
            "Expected MESSAGE_INBOUND even for non-@ group message"
        );
        if let Some(e) = event {
            assert_eq!(e.data["is_group"], true);
            assert_eq!(e.data["chat_id"], "GROUP_OPENID_NEW002");
            assert_eq!(e.data["mentioned"], false);
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_c2c() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        let data = serde_json::json!({
            "id": "c2cmsg001",
            "content": "private message hello",
            "author": {"user_openid": "USER_OPENID_001"},
            "timestamp": "2026-06-01T12:00:00+00:00"
        });
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(102),
            t: Some("C2C_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "C2C_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_id",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(event.is_some(), "Expected MESSAGE_INBOUND for C2C message");
        if let Some(e) = event {
            assert_eq!(e.data["is_group"], false);
            assert_eq!(e.data["chat_type"], "Dm");
            assert_eq!(e.data["chat_id"], "USER_OPENID_001");
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_self_filter_channel() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        let data = serde_json::json!({
            "id": "selfmsg",
            "channel_id": "ch123",
            "content": "I am the bot",
            "author": {"id": "bot_self", "username": "MyBot", "bot": true},
            "timestamp": "2026-06-01T12:00:00+00:00"
        });
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(103),
            t: Some("AT_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "AT_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_self",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(event.is_none(), "Self messages should be filtered out");
        assert_eq!(messages_in.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_handle_dispatch_ignored_event() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        let data = serde_json::json!({"dummy": true});
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(104),
            t: Some("MESSAGE_REACTION_UPDATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "MESSAGE_REACTION_UPDATE",
            &payload,
            &event_bus,
            "bot_id",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(event.is_none(), "Unknown event type should not publish");
        assert_eq!(messages_in.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_handle_dispatch_missing_data() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        let payload = GatewayPayload::<serde_json::Value> {
            op: 0,
            d: None,
            s: Some(105),
            t: Some("AT_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "AT_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_id",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(event.is_none(), "Missing data should not publish event");
        assert_eq!(messages_in.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_handle_dispatch_malformed_data() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        // missing required fields (author, timestamp)
        let data = serde_json::json!({
            "id": "partial_msg",
            "content": "partial"
        });
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(106),
            t: Some("AT_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "AT_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_id",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        assert!(event.is_none(), "Malformed data should not publish event");
        assert_eq!(messages_in.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_handle_dispatch_c2c_self_not_filtered() {
        // C2C 和 group 消息没有 bot 字段，无法通过 id 过滤自身
        // 验证即使 bot_id 出现在 user_openid 中，消息仍被处理
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        let data = serde_json::json!({
            "id": "c2c_self",
            "content": "hello from self",
            "author": {"user_openid": "bot_self"},
            "timestamp": "2026-06-01T12:00:00+00:00"
        });
        let payload = GatewayPayload {
            op: 0,
            d: Some(data),
            s: Some(107),
            t: Some("C2C_MESSAGE_CREATE".to_string()),
        };

        QqAdapter::handle_dispatch(
            "C2C_MESSAGE_CREATE",
            &payload,
            &event_bus,
            "bot_self",
            &messages_in,
        )
        .await;

        let event = rx.try_recv().ok();
        // C2C 没有 bot 字段，不会被过滤
        assert!(event.is_some(), "C2C messages should not be self-filtered");
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }
}
