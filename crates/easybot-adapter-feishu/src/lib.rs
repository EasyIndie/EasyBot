//! 飞书/Lark 平台适配器
//!
//! 使用飞书开放平台 API 实现消息收发。
//! - 发送: HTTP REST API (im/v1/messages)
//! - 接收: 事件订阅 (WebSocket 长连接 / Webhook)

mod event;
mod types;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::event::event_types;
use easybot_core::types::message::*;
use larksuite_oapi_sdk_rs::{Client, EventDispatcher};
use tokio::sync::broadcast;
use types::*;

/// 飞书开放平台 API 基础 URL
const FEISHU_API: &str = "https://open.feishu.cn/open-apis";

/// Token 刷新阈值（秒），在过期前提前刷新
const TOKEN_REFRESH_MARGIN: u64 = 300;

/// 飞书适配器
pub struct FeishuAdapter {
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
    /// Background liveness heartbeat (updated by the WebSocket task)
    heartbeat: Heartbeat,
    /// 缓存的 HTTP 客户端
    http_client: Option<reqwest::Client>,
    /// 当前 access token
    access_token: tokio::sync::RwLock<Option<String>>,
    /// token 过期时间戳（毫秒）
    token_expires_at: tokio::sync::RwLock<i64>,
}

impl FeishuAdapter {
    pub fn new() -> Self {
        Self {
            platform_name: "feishu".to_string(),
            display_name: "飞书".to_string(),
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
                    name: CapabilityName::Group,
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
            ],
            messages_in: AtomicU64::new(0),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            cancel_tx: None,
            heartbeat: Heartbeat::new(),
            http_client: None,
            access_token: tokio::sync::RwLock::new(None),
            token_expires_at: tokio::sync::RwLock::new(0),
        }
    }

    /// 设置事件总线（在 init 之前调用）
    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// 获取或创建 HTTP 客户端
    fn client(&self) -> Result<&reqwest::Client, GatewayError> {
        self.http_client
            .as_ref()
            .ok_or_else(|| GatewayError::Internal("HTTP client not initialized".to_string()))
    }

    /// 返回 API 基础 URL（支持通过 config.base_url 覆盖）
    fn api_base_url(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.base_url.as_deref())
            .unwrap_or(FEISHU_API)
    }

    /// 确保 access token 有效，必要时自动刷新
    async fn ensure_token(&self) -> Result<String, GatewayError> {
        let expires_at = *self.token_expires_at.read().await;
        let now_ms = chrono::Utc::now().timestamp_millis();

        // 如果 token 还在有效期内（含刷新余量），直接返回
        if expires_at > now_ms + (TOKEN_REFRESH_MARGIN as i64 * 1000)
            && let Some(token) = self.access_token.read().await.clone() {
                return Ok(token);
            }

        // 刷新 token
        self.refresh_token().await
    }

    /// 获取 tenant_access_token
    async fn refresh_token(&self) -> Result<String, GatewayError> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| GatewayError::ConfigError("Adapter not initialized".to_string()))?;

        let extra = &config.extra;
        let app_id = extra
            .get("app_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                GatewayError::ConfigError("Missing 'app_id' in feishu config.extra".to_string())
            })?;
        let app_secret = config.token.as_deref().ok_or_else(|| {
            GatewayError::ConfigError("Missing 'token' (app_secret) for feishu".to_string())
        })?;

        let client = self.client()?;
        let url = format!(
            "{}/auth/v3/tenant_access_token/internal",
            self.api_base_url()
        );

        let resp: FeishuTokenResponse = client
            .post(&url)
            .json(&serde_json::json!({
                "app_id": app_id,
                "app_secret": app_secret,
            }))
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Failed to get feishu token: {}", e)))?
            .json()
            .await
            .map_err(|e| {
                GatewayError::Internal(format!("Failed to parse feishu token response: {}", e))
            })?;

        if resp.code != 0 {
            return Err(GatewayError::Internal(format!(
                "Feishu auth failed: {} (code {})",
                resp.msg.unwrap_or_default(),
                resp.code
            )));
        }

        let token = resp
            .tenant_access_token
            .ok_or_else(|| GatewayError::Internal("No token in feishu response".to_string()))?;
        let expire = resp.expire.unwrap_or(7200) as i64;

        *self.access_token.write().await = Some(token.clone());
        *self.token_expires_at.write().await =
            chrono::Utc::now().timestamp_millis() + (expire * 1000);

        Ok(token)
    }

    /// 飞书 API GET 请求
    async fn api_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, GatewayError> {
        let token = self.ensure_token().await?;
        let client = self.client()?;
        let url = format!("{}{}", self.api_base_url(), path);

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu GET failed: {}", e)))?;

        let result: FeishuApiResponse<T> = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu GET parse failed: {}", e)))?;

        if result.code != 0 {
            return Err(GatewayError::Internal(format!(
                "Feishu API error (GET {}): {} (code {})",
                path,
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        result.data.ok_or_else(|| {
            GatewayError::Internal(format!("Feishu API returned no data for GET {}", path))
        })
    }

    /// 飞书 API POST 请求
    async fn api_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        let token = self.ensure_token().await?;
        let client = self.client()?;
        let url = format!("{}{}", self.api_base_url(), path);

        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu POST failed: {}", e)))?;

        let result: FeishuApiResponse<T> = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu POST parse failed: {}", e)))?;

        if result.code != 0 {
            return Err(GatewayError::Internal(format!(
                "Feishu API error (POST {}): {} (code {})",
                path,
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        result.data.ok_or_else(|| {
            GatewayError::Internal(format!("Feishu API returned no data for POST {}", path))
        })
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
            "feishu",
            serde_json::json!({
                "platform": "feishu",
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
impl PlatformAdapter for FeishuAdapter {
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
        let extra = &config.extra;
        let app_id = extra.get("app_id").and_then(|v| v.as_str());
        let app_secret = config.token.as_deref();

        if app_id.is_none() || app_secret.is_none() {
            return Ok(InitResult {
                ok: false,
                error: Some("飞书适配器需要配置 extra.app_id 和 token (app_secret)".to_string()),
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
        // 1. 获取 access token 验证凭证
        let _token = self.refresh_token().await?;

        // 2. 获取配置
        let config = self.config.as_ref().unwrap();
        let app_id = config
            .extra
            .get("app_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        self.state = AdapterState::Connected;
        self.bot_info = Some(BotInfo {
            name: app_id.to_string(),
            username: Some("feishu_bot".to_string()),
            id: app_id.to_string(),
        });

        tracing::info!("飞书适配器已连接");

        // 3. 如果配置了 EventBus，启动 WebSocket 事件订阅
        if let Some(ref event_bus) = self.event_bus {
            let (cancel_tx, mut cancel_rx) = broadcast::channel(1);
            // Subscribe before moving cancel_tx into self so the spawned task can use it
            let hb_cancel_rx = cancel_tx.subscribe();
            self.cancel_tx = Some(cancel_tx);

            let eb = event_bus.clone();
            let app_id_owned = app_id.to_string();
            let app_secret = config.token.clone().unwrap_or_default();

            // 创建 SDK Client + EventDispatcher
            let sdk_client = match Client::builder(&app_id_owned, &app_secret).build() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("飞书 SDK 客户端创建失败: {}", e);
                    return Ok(ConnectResult {
                        ok: true,
                        error: Some(format!("SDK client init failed: {}", e)),
                        bot_info: self.bot_info.clone(),
                    });
                }
            };

            let dispatcher = EventDispatcher::new("", "").skip_sign_verify().on_event(
                types::EVENT_MESSAGE_RECEIVE_V1,
                move |event_data| {
                    let eb = eb.clone();
                    let bot_id = app_id_owned.clone();
                    async move {
                        event::handle_message_receive(event_data, &eb, &bot_id).await;
                        Ok(())
                    }
                },
            );

            let ws_client = sdk_client.ws_client(dispatcher);
            let log_level = tracing::Level::DEBUG;
            let ws_client = ws_client.log_level(log_level);

            // 在后台任务中运行
            let hb = self.heartbeat.clone();
            tokio::spawn(async move {
                // Separate heartbeat ticker — beats every 30s while ws_client is alive
                let hb_for_tick = hb.clone();
                let mut hb_cancel_rx_inner = hb_cancel_rx;
                let hb_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
                    let mut tick = tokio::time::interval(Duration::from_secs(30));
                    loop {
                        tokio::select! {
                            _ = hb_cancel_rx_inner.recv() => break,
                            _ = tick.tick() => {
                                hb_for_tick.beat();
                            }
                        }
                    }
                });

                tokio::select! {
                    _ = cancel_rx.recv() => {
                        tracing::info!("飞书 WebSocket 事件订阅已停止");
                    }
                    result = ws_client.start() => {
                        match result {
                            Ok(()) => tracing::info!("飞书 WebSocket 连接正常关闭"),
                            Err(e) => tracing::error!("飞书 WebSocket 连接异常: {}", e),
                        }
                    }
                }

                // Stop the heartbeat task when we exit
                hb_task.abort();
            });
        }

        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: self.bot_info.clone(),
        })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        // 发送取消信号停止 WebSocket 事件订阅
        if let Some(cancel_tx) = &self.cancel_tx {
            let _ = cancel_tx.send(());
            tracing::info!("飞书 WebSocket 事件订阅停止信号已发送");
        }
        self.cancel_tx = None;
        self.state = AdapterState::Stopped;
        tracing::info!("飞书适配器已断开");
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state.clone()
    }

    fn heartbeat_age_ms(&self) -> Option<i64> {
        Some(self.heartbeat.age_ms())
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
        let content = serde_json::json!({
            "text": params.message.text,
        });

        // 飞书 send message API: POST /open-apis/im/v1/messages
        // Query param: receive_id_type=chat_id
        let path = "/im/v1/messages?receive_id_type=chat_id".to_string();

        let body = serde_json::json!({
            "receive_id": params.chat_id,
            "msg_type": "text",
            "content": content.to_string(),
        });

        let send_result = match self.api_post::<FeishuSendMessageData>(&path, &body).await {
            Ok(data) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(data.message_id),
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    error: None,
                    error_code: None,
                    retryable: false,
                }
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                SendResult::fail(e.to_string(), true)
            }
        };
        publish_send_event(
            &self.event_bus,
            if send_result.success {
                event_types::MESSAGE_SENT
            } else {
                event_types::MESSAGE_FAILED
            },
            &params.chat_id,
            &send_result,
        );
        Ok(send_result)
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        let msg_type = match params.media.media_type {
            MediaType::Image => "image",
            MediaType::Audio => "audio",
            MediaType::Video => "media",
            MediaType::Document => "file",
            MediaType::Sticker => "sticker",
            MediaType::Animation => "image",
        };

        // 先上传文件获取 file_key
        let file_key = self.upload_media(&params.media).await?;

        let path = "/im/v1/messages?receive_id_type=chat_id".to_string();
        let content = serde_json::json!({
            "file_key": file_key,
        });

        let body = serde_json::json!({
            "receive_id": params.chat_id,
            "msg_type": msg_type,
            "content": content.to_string(),
        });

        let send_result = match self.api_post::<FeishuSendMessageData>(&path, &body).await {
            Ok(data) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(data.message_id),
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    error: None,
                    error_code: None,
                    retryable: false,
                }
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                SendResult::fail(e.to_string(), true)
            }
        };
        publish_send_event(
            &self.event_bus,
            if send_result.success {
                event_types::MESSAGE_SENT
            } else {
                event_types::MESSAGE_FAILED
            },
            &params.chat_id,
            &send_result,
        );
        Ok(send_result)
    }

    async fn send_interactive(
        &self,
        params: SendInteractiveParams,
    ) -> Result<SendResult, GatewayError> {
        let content = serde_json::json!({
            "elements": params.keyboard.rows.iter().map(|row| {
                let buttons: Vec<serde_json::Value> = row.buttons.iter().map(|b| {
                    let text = b.text.clone();
                    let value = b.callback_data.as_ref().map(|d| serde_json::json!({"data": d})).unwrap_or_default();
                    serde_json::json!({
                        "tag": "button",
                        "text": {
                            "tag": "plain_text",
                            "content": text,
                        },
                        "value": value,
                    })
                }).collect();
                if buttons.len() == 1 {
                    buttons.into_iter().next().unwrap()
                } else {
                    serde_json::json!({
                        "tag": "action",
                        "actions": buttons,
                    })
                }
            }).collect::<Vec<_>>(),
        });

        let path = "/im/v1/messages?receive_id_type=chat_id".to_string();
        let body = serde_json::json!({
            "receive_id": params.chat_id,
            "msg_type": "interactive",
            "content": serde_json::to_string(&content).unwrap_or_default(),
        });

        let send_result = match self.api_post::<FeishuSendMessageData>(&path, &body).await {
            Ok(data) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(data.message_id),
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    error: None,
                    error_code: None,
                    retryable: false,
                }
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                SendResult::fail(e.to_string(), true)
            }
        };
        publish_send_event(
            &self.event_bus,
            if send_result.success {
                event_types::MESSAGE_SENT
            } else {
                event_types::MESSAGE_FAILED
            },
            &params.chat_id,
            &send_result,
        );
        Ok(send_result)
    }

    async fn edit_message(&self, params: EditMessageParams) -> Result<EditResult, GatewayError> {
        // 飞书支持编辑文本消息
        let path = format!("/im/v1/messages/{}/patch", params.message_id);
        let content = serde_json::json!({
            "text": params.message.text,
        });
        let body = serde_json::json!({
            "content": content.to_string(),
        });

        // api_post 已解包 FeishuApiResponse.data，此处只需 Value
        let _: serde_json::Value = self.api_post(&path, &body).await?;

        Ok(EditResult {
            success: true,
            updated_at: Some(chrono::Utc::now().timestamp_millis()),
            error: None,
        })
    }

    async fn delete_message(
        &self,
        _chat_id: &str,
        _message_id: &str,
    ) -> Result<DeleteResult, GatewayError> {
        // 飞书没有真正的删除消息 API，直接返回不支持
        Ok(DeleteResult {
            success: false,
            error: Some("飞书不支持删除消息".to_string()),
        })
    }

    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError> {
        let path = format!("/im/v1/chats/{}", chat_id);
        let data: FeishuChatInfo = self.api_get(&path).await?;

        Ok(ChatInfo {
            chat_id: chat_id.to_string(),
            name: Some(data.name),
            chat_type: if data.chat_type == "group" {
                ChatType::Group
            } else {
                ChatType::Dm
            },
            member_count: Some(data.member_count as u32),
        })
    }

    async fn list_chats(&self, _filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        let data: FeishuListChatData = self.api_get("/im/v1/chats?page_size=50").await?;

        Ok(data
            .items
            .into_iter()
            .map(|item| ChatInfo {
                chat_id: item.chat_id,
                name: Some(item.name),
                chat_type: if item.chat_type == "group" {
                    ChatType::Group
                } else {
                    ChatType::Dm
                },
                member_count: Some(item.member_count as u32),
            })
            .collect())
    }
}

// ── 辅助方法 ──

impl FeishuAdapter {
    /// 上传媒体文件到飞书，返回 file_key
    async fn upload_media(&self, media: &MediaAttachment) -> Result<String, GatewayError> {
        let token = self.ensure_token().await?;
        let client = self.client()?;
        let url = format!("{}/im/v1/files", self.api_base_url());

        // 确定文件类型
        let file_type = match media.media_type {
            MediaType::Image => "image",
            MediaType::Audio => "opus",
            MediaType::Video => "media",
            MediaType::Document => "file",
            MediaType::Sticker => "sticker",
            MediaType::Animation => "image",
        };

        // 如果有 URL，先下载
        let file_data = if let Some(ref url) = media.url {
            let resp = client
                .get(url)
                .send()
                .await
                .map_err(|e| GatewayError::Internal(format!("Download media failed: {}", e)))?;
            resp.bytes()
                .await
                .map_err(|e| GatewayError::Internal(format!("Read media bytes failed: {}", e)))?
                .to_vec()
        } else if let Some(ref base64_data) = media.data {
            // 使用 base64 数据作为文件内容
            base64_data.as_bytes().to_vec()
        } else {
            return Err(GatewayError::InvalidRequest(
                "No media data or URL provided".to_string(),
            ));
        };

        // 使用 multipart 上传
        let file_part = reqwest::multipart::Part::bytes(file_data)
            .file_name(media.filename.clone().unwrap_or_else(|| "file".to_string()))
            .mime_str(&media.mime_type)
            .map_err(|e| GatewayError::Internal(format!("Invalid mime type: {}", e)))?;

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("file_type", file_type.to_string())
            .text(
                "file_name",
                media.filename.clone().unwrap_or_else(|| "file".to_string()),
            );

        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu upload failed: {}", e)))?;

        let result: FeishuApiResponse<FeishuUploadData> = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu upload parse failed: {}", e)))?;

        if result.code != 0 {
            return Err(GatewayError::Internal(format!(
                "Feishu upload error: {} (code {})",
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        result
            .data
            .and_then(|d| d.file_key)
            .ok_or_else(|| GatewayError::Internal("No file_key in upload response".to_string()))
    }
}

impl Default for FeishuAdapter {
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
        let adapter = FeishuAdapter::new();
        assert_eq!(adapter.platform_name(), "feishu");
        assert_eq!(adapter.state(), AdapterState::Created);
        assert!(!adapter.capabilities.is_empty());
    }

    #[tokio::test]
    async fn test_init_missing_config() {
        let mut adapter = FeishuAdapter::new();
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
        assert!(!result.ok); // 应该失败，缺少配置
    }

    #[tokio::test]
    async fn test_init_missing_app_id() {
        let mut adapter = FeishuAdapter::new();
        let result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("test_secret".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert!(!result.ok); // 缺少 app_id
    }

    #[tokio::test]
    async fn test_init_valid_config() {
        let mut adapter = FeishuAdapter::new();
        let result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("test_secret".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({"app_id": "cli_xxxxx"}),
            })
            .await
            .unwrap();
        assert!(result.ok);
    }

    #[test]
    fn test_capabilities() {
        let adapter = FeishuAdapter::new();
        let caps = adapter.capabilities();
        assert!(
            caps.iter()
                .any(|c| c.name == CapabilityName::Text && c.supported)
        );
        assert!(
            caps.iter()
                .any(|c| c.name == CapabilityName::Group && c.supported)
        );
        assert!(
            caps.iter()
                .any(|c| c.name == CapabilityName::MessageEdit && c.supported)
        );
    }

    #[test]
    fn test_status_summary() {
        let adapter = FeishuAdapter::new();
        let status = adapter.status_summary();
        assert_eq!(status.platform, "feishu");
        assert_eq!(status.display_name, "飞书");
        assert!(!status.connected);
    }

    #[tokio::test]
    async fn test_runtime_config_before_init() {
        let adapter = FeishuAdapter::new();
        let rc = adapter.runtime_config();
        assert!(!rc.enabled);
        assert!(!rc.token_configured);
        // 未初始化时 extra 应为 null 或空对象
        assert!(rc.extra.is_object() || rc.extra.is_null());
    }

    #[tokio::test]
    async fn test_runtime_config_after_init() {
        let mut adapter = FeishuAdapter::new();
        adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("secret".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({"app_id": "cli_xxx"}),
            })
            .await
            .unwrap();
        let rc = adapter.runtime_config();
        assert!(rc.enabled);
        assert!(rc.token_configured);
        assert_eq!(
            rc.extra.get("app_id").and_then(|v| v.as_str()),
            Some("cli_xxx")
        );
    }

    #[tokio::test]
    async fn test_health_before_init() {
        let adapter = FeishuAdapter::new();
        let health = adapter.health().await;
        assert_eq!(health.status, HealthStatus::Down);
        assert!(!health.connected);
    }

    #[tokio::test]
    async fn test_disconnect_idempotent() {
        let mut adapter = FeishuAdapter::new();
        // 在 connect 之前调用 disconnect 不应 panic
        let result = adapter.disconnect().await;
        assert!(result.is_ok());
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_double_disconnect() {
        let mut adapter = FeishuAdapter::new();
        adapter.disconnect().await.unwrap();
        adapter.disconnect().await.unwrap(); // 连续两次 disconnect
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_get_chat_info_network_error() {
        let adapter = FeishuAdapter::new();
        // 未初始化时调用应返回错误
        let result = adapter.get_chat_info("oc_test").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_adapter_default() {
        let adapter = FeishuAdapter::default();
        assert_eq!(adapter.platform_name(), "feishu");
    }

    #[tokio::test]
    async fn test_send_before_connect() {
        let adapter = FeishuAdapter::new();
        let result = adapter
            .send(SendTextParams {
                chat_id: "oc_test".to_string(),
                message: OutboundMessage {
                    text: "hello".to_string(),
                    parse_mode: ParseMode::None,
                },
                reply_to: None,
                metadata: None,
            })
            .await;
        // 未初始化时发送应返回错误
        assert!(result.unwrap().error.is_some());
    }

    #[tokio::test]
    async fn test_delete_message_unsupported() {
        let adapter = FeishuAdapter::new();
        let result = adapter.delete_message("oc_test", "om_test").await.unwrap();
        assert!(!result.success);
        assert_eq!(result.error.as_deref(), Some("飞书不支持删除消息"));
    }

    #[test]
    fn test_capabilities_all_expected() {
        let adapter = FeishuAdapter::new();
        let caps = adapter.capabilities();
        let expected = vec![
            CapabilityName::Text,
            CapabilityName::Image,
            CapabilityName::Audio,
            CapabilityName::Video,
            CapabilityName::Document,
            CapabilityName::Interactive,
            CapabilityName::Markdown,
            CapabilityName::Group,
            CapabilityName::MessageEdit,
            CapabilityName::MessageDelete,
        ];
        for name in expected {
            assert!(
                caps.iter().any(|c| c.name == name && c.supported),
                "capability {:?} should be supported",
                name
            );
        }
    }
}
