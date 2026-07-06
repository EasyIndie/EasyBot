#![allow(missing_docs)]

//! QQ 频道机器人适配器
//!
//! 使用 QQ 频道机器人 API（WebSocket Gateway + HTTP API）实现消息收发。
//! 架构类似 Discord 适配器：
//! - Gateway WebSocket 用于接收消息（AT_MESSAGE_CREATE 等事件）
//! - HTTP REST API 用于发送消息
//!
//! # API 参考文档
//!
//! - 官方文档: <https://bot.q.qq.com/wiki/develop/api-v2/>
//! - 富媒体消息（上传文件 + 发送）: <https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/send-receive/rich-media.html>
//! - 消息类型: <https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/type-guide.html>
//! - 事件列表: <https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/send-receive/event.html>
//! - QQ 开放平台: <https://q.qq.com/>
//!
//! # 文件上传注意事项
//!
//! QQ Bot API v2 的文件上传端点（`/v2/groups/{id}/files` 和 `/v2/users/{id}/files`）
//! 使用 **JSON body** 方式上传，参数为 `file_data`（base64 编码），而非 multipart/form-data。
//!
//! 上传后可选择:
//! - `srv_send_msg=true` — 上传后自动发送（占用主动消息频次），返回消息 ID
//! - `srv_send_msg=false` — 仅上传获取 `file_info`，再用 `msg_type: 7` 手动发送
//!
//! `msg_type: 7` 的 body 不能带空 `content` 字段（已知 Bug 会渲染多余空行）。

mod auth;
mod gateway;
mod types;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::event_types;
use easybot_core::types::message::*;
use tokio::sync::broadcast;
use types::*;

/// QQ API 基础 URL（正式环境）
const QQ_API: &str = "https://api.sgroup.qq.com";
/// QQ 鉴权 API 基础 URL（正式环境）
const QQ_AUTH_API: &str = "https://bots.qq.com";

use auth::QqTokenStore;

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
    http_client: std::sync::OnceLock<reqwest::Client>,
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
            http_client: std::sync::OnceLock::new(),
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

    fn client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("Failed to create HTTP client")
        })
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

    /// 发送 QQ API 请求，检测 401 Unauthorized 时自动刷新 token 并重试一次
    async fn send_api_request<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, GatewayError> {
        let token = self.bot_token()?;
        let result = self
            .send_api_request_raw(&token, method.clone(), path, body)
            .await;

        // token 过期（HTTP 401）时刷新一次并重试
        if let Err(GatewayError::Internal(msg)) = &result
            && msg.contains("401")
        {
            tracing::warn!(
                "QQ API 返回 401 Unauthorized（token 可能已过期），尝试刷新 token 后重试"
            );
            if let Some(ref store) = self.token_store {
                let _ = store.refresh().await;
            }
            let new_token = self.bot_token()?;
            return self
                .send_api_request_raw(&new_token, method, path, body)
                .await;
        }

        result
    }

    /// 发送原始的 QQ API 请求（无 401 重试）
    async fn send_api_request_raw<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, GatewayError> {
        let client = self.client();
        let url = format!("{}{}", self.api_base_url(), path);
        let mut req = client
            .request(method.clone(), &url)
            .header("Authorization", token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("QQ {} {} failed: {}", method, path, e)))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "QQ API error ({} {}): {} - {}",
                method, path, s, b
            )));
        }
        resp.json::<T>().await.map_err(|e| {
            GatewayError::Internal(format!("QQ {} {} parse failed: {}", method, path, e))
        })
    }

    /// QQ API GET
    async fn api_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, GatewayError> {
        self.send_api_request(reqwest::Method::GET, path, None)
            .await
    }

    /// QQ API POST
    async fn api_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        self.send_api_request(reqwest::Method::POST, path, Some(body))
            .await
    }

    /// QQ API PATCH
    async fn api_patch<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        self.send_api_request(reqwest::Method::PATCH, path, Some(body))
            .await
    }

    /// QQ API DELETE（不期望 JSON body，仅验证状态码）
    async fn api_delete(&self, path: &str) -> Result<(), GatewayError> {
        let token = self.bot_token()?;
        let result = self.api_delete_raw(&token, path).await;

        // token 过期（HTTP 401）时刷新一次并重试
        if let Err(GatewayError::Internal(msg)) = &result
            && msg.contains("401")
        {
            tracing::warn!(
                "QQ API 返回 401 Unauthorized（token 可能已过期），尝试刷新 token 后重试"
            );
            if let Some(ref store) = self.token_store {
                let _ = store.refresh().await;
            }
            let new_token = self.bot_token()?;
            return self.api_delete_raw(&new_token, path).await;
        }

        result
    }

    /// 发送原始 DELETE 请求（无 401 重试）
    async fn api_delete_raw(&self, token: &str, path: &str) -> Result<(), GatewayError> {
        let client = self.client();
        let url = format!("{}{}", self.api_base_url(), path);
        let resp = client
            .delete(&url)
            .header("Authorization", token)
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

/// 从 MIME 类型推导 QQ 文件上传所需的 file_type 参数
/// 1=image, 2=video, 3=voice, 默认 1 (image)
fn mime_to_file_type(mime_type: &str) -> u32 {
    if mime_type.starts_with("image/") {
        1
    } else if mime_type.starts_with("video/") {
        2
    } else if mime_type.starts_with("audio/") {
        3
    } else {
        1 // 默认为图片
    }
}

// ── 消息发送（自动判断频道/群聊） ──

impl QqAdapter {
    /// 判断错误是否为"资源不存在"（级联到下一个端点），
    /// 如果是鉴权/限流/其他错误则立即返回，不级联。
    fn is_not_found_error(e: &GatewayError) -> bool {
        let msg = e.to_string();
        // QQ 业务错误码 11263 = 资源不存在（频道/群/用户）
        // HTTP 404 = 端点不存在
        msg.contains("404") || msg.contains("11263")
    }

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
                if Self::is_not_found_error(&e) {
                    tracing::debug!(
                        "QQ chat_id {} is not a channel (e={}), trying next endpoint",
                        chat_id,
                        e
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
                if Self::is_not_found_error(&e) {
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

    /// 通用文件上传（JSON body，base64 file_data）
    ///
    /// 向 QQ API 上传文件，使用 JSON body + base64 编码文件数据。
    /// API 文档: https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/send-receive/rich-media.html
    /// 上传文件到 QQ（JSON body + base64 file_data）
    ///
    /// 使用 JSON body 上传文件，兼容 C2C 和群聊端点。
    /// API 文档: https://bot.q.qq.com/wiki/develop/api-v2/server-inter/message/send-receive/rich-media.html
    async fn upload_file_via_json(
        &self,
        endpoint_type: &str,
        chat_id: &str,
        file_data: Vec<u8>,
        mime_type: &str,
        srv_send_msg: bool,
    ) -> Result<QqFileUploadResponse, GatewayError> {
        let path = format!("/v2/{}/{}/files", endpoint_type, chat_id);
        let file_type = mime_to_file_type(mime_type);
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&file_data);
        let body = serde_json::json!({
            "file_type": file_type,
            "srv_send_msg": srv_send_msg,
            "file_data": b64,
        });
        self.api_post::<QqFileUploadResponse>(&path, &body).await
    }

    /// 使用 msg_type: 7 发送媒体消息（通过已上传的 file_info）
    ///
    /// 先上传文件获取 file_info，再发送 msg_type: 7 消息。
    /// 使用 JSON body + base64 file_data（QQ Bot API 群聊端点必需此格式）。
    async fn send_media_via_upload(
        &self,
        chat_id: &str,
        endpoint_type: &str,
        file_data: Vec<u8>,
        _filename: &str,
        mime_type: &str,
        text: Option<String>,
    ) -> Result<QqSendMessageResponse, GatewayError> {
        // Step 1: Upload file to get file_info
        let uploaded = self
            .upload_file_via_json(endpoint_type, chat_id, file_data.clone(), mime_type, false)
            .await;

        let uploaded = match uploaded {
            Ok(resp) => resp,
            Err(e) => {
                // srv_send_msg=false upload failed → fall back to srv_send_msg=true (no text)
                tracing::warn!(
                    "QQ two-step upload (srv_send_msg=false) failed: {}, \
                     falling back to direct upload (srv_send_msg=true, no text)",
                    e
                );
                return self
                    .upload_file_via_json(endpoint_type, chat_id, file_data, mime_type, true)
                    .await
                    .map(|resp| QqSendMessageResponse {
                        id: resp
                            .id
                            .unwrap_or_else(|| format!("file:{}", resp.file_uuid)),
                        timestamp: resp.timestamp,
                    });
            }
        };

        // Step 2: Send msg_type: 7 with the file_info
        // QQ 已知 Bug: msg_type:7 带空 content 字段会渲染多余空行，只当有文本时才加
        let content = text
            .filter(|t| !t.is_empty())
            .map(|t| t.replace('[', "【").replace(']', "】"));
        let mut msg_body = serde_json::json!({
            "msg_type": 7,
            "media": { "file_info": uploaded.file_info },
        });
        if let Some(ref c) = content {
            msg_body["content"] = serde_json::json!(c);
        }

        let msg_path = format!("/v2/{}/{}/messages", endpoint_type, chat_id);
        match self
            .api_post::<QqSendMessageResponse>(&msg_path, &msg_body)
            .await
        {
            Ok(resp) => Ok(resp),
            Err(e) => {
                tracing::warn!(
                    "QQ msg_type 7 send failed: {}, falling back to direct upload",
                    e
                );
                self.upload_file_via_json(endpoint_type, chat_id, file_data, mime_type, true)
                    .await
                    .map(|resp| QqSendMessageResponse {
                        id: resp
                            .id
                            .unwrap_or_else(|| format!("file:{}", resp.file_uuid)),
                        timestamp: resp.timestamp,
                    })
            }
        }
    }

    /// 从 URL/base64 获取文件数据后上传到聊天（自动尝试 C2C 和群聊端点）
    ///
    /// 直接使用 QQ 文件上传端点（srv_send_msg=true），
    /// QQ 服务器会自动将文件作为消息发送给用户。
    /// 适用于 C2C 不支持 msg_type: 1/2 的场景。
    async fn send_c2c_media_upload_only(
        &self,
        chat_id: &str,
        media: &MediaAttachment,
        text: Option<String>,
    ) -> Result<QqSendMessageResponse, GatewayError> {
        // Resolve file data from base64 or URL
        let client = self.client().clone();

        let (file_data, filename, mime_type) = if let Some(data_b64) = &media.data {
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data_b64)
                .map_err(|e| {
                    GatewayError::Internal(format!("QQ upload: base64 decode failed: {}", e))
                })?;
            let fname = media.filename.clone().unwrap_or_else(|| "file".to_string());
            (decoded, fname, media.mime_type.clone())
        } else if let Some(file_url) = &media.url {
            let resp = client.get(file_url).send().await.map_err(|e| {
                GatewayError::Internal(format!("QQ upload: download failed: {}", e))
            })?;
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let data = resp.bytes().await.map_err(|e| {
                GatewayError::Internal(format!("QQ upload: download read failed: {}", e))
            })?;
            let fname = media
                .filename
                .clone()
                .or_else(|| file_url.split('/').next_back().map(|s| s.to_string()))
                .unwrap_or_else(|| "file".to_string());
            (data.to_vec(), fname, ct)
        } else {
            return Err(GatewayError::Internal(
                "No media data or URL provided for QQ upload".to_string(),
            ));
        };

        // Try C2C upload first, then group upload as fallback
        let mut last_error = None;
        for endpoint_type in &["users", "groups"] {
            let result = if text.as_ref().is_some_and(|t| !t.is_empty()) {
                self.send_media_via_upload(
                    chat_id,
                    endpoint_type,
                    file_data.clone(),
                    &filename,
                    &mime_type,
                    text.clone(),
                )
                .await
            } else {
                self.upload_file_via_json(
                    endpoint_type,
                    chat_id,
                    file_data.clone(),
                    &mime_type,
                    true,
                )
                .await
                .map(|resp| QqSendMessageResponse {
                    id: resp
                        .id
                        .unwrap_or_else(|| format!("file:{}", resp.file_uuid)),
                    timestamp: resp.timestamp,
                })
            };
            match result {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    tracing::debug!(
                        "QQ media upload to {} failed: {}, trying next endpoint",
                        endpoint_type,
                        e
                    );
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            GatewayError::Internal("QQ upload: all endpoints failed".to_string())
        }))
    }
}

fn publish_send_event(
    event_bus: &Option<Arc<EventBus>>,
    event_type: &str,
    chat_id: &str,
    result: &SendResult,
) {
    if let Some(bus) = event_bus {
        bus.publish_send_result(event_type, "qq", chat_id, result);
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
        let send_result = match self.try_send(&params.chat_id, &body).await {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(resp.id),
                    timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
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
        // 如果有 base64 data 但没有 URL，优先尝试 C2C 文件上传路径
        // （QQ API 中只有 C2C 端点支持直接二进制文件上传）。
        // 如果失败（可能因为 chat_id 是频道/群聊而非 C2C 用户），
        // 则降级到下方的 try_send 链路（频道→群聊→C2C）。
        if params.media.url.is_none() && params.media.data.is_some() {
            match self
                .send_c2c_media_upload_only(&params.chat_id, &params.media, params.text.clone())
                .await
            {
                Ok(resp) => {
                    self.messages_out.fetch_add(1, Ordering::Relaxed);
                    let send_result = SendResult {
                        success: true,
                        message_id: Some(resp.id),
                        timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
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
                    return Ok(send_result);
                }
                Err(c2c_err) => {
                    // C2C 上传失败 — chat_id 可能是频道/群聊而非 C2C 用户 openid。
                    // 降级到下方的 try_send 链路继续尝试。
                    tracing::debug!(
                        "QQ C2C media upload failed, falling through to try_send: {}",
                        c2c_err
                    );
                }
            }
        }

        // 当 URL 为空时，用 base64 data 构造 data URL（用于 try_send 的 msg_type 2/1 路径）
        let image_url = params
            .media
            .url
            .clone()
            .or_else(|| {
                params
                    .media
                    .data
                    .as_ref()
                    .map(|data| format!("data:{};base64,{}", params.media.mime_type, data))
            })
            .unwrap_or_default();
        let text_content = params.text.clone().unwrap_or_default();
        // 先试用 msg_type: 2（图文混排），频道/群聊支持此格式
        let body = serde_json::json!({
            "content": text_content,
            "image": image_url,
            "msg_type": 2,
        });
        let send_result = match self.try_send(&params.chat_id, &body).await {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(resp.id),
                    timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
                    error: None,
                    error_code: None,
                    retryable: false,
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                // C2C 私聊不支持 msg_type: 2（11255），降级为纯图片 msg_type: 1
                if err_str.contains("11255")
                    || (err_str.contains("/v2/users/") && err_str.contains("parse failed"))
                {
                    tracing::warn!(
                        "QQ C2C does not support msg_type: 2, retrying with msg_type: 1 (image only)"
                    );
                    let img_body = serde_json::json!({
                        "msg_type": 1,
                        "content": "",
                        "image": image_url,
                    });
                    match self.try_send(&params.chat_id, &img_body).await {
                        Ok(resp) => {
                            self.messages_out.fetch_add(1, Ordering::Relaxed);
                            SendResult {
                                success: true,
                                message_id: Some(resp.id),
                                timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
                                error: None,
                                error_code: None,
                                retryable: false,
                            }
                        }
                        Err(e2) => {
                            let err2_str = e2.to_string();
                            // 群聊/频道场景：try_send 可能回退到 C2C 端点返回 11255
                            // 此时不再重试 C2C 文件上传（Branch 1 已经试过且失败了）
                            self.errors.fetch_add(1, Ordering::Relaxed);
                            SendResult::fail(err2_str, true)
                        }
                    }
                } else {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    SendResult::fail(err_str, true)
                }
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
                                    (2u32, btn.callback_data.clone().unwrap_or_default())
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

        let send_result = match self.try_send(&params.chat_id, &body).await {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(resp.id),
                    timestamp: resp.timestamp.and_then(|t| t.parse::<i64>().ok()),
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

    // ── 会话富化 ──

    async fn enrich_source(
        &self,
        source: &easybot_core::types::session::SessionSource,
    ) -> Option<easybot_core::types::session::SessionSource> {
        // 仅对群聊消息尝试富化：chat_id=group_openid, user_id=member_openid
        if source.chat_type == ChatType::Group {
            let group_openid = &source.chat_id;
            let member_openid = source.user_id.as_ref()?;
            let path = format!("/v2/groups/{}/members/{}", group_openid, member_openid);
            match self.api_get::<serde_json::Value>(&path).await {
                Ok(member_info) => {
                    let mut enriched = source.clone();
                    // 尝试从响应中提取用户昵称
                    if let Some(nick) = member_info
                        .get("nickname")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                    {
                        enriched.user_name = Some(nick);
                    }
                    // 尝试提取角色
                    if let Some(role_str) = member_info.get("role").and_then(|v| v.as_str()) {
                        enriched.user_role = match role_str {
                            "admin" => Some(SenderRole::Admin),
                            "owner" => Some(SenderRole::Owner),
                            _ => Some(SenderRole::Member),
                        };
                    }
                    Some(enriched)
                }
                Err(_) => None,
            }
        } else {
            None
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
        assert!(
            adapter
                .capabilities()
                .iter()
                .any(|c| c.name == CapabilityName::Text && c.supported)
        );
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
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
        let messages_in = AtomicU64::new(0);

        let data = serde_json::json!({
            "id": "gmsg001",
            "group_openid": "GROUP_OPENID_001",
            "content": "@bot hello group",
            "author": {"member_openid": "MEMBER_001", "member_role": "admin"},
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
            assert_eq!(e.data["chat_type"], "Group");
            assert_eq!(e.data["chat_type"], "Group");
            assert_eq!(e.data["chat_id"], "GROUP_OPENID_001");
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_group_message_create_mentioned() {
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
            assert_eq!(e.data["chat_type"], "Group");
            assert_eq!(e.data["chat_type"], "Group");
            assert_eq!(e.data["chat_id"], "GROUP_OPENID_NEW001");
            assert_eq!(e.data["mentioned"], true);
            // metadata contains mentions info
            assert!(e.data["mentions"].is_array());
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_group_message_create_not_mentioned() {
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
            assert_eq!(e.data["chat_type"], "Group");
            assert_eq!(e.data["chat_id"], "GROUP_OPENID_NEW002");
            assert_eq!(e.data["mentioned"], false);
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_c2c() {
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
            assert_eq!(e.data["chat_type"], "Dm");
            assert_eq!(e.data["chat_type"], "Dm");
            assert_eq!(e.data["chat_id"], "USER_OPENID_001");
        }
        assert_eq!(messages_in.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_handle_dispatch_self_filter_channel() {
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::MESSAGE_INBOUND);
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
