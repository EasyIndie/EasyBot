//! Discord 平台适配器
//!
//! 使用 Discord Bot API + Gateway WebSocket 实现消息收发。
//! Phase 3 实现:
//! - REST API 消息发送（sendMessage）
//! - Gateway WebSocket 实时接收消息（MESSAGE_CREATE）
//! - 通过 EventBus 发布入站消息事件

mod types;

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::event::event_types;
use easybot_core::types::message::*;
use tokio::sync::broadcast;
use twilight_gateway::{CloseFrame, EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
use twilight_model::gateway::event::Event;
use types::*;

/// Discord REST API 基础 URL (v10)
const DISCORD_API: &str = "https://discord.com/api/v10";

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
    /// Background liveness heartbeat (updated by the Gateway task)
    heartbeat: Heartbeat,
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
                // Discord 支持 Message Components (buttons, select menus)
                Capability {
                    name: CapabilityName::Interactive,
                    supported: true,
                    limits: Some(CapabilityLimits {
                        max_text_length: None,
                        max_file_size: None,
                        max_buttons: Some(25), // 5 rows × 5 buttons
                    }),
                },
                Capability {
                    name: CapabilityName::Image,
                    supported: true,
                    limits: Some(CapabilityLimits {
                        max_text_length: None,
                        max_file_size: Some(8 * 1024 * 1024),
                        max_buttons: None,
                    }),
                },
                Capability {
                    name: CapabilityName::Audio,
                    supported: true,
                    limits: Some(CapabilityLimits {
                        max_text_length: None,
                        max_file_size: Some(8 * 1024 * 1024),
                        max_buttons: None,
                    }),
                },
                Capability {
                    name: CapabilityName::Video,
                    supported: true,
                    limits: Some(CapabilityLimits {
                        max_text_length: None,
                        max_file_size: Some(8 * 1024 * 1024),
                        max_buttons: None,
                    }),
                },
                Capability {
                    name: CapabilityName::Document,
                    supported: true,
                    limits: Some(CapabilityLimits {
                        max_text_length: None,
                        max_file_size: Some(8 * 1024 * 1024),
                        max_buttons: None,
                    }),
                },
                Capability {
                    name: CapabilityName::ChatList,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Streaming,
                    supported: true,
                    limits: None,
                },
            ],
            messages_in: AtomicU64::new(0),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            cancel_tx: None,
            heartbeat: Heartbeat::new(),
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
    /// 将 twilight_model::channel::Message 转换为 EasyBot InboundMessage
    fn convert_message(
        msg: &twilight_model::channel::Message,
        bot_user_id: &str,
    ) -> Option<InboundMessage> {
        // 过滤自身消息，避免回环
        if msg.author.id.to_string() == bot_user_id {
            return None;
        }

        let chat_type = if msg.guild_id.is_some() {
            ChatType::Group
        } else {
            ChatType::Dm
        };

        let author = MessageAuthor {
            id: msg.author.id.to_string(),
            name: Some(
                msg.author
                    .global_name
                    .clone()
                    .unwrap_or(msg.author.name.clone()),
            ),
            is_bot: msg.author.bot,
        };

        let timestamp = msg.timestamp.as_micros() / 1000;

        Some(InboundMessage {
            id: msg.id.to_string(),
            platform: "discord".to_string(),
            chat_id: msg.channel_id.to_string(),
            chat_name: None, // 消息对象不含频道名称，需额外 API 查询
            chat_type,
            text: Some(msg.content.clone()).filter(|s| !s.is_empty()),
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

    /// Gateway Shard 事件循环（使用 twilight-gateway SDK）
    async fn gateway_shard_loop(
        token: String,
        event_bus: Arc<EventBus>,
        bot_user_id: String,
        mut cancel_rx: broadcast::Receiver<()>,
        heartbeat: Heartbeat,
    ) {
        let intents = Intents::GUILD_MESSAGES | Intents::DIRECT_MESSAGES | Intents::MESSAGE_CONTENT;
        let mut shard = Shard::new(ShardId::ONE, token, intents);

        tracing::info!("Discord Gateway connecting...");

        loop {
            tokio::select! {
                _ = cancel_rx.recv() => {
                    tracing::info!("Discord Gateway cancelled");
                    shard.close(CloseFrame::NORMAL);
                    return;
                }
                event = shard.next_event(EventTypeFlags::all()) => {
                    match handle_gateway_event(event, &event_bus, &bot_user_id, &heartbeat) {
                        EventAction::Continue => {}
                        EventAction::Stop => return,
                    }
                }
            }
        }
    }
}

/// 处理单个 Gateway 事件的结果
#[derive(Debug, PartialEq, Eq)]
enum EventAction {
    /// 继续事件循环
    Continue,
    /// 停止事件循环（流已结束）
    Stop,
}

/// 处理单个 Discord Gateway 事件
///
/// 从 `gateway_shard_loop` 中提取以便对事件分发逻辑进行单元测试。
/// 泛型参数 `E: Display` 允许在测试中使用 `String` 作为错误类型，
/// 而生产代码使用 `twilight_gateway::error::ReceiveMessageError`。
fn handle_gateway_event<E: std::fmt::Display>(
    event: Option<Result<Event, E>>,
    event_bus: &EventBus,
    bot_user_id: &str,
    heartbeat: &Heartbeat,
) -> EventAction {
    match event {
        Some(Ok(Event::Ready(_))) => {
            heartbeat.beat();
            tracing::info!("Discord Gateway connected");
            EventAction::Continue
        }
        Some(Ok(Event::MessageCreate(msg))) => {
            heartbeat.beat();
            if let Some(inbound) = DiscordAdapter::convert_message(&msg.0, bot_user_id) {
                let event = GatewayEvent::new(
                    easybot_core::types::event::event_types::MESSAGE_INBOUND,
                    "discord",
                    serde_json::to_value(&inbound).unwrap_or_default(),
                );
                event_bus.publish(event);
            }
            EventAction::Continue
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "Discord Gateway error, shard will auto-reconnect");
            EventAction::Continue
        }
        Some(_) => {
            heartbeat.beat();
            EventAction::Continue
        }
        None => {
            tracing::info!("Discord Gateway stream ended");
            EventAction::Stop
        }
    }
}

impl Default for DiscordAdapter {
    fn default() -> Self {
        Self::new()
    }
}

fn publish_send_event(
    event_bus: &Option<Arc<EventBus>>,
    event_type: &str,
    chat_id: &str,
    result: &SendResult,
) {
    if let Some(bus) = event_bus {
        bus.publish(GatewayEvent::new(
            event_type,
            "discord",
            serde_json::json!({
                "platform": "discord",
                "chat_id": chat_id,
                "message_id": result.message_id,
                "success": result.success,
                "error": result.error,
                "error_code": result.error_code,
            }),
        ));
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
            let hb = self.heartbeat.clone();

            tokio::spawn(async move {
                Self::gateway_shard_loop(token_clone, eb, bot_id, cancel_rx, hb).await;
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

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        let body = serde_json::json!({
            "content": params.message.text,
        });

        let endpoint = format!("/channels/{}/messages", params.chat_id);

        let result = match self
            .api_call::<DiscordMessage>(reqwest::Method::POST, &endpoint, Some(body))
            .await
        {
            Ok(msg) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult::ok(msg.id)
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                SendResult::fail(e.to_string(), true)
            }
        };
        publish_send_event(
            &self.event_bus,
            if result.success {
                event_types::MESSAGE_SENT
            } else {
                event_types::MESSAGE_FAILED
            },
            &params.chat_id,
            &result,
        );
        Ok(result)
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

    async fn send_draft(&self, params: SendDraftParams) -> Result<DraftResult, GatewayError> {
        if let Some(ref msg_id) = params.message_id {
            // 更新已有草稿 → PATCH
            let endpoint = format!("/channels/{}/messages/{}", params.chat_id, msg_id);
            let body = serde_json::json!({ "content": params.text });

            match self
                .api_call::<DiscordMessage>(reqwest::Method::PATCH, &endpoint, Some(body))
                .await
            {
                Ok(_) => Ok(DraftResult {
                    success: true,
                    message_id: Some(msg_id.clone()),
                    error: None,
                }),
                Err(e) => {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    Ok(DraftResult {
                        success: false,
                        message_id: Some(msg_id.clone()),
                        error: Some(e.to_string()),
                    })
                }
            }
        } else {
            // 创建新草稿 → POST
            let endpoint = format!("/channels/{}/messages", params.chat_id);
            let body = serde_json::json!({ "content": params.text });

            match self
                .api_call::<DiscordMessage>(reqwest::Method::POST, &endpoint, Some(body))
                .await
            {
                Ok(msg) => {
                    self.messages_out.fetch_add(1, Ordering::Relaxed);
                    Ok(DraftResult {
                        success: true,
                        message_id: Some(msg.id),
                        error: None,
                    })
                }
                Err(e) => {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    Ok(DraftResult {
                        success: false,
                        message_id: None,
                        error: Some(e.to_string()),
                    })
                }
            }
        }
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        let client = self.http_client();
        let url = format!(
            "{}/channels/{}/messages",
            self.api_base_url(),
            params.chat_id
        );

        // Resolve file data and filename from base64 data or URL
        let (file_data, filename, content_type) = if let Some(data_b64) = &params.media.data {
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data_b64)
                .map_err(|e| GatewayError::Internal(format!("Base64 decode failed: {}", e)))?;
            let fname = params
                .media
                .filename
                .clone()
                .unwrap_or_else(|| "file".to_string());
            (decoded, fname, params.media.mime_type.clone())
        } else if let Some(file_url) = &params.media.url {
            let resp = client
                .get(file_url)
                .send()
                .await
                .map_err(|e| GatewayError::Internal(format!("Download failed: {}", e)))?;
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let data = resp
                .bytes()
                .await
                .map_err(|e| GatewayError::Internal(format!("Download read failed: {}", e)))?;
            let fname = params
                .media
                .filename
                .clone()
                .or_else(|| file_url.split('/').next_back().map(|s| s.to_string()))
                .unwrap_or_else(|| "file".to_string());
            (data.to_vec(), fname, ct)
        } else {
            let fail = SendResult::fail("No media data or URL provided".to_string(), false);
            publish_send_event(
                &self.event_bus,
                event_types::MESSAGE_FAILED,
                &params.chat_id,
                &fail,
            );
            return Ok(fail);
        };

        // Build the multipart file part
        let mut file_part = reqwest::multipart::Part::bytes(file_data).file_name(filename.clone());
        if !content_type.is_empty() {
            file_part = file_part
                .mime_str(&content_type)
                .map_err(|e| GatewayError::Internal(format!("Invalid mime type: {}", e)))?;
        }

        // Build payload_json with optional content, reply, and attachment reference
        let mut payload = serde_json::Map::new();
        let caption = params.text.or(params.media.caption);
        if let Some(ref text) = caption {
            payload.insert(
                "content".to_string(),
                serde_json::Value::String(text.clone()),
            );
        }
        if let Some(ref reply_to) = params.reply_to {
            payload.insert(
                "message_reference".to_string(),
                serde_json::json!({"message_id": reply_to}),
            );
        }
        // Discord multipart requires attachments array linking files[0] to the message
        payload.insert(
            "attachments".to_string(),
            serde_json::json!([{"id": 0, "filename": filename.clone()}]),
        );

        let payload_text = serde_json::Value::Object(payload).to_string();
        let payload_part = reqwest::multipart::Part::text(payload_text.clone())
            .mime_str("application/json")
            .map_err(|e| {
                GatewayError::Internal(format!("Failed to set payload_json MIME: {}", e))
            })?;

        let form = reqwest::multipart::Form::new()
            .part("payload_json", payload_part)
            .part("files[0]", file_part);

        let resp = client
            .post(&url)
            .header("Authorization", self.auth_header())
            .multipart(form)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Discord API upload failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 429 {
                let fail = SendResult::fail(format!("Rate limited: {}", error_text), true);
                publish_send_event(
                    &self.event_bus,
                    event_types::MESSAGE_FAILED,
                    &params.chat_id,
                    &fail,
                );
                return Ok(fail);
            }
            self.errors.fetch_add(1, Ordering::Relaxed);
            let fail = SendResult::fail(
                format!("Discord API {}: {}", status.as_u16(), error_text),
                false,
            );
            publish_send_event(
                &self.event_bus,
                event_types::MESSAGE_FAILED,
                &params.chat_id,
                &fail,
            );
            return Ok(fail);
        }

        let msg: DiscordMessage = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Discord API JSON parse failed: {}", e)))?;

        if msg.attachments.is_empty() {
            tracing::warn!(
                "Discord send_media: message {} sent but no attachments returned. \
                 content={:?}, payload_json text was: {}",
                msg.id,
                msg.content,
                payload_text,
            );
        } else {
            tracing::info!(
                "Discord send_media: message {} sent with {} attachment(s)",
                msg.id,
                msg.attachments.len(),
            );
        }

        self.messages_out.fetch_add(1, Ordering::Relaxed);
        let send_result = SendResult::ok(msg.id);
        publish_send_event(
            &self.event_bus,
            event_types::MESSAGE_SENT,
            &params.chat_id,
            &send_result,
        );
        Ok(send_result)
    }

    async fn send_interactive(
        &self,
        params: SendInteractiveParams,
    ) -> Result<SendResult, GatewayError> {
        // 构建 Discord Message Components
        // 每行 → 一个 Action Row (type=1)，每个按钮 → Button (type=2)
        let components: Vec<serde_json::Value> = params
            .keyboard
            .rows
            .iter()
            .map(|row| {
                let buttons: Vec<serde_json::Value> = row
                    .buttons
                    .iter()
                    .map(|btn| {
                        if let Some(ref url) = btn.url {
                            // Link 按钮 (style=5)
                            serde_json::json!({
                                "type": 2,
                                "style": 5,
                                "label": btn.text,
                                "url": url,
                            })
                        } else {
                            // 回调按钮 (style=1, Primary)
                            let custom_id = btn
                                .callback_data
                                .clone()
                                .unwrap_or_else(|| btn.text.clone());
                            serde_json::json!({
                                "type": 2,
                                "style": 1,
                                "label": btn.text,
                                "custom_id": custom_id,
                            })
                        }
                    })
                    .collect();

                serde_json::json!({
                    "type": 1,
                    "components": buttons,
                })
            })
            .collect();

        let body = serde_json::json!({
            "content": params.text,
            "components": components,
        });

        let endpoint = format!("/channels/{}/messages", params.chat_id);

        let result = match self
            .api_call::<DiscordMessage>(reqwest::Method::POST, &endpoint, Some(body))
            .await
        {
            Ok(msg) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult::ok(msg.id)
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                SendResult::fail(e.to_string(), true)
            }
        };
        publish_send_event(
            &self.event_bus,
            if result.success {
                event_types::MESSAGE_SENT
            } else {
                event_types::MESSAGE_FAILED
            },
            &params.chat_id,
            &result,
        );
        Ok(result)
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

    async fn list_chats(&self, filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        let mut chats: Vec<ChatInfo> = Vec::new();

        let want_dm = filter
            .as_ref()
            .and_then(|f| f.chat_type.as_ref())
            .map(|t| *t == ChatType::Dm)
            .unwrap_or(true);
        let want_group = filter
            .as_ref()
            .and_then(|f| f.chat_type.as_ref())
            .map(|t| *t == ChatType::Group)
            .unwrap_or(true);

        // 获取 DM 频道列表
        if want_dm {
            match self
                .api_call::<Vec<DiscordChannel>>(reqwest::Method::GET, "/users/@me/channels", None)
                .await
            {
                Ok(channels) => {
                    for ch in channels {
                        chats.push(ChatInfo {
                            chat_id: ch.id,
                            name: ch.name,
                            chat_type: ChatType::Dm,
                            member_count: None,
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!("Discord list_chats: failed to get DM channels: {}", e);
                }
            }
        }

        // 获取服务器列表
        if want_group {
            match self
                .api_call::<Vec<DiscordGuild>>(reqwest::Method::GET, "/users/@me/guilds", None)
                .await
            {
                Ok(guilds) => {
                    for g in guilds {
                        chats.push(ChatInfo {
                            chat_id: g.id,
                            name: Some(g.name),
                            chat_type: ChatType::Group,
                            member_count: None,
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!("Discord list_chats: failed to get guilds: {}", e);
                }
            }
        }

        Ok(chats)
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
        assert!(
            adapter
                .capabilities()
                .iter()
                .any(|c| c.name == CapabilityName::Text)
        );
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

    /// 测试辅助：从 JSON 构造 twilight Message
    fn make_msg(
        channel_id: &str,
        author_id: &str,
        author_name: &str,
        global_name: Option<&str>,
        bot: bool,
        content: &str,
        guild_id: Option<&str>,
    ) -> twilight_model::channel::Message {
        let mut v = serde_json::json!({
            "id": "111111111",
            "channel_id": channel_id,
            "author": {
                "id": author_id, "username": author_name,
                "global_name": global_name, "bot": bot, "discriminator": "0000"
            },
            "content": content,
            "timestamp": "2024-06-01T12:00:00.000000+00:00",
            "tts": false, "mention_everyone": false,
            "mentions": [], "mention_roles": [], "mention_channels": [],
            "attachments": [], "embeds": [], "pinned": false, "type": 0
        });
        if let Some(gid) = guild_id {
            v["guild_id"] = serde_json::json!(gid);
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn test_convert_dm_message() {
        let msg = make_msg(
            "222222222",
            "333333333",
            "testuser",
            Some("TestUser"),
            false,
            "Hello from Discord!",
            None,
        );

        let inbound = DiscordAdapter::convert_message(&msg, "999999999").unwrap();
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
        let msg = make_msg(
            "222222222",
            "333333333",
            "guilduser",
            None,
            false,
            "Guild message",
            Some("444444444"),
        );

        let inbound = DiscordAdapter::convert_message(&msg, "999999999").unwrap();
        assert_eq!(inbound.chat_type, ChatType::Group);
        assert!(inbound.is_group);
        assert_eq!(inbound.author.name.as_deref(), Some("guilduser"));
    }

    #[test]
    fn test_convert_own_message_is_filtered() {
        let msg = make_msg(
            "222222222",
            "888888888",
            "mybot",
            None,
            true,
            "I said this",
            None,
        );

        let result = DiscordAdapter::convert_message(&msg, "888888888");
        assert!(result.is_none(), "Should filter bot's own messages");
    }

    #[test]
    fn test_build_keyboard_components_callback_button() {
        // 验证回调按钮的 JSON 格式
        let components: Vec<serde_json::Value> = vec![serde_json::json!({
            "type": 1,
            "components": [{
                "type": 2,
                "style": 1,
                "label": "点击我",
                "custom_id": "/start"
            }]
        })];

        let body = serde_json::json!({
            "content": "测试消息",
            "components": components,
        });

        assert_eq!(body["content"], "测试消息");
        let row = &body["components"][0];
        assert_eq!(row["type"], 1);
        let btn = &row["components"][0];
        assert_eq!(btn["type"], 2);
        assert_eq!(btn["style"], 1); // Primary
        assert_eq!(btn["label"], "点击我");
        assert_eq!(btn["custom_id"], "/start");
        assert!(btn["url"].is_null());
    }

    #[test]
    fn test_build_keyboard_components_url_button() {
        // 验证 URL 按钮的 JSON 格式
        let components: Vec<serde_json::Value> = vec![serde_json::json!({
            "type": 1,
            "components": [{
                "type": 2,
                "style": 5,
                "label": "打开链接",
                "url": "https://example.com"
            }]
        })];

        let body = serde_json::json!({
            "content": "链接消息",
            "components": components,
        });

        let btn = &body["components"][0]["components"][0];
        assert_eq!(btn["style"], 5); // Link
        assert_eq!(btn["url"], "https://example.com");
        assert!(btn["custom_id"].is_null());
    }

    #[test]
    fn test_build_keyboard_components_multi_row() {
        // 验证多行键盘
        let components: Vec<serde_json::Value> = vec![
            serde_json::json!({
                "type": 1,
                "components": [{
                    "type": 2,
                    "style": 1,
                    "label": "按钮1",
                    "custom_id": "cb_1"
                }]
            }),
            serde_json::json!({
                "type": 1,
                "components": [
                    {
                        "type": 2,
                        "style": 1,
                        "label": "按钮2",
                        "custom_id": "cb_2"
                    },
                    {
                        "type": 2,
                        "style": 5,
                        "label": "链接",
                        "url": "https://example.com"
                    }
                ]
            }),
        ];

        let body = serde_json::json!({
            "content": "多行键盘",
            "components": components,
        });

        assert_eq!(body["components"].as_array().unwrap().len(), 2);
        // 第一行有 1 个按钮
        assert_eq!(
            body["components"][0]["components"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // 第二行有 2 个按钮
        assert_eq!(
            body["components"][1]["components"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    // ── convert_message 边界测试 ──

    #[test]
    fn test_convert_message_empty_content() {
        let msg = make_msg("222222222", "333333333", "emptyuser", None, false, "", None);
        let inbound = DiscordAdapter::convert_message(&msg, "999999999").unwrap();
        assert!(inbound.text.is_none(), "Empty content should yield None");
    }

    #[test]
    fn test_convert_message_bot_author_not_self() {
        // 其他 bot 的消息不应被过滤（仅过滤自身 bot）
        let msg = make_msg(
            "111111111",
            "222222222",
            "OtherBot",
            None,
            true,
            "I am another bot",
            None,
        );
        let inbound = DiscordAdapter::convert_message(&msg, "999999999").unwrap();
        assert_eq!(inbound.author.id, "222222222");
        assert!(inbound.author.is_bot);
        assert_eq!(inbound.text.as_deref(), Some("I am another bot"));
    }

    #[test]
    fn test_convert_message_timestamp_parsing() {
        let msg = make_msg("111", "222", "user", None, false, "hi", None);
        // timestamp 字段在 JSON 中为 "2024-06-01T12:00:00.000000+00:00"
        // as_micros() / 1000 应得到合理的毫秒时间戳
        let inbound = DiscordAdapter::convert_message(&msg, "999999999").unwrap();
        assert!(inbound.timestamp > 0, "Timestamp should be positive");
    }

    // ── Gateway 事件处理测试 ──

    use easybot_core::types::event::event_types::MESSAGE_INBOUND;
    use twilight_model::gateway::payload::incoming::{MessageCreate, Ready};

    /// 构造一个最小可用的 Ready 事件用于测试
    fn make_ready_event() -> Event {
        let json = serde_json::json!({
            "v": 10,
            "user": {
                "id": "123456789",
                "username": "testbot",
                "discriminator": "0000",
                "avatar": null,
                "bot": true,
                "mfa_enabled": false
            },
            "session_id": "test-session-id",
            "resume_gateway_url": "wss://gateway.discord.gg",
            "guilds": [],
            "application": {"id": "987654321", "flags": 0}
        });
        let ready: Ready = serde_json::from_value(json).expect("Failed to deserialize Ready");
        Event::Ready(ready)
    }

    #[test]
    fn test_handle_event_ready_updates_heartbeat() {
        let event_bus = EventBus::new();
        let heartbeat = Heartbeat::new();
        // 先等待一点时间让 heartbeat 变"旧"
        std::thread::sleep(std::time::Duration::from_millis(5));

        let action = handle_gateway_event::<&str>(
            Some(Ok(make_ready_event())),
            &event_bus,
            "bot_id",
            &heartbeat,
        );

        assert_eq!(action, EventAction::Continue);
        assert!(
            heartbeat.age_ms() < 100,
            "Heartbeat should be fresh after Ready (age: {}ms)",
            heartbeat.age_ms()
        );
    }

    #[test]
    fn test_handle_event_message_create_publishes() {
        let event_bus = EventBus::new();
        let mut rx = event_bus.subscribe(MESSAGE_INBOUND);
        let heartbeat = Heartbeat::new();

        let msg = make_msg(
            "111111111",
            "222222222",
            "TestUser",
            Some("Test User"),
            false,
            "Hello, world!",
            None,
        );
        let event = Event::MessageCreate(Box::new(MessageCreate(msg)));

        let action =
            handle_gateway_event::<&str>(Some(Ok(event)), &event_bus, "bot_self", &heartbeat);

        assert_eq!(action, EventAction::Continue);

        // 验证事件已发布到总线
        let published = rx
            .try_recv()
            .expect("Expected MESSAGE_INBOUND event to be published");
        assert_eq!(published.event_type, MESSAGE_INBOUND);
        assert_eq!(published.source, "discord");

        let inbound: InboundMessage =
            serde_json::from_value(published.data).expect("Failed to deserialize inbound message");
        assert_eq!(inbound.text.as_deref(), Some("Hello, world!"));
        assert_eq!(inbound.author.id, "222222222");
    }

    #[test]
    fn test_handle_event_message_create_self_ignored() {
        let event_bus = EventBus::new();
        let mut rx = event_bus.subscribe(MESSAGE_INBOUND);
        let heartbeat = Heartbeat::new();

        let msg = make_msg(
            "111111111",
            "888888888",
            "MyBot",
            Some("My Bot"),
            true,
            "I said this",
            None,
        );
        let event = Event::MessageCreate(Box::new(MessageCreate(msg)));

        let action = handle_gateway_event::<&str>(
            Some(Ok(event)),
            &event_bus,
            "888888888", // same as author → 应被过滤
            &heartbeat,
        );

        assert_eq!(action, EventAction::Continue);

        // 自消息不应发布到总线
        assert!(
            rx.try_recv().is_err(),
            "Self messages should NOT be published"
        );
    }

    #[test]
    fn test_handle_event_error_continues() {
        let event_bus = EventBus::new();
        let heartbeat = Heartbeat::new();

        let action = handle_gateway_event(
            Some(Err("simulated gateway error")),
            &event_bus,
            "bot_id",
            &heartbeat,
        );

        assert_eq!(action, EventAction::Continue);
    }

    #[test]
    fn test_handle_event_none_stops() {
        let event_bus = EventBus::new();
        let heartbeat = Heartbeat::new();

        let action = handle_gateway_event::<&str>(None, &event_bus, "bot_id", &heartbeat);

        assert_eq!(action, EventAction::Stop);
    }
}
