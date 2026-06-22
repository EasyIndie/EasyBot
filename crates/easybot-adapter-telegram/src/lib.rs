//! Telegram 平台适配器
//!
//! 使用 Telegram Bot API 实现消息收发。
//! Phase 2 实现:
//! - 真实 HTTP 消息发送（sendMessage API）
//! - getUpdates 长轮询接收消息
//! - 通过 EventBus 发布入站消息事件

mod types;

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::event::event_types;
use easybot_core::types::message::*;
use tokio::sync::broadcast;
use types::*;

/// Telegram Bot API 基础 URL
const TELEGRAM_API: &str = "https://api.telegram.org/bot";

/// 长轮询超时（秒）
const POLL_TIMEOUT: i64 = 30;

/// Telegram 适配器
pub struct TelegramAdapter {
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
    /// Background liveness heartbeat (updated by the polling task)
    heartbeat: Heartbeat,
    /// 缓存的 HTTP 客户端（连接池复用，延迟初始化）
    http_client: OnceLock<reqwest::Client>,
}

impl TelegramAdapter {
    /// 创建 Telegram 适配器
    pub fn new() -> Self {
        Self {
            platform_name: "telegram".to_string(),
            display_name: "Telegram".to_string(),
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
                    name: CapabilityName::Audio,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Video,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Document,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Interactive,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Markdown,
                    supported: true,
                    limits: None,
                },
                Capability {
                    name: CapabilityName::Html,
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
                Capability {
                    name: CapabilityName::ChatList,
                    supported: false,
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
        }
    }

    /// 获取或创建缓存的 HTTP 客户端（延迟初始化，连接池复用）
    fn http_client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(reqwest::Client::new)
    }

    /// 设置事件总线（在 init 之前调用）
    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// 构造 Bot API URL
    fn api_url(&self, method: &str) -> String {
        let config = self.config.as_ref();
        let base = config
            .and_then(|c| c.base_url.as_deref())
            .unwrap_or(TELEGRAM_API);
        let token = config.and_then(|c| c.token.clone()).unwrap_or_default();
        format!("{}{}/{}", base, token, method)
    }

    /// 将 Telegram 消息转换为网关 InboundMessage
    fn convert_message(tg_msg: TelegramMessage) -> Option<InboundMessage> {
        let chat_id = tg_msg.chat.id.to_string();
        let platform = "telegram".to_string();
        let text = tg_msg.text.or(tg_msg.caption);

        let chat_type = match tg_msg.chat.chat_type.as_str() {
            "private" => ChatType::Dm,
            "group" => ChatType::Group,
            "supergroup" => ChatType::Group,
            "channel" => ChatType::Channel,
            _ => ChatType::Dm,
        };

        let author = tg_msg
            .from
            .map(|u| MessageAuthor {
                id: u.id.to_string(),
                name: Some(u.first_name),
                is_bot: u.is_bot,
            })
            .unwrap_or_else(|| MessageAuthor {
                id: "0".to_string(),
                name: None,
                is_bot: false,
            });

        // 检测斜杠命令
        let command = text.as_ref().and_then(|t| {
            if t.starts_with('/') {
                let parts: Vec<&str> = t.splitn(2, char::is_whitespace).collect();
                let name = parts[0].trim_start_matches('/').to_string();
                let args = parts.get(1).unwrap_or(&"").to_string();
                Some(CommandData { name, args })
            } else {
                None
            }
        });

        // 检测回复引用
        let reply_to = tg_msg.reply_to_message.map(|reply| MessageReference {
            message_id: reply.message_id.to_string(),
            text: reply.text.or(reply.caption),
        });

        Some(InboundMessage {
            id: tg_msg.message_id.to_string(),
            platform,
            chat_id,
            chat_name: tg_msg.chat.title.or(tg_msg.chat.first_name),
            chat_type,
            text,
            author,
            timestamp: tg_msg.date * 1000,
            media: None,
            command,
            callback: None,
            reply_to,
            thread_id: None,
            mentioned: None,
            is_group: tg_msg.chat.chat_type != "private",
            metadata: None,
        })
    }

    /// 调用 Telegram API 的辅助方法
    async fn api_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, GatewayError> {
        let client = self.http_client();
        let url = self.api_url(method);

        let req = if let Some(json) = body {
            client.post(&url).json(&json)
        } else {
            client.get(&url)
        };

        let resp = req
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("HTTP request failed: {}", e)))?;

        let api_resp: TelegramApiResponse<T> = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("JSON parse failed: {}", e)))?;

        if api_resp.ok {
            api_resp.result.ok_or_else(|| {
                GatewayError::Internal(format!(
                    "Telegram API returned ok but no result for {}",
                    method
                ))
            })
        } else {
            let desc = api_resp
                .description
                .unwrap_or_else(|| "Unknown error".to_string());
            Err(GatewayError::Internal(format!(
                "Telegram API error: {}",
                desc
            )))
        }
    }

    /// getUpdates 长轮询循环
    async fn polling_loop(
        token: String,
        base_url: String,
        event_bus: Arc<EventBus>,
        mut cancel_rx: broadcast::Receiver<()>,
        heartbeat: Heartbeat,
    ) {
        let client = reqwest::Client::new();
        let mut offset: i64 = 0;
        tracing::info!("Telegram long polling started");

        loop {
            tokio::select! {
                _ = cancel_rx.recv() => {
                    tracing::info!("Telegram polling cancelled");
                    break;
                }
                result = Self::poll_once(&client, &token, &base_url, &mut offset) => {
                    match result {
                        Ok(updates) => {
                            heartbeat.beat(); // liveness: successful poll
                            for update in updates {
                                if update.update_id >= offset {
                                    offset = update.update_id + 1;
                                }
                                if let Some(tg_msg) = update.message
                                    && let Some(inbound) = Self::convert_message(tg_msg) {
                                        let event = GatewayEvent::new(
                                            easybot_core::types::event::event_types::MESSAGE_INBOUND,
                                            "telegram",
                                            serde_json::to_value(&inbound).unwrap_or_default(),
                                        );
                                        event_bus.publish(event);
                                    }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Telegram polling error: {}", e);
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                    }
                }
            }
        }
    }

    /// 单次 getUpdates 调用
    async fn poll_once(
        client: &reqwest::Client,
        token: &str,
        base_url: &str,
        offset: &mut i64,
    ) -> Result<Vec<TelegramUpdate>, GatewayError> {
        let url = format!("{}{}/getUpdates", base_url, token);
        let params = serde_json::json!({
            "offset": *offset,
            "timeout": POLL_TIMEOUT,
            "allowed_updates": ["message"],
        });

        let resp = client
            .post(&url)
            .json(&params)
            .timeout(Duration::from_secs(POLL_TIMEOUT as u64 + 10))
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Poll request failed: {}", e)))?;

        let api_resp: TelegramApiResponse<Vec<TelegramUpdate>> = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Poll parse failed: {}", e)))?;

        if api_resp.ok {
            Ok(api_resp.result.unwrap_or_default())
        } else {
            Err(GatewayError::Internal(
                api_resp
                    .description
                    .unwrap_or_else(|| "Unknown polling error".to_string()),
            ))
        }
    }
}

impl Default for TelegramAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// Publish send result event via event bus
fn publish_send_event(
    event_bus: &Option<Arc<EventBus>>,
    event_type: &str,
    chat_id: &str,
    result: &SendResult,
) {
    if let Some(bus) = event_bus {
        bus.publish(GatewayEvent::new(
            event_type,
            "telegram",
            serde_json::json!({
                "platform": "telegram",
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
impl PlatformAdapter for TelegramAdapter {
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
                error: Some("Telegram bot token is required".to_string()),
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
            .ok_or_else(|| {
                GatewayError::ConfigError("Telegram token not configured".to_string())
            })?;

        // 通过 getMe 验证 Token 并获取 Bot 信息
        let client = self.http_client();
        let url = self.api_url("getMe");

        let resp =
            client.get(&url).send().await.map_err(|e| {
                GatewayError::Internal(format!("Failed to connect to Telegram: {}", e))
            })?;

        let api_resp: TelegramApiResponse<TelegramBotInfo> = resp.json().await.map_err(|e| {
            GatewayError::Internal(format!("Failed to parse getMe response: {}", e))
        })?;

        if !api_resp.ok {
            let desc = api_resp
                .description
                .unwrap_or_else(|| "Invalid token".to_string());
            return Ok(ConnectResult {
                ok: false,
                error: Some(format!("Telegram auth failed: {}", desc)),
                bot_info: None,
            });
        }

        let bot = api_resp
            .result
            .ok_or_else(|| GatewayError::Internal("getMe returned no bot info".to_string()))?;

        self.state = AdapterState::Connected;
        self.bot_info = Some(BotInfo {
            name: bot.first_name.clone(),
            username: bot.username.clone(),
            id: bot.id.to_string(),
        });

        tracing::info!(
            "Telegram adapter connected: {} (@{})",
            bot.first_name,
            bot.username.as_deref().unwrap_or("unknown")
        );

        // 启动长轮询（如果配置了 EventBus）
        if let Some(event_bus) = self.event_bus.clone() {
            let (cancel_tx, cancel_rx) = broadcast::channel(1);
            self.cancel_tx = Some(cancel_tx);
            let token_clone = token.clone();
            let base_url = self
                .config
                .as_ref()
                .and_then(|c| c.base_url.clone())
                .unwrap_or_else(|| TELEGRAM_API.to_string());
            let hb = self.heartbeat.clone();

            tokio::spawn(async move {
                Self::polling_loop(token_clone, base_url, event_bus, cancel_rx, hb).await;
            });
        }

        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: self.bot_info.clone(),
        })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        // 发送取消信号停止轮询
        if let Some(cancel_tx) = &self.cancel_tx {
            let _ = cancel_tx.send(());
        }
        self.cancel_tx = None;
        self.state = AdapterState::Stopped;
        tracing::info!("Telegram adapter disconnected");
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
        let mut body = serde_json::json!({
            "chat_id": params.chat_id,
            "text": params.message.text,
        });

        // 解析模式
        match params.message.parse_mode {
            ParseMode::Markdown => {
                body["parse_mode"] = serde_json::Value::String("MarkdownV2".into());
            }
            ParseMode::Html => {
                body["parse_mode"] = serde_json::Value::String("HTML".into());
            }
            ParseMode::None => {}
        }

        // 回复引用
        if let Some(reply_to) = &params.reply_to {
            body["reply_to_message_id"] = serde_json::json!(reply_to);
        }

        // 平台特定参数
        if let Some(meta) = &params.metadata
            && let Some(obj) = meta.as_object()
        {
            for (k, v) in obj {
                body[k] = v.clone();
            }
        }

        let result: TelegramMessage = match self.api_call("sendMessage", Some(body)).await {
            Ok(msg) => msg,
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                let fail = SendResult::fail(e.to_string(), true);
                publish_send_event(
                    &self.event_bus,
                    event_types::MESSAGE_FAILED,
                    &params.chat_id,
                    &fail,
                );
                return Ok(fail);
            }
        };

        self.messages_out.fetch_add(1, Ordering::Relaxed);

        let send_result = SendResult {
            success: true,
            message_id: Some(result.message_id.to_string()),
            timestamp: Some(result.date * 1000),
            error: None,
            error_code: None,
            retryable: false,
        };
        publish_send_event(
            &self.event_bus,
            event_types::MESSAGE_SENT,
            &params.chat_id,
            &send_result,
        );
        Ok(send_result)
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        let client = self.http_client();
        let token = self
            .config
            .as_ref()
            .and_then(|c| c.token.clone())
            .ok_or_else(|| {
                GatewayError::ConfigError("Telegram token not configured".to_string())
            })?;
        let base = self
            .config
            .as_ref()
            .and_then(|c| c.base_url.clone())
            .unwrap_or_else(|| TELEGRAM_API.to_string());
        let _ = &base; // used via self.api_url below

        // 映射 MediaType → Telegram API 方法名
        let (method, field) = match params.media.media_type {
            MediaType::Image => ("sendPhoto", "photo"),
            MediaType::Audio => ("sendAudio", "audio"),
            MediaType::Video => ("sendVideo", "video"),
            MediaType::Document => ("sendDocument", "document"),
            MediaType::Sticker => ("sendSticker", "sticker"),
            MediaType::Animation => ("sendAnimation", "animation"),
        };

        if let Some(url) = &params.media.url {
            // 通过 URL/file_id 发送 — 使用 JSON body
            let mut body = serde_json::json!({
                "chat_id": params.chat_id,
                field: url,
            });

            if let Some(caption) = &params.media.caption {
                body["caption"] = serde_json::json!(caption);
            }

            if let Some(reply_to) = &params.reply_to {
                body["reply_to_message_id"] = serde_json::json!(reply_to);
            }

            let result: TelegramMessage = match self.api_call(method, Some(body)).await {
                Ok(msg) => msg,
                Err(e) => {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    let fail = SendResult::fail(e.to_string(), true);
                    publish_send_event(
                        &self.event_bus,
                        event_types::MESSAGE_FAILED,
                        &params.chat_id,
                        &fail,
                    );
                    return Ok(fail);
                }
            };

            self.messages_out.fetch_add(1, Ordering::Relaxed);

            let send_result = SendResult {
                success: true,
                message_id: Some(result.message_id.to_string()),
                timestamp: Some(result.date * 1000),
                error: None,
                error_code: None,
                retryable: false,
            };
            publish_send_event(
                &self.event_bus,
                event_types::MESSAGE_SENT,
                &params.chat_id,
                &send_result,
            );
            Ok(send_result)
        } else if let Some(data_b64) = &params.media.data {
            // Base64 数据 → multipart/form-data 上传
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data_b64)
                .map_err(|e| GatewayError::Internal(format!("Base64 decode failed: {}", e)))?;

            let filename = params
                .media
                .filename
                .clone()
                .unwrap_or_else(|| "file".to_string());

            let mut part = reqwest::multipart::Part::bytes(decoded).file_name(filename);

            // 设置 Content-Type（如果提供了 mime_type）
            if !params.media.mime_type.is_empty() {
                part = part
                    .mime_str(&params.media.mime_type)
                    .map_err(|e| GatewayError::Internal(format!("Invalid mime type: {}", e)))?;
            }

            let mut form = reqwest::multipart::Form::new()
                .part(field, part)
                .text("chat_id", params.chat_id.clone());

            if let Some(caption) = &params.media.caption {
                form = form.text("caption", caption.clone());
            }

            if let Some(reply_to) = &params.reply_to {
                form = form.text("reply_to_message_id", reply_to.clone());
            }

            let url = format!("{}{}/{}", base, token, method);
            let resp = client
                .post(&url)
                .multipart(form)
                .send()
                .await
                .map_err(|e| GatewayError::Internal(format!("HTTP upload failed: {}", e)))?;

            let api_resp: TelegramApiResponse<TelegramMessage> = resp
                .json()
                .await
                .map_err(|e| GatewayError::Internal(format!("JSON parse failed: {}", e)))?;

            if api_resp.ok {
                if let Some(result) = api_resp.result {
                    self.messages_out.fetch_add(1, Ordering::Relaxed);
                    let send_result = SendResult {
                        success: true,
                        message_id: Some(result.message_id.to_string()),
                        timestamp: Some(result.date * 1000),
                        error: None,
                        error_code: None,
                        retryable: false,
                    };
                    publish_send_event(
                        &self.event_bus,
                        event_types::MESSAGE_SENT,
                        &params.chat_id,
                        &send_result,
                    );
                    Ok(send_result)
                } else {
                    let fail = SendResult::fail(
                        "Telegram API returned ok but no result".to_string(),
                        false,
                    );
                    publish_send_event(
                        &self.event_bus,
                        event_types::MESSAGE_FAILED,
                        &params.chat_id,
                        &fail,
                    );
                    Err(GatewayError::Internal(
                        "Telegram API returned ok but no result".to_string(),
                    ))
                }
            } else {
                let desc = api_resp
                    .description
                    .unwrap_or_else(|| "Unknown error".to_string());
                self.errors.fetch_add(1, Ordering::Relaxed);
                let fail = SendResult::fail(format!("Telegram API upload error: {}", desc), true);
                publish_send_event(
                    &self.event_bus,
                    event_types::MESSAGE_FAILED,
                    &params.chat_id,
                    &fail,
                );
                Ok(fail)
            }
        } else {
            self.errors.fetch_add(1, Ordering::Relaxed);
            let fail = SendResult::fail("No media URL or data provided".to_string(), false);
            publish_send_event(
                &self.event_bus,
                event_types::MESSAGE_FAILED,
                &params.chat_id,
                &fail,
            );
            Ok(fail)
        }
    }

    async fn send_interactive(
        &self,
        params: SendInteractiveParams,
    ) -> Result<SendResult, GatewayError> {
        let mut body = serde_json::json!({
            "chat_id": params.chat_id,
            "text": params.text,
        });

        // 转换键盘格式
        let inline_keyboard: Vec<Vec<serde_json::Value>> = params
            .keyboard
            .rows
            .iter()
            .map(|row| {
                row.buttons
                    .iter()
                    .map(|btn| {
                        let mut btn_json = serde_json::json!({
                            "text": btn.text,
                        });
                        if let Some(cb) = &btn.callback_data {
                            btn_json["callback_data"] = serde_json::json!(cb);
                        }
                        if let Some(url) = &btn.url {
                            btn_json["url"] = serde_json::json!(url);
                        }
                        btn_json
                    })
                    .collect()
            })
            .collect();

        body["reply_markup"] = serde_json::json!({
            "inline_keyboard": inline_keyboard,
        });

        if let Some(reply_to) = &params.reply_to {
            body["reply_to_message_id"] = serde_json::json!(reply_to);
        }

        let result: TelegramMessage = match self.api_call("sendMessage", Some(body)).await {
            Ok(msg) => msg,
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                let fail = SendResult::fail(e.to_string(), true);
                publish_send_event(
                    &self.event_bus,
                    event_types::MESSAGE_FAILED,
                    &params.chat_id,
                    &fail,
                );
                return Ok(fail);
            }
        };

        self.messages_out.fetch_add(1, Ordering::Relaxed);

        let send_result = SendResult {
            success: true,
            message_id: Some(result.message_id.to_string()),
            timestamp: Some(result.date * 1000),
            error: None,
            error_code: None,
            retryable: false,
        };
        publish_send_event(
            &self.event_bus,
            event_types::MESSAGE_SENT,
            &params.chat_id,
            &send_result,
        );
        Ok(send_result)
    }

    async fn send_typing(&self, chat_id: &str) -> Result<(), GatewayError> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "action": "typing",
        });
        self.api_call::<serde_json::Value>("sendChatAction", Some(body))
            .await?;
        Ok(())
    }

    async fn send_draft(&self, params: SendDraftParams) -> Result<DraftResult, GatewayError> {
        let mut body = serde_json::json!({
            "chat_id": params.chat_id,
            "text": params.text,
        });

        // Parse mode
        if let Some(ref pm) = params.parse_mode {
            match pm {
                ParseMode::Markdown => {
                    body["parse_mode"] = "MarkdownV2".into();
                }
                ParseMode::Html => {
                    body["parse_mode"] = "HTML".into();
                }
                ParseMode::None => {}
            }
        }

        if let Some(ref reply_to) = params.reply_to {
            body["reply_to_message_id"] = serde_json::json!(reply_to);
        }

        if let Some(ref msg_id) = params.message_id {
            // 更新已有草稿 → 使用 editMessageText
            body["message_id"] = serde_json::json!(msg_id);
            match self
                .api_call::<serde_json::Value>("editMessageText", Some(body))
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
            // 创建新草稿 → 使用 sendMessage
            match self
                .api_call::<TelegramMessage>("sendMessage", Some(body))
                .await
            {
                Ok(msg) => {
                    self.messages_out.fetch_add(1, Ordering::Relaxed);
                    Ok(DraftResult {
                        success: true,
                        message_id: Some(msg.message_id.to_string()),
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

    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError> {
        let body = serde_json::json!({ "chat_id": chat_id });
        let chat: TelegramChat = self.api_call("getChat", Some(body)).await?;

        let chat_type = match chat.chat_type.as_str() {
            "private" => ChatType::Dm,
            "group" => ChatType::Group,
            "supergroup" => ChatType::Group,
            "channel" => ChatType::Channel,
            _ => ChatType::Dm,
        };

        Ok(ChatInfo {
            chat_id: chat.id.to_string(),
            name: chat.title.or(chat.first_name),
            chat_type,
            member_count: None,
        })
    }

    async fn edit_message(&self, params: EditMessageParams) -> Result<EditResult, GatewayError> {
        let mut body = serde_json::json!({
            "chat_id": params.chat_id,
            "message_id": params.message_id,
            "text": params.message.text,
        });

        match params.message.parse_mode {
            ParseMode::Markdown => {
                body["parse_mode"] = "MarkdownV2".into();
            }
            ParseMode::Html => {
                body["parse_mode"] = "HTML".into();
            }
            ParseMode::None => {}
        }

        match self
            .api_call::<TelegramMessage>("editMessageText", Some(body))
            .await
        {
            Ok(msg) => Ok(EditResult {
                success: true,
                updated_at: Some(msg.date * 1000),
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
        let body = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id,
        });

        match self
            .api_call::<serde_json::Value>("deleteMessage", Some(body))
            .await
        {
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
        let adapter = TelegramAdapter::new();
        assert_eq!(adapter.platform_name(), "telegram");
    }

    #[test]
    fn test_display_name() {
        let adapter = TelegramAdapter::new();
        assert_eq!(adapter.display_name(), "Telegram");
    }

    #[test]
    fn test_capabilities() {
        let adapter = TelegramAdapter::new();
        assert!(
            adapter
                .capabilities()
                .iter()
                .any(|c| c.name == CapabilityName::Text)
        );
    }

    #[test]
    fn test_default_state() {
        let adapter = TelegramAdapter::new();
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[test]
    fn test_default() {
        let adapter = TelegramAdapter::default();
        assert_eq!(adapter.platform_name(), "telegram");
    }

    #[test]
    fn test_status_summary() {
        let adapter = TelegramAdapter::new();
        let s = adapter.status_summary();
        assert_eq!(s.platform, "telegram");
        assert_eq!(s.display_name, "Telegram");
        assert!(!s.connected);
    }

    #[tokio::test]
    async fn test_disconnect_idempotent() {
        let mut adapter = TelegramAdapter::new();
        // 在连接前 disconnect 不应 panic
        adapter.disconnect().await.unwrap();
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_double_disconnect() {
        let mut adapter = TelegramAdapter::new();
        adapter.disconnect().await.unwrap();
        adapter.disconnect().await.unwrap();
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_health_before_init() {
        let adapter = TelegramAdapter::new();
        let h = adapter.health().await;
        assert_eq!(h.status, HealthStatus::Down);
        assert!(!h.connected);
    }

    #[tokio::test]
    async fn test_runtime_config_before_init() {
        let adapter = TelegramAdapter::new();
        let r = adapter.runtime_config();
        assert!(!r.enabled);
        assert!(!r.token_configured);
    }

    #[tokio::test]
    async fn test_runtime_config_after_init() {
        let mut adapter = TelegramAdapter::new();
        adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("123:token".to_string()),
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
        let adapter = TelegramAdapter::new();
        // 未初始化时调用 get_chat_info 应返回错误（不 panic）
        let result = adapter.get_chat_info("-100123456").await;
        assert!(
            result.is_err(),
            "Expected error when adapter is not initialized"
        );
    }

    #[tokio::test]
    async fn test_send_before_connect_errors() {
        let adapter = TelegramAdapter::new();
        let result = adapter
            .send(SendTextParams {
                chat_id: "123".to_string(),
                message: OutboundMessage {
                    text: "test".to_string(),
                    parse_mode: ParseMode::None,
                },
                reply_to: None,
                metadata: None,
            })
            .await
            .unwrap();
        // 未初始化时 send 应返回错误
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_init_without_token() {
        let mut adapter = TelegramAdapter::new();
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
        let mut adapter = TelegramAdapter::new();
        let init_result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("123456:test-token".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert!(init_result.ok);

        // Without a real token, getMe fails → return ok:false, state stays Created
        let result = adapter.connect().await.unwrap();
        assert!(!result.ok);
        assert!(result.error.is_some());
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[tokio::test]
    async fn test_send_message_mocked() {
        let mut adapter = TelegramAdapter::new();
        adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("123456:test-token".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();

        // 跳过 connect（因为没有真实的 HTTP 连接）
        // send 会尝试发送 HTTP，这里预期会失败（因为 token 无效）
        let result = adapter
            .send(SendTextParams {
                chat_id: "123456789".to_string(),
                message: OutboundMessage {
                    text: "Hello, World!".to_string(),
                    parse_mode: ParseMode::Markdown,
                },
                reply_to: None,
                metadata: None,
            })
            .await
            .unwrap();

        // 因为 HTTP 请求会失败，返回 fail 结果
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_convert_message() {
        let tg_msg = TelegramMessage {
            message_id: 42,
            from: Some(TelegramUser {
                id: 12345,
                is_bot: false,
                first_name: "TestUser".to_string(),
                last_name: None,
                username: None,
            }),
            chat: TelegramChat {
                id: -100123456,
                chat_type: "group".to_string(),
                title: Some("Test Group".to_string()),
                username: None,
                first_name: None,
                last_name: None,
            },
            date: 1700000000,
            text: Some("/start hello".to_string()),
            entities: None,
            reply_to_message: None,
            caption: None,
        };

        let inbound = TelegramAdapter::convert_message(tg_msg).unwrap();
        assert_eq!(inbound.id, "42");
        assert_eq!(inbound.platform, "telegram");
        assert_eq!(inbound.chat_id, "-100123456");
        assert_eq!(inbound.chat_name.as_deref(), Some("Test Group"));
        assert_eq!(inbound.chat_type, ChatType::Group);
        assert_eq!(inbound.author.id, "12345");
        assert_eq!(inbound.author.name.as_deref(), Some("TestUser"));
        assert!(inbound.is_group);
        assert_eq!(inbound.text.as_deref(), Some("/start hello"));

        // 验证命令解析
        assert!(inbound.command.is_some());
        let cmd = inbound.command.unwrap();
        assert_eq!(cmd.name, "start");
        assert_eq!(cmd.args, "hello");
    }

    // ── rstest 参数化测试 ──

    use rstest::rstest;

    #[rstest]
    #[case("private", easybot_core::types::message::ChatType::Dm)]
    #[case("group", easybot_core::types::message::ChatType::Group)]
    #[case("supergroup", easybot_core::types::message::ChatType::Group)]
    #[case("channel", easybot_core::types::message::ChatType::Channel)]
    fn test_chat_type_mapping(
        #[case] tg_type: &str,
        #[case] expected: easybot_core::types::message::ChatType,
    ) {
        let chat = TelegramChat {
            id: 1,
            chat_type: tg_type.to_string(),
            title: None,
            username: None,
            first_name: None,
            last_name: None,
        };
        let msg = TelegramMessage {
            message_id: 1,
            date: 1000000,
            text: Some("hello".to_string()),
            caption: None,
            chat,
            from: None,
            reply_to_message: None,
            entities: None,
        };
        let inbound = TelegramAdapter::convert_message(msg).unwrap();
        assert_eq!(
            inbound.chat_type, expected,
            "chat_type mapping for '{}'",
            tg_type
        );
    }
}
