//! 个人微信 (WeChat) 平台适配器
//!
//! 使用腾讯官方 iLink Bot API 实现个人微信消息收发。
//! 协议文档：https://ilinkai.weixin.qq.com
//!
//! # 配置
//! ```yaml
//! wechat:
//!   enabled: true
//!   # 可选：预填凭据（免二次扫码）
//!   extra:
//!     bot_token: "<saved_bot_token>"
//!     ilink_bot_id: "<saved_bot_id>"
//!     ilink_user_id: "<saved_user_id>"
//!     baseurl: "https://ilinkai.weixin.qq.com"
//! ```
//!
//! # 登录流程
//! 首次启动时终端打印 QR 码，微信扫码确认后自动保存凭据。
//!
//! # 已知限制
//! - 仅支持 DM（一对一聊天），不支持群聊
//! - 不支持 Markdown、贴纸、小程序消息
//! - 媒体文件需要 AES-128-ECB 加解密（当前仅支持文本消息）

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::message::*;
use easybot_core::types::error::GatewayError;

/// iLink Bot API 基础 URL
const ILINK_API: &str = "https://ilinkai.weixin.qq.com";

/// 长轮询超时（秒）
const LONGPOLL_TIMEOUT: u64 = 35;

/// Session 刷新间隔（秒），24 小时后过期需重连
const SESSION_REFRESH_INTERVAL: u64 = 82800; // 23 小时

// ── iLink API 响应类型 ──

/// QR 码响应
#[derive(Debug, serde::Deserialize)]
struct QrCodeResponse {
    ret: i64,
    errmsg: Option<String>,
    qrcode: Option<String>,
    #[serde(rename = "qrcode_img_content")]
    qrcode_img: Option<String>,
}

/// QR 码状态响应
#[derive(Debug, serde::Deserialize)]
struct QrCodeStatusResponse {
    ret: i64,
    errmsg: Option<String>,
    status: Option<String>,
    #[serde(default)]
    bot_token: Option<String>,
    #[serde(default)]
    ilink_bot_id: Option<String>,
    #[serde(default)]
    ilink_user_id: Option<String>,
    #[serde(default)]
    baseurl: Option<String>,
}

/// 长轮询消息响应
#[derive(Debug, serde::Deserialize)]
struct GetUpdatesResponse {
    ret: i64,
    errmsg: Option<String>,
    #[serde(default)]
    msgs: Vec<WeixinMessage>,
    #[serde(default)]
    get_updates_buf: Option<String>,
    #[serde(default)]
    longpolling_timeout_ms: Option<u64>,
}

/// 微信消息
#[derive(Debug, serde::Deserialize)]
struct WeixinMessage {
    #[serde(default)]
    msg_id: String,
    #[serde(default)]
    from_user: String,
    #[serde(default)]
    from_user_id: String,
    #[serde(default)]
    to_user_id: String,
    #[serde(default)]
    context_token: String,
    #[serde(default)]
    msg_type: i64,
    #[serde(default)]
    create_time: i64,
    #[serde(default)]
    content: Option<WeixinTextContent>,
    #[serde(default)]
    image: Option<WeixinMediaContent>,
    #[serde(default)]
    file: Option<WeixinFileContent>,
    #[serde(default)]
    voice: Option<WeixinVoiceContent>,
    #[serde(default)]
    video: Option<WeixinMediaContent>,
}

#[derive(Debug, serde::Deserialize)]
struct WeixinTextContent {
    text: String,
}

#[derive(Debug, serde::Deserialize)]
struct WeixinMediaContent {
    #[serde(default)]
    md5sum: Option<String>,
    #[serde(default)]
    file_size: Option<i64>,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    aes_key: Option<String>,
    #[serde(default)]
    file_url: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct WeixinFileContent {
    #[serde(default)]
    md5sum: Option<String>,
    #[serde(default)]
    file_size: Option<i64>,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    aes_key: Option<String>,
    #[serde(default)]
    file_url: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct WeixinVoiceContent {
    #[serde(default)]
    md5sum: Option<String>,
    #[serde(default)]
    file_size: Option<i64>,
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    aes_key: Option<String>,
    #[serde(default)]
    file_url: Option<String>,
    #[serde(default)]
    voice_seconds: Option<i64>,
    #[serde(default)]
    transcription: Option<String>,
}

/// 发送消息响应
#[derive(Debug, serde::Deserialize)]
struct SendMessageResponse {
    ret: i64,
    errmsg: Option<String>,
    #[serde(default)]
    msg_id: Option<String>,
    #[serde(default)]
    local_id: Option<String>,
}

/// Upload URL 响应
#[derive(Debug, serde::Deserialize)]
struct UploadUrlResponse {
    ret: i64,
    errmsg: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    key: Option<String>,
}

// ── 适配器 ──

/// 个人微信适配器
pub struct WeChatAdapter {
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
    http_client: Option<reqwest::Client>,
    /// iLink Bot Token（登录后获取）
    bot_token: tokio::sync::RwLock<Option<String>>,
    /// 长轮询游标
    updates_buf: tokio::sync::RwLock<Option<String>>,
    /// 取消信号
    cancel_tx: Option<tokio::sync::broadcast::Sender<()>>,
    /// iLink Bot ID
    ilink_bot_id: tokio::sync::RwLock<Option<String>>,
    /// iLink User ID
    ilink_user_id: tokio::sync::RwLock<Option<String>>,
}

impl WeChatAdapter {
    pub fn new() -> Self {
        Self {
            platform_name: "wechat".to_string(),
            display_name: "个人微信".to_string(),
            config: None,
            state: AdapterState::Created,
            bot_info: None,
            capabilities: vec![
                Capability { name: CapabilityName::Text, supported: true, limits: None },
                Capability { name: CapabilityName::Image, supported: true, limits: None },
                Capability { name: CapabilityName::Audio, supported: true, limits: None },
                Capability { name: CapabilityName::Video, supported: true, limits: None },
                Capability { name: CapabilityName::Document, supported: true, limits: None },
            ],
            messages_in: AtomicU64::new(0),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            http_client: None,
            bot_token: tokio::sync::RwLock::new(None),
            updates_buf: tokio::sync::RwLock::new(None),
            cancel_tx: None,
            ilink_bot_id: tokio::sync::RwLock::new(None),
            ilink_user_id: tokio::sync::RwLock::new(None),
        }
    }

    /// 创建适配器并设置 EventBus（用于注册时简化）
    pub fn new_with_event_bus(event_bus: Arc<EventBus>) -> Self {
        let mut adapter = Self::new();
        adapter.event_bus = Some(event_bus);
        adapter
    }

    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    fn client(&self) -> Result<&reqwest::Client, GatewayError> {
        self.http_client.as_ref().ok_or_else(|| {
            GatewayError::Internal("HTTP client not initialized".to_string())
        })
    }

    /// 构建 iLink API 请求的认证头
    fn auth_headers(&self, token: &str) -> reqwest::header::HeaderMap {
        use reqwest::header;
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        );
        headers.insert(
            header::HeaderName::from_static("authorizationtype"),
            header::HeaderValue::from_static("ilink_bot_token"),
        );
        // X-WECHAT-UIN：防重放，随机 uint32 base64
        let uin = uuid::Uuid::new_v4().as_u64_pair().0 as u32;
        headers.insert(
            header::HeaderName::from_static("x-wechat-uin"),
            header::HeaderValue::from_str(&base64_encode_uin(uin)).unwrap(),
        );
        headers
    }
}

/// Base64 编码 uint32（与官方 SDK 对齐）
fn base64_encode_uin(uin: u32) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(&uin.to_le_bytes())
}

#[async_trait]
impl PlatformAdapter for WeChatAdapter {
    fn platform_name(&self) -> &str { &self.platform_name }
    fn display_name(&self) -> &str { &self.display_name }
    fn capabilities(&self) -> &[Capability] { &self.capabilities }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        self.config = Some(config);
        self.http_client = Some(reqwest::Client::builder()
            .timeout(Duration::from_secs(60)) // 长轮询需要较长超时
            .build()
            .map_err(|e| GatewayError::Internal(format!("Failed to create HTTP client: {}", e)))?);

        // 尝试从配置中恢复凭据
        let extra = self.config.as_ref().unwrap().extra.clone();
        if let Some(token) = extra.get("bot_token").and_then(|v| v.as_str()) {
            *self.bot_token.write().await = Some(token.to_string());
        }
        if let Some(bot_id) = extra.get("ilink_bot_id").and_then(|v| v.as_str()) {
            *self.ilink_bot_id.write().await = Some(bot_id.to_string());
        }
        if let Some(user_id) = extra.get("ilink_user_id").and_then(|v| v.as_str()) {
            *self.ilink_user_id.write().await = Some(user_id.to_string());
        }

        self.state = AdapterState::Starting;
        Ok(InitResult { ok: true, error: None })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        let client = self.client()?;

        // 如果没有 bot_token，执行 QR 码登录
        if self.bot_token.read().await.is_none() {
            tracing::info!("个人微信适配器：需要扫码登录");

            // 获取 QR 码
            let qr_url = format!("{}/ilink/bot/get_bot_qrcode?bot_type=3", ILINK_API);
            let qr_resp: QrCodeResponse = client.get(&qr_url)
                .send()
                .await
                .map_err(|e| GatewayError::Internal(format!("Failed to get QR code: {}", e)))?
                .json()
                .await
                .map_err(|e| GatewayError::Internal(format!("Failed to parse QR response: {}", e)))?;

            if qr_resp.ret != 0 {
                return Err(GatewayError::Internal(format!(
                    "Get QR code failed: {} (ret {})", qr_resp.errmsg.unwrap_or_default(), qr_resp.ret
                )));
            }

            let qrcode = qr_resp.qrcode.ok_or_else(|| {
                GatewayError::Internal("No qrcode in response".to_string())
            })?;

            // 打印 QR 码（终端 ASCII + URL 备用）
            if let Some(img) = &qr_resp.qrcode_img {
                tracing::info!("扫描以下二维码登录个人微信（或访问备用 URL）：");
                // 简易终端打印 QR ASCII
                println!("\n{}", img);
            }

            // 轮询扫码状态（最多 120 秒）
            let status_url = format!("{}/ilink/bot/get_qrcode_status?qrcode={}", ILINK_API, qrcode);
            let mut logged = false;
            let mut token: Option<String> = None;
            let mut bot_id: Option<String> = None;
            let mut user_id: Option<String> = None;
            let mut baseurl: Option<String> = None;

            for _ in 0..120 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let status_resp: QrCodeStatusResponse = client.get(&status_url)
                    .send()
                    .await
                    .map_err(|e| GatewayError::Internal(format!("QR status poll failed: {}", e)))?
                    .json()
                    .await
                    .map_err(|e| GatewayError::Internal(format!("QR status parse failed: {}", e)))?;

                match status_resp.status.as_deref() {
                    Some("confirmed") => {
                        token = status_resp.bot_token;
                        bot_id = status_resp.ilink_bot_id;
                        user_id = status_resp.ilink_user_id;
                        baseurl = status_resp.baseurl;
                        break;
                    }
                    Some("scaned") => {
                        if !logged {
                            tracing::info!("微信已扫码，请在手机上确认");
                            logged = true;
                        }
                    }
                    Some("wait") | None => {
                        if !logged {
                            tracing::info!("等待扫码...");
                            logged = true;
                        }
                    }
                    Some("expired") => {
                        return Err(GatewayError::Internal("QR code expired".to_string()));
                    }
                    _ => {}
                }
            }

            let bot_token = token.ok_or_else(|| {
                GatewayError::Internal("QR login timeout or failed".to_string())
            })?;

            // 保存凭据
            *self.bot_token.write().await = Some(bot_token.clone());
            if let Some(id) = bot_id {
                *self.ilink_bot_id.write().await = Some(id.clone());
            }
            if let Some(uid) = user_id {
                *self.ilink_user_id.write().await = Some(uid.clone());
            }
            if let Some(url) = baseurl {
                tracing::info!("个人微信登录成功，baseurl: {}", url);
            }

            // 注意：凭据可以持久化到配置文件中，方便下次自动登录
            tracing::info!("个人微信适配器：扫码登录成功");
        }

        // 设置 bot_info
        let bot_id = self.ilink_bot_id.read().await.clone().unwrap_or_else(|| "wechat_bot".to_string());
        self.bot_info = Some(BotInfo {
            name: "个人微信".to_string(),
            username: Some(bot_id.clone()),
            id: bot_id,
        });

        self.state = AdapterState::Connected;
        tracing::info!("个人微信适配器已连接");

        // 启动长轮询消息接收
        if let Some(ref event_bus) = self.event_bus {
            let (cancel_tx, cancel_rx) = tokio::sync::broadcast::channel(1);
            self.cancel_tx = Some(cancel_tx);

            let eb = event_bus.clone();
            let client = self.client()?.clone();
            let token = self.bot_token.read().await.clone().unwrap_or_default();
            let buf = self.updates_buf.read().await.clone().unwrap_or_default();

            tokio::spawn(async move {
                longpoll_loop(client, token, buf, eb, cancel_rx).await;
            });
        }

        Ok(ConnectResult { ok: true, error: None, bot_info: self.bot_info.clone() })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        if let Some(cancel_tx) = &self.cancel_tx {
            let _ = cancel_tx.send(());
        }
        self.cancel_tx = None;
        self.state = AdapterState::Stopped;
        tracing::info!("个人微信适配器已断开");
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
            token_configured: self.bot_token.blocking_read().is_some(),
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
        let token = self.bot_token.read().await.clone().ok_or_else(|| {
            GatewayError::Internal("Not authenticated (no bot_token)".to_string())
        })?;
        let client = self.client()?;
        let url = format!("{}/ilink/bot/sendmessage", ILINK_API);

        let body = serde_json::json!({
            "msg": {
                "to_user_id": params.chat_id,
                "context_token": "",
                "item_list": [
                    {
                        "type": 1,
                        "text_item": {
                            "text": params.message.text,
                        }
                    }
                ]
            }
        });

        let resp: SendMessageResponse = client.post(&url)
            .headers(self.auth_headers(&token))
            .json(&body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send failed: {}", e)))?
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send parse failed: {}", e)))?;

        self.messages_out.fetch_add(1, Ordering::Relaxed);

        if resp.ret == 0 {
            Ok(SendResult {
                success: true,
                message_id: resp.msg_id.or(resp.local_id),
                timestamp: Some(chrono::Utc::now().timestamp_millis()),
                error: None, error_code: None, retryable: false,
            })
        } else {
            Ok(SendResult::fail(
                format!("WeChat send error: {} (ret {})", resp.errmsg.unwrap_or_default(), resp.ret),
                true,
            ))
        }
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        // 当前仅支持文本消息，媒体消息使用 AES-128-ECB 加密/解密
        // 为简化第一阶段实现，媒体发送暂返回不支持
        self.errors.fetch_add(1, Ordering::Relaxed);
        Ok(SendResult::fail(
            format!("WeChat media send not yet implemented (type={:?})", params.media.media_type),
            false,
        ))
    }

    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError> {
        Ok(ChatInfo {
            chat_id: chat_id.to_string(),
            name: None,
            chat_type: ChatType::Dm, // 个人微信仅支持 DM
            member_count: None,
        })
    }

    async fn list_chats(&self, _filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        Ok(Vec::new()) // iLink API 不提供会话列表
    }
}

impl Default for WeChatAdapter {
    fn default() -> Self { Self::new() }
}

// ── 长轮询后台任务 ──

async fn longpoll_loop(
    client: reqwest::Client,
    token: String,
    initial_buf: String,
    event_bus: Arc<EventBus>,
    mut cancel_rx: tokio::sync::broadcast::Receiver<()>,
) {
    let url = format!("{}/ilink/bot/getupdates", ILINK_API);
    let mut buf = initial_buf;

    loop {
        tokio::select! {
            _ = cancel_rx.recv() => {
                tracing::info!("个人微信长轮询已停止");
                break;
            }
            result = poll_messages(&client, &url, &token, &buf) => {
                match result {
                    Ok(Some((new_buf, msgs))) => {
                        buf = new_buf;
                        for msg in msgs {
                            if let Some(inbound) = convert_message(msg) {
                                let event = easybot_core::types::event::GatewayEvent::new(
                                    easybot_core::types::event::event_types::MESSAGE_INBOUND,
                                    "wechat",
                                    serde_json::to_value(&inbound).unwrap_or_default(),
                                );
                                event_bus.publish(event);
                            }
                        }
                    }
                    Ok(None) => {
                        // 超时无消息，继续轮询
                    }
                    Err(e) => {
                        tracing::warn!("个人微信长轮询错误: {}", e);
                        // 等待后重试
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        }
    }
}

async fn poll_messages(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    buf: &str,
) -> Result<Option<(String, Vec<WeixinMessage>)>, GatewayError> {
    let body = serde_json::json!({
        "get_updates_buf": buf,
    });

    let resp: GetUpdatesResponse = client.post(url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", token))
        .header("AuthorizationType", "ilink_bot_token")
        .header("X-Wechat-Uin", base64_encode_uin(uuid::Uuid::new_v4().as_u64_pair().0 as u32))
        .json(&body)
        .timeout(Duration::from_secs(LONGPOLL_TIMEOUT + 10))
        .send()
        .await
        .map_err(|e| GatewayError::Internal(format!("Longpoll request failed: {}", e)))?
        .json()
        .await
        .map_err(|e| GatewayError::Internal(format!("Longpoll parse failed: {}", e)))?;

    if resp.ret != 0 {
        // -14 = session expired
        if resp.ret == -14 {
            tracing::warn!("个人微信 session 过期，需要重新登录");
        }
        return Err(GatewayError::Internal(format!(
            "getupdates error: {} (ret {})", resp.errmsg.unwrap_or_default(), resp.ret
        )));
    }

    let new_buf = resp.get_updates_buf.unwrap_or_else(|| buf.to_string());
    if resp.msgs.is_empty() {
        Ok(None)
    } else {
        Ok(Some((new_buf, resp.msgs)))
    }
}

/// 将 iLink 消息转换为 InboundMessage
fn convert_message(msg: WeixinMessage) -> Option<InboundMessage> {
    let text = match msg.msg_type {
        1 => msg.content.map(|c| c.text).unwrap_or_default(),
        2 => "[图片]".to_string(),
        3 => {
            if let Some(ref v) = msg.voice {
                v.transcription.clone().unwrap_or_else(|| "[语音]".to_string())
            } else {
                "[语音]".to_string()
            }
        }
        4 => {
            if let Some(ref f) = msg.file {
                f.file_name.clone().unwrap_or_else(|| "[文件]".to_string())
            } else {
                "[文件]".to_string()
            }
        }
        5 => "[视频]".to_string(),
        _ => "[未知消息类型]".to_string(),
    };

    let media: Option<Vec<MediaAttachment>> = match msg.msg_type {
        2 => msg.image.map(|img| vec![MediaAttachment {
            media_type: MediaType::Image,
            url: img.file_url,
            data: None,
            mime_type: "image/jpeg".to_string(),
            filename: img.file_name,
            caption: None,
            thumbnail_url: None,
            file_size: img.file_size.map(|s| s as u64),
            duration: None,
        }]),
        4 => msg.file.map(|f| vec![MediaAttachment {
            media_type: MediaType::Document,
            url: f.file_url,
            data: None,
            mime_type: "application/octet-stream".to_string(),
            filename: f.file_name,
            caption: None,
            thumbnail_url: None,
            file_size: f.file_size.map(|s| s as u64),
            duration: None,
        }]),
        5 => msg.video.map(|v| vec![MediaAttachment {
            media_type: MediaType::Video,
            url: v.file_url,
            data: None,
            mime_type: "video/mp4".to_string(),
            filename: v.file_name,
            caption: None,
            thumbnail_url: None,
            file_size: v.file_size.map(|s| s as u64),
            duration: None,
        }]),
        _ => None,
    };

    let author_name = if msg.from_user.is_empty() {
        msg.from_user_id.clone()
    } else {
        msg.from_user.clone()
    };

    Some(InboundMessage {
        id: msg.msg_id,
        platform: "wechat".to_string(),
        chat_id: msg.from_user_id.clone(),
        chat_type: ChatType::Dm,
        chat_name: None,
        text: Some(text),
        author: MessageAuthor {
            id: msg.from_user_id,
            name: Some(author_name),
            is_bot: false,
        },
        timestamp: msg.create_time * 1000, // iLink 是秒级时间戳，转为毫秒
        media,
        command: None,
        callback: None,
        reply_to: None,
        thread_id: None,
        is_group: false,
        metadata: Some(serde_json::json!({
            "context_token": msg.context_token,
        })),
    })
}

// ── 单元测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_adapter() {
        let adapter = WeChatAdapter::new();
        assert_eq!(adapter.platform_name(), "wechat");
        assert_eq!(adapter.state(), AdapterState::Created);
        assert!(!adapter.capabilities.is_empty());
    }

    #[test]
    fn test_capabilities() {
        let adapter = WeChatAdapter::new();
        let caps = adapter.capabilities();
        assert!(caps.iter().any(|c| c.name == CapabilityName::Text));
        assert!(caps.iter().any(|c| c.name == CapabilityName::Image));
    }

    #[tokio::test]
    async fn test_init() {
        let mut adapter = WeChatAdapter::new();
        let result = adapter.init(AdapterConfig {
            enabled: true,
            token: None,
            api_key: None,
            extra: serde_json::json!({}),
        }).await.unwrap();
        assert!(result.ok);
    }

    #[tokio::test]
    async fn test_init_with_saved_credentials() {
        let mut adapter = WeChatAdapter::new();
        let result = adapter.init(AdapterConfig {
            enabled: true,
            token: None,
            api_key: None,
            extra: serde_json::json!({
                "bot_token": "saved_token",
                "ilink_bot_id": "saved_bot",
                "ilink_user_id": "saved_user",
            }),
        }).await.unwrap();
        assert!(result.ok);
        assert_eq!(adapter.bot_token.read().await.clone(), Some("saved_token".to_string()));
    }

    #[test]
    fn test_status_summary() {
        let adapter = WeChatAdapter::new();
        let status = adapter.status_summary();
        assert_eq!(status.platform, "wechat");
        assert!(!status.connected);
    }

    #[test]
    fn test_base64_encode_uin() {
        let encoded = base64_encode_uin(12345);
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_default() {
        let adapter = WeChatAdapter::default();
        assert_eq!(adapter.platform_name(), "wechat");
    }

    #[test]
    fn test_convert_text_message() {
        let msg = WeixinMessage {
            msg_id: "msg_123".to_string(),
            from_user: "好友A".to_string(),
            from_user_id: "user@im.wechat".to_string(),
            to_user_id: "bot@im.wechat".to_string(),
            context_token: "ctx_token_abc".to_string(),
            msg_type: 1,
            create_time: 1700000000,
            content: Some(WeixinTextContent { text: "你好".to_string() }),
            image: None,
            file: None,
            voice: None,
            video: None,
        };

        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.id, "msg_123");
        assert_eq!(inbound.text.as_deref(), Some("你好"));
        assert_eq!(inbound.chat_type, ChatType::Dm);
        assert!(!inbound.is_group);
        assert_eq!(inbound.author.name.as_deref(), Some("好友A"));
        assert_eq!(inbound.author.id, "user@im.wechat");
        let meta = inbound.metadata.unwrap();
        assert_eq!(meta.get("context_token").and_then(|v| v.as_str()), Some("ctx_token_abc"));
    }

    #[test]
    fn test_convert_image_message() {
        let msg = WeixinMessage {
            msg_id: "msg_img".to_string(),
            from_user: "".to_string(),
            from_user_id: "user@im.wechat".to_string(),
            to_user_id: "bot@im.wechat".to_string(),
            context_token: "ctx".to_string(),
            msg_type: 2,
            create_time: 1700000000,
            content: None,
            image: Some(WeixinMediaContent {
                md5sum: Some("abc".to_string()),
                file_size: Some(1024),
                file_name: Some("photo.jpg".to_string()),
                aes_key: None,
                file_url: Some("https://cdn.url/img".to_string()),
            }),
            file: None,
            voice: None,
            video: None,
        };

        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.text.as_deref(), Some("[图片]"));
        assert!(inbound.media.is_some());
        // MediaType 未实现 PartialEq，通过字符串对比
        let media_type = &inbound.media.as_ref().unwrap().first().unwrap().media_type;
        match media_type {
            MediaType::Image => {} // expected
            _ => panic!("expected Image media type, got {:?}", media_type),
        }
    }

    #[test]
    fn test_convert_voice_with_transcription() {
        let msg = WeixinMessage {
            msg_id: "msg_voice".to_string(),
            from_user: "".to_string(),
            from_user_id: "user@im.wechat".to_string(),
            to_user_id: "bot@im.wechat".to_string(),
            context_token: "ctx".to_string(),
            msg_type: 3,
            create_time: 1700000000,
            content: None,
            image: None,
            file: None,
            voice: Some(WeixinVoiceContent {
                md5sum: None,
                file_size: None,
                file_name: None,
                aes_key: None,
                file_url: None,
                voice_seconds: Some(3),
                transcription: Some("你好，这是语音".to_string()),
            }),
            video: None,
        };

        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.text.as_deref(), Some("你好，这是语音"));
    }

    #[tokio::test]
    async fn test_send_before_connect_errors() {
        let adapter = WeChatAdapter::new();
        let result = adapter.send(SendTextParams {
            chat_id: "user@im.wechat".to_string(),
            message: OutboundMessage { text: "hi".to_string(), parse_mode: ParseMode::None },
            reply_to: None,
            metadata: None,
        }).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Not authenticated"));
    }

    #[tokio::test]
    async fn test_disconnect_idempotent() {
        let mut adapter = WeChatAdapter::new();
        assert!(adapter.disconnect().await.is_ok());
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }
}
