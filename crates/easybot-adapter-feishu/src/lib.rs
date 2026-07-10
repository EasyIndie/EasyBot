#![allow(missing_docs)]

//! 飞书/Lark 平台适配器
//!
//! 使用飞书开放平台 API 实现消息收发。
//! - 发送: HTTP REST API (im/v1/messages)
//! - 接收: 事件订阅 (WebSocket 长连接 / Webhook)

mod event;
mod types;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::capabilities;
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::event_types;
use easybot_core::types::message::*;
use larksuite_oapi_sdk_rs::{Client, EventDispatcher};
use tokio::sync::broadcast;
use types::*;

/// 飞书开放平台 API 基础 URL
const FEISHU_API: &str = "https://open.feishu.cn/open-apis";

/// Token 刷新阈值（秒），在过期前提前刷新
const TOKEN_REFRESH_MARGIN: u64 = 300;

/// 飞书 tenant_access_token 共享存储
///
/// 适配器实例和 WebSocket 后台任务共用同一实例，
/// 避免两套独立的 token 缓存同时刷新浪费 API 调用。
#[derive(Clone, Default)]
struct FeishuTokenStore {
    inner: Arc<tokio::sync::RwLock<FeishuTokenInner>>,
}

#[derive(Default)]
struct FeishuTokenInner {
    token: Option<String>,
    expires_at: i64,
}

impl FeishuTokenStore {
    fn new() -> Self {
        Self::default()
    }

    /// 获取有效 token，必要时自动刷新
    async fn get(
        &self,
        client: &reqwest::Client,
        app_id: &str,
        app_secret: &str,
        base_url: &str,
    ) -> Result<String, GatewayError> {
        let now_ms = chrono::Utc::now().timestamp_millis();

        {
            let inner = self.inner.read().await;
            let margin = TOKEN_REFRESH_MARGIN as i64 * 1000;
            if inner.expires_at > now_ms + margin
                && let Some(ref token) = inner.token
            {
                return Ok(token.clone());
            }
        }

        // 需要刷新
        self.refresh(client, app_id, app_secret, base_url).await
    }

    /// 刷新 token
    async fn refresh(
        &self,
        client: &reqwest::Client,
        app_id: &str,
        app_secret: &str,
        base_url: &str,
    ) -> Result<String, GatewayError> {
        let url = format!("{}/auth/v3/tenant_access_token/internal", base_url);

        let resp: FeishuTokenResponse = client
            .post(&url)
            .json(&serde_json::json!({
                "app_id": app_id,
                "app_secret": app_secret,
            }))
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu token refresh failed: {}", e)))?
            .json()
            .await
            .map_err(|e| {
                GatewayError::Internal(format!("Feishu token refresh parse failed: {}", e))
            })?;

        if resp.code != 0 {
            return Err(GatewayError::Internal(format!(
                "Feishu auth failed: {} (code {})",
                resp.msg.unwrap_or_default(),
                resp.code
            )));
        }

        let token = resp.tenant_access_token.ok_or_else(|| {
            GatewayError::Internal("No token in feishu refresh response".to_string())
        })?;
        let expire = resp.expire.unwrap_or(7200) as i64;

        let mut inner = self.inner.write().await;
        inner.token = Some(token.clone());
        inner.expires_at = chrono::Utc::now().timestamp_millis() + (expire * 1000);

        Ok(token)
    }
}

/// 飞书适配器
pub struct FeishuAdapter {
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
    /// Background liveness heartbeat (updated by the WebSocket task)
    heartbeat: Heartbeat,
    /// 缓存的 HTTP 客户端（OnceLock 延迟初始化，与 Telegram 适配器模式一致）
    http_client: std::sync::OnceLock<reqwest::Client>,
    /// 共享的 access token 存储（适配器 + WebSocket 后台任务共用）
    token_store: FeishuTokenStore,
}

impl FeishuAdapter {
    pub fn new() -> Self {
        Self {
            platform_name: "feishu".to_string(),
            display_name: "飞书".to_string(),
            config: None,
            state: AdapterState::Created,
            bot_info: None,
            capabilities: capabilities![
                (Text, true),
                (Image, true),
                (Audio, true),
                (Video, true),
                (Document, true),
                (Interactive, true),
                (Markdown, true),
                (Group, true),
                (MessageEdit, true),
                (MessageDelete, true),
            ],
            messages_in: Arc::new(AtomicU64::new(0)),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            cancel_tx: None,
            heartbeat: Heartbeat::new(),
            http_client: std::sync::OnceLock::new(),
            token_store: FeishuTokenStore::new(),
        }
    }

    /// 设置事件总线（在 init 之前调用）
    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// 获取或创建 HTTP 客户端（OnceLock 延迟初始化，首次调用时创建）
    fn client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_else(|e| {
                    tracing::error!("Failed to create Feishu HTTP client: {}", e);
                    // Fallback to default client — connection will fail later
                    // with a more descriptive error rather than panicking
                    reqwest::Client::new()
                })
        })
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
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| GatewayError::ConfigError("Adapter not initialized".to_string()))?;
        let app_id = config
            .extra
            .get("app_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                GatewayError::ConfigError("Missing 'app_id' in feishu config.extra".to_string())
            })?;
        let app_secret = config.token.as_deref().ok_or_else(|| {
            GatewayError::ConfigError("Missing 'token' (app_secret) for feishu".to_string())
        })?;
        let base_url = self.api_base_url();
        self.token_store
            .get(self.client(), app_id, app_secret, base_url)
            .await
    }

    /// 统一的飞书 API 请求方法
    async fn send_api_request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, GatewayError> {
        let token = self.ensure_token().await?;
        let client = self.client();
        let url = format!("{}{}", self.api_base_url(), path);

        let mut req = client
            .request(method.clone(), &url)
            .header("Authorization", format!("Bearer {}", token));
        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu {} failed: {}", method, e)))?;

        let result: FeishuApiResponse<T> = resp.json().await.map_err(|e| {
            GatewayError::Internal(format!("Feishu {} parse failed: {}", method, e))
        })?;

        if result.code != 0 {
            return Err(GatewayError::Internal(format!(
                "Feishu API error ({} {}): {} (code {})",
                method,
                path,
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        result.data.ok_or_else(|| {
            GatewayError::Internal(format!(
                "Feishu API returned no data for {} {}",
                method, path
            ))
        })
    }

    /// 飞书 API GET 请求
    async fn api_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, GatewayError> {
        self.send_api_request::<T>(reqwest::Method::GET, path, None)
            .await
    }

    /// 飞书 API POST 请求
    async fn api_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        self.send_api_request::<T>(reqwest::Method::POST, path, Some(body))
            .await
    }

    /// 飞书 API PUT 请求
    async fn api_put<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        self.send_api_request::<T>(reqwest::Method::PUT, path, Some(body))
            .await
    }

    /// 飞书 API DELETE 请求（不要求 data 字段）
    async fn api_delete(&self, path: &str) -> Result<(), GatewayError> {
        let token = self.ensure_token().await?;
        let client = self.client();
        let url = format!("{}{}", self.api_base_url(), path);

        let resp = client
            .delete(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu DELETE failed: {}", e)))?;

        let result: FeishuApiResponse<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu DELETE parse failed: {}", e)))?;

        if result.code != 0 {
            return Err(GatewayError::Internal(format!(
                "Feishu API error (DELETE {}): {} (code {})",
                path,
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        Ok(())
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
        self.state = AdapterState::Starting;

        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        // 1. 获取配置并初始化 token store
        let config = self.config.as_ref().ok_or_else(|| {
            GatewayError::Internal("connect() called before init() — config not set".into())
        })?;
        let app_id = config
            .extra
            .get("app_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                GatewayError::ConfigError("Missing 'app_id' in feishu config.extra".to_string())
            })?;
        let app_secret = config.token.as_deref().ok_or_else(|| {
            GatewayError::ConfigError("Missing 'token' (app_secret) for feishu".to_string())
        })?;
        let base_url = self.api_base_url();

        // 获取 access token 验证凭证
        self.token_store
            .get(self.client(), app_id, app_secret, base_url)
            .await?;

        self.state = AdapterState::Connected;
        self.bot_info = Some(BotInfo {
            name: app_id.to_string(),
            username: Some("feishu_bot".to_string()),
            id: app_id.to_string(),
        });

        tracing::info!("飞书适配器已连接");
        self.heartbeat.record_connection();

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

            // 群成员角色缓存（chat_id:member_open_id → role + TTL），30 秒过期
            let role_cache: Arc<Mutex<HashMap<String, (SenderRole, Instant)>>> =
                Arc::new(Mutex::new(HashMap::new()));
            // 共享 token 存储（与适配器实例共用，避免两套缓存）
            let shared_token_store = self.token_store.clone();
            let feishu_http = reqwest::Client::new();
            let feishu_base_url = self.api_base_url().to_string();
            let mi = self.messages_in.clone();

            // 读取事件验证配置（优先从 config.extra 读取，再从环境变量回退）
            // verification_token: 飞书开放平台「事件订阅」→「配置」→「Verification Token」
            // encrypt_key: 飞书开放平台「事件订阅」→「配置」→「Encrypt Key」（用于 AES 解密）
            // 从 config.extra 或环境变量读取事件验证配置
            let env_verify_token = std::env::var("FEISHU_VERIFICATION_TOKEN").ok();
            let env_encrypt_key = std::env::var("FEISHU_ENCRYPT_KEY").ok();
            let verify_token = config
                .extra
                .get("verification_token")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| env_verify_token.as_deref().filter(|s| !s.is_empty()))
                .unwrap_or("");
            let encrypt_key = config
                .extra
                .get("encrypt_key")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| env_encrypt_key.as_deref().filter(|s| !s.is_empty()))
                .unwrap_or("");

            let dispatcher = if verify_token.is_empty() && encrypt_key.is_empty() {
                tracing::info!(
                    "飞书事件签名验证未配置（WebSocket 连接使用 OAuth 鉴权），跳过 per-event 验证"
                );
                EventDispatcher::new("", "").skip_sign_verify()
            } else {
                tracing::info!(
                    "飞书事件签名验证已启用（verify_token={}, encrypt_key={}）",
                    if verify_token.is_empty() {
                        "未设置"
                    } else {
                        "已配置"
                    },
                    if encrypt_key.is_empty() {
                        "未设置"
                    } else {
                        "已配置"
                    },
                );
                EventDispatcher::new(verify_token, encrypt_key)
            }
            // 处理入站消息
            .on_event(EVENT_MESSAGE_RECEIVE_V1, {
                let rc = role_cache.clone();
                move |event_data| {
                    let eb = eb.clone();
                    let bot_id = app_id_owned.clone();
                    let secret = app_secret.clone();
                    let hc = feishu_http.clone();
                    let bu = feishu_base_url.clone();
                    let ts = shared_token_store.clone();
                    let rc = rc.clone();
                    let mi = mi.clone();
                    async move {
                        let sender_role = Self::resolve_feishu_role(
                            &event_data,
                            &hc,
                            &bu,
                            &ts,
                            &bot_id,
                            &secret,
                            &rc,
                        )
                        .await;
                        event::handle_message_receive(event_data, &eb, &bot_id, sender_role).await;
                        mi.fetch_add(1, Ordering::Relaxed);
                        Ok(())
                    }
                }
            })
            // 监听群配置变更事件（群主转移、管理员变更等）
            .on_event("im.chat.updated_v1", {
                let rc = role_cache.clone();
                move |event_data| {
                    let rc = rc.clone();
                    async move {
                        // 清除该群的缓存，下次消息会重新获取角色
                        if let Some(chat_id) =
                            event_data.pointer("/chat_id").and_then(|v| v.as_str())
                        {
                            if let Ok(mut cache) = rc.lock() {
                                cache.retain(|key, _| !key.starts_with(&format!("{}:", chat_id)));
                            }
                            tracing::info!("飞书群配置变更，已清除群 {} 的角色缓存", chat_id);
                        }
                        Ok(())
                    }
                }
            });

            let ws_client = sdk_client.ws_client(dispatcher);
            let ws_client = ws_client.log_level(tracing::Level::WARN);

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
            uptime: self.heartbeat.uptime_secs().into(),
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
            uptime: self.heartbeat.uptime_secs().into(),
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
        if let Some(bus) = &self.event_bus {
            bus.publish_send_result(
                if send_result.success {
                    event_types::MESSAGE_SENT
                } else {
                    event_types::MESSAGE_FAILED
                },
                "feishu",
                &params.chat_id,
                &send_result,
            );
        }
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
        if let Some(bus) = &self.event_bus {
            bus.publish_send_result(
                if send_result.success {
                    event_types::MESSAGE_SENT
                } else {
                    event_types::MESSAGE_FAILED
                },
                "feishu",
                &params.chat_id,
                &send_result,
            );
        }
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
        if let Some(bus) = &self.event_bus {
            bus.publish_send_result(
                if send_result.success {
                    event_types::MESSAGE_SENT
                } else {
                    event_types::MESSAGE_FAILED
                },
                "feishu",
                &params.chat_id,
                &send_result,
            );
        }
        Ok(send_result)
    }

    async fn edit_message(&self, params: EditMessageParams) -> Result<EditResult, GatewayError> {
        // 飞书 open-api: PUT /im/v1/messages/{message_id} (文本编辑)
        // 消息卡片编辑用 PATCH，文本编辑用 PUT
        let path = format!("/im/v1/messages/{}", params.message_id);
        let content = serde_json::json!({
            "text": params.message.text,
        });
        let body = serde_json::json!({
            "content": content.to_string(),
            "msg_type": "text",
        });

        let _: serde_json::Value = self.api_put(&path, &body).await?;

        Ok(EditResult {
            success: true,
            updated_at: Some(chrono::Utc::now().timestamp_millis()),
            error: None,
        })
    }

    async fn delete_message(
        &self,
        _chat_id: &str,
        message_id: &str,
    ) -> Result<DeleteResult, GatewayError> {
        // 飞书开放平台支持撤回自己的消息（24h 内）
        // DELETE /im/v1/messages/{message_id}
        let path = format!("/im/v1/messages/{}", message_id);
        match self.api_delete(&path).await {
            Ok(()) => Ok(DeleteResult {
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

    // ── 会话富化 ──

    async fn enrich_source(
        &self,
        source: &easybot_core::types::session::SessionSource,
    ) -> Option<easybot_core::types::session::SessionSource> {
        let mut enriched = source.clone();

        // 通过飞书群信息 API 获取群名称
        let chat_id = &source.chat_id;
        let chat_path = format!("/open-apis/im/v1/chats/{}", chat_id);
        if let Ok(chat_info) = self.api_get::<serde_json::Value>(&chat_path).await
            && let Some(name) = chat_info
                .get("data")
                .and_then(|d| d.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        {
            enriched.chat_name = Some(name);
        }

        // 通过飞书联系人 API 查询用户信息
        if let Some(user_id) = &source.user_id {
            let user_path = format!("/open-apis/contact/v3/users/{}", user_id);
            if let Ok(user_info) = self.api_get::<serde_json::Value>(&user_path).await
                && let Some(name) = user_info
                    .get("data")
                    .and_then(|d| d.get("user"))
                    .and_then(|u| u.get("name"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            {
                enriched.user_name = Some(name);
            }
        }

        Some(enriched)
    }
}

// ── 辅助方法 ──

impl FeishuAdapter {
    /// 上传媒体文件到飞书，返回 file_key
    async fn upload_media(&self, media: &MediaAttachment) -> Result<String, GatewayError> {
        let token = self.ensure_token().await?;
        let client = self.client();
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
            // SECURITY: Reject oversized downloads to prevent OOM
            if let Some(content_length) = resp.content_length() {
                const MAX_DOWNLOAD_BYTES: u64 = 25 * 1024 * 1024; // 25MB
                if content_length > MAX_DOWNLOAD_BYTES {
                    return Err(GatewayError::Internal(format!(
                        "Rejected media download: {} bytes exceeds {} limit",
                        content_length, MAX_DOWNLOAD_BYTES
                    )));
                }
            }
            resp.bytes()
                .await
                .map_err(|e| GatewayError::Internal(format!("Read media bytes failed: {}", e)))?
                .to_vec()
        } else if let Some(ref base64_data) = media.data {
            // 解码 base64 数据作为文件内容
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(base64_data)
                .map_err(|e| GatewayError::Internal(format!("Base64 decode failed: {}", e)))?
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

    // ── 角色解析（WebSocket 事件用） ──

    /// 解析群消息发送者在飞书群内的角色。
    /// 对非群消息直接返回 None，群消息调用飞书 API 查询后缓存 30 秒。
    pub(crate) async fn resolve_feishu_role(
        event_data: &serde_json::Value,
        client: &reqwest::Client,
        base_url: &str,
        token_store: &FeishuTokenStore,
        app_id: &str,
        app_secret: &str,
        role_cache: &Mutex<HashMap<String, (SenderRole, Instant)>>,
    ) -> Option<SenderRole> {
        // 只处理群聊消息
        let chat_type = event_data
            .pointer("/message/chat_type")
            .and_then(|v| v.as_str())?;
        if chat_type != "group" {
            return None;
        }
        let chat_id = event_data
            .pointer("/message/chat_id")
            .and_then(|v| v.as_str())?;
        let sender_id = event_data
            .pointer("/sender/sender_id/open_id")
            .and_then(|v| v.as_str())?;

        let cache_key = format!("{}:{}", chat_id, sender_id);

        // 检查缓存（30 秒 TTL 过期后重新查询）
        {
            let mut cache = role_cache.lock().ok()?;
            if let Some((role, expiry)) = cache.get(&cache_key) {
                if expiry.elapsed() < Duration::from_secs(30) {
                    return Some(role.clone());
                }
                // 缓存过期，移除
                cache.remove(&cache_key);
            }
        }

        // 获取 token（通过共享 token store，与适配器实例共用缓存）
        let token = token_store
            .get(client, app_id, app_secret, base_url)
            .await
            .ok()?;

        // 使用获取群信息 API 查询群主和管理员列表（无需分页，一次调用即可）
        let chat_url = format!("{}/im/v1/chats/{}", base_url.trim_end_matches('/'), chat_id);
        let resp = client
            .get(&chat_url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            tracing::warn!("飞书群信息 API 返回 {}", resp.status());
            return None;
        }

        let body: serde_json::Value = resp.json().await.ok()?;
        let data = body.get("data")?;

        // 获取群主 ID
        let owner_id = data.get("owner_id").and_then(|v| v.as_str());
        // 获取管理员 ID 列表
        let admin_ids: Vec<&str> = data
            .get("user_manager_id_list")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        // 判断角色
        let role = if owner_id == Some(sender_id) {
            SenderRole::Owner
        } else if admin_ids.contains(&sender_id) {
            SenderRole::Admin
        } else {
            SenderRole::Member
        };

        // 写入缓存（30 秒 TTL + im.chat.updated_v1 事件双重失效机制）
        if let Ok(mut cache) = role_cache.lock() {
            cache.insert(cache_key, (role.clone(), Instant::now()));
        }

        Some(role)
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
    async fn test_delete_message_before_connect() {
        let adapter = FeishuAdapter::new();
        let result = adapter.delete_message("oc_test", "om_test").await.unwrap();
        // 未初始化时，删除应返回失败（token refresh 失败）
        assert!(!result.success);
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
