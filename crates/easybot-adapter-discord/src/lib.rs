//! Discord 平台适配器
//!
//! 使用 Discord Bot API + Gateway WebSocket 实现消息收发。
//! Phase 3 实现:
//! - REST API 消息发送（sendMessage）
//! - Gateway WebSocket 实时接收消息（MESSAGE_CREATE）
//! - 通过 EventBus 发布入站消息事件

mod types;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::message::*;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use types::*;

/// Discord REST API 基础 URL (v10)
const DISCORD_API: &str = "https://discord.com/api/v10";

/// Discord Gateway WebSocket URL (v10)
const DISCORD_GATEWAY: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

/// Discord Gateway 主机名（用于 DNS 解析和 TLS SNI）
const DISCORD_GATEWAY_HOST: &str = "gateway.discord.gg";

/// Discord 适配器
pub struct DiscordAdapter {
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
    /// 缓存的 HTTP 客户端（连接池复用，延迟初始化）
    http_client: OnceLock<reqwest::Client>,
    /// Gateway 连接后得到的 bot user id，用于过滤自身消息
    bot_user_id: Option<String>,
}

impl DiscordAdapter {
    /// 创建 Discord 适配器
    pub fn new() -> Self {
        Self {
            platform_name: "discord".to_string(),
            display_name: "Discord".to_string(),
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
                    name: CapabilityName::TypingIndicator,
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
                // Discord 不支持 HTML 格式
                Capability {
                    name: CapabilityName::Html,
                    supported: false,
                    limits: None,
                },
                // Discord 没有内置交互式按钮（需要组件）
                Capability {
                    name: CapabilityName::Interactive,
                    supported: false,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Image,
                    supported: false,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Audio,
                    supported: false,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Video,
                    supported: false,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Document,
                    supported: false,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::ChatList,
                    supported: false,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Streaming,
                    supported: false,
                    limits: None,
                },
            ],
            messages_in: AtomicU64::new(0),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            cancel_tx: None,
            http_client: OnceLock::new(),
            bot_user_id: None,
        }
    }

    /// 获取或创建缓存的 HTTP 客户端
    fn http_client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(reqwest::Client::new)
    }

    /// 返回 REST API 基础 URL（支持通过 config.base_url 覆盖）
    fn api_base_url(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.base_url.as_deref())
            .unwrap_or(DISCORD_API)
    }

    /// 设置事件总线（在 init 之前调用）
    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// 构造 Authorization 头值
    fn auth_header(&self) -> String {
        let token = self
            .config
            .as_ref()
            .and_then(|c| c.token.clone())
            .unwrap_or_default();
        format!("Bot {}", token)
    }

    /// 统一调用 Discord REST API
    async fn api_call<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        endpoint: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, GatewayError> {
        let client = self.http_client();
        let url = format!("{}{}", self.api_base_url(), endpoint);

        let mut req = client
            .request(method, &url)
            .header("Authorization", self.auth_header());

        if let Some(json) = body {
            req = req.json(&json);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Discord API request failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                return Err(GatewayError::RateLimited {
                    retry_after_ms: 1000,
                });
            }
            return Err(GatewayError::Internal(format!(
                "Discord API {} {}: {}",
                status.as_u16(),
                endpoint,
                error_text
            )));
        }

        resp.json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Discord API JSON parse failed: {}", e)))
    }

    /// 将 Discord 消息转换为网关 InboundMessage
    fn convert_message(msg: DiscordMessage, bot_user_id: &str) -> Option<InboundMessage> {
        // 过滤自身消息，避免回环
        if msg.author.id == bot_user_id {
            return None;
        }

        let chat_type = if msg.guild_id.is_some() {
            ChatType::Group
        } else {
            ChatType::Dm
        };

        let author = MessageAuthor {
            id: msg.author.id,
            name: Some(msg.author.global_name.unwrap_or(msg.author.username)),
            is_bot: msg.author.bot.unwrap_or(false),
        };

        let timestamp = chrono::DateTime::parse_from_rfc3339(&msg.timestamp)
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(0);

        Some(InboundMessage {
            id: msg.id,
            platform: "discord".to_string(),
            chat_id: msg.channel_id,
            chat_name: None, // 消息对象不含频道名称，需额外 API 查询
            chat_type,
            text: msg.content.filter(|s| !s.is_empty()),
            author,
            timestamp,
            media: None,
            command: None,
            callback: None,
            reply_to: None,
            thread_id: None,
            mentioned: None,
            is_group: msg.guild_id.is_some(),
            metadata: None,
        })
    }

    /// 处理 Gateway Dispatch 事件
    fn handle_dispatch(
        event_type: &str,
        data: serde_json::Value,
        event_bus: &EventBus,
        bot_user_id: &str,
    ) {
        match event_type {
            "MESSAGE_CREATE" => match serde_json::from_value::<DiscordMessage>(data) {
                Ok(msg) => {
                    if let Some(inbound) = Self::convert_message(msg, bot_user_id) {
                        let event = GatewayEvent::new(
                            easybot_core::types::event::event_types::MESSAGE_INBOUND,
                            "discord",
                            serde_json::to_value(&inbound).unwrap_or_default(),
                        );
                        event_bus.publish(event);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to parse MESSAGE_CREATE: {}", e);
                }
            },
            _ => {
                tracing::debug!("Unhandled Discord Gateway event: {}", event_type);
            }
        }
    }

    /// 建立到 Discord Gateway 的 WebSocket 连接（使用 webpki-roots 验证 TLS）
    async fn connect_gateway() -> Result<
        (
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::http::Response<Option<Vec<u8>>>,
        ),
        Box<dyn std::error::Error + Send + Sync>,
    > {
        use rustls::pki_types::ServerName;
        use std::sync::Arc;
        use tokio::net::TcpStream;
        use tokio_rustls::TlsConnector;

        // 注册默认 CryptoProvider（rustls 0.23 需要显式指定）
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        // DNS 解析
        let addr = tokio::net::lookup_host((DISCORD_GATEWAY_HOST, 443))
            .await?
            .next()
            .ok_or("DNS resolution failed")?;

        // TCP 连接
        let tcp = TcpStream::connect(addr).await?;

        // TLS 配置（使用 webpki-roots 作为根证书）
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(config));
        let domain = ServerName::try_from(DISCORD_GATEWAY_HOST)?;
        let tls = connector.connect(domain, tcp).await?;

        // 包装为 MaybeTlsStream
        let stream = tokio_tungstenite::MaybeTlsStream::Rustls(tls);

        // 升级到 WebSocket
        let request = DISCORD_GATEWAY.into_client_request()?;
        let (ws_stream, resp) = tokio_tungstenite::client_async(request, stream).await?;
        Ok((ws_stream, resp))
    }

    /// 网关主循环（WebSocket 连接 + 心跳 + 事件接收）
    async fn gateway_loop(
        token: String,
        event_bus: Arc<EventBus>,
        bot_user_id: String,
        cancel_rx: broadcast::Receiver<()>,
    ) {
        tracing::info!("Discord Gateway connecting...");

        let (ws_stream, _) = match Self::connect_gateway().await {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Discord Gateway connect failed: {}", e);
                return;
            }
        };

        let (mut write, mut read) = ws_stream.split();

        // Step 1: 等待 Hello
        let hello_data = match Self::recv_hello(&mut read).await {
            Some(h) => h,
            None => return,
        };

        let hb_interval = Duration::from_millis(hello_data.heartbeat_interval);
        tracing::debug!(
            "Discord Gateway Hello received, heartbeat interval: {:?}",
            hb_interval
        );

        // Step 2: 发送 Identify
        let identify_msg = serde_json::json!({
            "op": OP_IDENTIFY,
            "d": {
                "token": token,
                "intents": DEFAULT_INTENTS,
                "properties": {
                    "$os": std::env::consts::OS,
                    "$browser": "easybot",
                    "$device": "easybot",
                },
            },
        });

        if let Err(e) = write.send(WsMessage::Text(identify_msg.to_string())).await {
            tracing::error!("Failed to send Identify: {}", e);
            return;
        }

        // Step 3: 等待 Ready
        let mut seq: Option<u64> = None;
        if !Self::wait_for_ready(&mut read, &mut write, &mut seq, &cancel_rx).await {
            return;
        }

        tracing::info!("Discord Gateway connected");

        // Step 4: 主循环（事件接收 + 心跳发送）
        Self::event_loop(
            &mut read,
            &mut write,
            &mut seq,
            hb_interval,
            &event_bus,
            &bot_user_id,
            &cancel_rx,
        )
        .await;

        tracing::info!("Discord Gateway loop ended");
    }

    /// 接收 Hello 并返回 HeartbeatInterval
    async fn recv_hello(
        read: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
                  + Unpin),
    ) -> Option<HelloData> {
        match read.next().await? {
            Ok(WsMessage::Text(text)) => {
                let payload: GatewayPayload = serde_json::from_str(&text).ok()?;
                if payload.op != OP_HELLO {
                    tracing::error!("Expected Hello (op=10), got op={}", payload.op);
                    return None;
                }
                serde_json::from_value(payload.d?).ok()
            }
            Ok(WsMessage::Close(frame)) => {
                tracing::error!("Gateway closed during Hello: {:?}", frame);
                None
            }
            Ok(_) => {
                tracing::error!("Unexpected message type during Hello");
                None
            }
            Err(e) => {
                tracing::error!("Gateway error during Hello: {}", e);
                None
            }
        }
    }

    /// 等待 Ready 事件（验证 Identify 成功）
    async fn wait_for_ready(
        read: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
                  + Unpin),
        _write: &mut (impl SinkExt<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
        seq: &mut Option<u64>,
        cancel_rx: &broadcast::Receiver<()>,
    ) -> bool {
        let mut cancel_rx = cancel_rx.resubscribe();

        loop {
            tokio::select! {
                _ = cancel_rx.recv() => {
                    tracing::info!("Discord Gateway cancelled before Ready");
                    return false;
                }
                msg = read.next() => {
                    match msg {
                        Some(Ok(WsMessage::Text(text))) => {
                            match serde_json::from_str::<GatewayPayload>(&text) {
                                Ok(payload) => {
                                    *seq = payload.s;
                                    if payload.op == OP_DISPATCH && payload.t.as_deref() == Some("READY") {
                                        return true;
                                    }
                                }
                                Err(e) => tracing::warn!("Pre-ready parse error: {}", e),
                            }
                        }
                        Some(Ok(WsMessage::Close(frame))) => {
                            tracing::error!("Gateway closed before Ready: {:?}", frame);
                            return false;
                        }
                        Some(Err(e)) => {
                            tracing::error!("Gateway error before Ready: {}", e);
                            return false;
                        }
                        None => {
                            tracing::error!("Gateway connection ended before Ready");
                            return false;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// 主事件循环：接收 Dispatch 事件 + 定时发送 Heartbeat
    async fn event_loop(
        read: &mut (impl StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
                  + Unpin),
        write: &mut (impl SinkExt<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
        seq: &mut Option<u64>,
        hb_interval: Duration,
        event_bus: &EventBus,
        bot_user_id: &str,
        cancel_rx: &broadcast::Receiver<()>,
    ) {
        let mut cancel_rx = cancel_rx.resubscribe();
        let mut heartbeat_timer = tokio::time::interval(hb_interval);
        heartbeat_timer.tick().await; // 消耗立即触发

        loop {
            tokio::select! {
                _ = cancel_rx.recv() => {
                    tracing::info!("Discord Gateway cancelled");
                    let _ = write.close().await;
                    break;
                }
                _ = heartbeat_timer.tick() => {
                    let hb = serde_json::json!({
                        "op": OP_HEARTBEAT,
                        "d": seq,
                    });
                    match write.send(WsMessage::Text(hb.to_string())).await {
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!("Discord heartbeat failed: {}", e);
                            break;
                        }
                    }
                }
                msg = read.next() => {
                    match msg {
                        Some(Ok(WsMessage::Text(text))) => {
                            match serde_json::from_str::<GatewayPayload>(&text) {
                                Ok(payload) => {
                                    match payload.op {
                                        OP_DISPATCH => {
                                            *seq = payload.s;
                                            if let (Some(event_type), Some(d)) = (payload.t, payload.d) {
                                                Self::handle_dispatch(
                                                    &event_type,
                                                    d,
                                                    event_bus,
                                                    bot_user_id,
                                                );
                                            }
                                        }
                                        OP_HEARTBEAT_ACK => {
                                            // 心跳确认，无额外操作
                                        }
                                        OP_RECONNECT => {
                                            tracing::warn!("Discord Gateway requested reconnect");
                                            break;
                                        }
                                        OP_INVALID_SESSION => {
                                            tracing::warn!("Discord Gateway invalid session, need re-identify");
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to parse Gateway payload: {}", e);
                                }
                            }
                        }
                        Some(Ok(WsMessage::Close(frame))) => {
                            tracing::info!("Discord Gateway closed: {:?}", frame);
                            break;
                        }
                        Some(Err(e)) => {
                            tracing::error!("Discord Gateway error: {}", e);
                            break;
                        }
                        None => {
                            tracing::info!("Discord Gateway connection ended");
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

impl Default for DiscordAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PlatformAdapter for DiscordAdapter {
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
        if config.token.is_none() || config.token.as_ref().is_none_or(|t| t.is_empty()) {
            return Ok(InitResult {
                ok: false,
                error: Some("Discord bot token is required".to_string()),
            });
        }
        self.config = Some(config);
        self.state = AdapterState::Created;
        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        let token = self
            .config
            .as_ref()
            .and_then(|c| c.token.clone())
            .ok_or_else(|| GatewayError::ConfigError("Discord token not configured".to_string()))?;

        // Step 1: 通过 REST API 验证 Token 并获取 Bot 用户信息
        let bot_user: DiscordUser = match self
            .api_call(reqwest::Method::GET, "/users/@me", None)
            .await
        {
            Ok(u) => u,
            Err(e) => {
                return Ok(ConnectResult {
                    ok: false,
                    error: Some(format!("Discord auth failed: {}", e)),
                    bot_info: None,
                });
            }
        };

        let bot_id = bot_user.id.clone();
        let bot_info = BotInfo {
            name: bot_user.global_name.unwrap_or(bot_user.username.clone()),
            username: Some(bot_user.username),
            id: bot_id.clone(),
        };

        self.state = AdapterState::Connected;
        self.bot_info = Some(bot_info.clone());
        self.bot_user_id = Some(bot_id.clone());

        tracing::info!(
            "Discord adapter connected: {} (id={})",
            bot_info.name,
            bot_info.id,
        );

        // Step 2: 启动 Gateway WebSocket 连接（如果配置了 EventBus）
        if let Some(ref event_bus) = self.event_bus {
            let (cancel_tx, cancel_rx) = broadcast::channel(1);
            self.cancel_tx = Some(cancel_tx);

            let token_clone = token;
            let eb = event_bus.clone();

            tokio::spawn(async move {
                Self::gateway_loop(token_clone, eb, bot_id, cancel_rx).await;
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
        tracing::info!("Discord adapter disconnected");
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state.clone()
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            status: if self.state == AdapterState::Connected {
                HealthStatus::Healthy
            } else {
                HealthStatus::Down
            },
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

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        let body = serde_json::json!({
            "content": params.message.text,
        });

        let endpoint = format!("/channels/{}/messages", params.chat_id);

        match self
            .api_call::<DiscordMessage>(reqwest::Method::POST, &endpoint, Some(body))
            .await
        {
            Ok(msg) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult::ok(msg.id))
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult::fail(e.to_string(), true))
            }
        }
    }

    async fn send_typing(&self, chat_id: &str) -> Result<(), GatewayError> {
        let endpoint = format!("/channels/{}/typing", chat_id);
        self.api_call::<serde_json::Value>(
            reqwest::Method::POST,
            &endpoint,
            Some(serde_json::json!({})),
        )
        .await?;
        Ok(())
    }

    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError> {
        let endpoint = format!("/channels/{}", chat_id);
        let channel: DiscordChannel = self.api_call(reqwest::Method::GET, &endpoint, None).await?;

        let chat_type = match channel.channel_type {
            1 | 3 => ChatType::Dm, // DM 或 GROUP_DM
            _ => ChatType::Group,
        };

        Ok(ChatInfo {
            chat_id: channel.id,
            name: channel.name,
            chat_type,
            member_count: None,
        })
    }

    async fn edit_message(&self, params: EditMessageParams) -> Result<EditResult, GatewayError> {
        let body = serde_json::json!({
            "content": params.message.text,
        });

        let endpoint = format!(
            "/channels/{}/messages/{}",
            params.chat_id, params.message_id
        );

        match self
            .api_call::<DiscordMessage>(reqwest::Method::PATCH, &endpoint, Some(body))
            .await
        {
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
        let client = self.http_client();
        let url = format!(
            "{}/channels/{}/messages/{}",
            self.api_base_url(),
            chat_id,
            message_id
        );

        let resp = client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Discord API delete failed: {}", e)))?;

        let status = resp.status();
        if status.is_success() {
            Ok(DeleteResult {
                success: true,
                error: None,
            })
        } else {
            let error_text = resp.text().await.unwrap_or_default();
            Ok(DeleteResult {
                success: false,
                error: Some(format!("Discord API {}: {}", status.as_u16(), error_text)),
            })
        }
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: self
                .config
                .as_ref()
                .is_some_and(|c| c.enabled != Some(false)),
            token_configured: self
                .config
                .as_ref()
                .and_then(|c| c.token.as_ref())
                .is_some_and(|t| !t.is_empty()),
            extra: serde_json::json!({}),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_platform_name() {
        let adapter = DiscordAdapter::new();
        assert_eq!(adapter.platform_name(), "discord");
    }

    #[test]
    fn test_display_name() {
        let adapter = DiscordAdapter::new();
        assert_eq!(adapter.display_name(), "Discord");
    }

    #[test]
    fn test_capabilities() {
        let adapter = DiscordAdapter::new();
        assert!(adapter
            .capabilities()
            .iter()
            .any(|c| c.name == CapabilityName::Text));
    }

    #[test]
    fn test_default_state() {
        let adapter = DiscordAdapter::new();
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[test]
    fn test_default() {
        let adapter = DiscordAdapter::default();
        assert_eq!(adapter.platform_name(), "discord");
    }

    #[test]
    fn test_status_summary() {
        let adapter = DiscordAdapter::new();
        let s = adapter.status_summary();
        assert_eq!(s.platform, "discord");
        assert_eq!(s.display_name, "Discord");
        assert!(!s.connected);
    }

    #[tokio::test]
    async fn test_disconnect_idempotent() {
        let mut adapter = DiscordAdapter::new();
        adapter.disconnect().await.unwrap();
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_double_disconnect() {
        let mut adapter = DiscordAdapter::new();
        adapter.disconnect().await.unwrap();
        adapter.disconnect().await.unwrap();
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_health_before_init() {
        let adapter = DiscordAdapter::new();
        let h = adapter.health().await;
        assert_eq!(h.status, HealthStatus::Down);
        assert!(!h.connected);
    }

    #[tokio::test]
    async fn test_runtime_config_before_init() {
        let adapter = DiscordAdapter::new();
        let r = adapter.runtime_config();
        assert!(!r.enabled);
        assert!(!r.token_configured);
    }

    #[tokio::test]
    async fn test_runtime_config_after_init() {
        let mut adapter = DiscordAdapter::new();
        adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("token".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();
        let r = adapter.runtime_config();
        assert!(r.enabled);
        assert!(r.token_configured);
    }

    #[tokio::test]
    async fn test_get_chat_info() {
        let adapter = DiscordAdapter::new();
        let result = adapter.get_chat_info("123456").await;
        assert!(result.is_err(), "Expected error when not initialized");
    }

    #[tokio::test]
    async fn test_init_without_token() {
        let mut adapter = DiscordAdapter::new();
        let result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: None,
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn test_init_and_connect_without_real_token() {
        let mut adapter = DiscordAdapter::new();
        let init_result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("fake_discord_token".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert!(init_result.ok);

        // Without a real token, /users/@me fails → returns ok:false
        let result = adapter.connect().await.unwrap();
        assert!(!result.ok);
        assert!(result.error.is_some());
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[tokio::test]
    async fn test_send_message_mocked() {
        let mut adapter = DiscordAdapter::new();
        adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("fake_discord_token".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();

        let result = adapter
            .send(SendTextParams {
                chat_id: "123456789".to_string(),
                message: OutboundMessage {
                    text: "Hello, Discord!".to_string(),
                    parse_mode: ParseMode::Markdown,
                },
                reply_to: None,
                metadata: None,
            })
            .await
            .unwrap();

        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_convert_dm_message() {
        let msg = DiscordMessage {
            id: "111111111".to_string(),
            channel_id: "222222222".to_string(),
            guild_id: None,
            author: DiscordUser {
                id: "333333333".to_string(),
                username: "testuser".to_string(),
                global_name: Some("TestUser".to_string()),
                bot: Some(false),
                avatar: None,
            },
            content: Some("Hello from Discord!".to_string()),
            timestamp: "2024-06-01T12:00:00.000000+00:00".to_string(),
            edited_timestamp: None,
            mention_everyone: false,
            tts: false,
            msg_type: 0,
        };

        let inbound = DiscordAdapter::convert_message(msg, "bot_id").unwrap();
        assert_eq!(inbound.id, "111111111");
        assert_eq!(inbound.platform, "discord");
        assert_eq!(inbound.chat_id, "222222222");
        assert_eq!(inbound.chat_type, ChatType::Dm);
        assert!(!inbound.is_group);
        assert_eq!(inbound.text.as_deref(), Some("Hello from Discord!"));
        assert_eq!(inbound.author.id, "333333333");
        assert_eq!(inbound.author.name.as_deref(), Some("TestUser"));
    }

    #[test]
    fn test_convert_guild_message() {
        let msg = DiscordMessage {
            id: "111111111".to_string(),
            channel_id: "222222222".to_string(),
            guild_id: Some("444444444".to_string()),
            author: DiscordUser {
                id: "333333333".to_string(),
                username: "guilduser".to_string(),
                global_name: None,
                bot: Some(false),
                avatar: None,
            },
            content: Some("Guild message".to_string()),
            timestamp: "2024-06-01T12:00:00.000000+00:00".to_string(),
            edited_timestamp: None,
            mention_everyone: false,
            tts: false,
            msg_type: 0,
        };

        let inbound = DiscordAdapter::convert_message(msg, "bot_id").unwrap();
        assert_eq!(inbound.chat_type, ChatType::Group);
        assert!(inbound.is_group);
        assert_eq!(inbound.author.name.as_deref(), Some("guilduser"));
    }

    #[test]
    fn test_convert_own_message_is_filtered() {
        let msg = DiscordMessage {
            id: "111111111".to_string(),
            channel_id: "222222222".to_string(),
            guild_id: None,
            author: DiscordUser {
                id: "bot_id".to_string(),
                username: "mybot".to_string(),
                global_name: None,
                bot: Some(true),
                avatar: None,
            },
            content: Some("I said this".to_string()),
            timestamp: "2024-06-01T12:00:00.000000+00:00".to_string(),
            edited_timestamp: None,
            mention_everyone: false,
            tts: false,
            msg_type: 0,
        };

        let result = DiscordAdapter::convert_message(msg, "bot_id");
        assert!(result.is_none(), "Should filter bot's own messages");
    }

    // ── Gateway dispatch 测试 ──

    #[test]
    fn test_handle_dispatch_message_create() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        // 先订阅确保 publish 能找到 sender
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);

        let data = serde_json::json!({
            "id": "12345",
            "channel_id": "67890",
            "content": "Hello from Discord",
            "timestamp": "2024-06-01T12:00:00.000000+00:00",
            "author": {
                "id": "user_001",
                "username": "TestUser",
                "global_name": "Test User",
                "bot": false
            }
        });

        DiscordAdapter::handle_dispatch("MESSAGE_CREATE", data, &event_bus, "bot_id_001");

        let event = rx.try_recv().ok();
        assert!(event.is_some(), "Expected MESSAGE_INBOUND event");
        if let Some(e) = event {
            assert_eq!(e.event_type, "message.inbound");
            assert_eq!(e.source, "discord");
        }
    }

    #[test]
    fn test_handle_dispatch_self_message() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);

        let data = serde_json::json!({
            "id": "99999",
            "channel_id": "67890",
            "content": "I am the bot",
            "timestamp": "2024-06-01T12:00:00.000000+00:00",
            "author": {
                "id": "bot_self",
                "username": "MyBot",
                "global_name": "My Bot",
                "bot": true
            }
        });

        DiscordAdapter::handle_dispatch("MESSAGE_CREATE", data, &event_bus, "bot_self");

        let event = rx.try_recv().ok();
        assert!(event.is_none(), "Self messages should be filtered out");
    }

    #[test]
    fn test_handle_dispatch_ignored_event() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());
        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);

        let data = serde_json::json!({"dummy": true});

        DiscordAdapter::handle_dispatch("MESSAGE_UPDATE", data, &event_bus, "bot_id");

        let event = rx.try_recv().ok();
        assert!(event.is_none(), "Unhandled event type should not publish");
    }

    #[test]
    fn test_handle_dispatch_malformed_data() {
        let event_bus = Arc::new(easybot_core::bus::EventBus::new());

        // 缺少 author/id 等必需字段
        let data = serde_json::json!({
            "id": "12345",
            "content": "partial message"
        });

        DiscordAdapter::handle_dispatch("MESSAGE_CREATE", data, &event_bus, "bot_id");

        let mut rx = event_bus.subscribe(easybot_core::types::event::event_types::MESSAGE_INBOUND);
        let event = rx.try_recv().ok();
        assert!(event.is_none(), "Malformed message should not be published");
    }
}
