//! 个人微信 (WeChat) 平台适配器
//!
//! 使用腾讯官方 iLink Bot API 实现个人微信消息收发。
//! 协议文档：https://ilinkai.weixin.qq.com
//!
//! 凭据文件：`~/.easybot/.wechat-credentials.json`（扫码登录后自动保存，避免重复扫码）
//!
//! # 配置
//! ```yaml
//! wechat:
//!   enabled: true
//!   # 可选：预填凭据到配置中（免二次扫码）
//!   extra:
//!     bot_token: "<saved_bot_token>"
//!     ilink_bot_id: "<saved_bot_id>"
//!     ilink_user_id: "<saved_user_id>"
//!     baseurl: "https://ilinkai.weixin.qq.com"
//! ```
//!
//! # 登录流程
//! 首次启动时终端打印 QR 码，微信扫码确认后自动保存凭据到 `~/.easybot/.wechat-credentials.json`。
//! 后续启动会自动读取凭据，无需重复扫码。
//!
//! # 已知限制
//! - 仅支持 DM（一对一聊天），不支持群聊
//! - 不支持 Markdown、贴纸、小程序消息

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::message::*;

/// iLink Bot API 基础 URL
const ILINK_API: &str = "https://ilinkai.weixin.qq.com";

/// 凭据文件路径（相对于 home 目录的 .easybot/）
const CREDENTIALS_FILE: &str = ".easybot/.wechat-credentials.json";

/// 微信凭据（持久化到磁盘）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WeChatCredentials {
    bot_token: String,
    ilink_bot_id: String,
    ilink_user_id: String,
    baseurl: String,
}

/// 获取凭据文件路径
fn credential_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(CREDENTIALS_FILE))
}

/// 从磁盘加载凭据
fn load_credentials_from_disk() -> Option<WeChatCredentials> {
    let path = credential_path()?;
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).ok(),
        Err(_) => None,
    }
}

/// 保存凭据到磁盘
fn save_credentials_to_disk(creds: &WeChatCredentials) {
    let path = match credential_path() {
        Some(p) => p,
        None => {
            tracing::warn!("无法确定凭据文件路径");
            return;
        }
    };
    // 确保目录存在
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(creds) {
        Ok(json) => {
            match std::fs::write(&path, &json) {
                Ok(_) => {
                    // 设置仅用户可读写（类 Unix）
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ =
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                    }
                    tracing::info!("个人微信凭据已保存到 {:?}", path);
                }
                Err(e) => tracing::warn!("保存凭据失败: {}", e),
            }
        }
        Err(e) => tracing::warn!("序列化凭据失败: {}", e),
    }
}

/// 长轮询超时（秒）
const LONGPOLL_TIMEOUT: u64 = 35;

/// Session 刷新间隔（秒），24 小时后过期需重连
#[allow(dead_code)]
const SESSION_REFRESH_INTERVAL: u64 = 82800; // 23 小时

/// iLink 媒体类型常量
const MEDIA_TYPE_IMAGE: i32 = 1;
const MEDIA_TYPE_VIDEO: i32 = 2;
const MEDIA_TYPE_FILE: i32 = 3;
const MEDIA_TYPE_VOICE: i32 = 4;

/// iLink 消息项类型常量
const ITEM_TYPE_IMAGE: i32 = 2;
const ITEM_TYPE_VOICE: i32 = 3;
const ITEM_TYPE_FILE: i32 = 4;
const ITEM_TYPE_VIDEO: i32 = 5;

/// CDN 上传基础 URL
const CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";

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
#[allow(dead_code)]
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

/// 长轮询消息响应（实际 API 无 ret 字段，直接返回数据）
#[derive(Debug, serde::Deserialize)]
struct GetUpdatesResponse {
    #[serde(default)]
    msgs: Vec<WeixinMessage>,
    #[serde(default)]
    get_updates_buf: Option<String>,
    #[serde(default)]
    sync_buf: Option<String>,
}

/// 微信消息（iLink Bot API 实际格式）
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct WeixinMessage {
    #[serde(default)]
    message_id: Option<i64>,
    #[serde(default)]
    from_user_id: String,
    #[serde(default)]
    to_user_id: String,
    #[serde(default)]
    message_type: i64,
    #[serde(default)]
    create_time_ms: i64,
    #[serde(default)]
    item_list: Vec<WeixinMessageItem>,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    group_id: String,
    /// 消息上下文令牌（回复时必须回传）
    #[serde(default)]
    context_token: Option<String>,
}

/// 消息内容项
#[derive(Debug, serde::Deserialize)]
struct WeixinMessageItem {
    #[serde(rename = "type")]
    item_type: i64,
    #[serde(default)]
    text_item: Option<WeixinTextItem>,
    #[serde(default)]
    image_item: Option<WeixinImageItem>,
    #[serde(default)]
    file_item: Option<WeixinFileItem>,
}

#[derive(Debug, serde::Deserialize)]
struct WeixinTextItem {
    text: String,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct WeixinImageItem {
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
    height: Option<i64>,
    #[serde(default)]
    width: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct WeixinFileItem {
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

/// 发送消息响应
///
/// iLink send API 可能返回空的 {}，或包含 ret/errmsg（错误时），
/// 或包含 message_id/seq（成功时）。
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct SendMessageResponse {
    #[serde(default)]
    ret: Option<i64>,
    #[serde(default)]
    errmsg: Option<String>,
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    msg_id: Option<i64>,
    #[serde(default)]
    local_id: Option<String>,
    #[serde(default)]
    msg_id_str: Option<String>,
    #[serde(default)]
    seq: Option<i64>,
}

/// Upload URL 响应
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct UploadUrlResponse {
    #[serde(default)]
    ret: i64,
    #[serde(default)]
    errmsg: Option<String>,
    #[serde(default)]
    upload_param: Option<String>,
    #[serde(default)]
    upload_full_url: Option<String>,
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
    http_client: std::sync::OnceLock<reqwest::Client>,
    /// iLink Bot Token（登录后获取）
    bot_token: tokio::sync::RwLock<Option<String>>,
    /// 长轮询游标
    updates_buf: tokio::sync::RwLock<Option<String>>,
    /// 取消信号
    cancel_tx: Option<tokio::sync::broadcast::Sender<()>>,
    /// Background liveness heartbeat (updated by the longpoll task)
    heartbeat: Heartbeat,
    /// iLink Bot ID
    ilink_bot_id: tokio::sync::RwLock<Option<String>>,
    /// iLink User ID
    ilink_user_id: tokio::sync::RwLock<Option<String>>,
    /// 最近收到的 context_token（回复时需回传，与长轮询任务共享）
    context_token: Arc<tokio::sync::RwLock<Option<String>>>,
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
            ],
            messages_in: AtomicU64::new(0),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            http_client: std::sync::OnceLock::new(),
            bot_token: tokio::sync::RwLock::new(None),
            updates_buf: tokio::sync::RwLock::new(None),
            cancel_tx: None,
            heartbeat: Heartbeat::new(),
            ilink_bot_id: tokio::sync::RwLock::new(None),
            ilink_user_id: tokio::sync::RwLock::new(None),
            context_token: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }

    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    fn client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60)) // 长轮询需要较长超时
                .build()
                .expect("Failed to create HTTP client")
        })
    }

    /// 返回 API 基础 URL（支持通过 config.base_url 覆盖）
    fn api_base_url(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.base_url.as_deref())
            .unwrap_or(ILINK_API)
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

    /// 上传媒体文件到 iLink CDN 并返回加密参数
    ///
    /// 完整上传流程：
    /// 1. 获取文件数据（URL 下载或 base64 解码）
    /// 2. 生成随机 AES key + filekey
    /// 3. 调用 getuploadurl 获取 CDN 上传地址
    /// 4. AES-128-ECB 加密文件内容
    /// 5. POST 到 CDN，提取 x-encrypted-param 下载密钥
    /// 6. 返回构建 media item 所需的字段
    async fn upload_media_to_cdn(
        &self,
        media: &MediaAttachment,
        chat_id: &str,
        media_type: i32,
    ) -> Result<serde_json::Value, GatewayError> {
        let token = self.bot_token.read().await.clone().ok_or_else(|| {
            GatewayError::Internal("Not authenticated (no bot_token)".to_string())
        })?;
        let client = self.client();

        // 1. 获取文件数据
        let file_data = resolve_media_data(media, client).await?;

        // 2. 生成 AES key 和 filekey
        let aes_key = uuid::Uuid::new_v4();
        let aes_key_bytes: [u8; 16] = *aes_key.as_bytes();
        let filekey = generate_filekey();

        // 3. 计算元数据
        let rawsize = file_data.len() as u64;
        let rawfilemd5 = md5_hex(&file_data);
        let filesize = aes_padded_size(file_data.len()) as u64;

        // 4. 获取上传 URL
        let upload_url = format!("{}/ilink/bot/getuploadurl", self.api_base_url());

        let aeskey_hex: String = aes_key_bytes.iter().map(|b| format!("{:02x}", b)).collect();

        let upload_req_body = serde_json::json!({
            "base_info": {
                "channel_version": "2.2.0"
            },
            "filekey": filekey,
            "media_type": media_type,
            "to_user_id": chat_id,
            "rawsize": rawsize,
            "rawfilemd5": rawfilemd5,
            "filesize": filesize,
            "no_need_thumb": true,
            "aeskey": aeskey_hex,
        });

        let raw_resp = client
            .post(&upload_url)
            .headers(self.auth_headers(&token))
            .json(&upload_req_body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("getuploadurl request failed: {}", e)))?;

        let resp_text = raw_resp
            .text()
            .await
            .map_err(|e| GatewayError::Internal(format!("getuploadurl read failed: {}", e)))?;

        tracing::debug!(
            "WeChat getuploadurl response: {}",
            &resp_text[..resp_text.len().min(500)]
        );

        let upload_resp: UploadUrlResponse = serde_json::from_str(&resp_text).map_err(|e| {
            GatewayError::Internal(format!(
                "getuploadurl parse failed: {} (body: {})",
                e,
                &resp_text[..resp_text.len().min(200)]
            ))
        })?;

        if upload_resp.ret != 0 {
            return Err(GatewayError::Internal(format!(
                "getuploadurl API error (ret={}): {}",
                upload_resp.ret,
                upload_resp.errmsg.unwrap_or_default()
            )));
        }

        // 5. 确定 CDN 上传 URL
        let cdn_url = if let Some(ref full_url) = upload_resp.upload_full_url {
            full_url.clone()
        } else if let Some(ref param) = upload_resp.upload_param {
            build_cdn_upload_url(CDN_BASE_URL, param, &filekey)
        } else {
            return Err(GatewayError::Internal(
                "getuploadurl response missing both upload_full_url and upload_param".to_string(),
            ));
        };

        tracing::debug!(
            "WeChat CDN upload URL: {}...",
            &cdn_url[..cdn_url.len().min(150)]
        );

        // 6. AES-128-ECB 加密文件
        let ciphertext = aes_128_ecb_encrypt(&file_data, &aes_key_bytes);

        tracing::debug!(
            "WeChat media encrypted: raw={} bytes, padded={} bytes, aes_key_hex={}",
            file_data.len(),
            ciphertext.len(),
            aeskey_hex,
        );

        // 7. 上传到 CDN（使用专用 HTTP/1.1 客户端，避免 HTTP/2 兼容性问题）
        let cdn_client = reqwest::Client::builder()
            .http1_only()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| GatewayError::Internal(format!("Failed to create CDN client: {}", e)))?;

        tracing::debug!(
            "WeChat CDN upload: url_len={}, body_len={}, first_16_key={}",
            cdn_url.len(),
            ciphertext.len(),
            &aeskey_hex[..16]
        );

        let cdn_resp = cdn_client
            .post(&cdn_url)
            .header("Content-Type", "application/octet-stream")
            .body(ciphertext.clone())
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("CDN upload failed: {}", e)))?;

        let cdn_status = cdn_resp.status();

        // Log all response headers for debugging
        let resp_headers: Vec<String> = cdn_resp
            .headers()
            .iter()
            .map(|(k, v)| format!("{}: {:?}", k, v))
            .collect();
        tracing::debug!(
            "WeChat CDN response: status={}, headers=[{}]",
            cdn_status.as_u16(),
            resp_headers.join(", ")
        );

        if !cdn_status.is_success() {
            let cdn_body = cdn_resp.text().await.unwrap_or_default();
            tracing::warn!(
                "WeChat CDN upload failed: status={}, body_len={}",
                cdn_status.as_u16(),
                cdn_body.len()
            );
            return Err(GatewayError::Internal(format!(
                "CDN upload HTTP {}: {}",
                cdn_status.as_u16(),
                &cdn_body[..cdn_body.len().min(200)]
            )));
        }

        let encrypt_query_param = cdn_resp
            .headers()
            .get("x-encrypted-param")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                GatewayError::Internal(
                    "CDN upload response missing x-encrypted-param header".to_string(),
                )
            })?;

        // 8. 编码 aes_key
        let aes_key_for_api = encode_aes_key_for_api(&aes_key_bytes);

        tracing::info!(
            "WeChat media upload success: type={}, rawsize={}, filekey={}",
            media_type,
            rawsize,
            filekey
        );

        // 9. 构建 media 子对象（各消息类型共用）
        Ok(serde_json::json!({
            "media": {
                "encrypt_query_param": encrypt_query_param,
                "aes_key": aes_key_for_api,
                "encrypt_type": 1,
            },
            "mid_size": ciphertext.len(),
            "rawfilemd5": rawfilemd5,
            "rawsize": rawsize,
        }))
    }
}

/// Base64 编码 uint32（与官方 SDK 对齐）
fn base64_encode_uin(uin: u32) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(uin.to_le_bytes())
}

// ── AES-128-ECB 媒体加密工具函数 ──

/// PKCS7 填充
fn pkcs7_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let pad_len = block_size - (data.len() % block_size);
    let mut padded = Vec::with_capacity(data.len() + pad_len);
    padded.extend_from_slice(data);
    padded.resize(data.len() + pad_len, pad_len as u8);
    padded
}

/// AES-128-ECB 加密
fn aes_128_ecb_encrypt(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};

    let cipher = aes::Aes128::new_from_slice(key).expect("AES-128 key must be 16 bytes");
    let padded = pkcs7_pad(plaintext, 16);
    let mut result = Vec::with_capacity(padded.len());

    for chunk in padded.chunks(16) {
        let mut block = GenericArray::clone_from_slice(chunk);
        cipher.encrypt_block(&mut block);
        result.extend_from_slice(&block);
    }

    result
}

/// 计算 AES 加密后的文件大小（含 PKCS7 填充）
fn aes_padded_size(raw_size: usize) -> usize {
    (raw_size + 1).div_ceil(16) * 16
}

/// 编码 AES key 为 iLink API 期望的格式
///
/// **关键**: iLink API 期望 `base64(key.hex().encode())`，而非 `base64(key)`。
/// 微信客户端的解码链为：base64_decode → 32 ASCII hex chars → bytes.fromhex() → 16-byte AES key。
fn encode_aes_key_for_api(key: &[u8; 16]) -> String {
    use base64::Engine;
    let hex_str: String = key.iter().map(|b| format!("{:02x}", b)).collect();
    base64::engine::general_purpose::STANDARD.encode(hex_str.as_bytes())
}

/// 生成新的 32 位十六进制 filekey（用于上传）
fn generate_filekey() -> String {
    let uuid = uuid::Uuid::new_v4();
    let hex_str: String = uuid
        .as_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    hex_str[..32].to_string()
}

/// 构建 CDN 上传 URL
fn build_cdn_upload_url(cdn_base: &str, upload_param: &str, filekey: &str) -> String {
    let encoded_param = url_encode_for_cdn(upload_param);
    format!(
        "{}/upload?encrypted_query_param={}&filekey={}",
        cdn_base.trim_end_matches('/'),
        encoded_param,
        filekey
    )
}

/// CDN URL 百分号编码（仅编码 base64 特殊字符）
fn url_encode_for_cdn(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

/// 计算数据的 MD5 并返回十六进制字符串
fn md5_hex(data: &[u8]) -> String {
    format!("{:x}", md5::compute(data))
}

/// 从 URL 下载文件内容
async fn download_media(url: &str, client: &reqwest::Client) -> Result<Vec<u8>, GatewayError> {
    let resp =
        client.get(url).send().await.map_err(|e| {
            GatewayError::Internal(format!("Failed to download media from URL: {}", e))
        })?;

    if !resp.status().is_success() {
        return Err(GatewayError::Internal(format!(
            "Failed to download media: HTTP {}",
            resp.status().as_u16()
        )));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| GatewayError::Internal(format!("Failed to read media bytes: {}", e)))
}

/// 从 MediaAttachment 获取文件数据（优先 URL，其次 base64 data）
async fn resolve_media_data(
    media: &MediaAttachment,
    client: &reqwest::Client,
) -> Result<Vec<u8>, GatewayError> {
    if let Some(ref url) = media.url {
        download_media(url, client).await
    } else if let Some(ref b64_data) = media.data {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64_data)
            .map_err(|e| {
                GatewayError::Internal(format!("Failed to decode base64 media data: {}", e))
            })
    } else {
        Err(GatewayError::Internal(
            "Media attachment has neither url nor data".to_string(),
        ))
    }
}

fn publish_send_event(
    event_bus: &Option<Arc<EventBus>>,
    event_type: &str,
    chat_id: &str,
    result: &SendResult,
) {
    if let Some(bus) = event_bus {
        bus.publish(easybot_core::types::event::GatewayEvent::new(
            event_type,
            "wechat",
            serde_json::json!({
                "platform": "wechat",
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
impl PlatformAdapter for WeChatAdapter {
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
        self.config = Some(config);

        // 尝试从配置中恢复凭据
        let extra = self
            .config
            .as_ref()
            .ok_or_else(|| GatewayError::Internal("config not set after init".into()))?
            .extra
            .clone();
        if let Some(token) = extra.get("bot_token").and_then(|v| v.as_str()) {
            *self.bot_token.write().await = Some(token.to_string());
        }
        if let Some(bot_id) = extra.get("ilink_bot_id").and_then(|v| v.as_str()) {
            *self.ilink_bot_id.write().await = Some(bot_id.to_string());
        }
        if let Some(user_id) = extra.get("ilink_user_id").and_then(|v| v.as_str()) {
            *self.ilink_user_id.write().await = Some(user_id.to_string());
        }

        // 如果配置中没有但磁盘上有保存的凭据，自动加载
        if self.bot_token.read().await.is_none()
            && let Some(creds) = load_credentials_from_disk()
        {
            tracing::info!("个人微信适配器：从磁盘加载保存的凭据");
            *self.bot_token.write().await = Some(creds.bot_token);
            *self.ilink_bot_id.write().await = Some(creds.ilink_bot_id);
            *self.ilink_user_id.write().await = Some(creds.ilink_user_id);
        }

        self.state = AdapterState::Starting;
        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        let client = self.client();

        // 如果没有 bot_token，执行 QR 码登录
        if self.bot_token.read().await.is_none() {
            tracing::info!("个人微信适配器：需要扫码登录");

            // 获取 QR 码
            let qr_url = format!(
                "{}/ilink/bot/get_bot_qrcode?bot_type=3",
                self.api_base_url()
            );
            let qr_resp: QrCodeResponse = client
                .get(&qr_url)
                .send()
                .await
                .map_err(|e| GatewayError::Internal(format!("Failed to get QR code: {}", e)))?
                .json()
                .await
                .map_err(|e| {
                    GatewayError::Internal(format!("Failed to parse QR response: {}", e))
                })?;

            if qr_resp.ret != 0 {
                return Err(GatewayError::Internal(format!(
                    "Get QR code failed: {} (ret {})",
                    qr_resp.errmsg.unwrap_or_default(),
                    qr_resp.ret
                )));
            }

            let qrcode = qr_resp
                .qrcode
                .ok_or_else(|| GatewayError::Internal("No qrcode in response".to_string()))?;

            // 显示 QR 码（可能是 URL 链接或 ASCII 二维码）
            if let Some(img) = &qr_resp.qrcode_img {
                if img.starts_with("http://") || img.starts_with("https://") {
                    // iLink API 返回的是微信 liteapp 链接（浏览器打开后扫码）
                    tracing::info!("微信登录链接请在浏览器打开后扫码：");
                    println!("\n    {}\n", img);
                    tracing::info!("微信登录链接: {}", img);
                } else {
                    // 旧格式：ASCII 二维码
                    tracing::info!("扫描以下二维码登录个人微信：");
                    println!("\n{}", img);
                }
                // 将 token 写入日志（stderr），供脚本/Docker/headless 场景提取
                tracing::info!(
                    "微信登录 qrcode_token={}，扫码后凭据将自动保存到 ~/.easybot/.wechat-credentials.json",
                    qrcode
                );
            }

            // 轮询扫码状态（最多 120 秒）
            let status_url = format!(
                "{}/ilink/bot/get_qrcode_status?qrcode={}",
                self.api_base_url(),
                qrcode
            );
            let mut logged = false;
            let mut token: Option<String> = None;
            let mut bot_id: Option<String> = None;
            let mut user_id: Option<String> = None;
            let mut baseurl: Option<String> = None;

            for _ in 0..120 {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let status_resp: QrCodeStatusResponse = client
                    .get(&status_url)
                    .send()
                    .await
                    .map_err(|e| GatewayError::Internal(format!("QR status poll failed: {}", e)))?
                    .json()
                    .await
                    .map_err(|e| {
                        GatewayError::Internal(format!("QR status parse failed: {}", e))
                    })?;

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

            let bot_token = token
                .ok_or_else(|| GatewayError::Internal("QR login timeout or failed".to_string()))?;

            // 保存凭据（内存）
            *self.bot_token.write().await = Some(bot_token.clone());
            if let Some(id) = bot_id {
                *self.ilink_bot_id.write().await = Some(id.clone());
            }
            if let Some(uid) = user_id {
                *self.ilink_user_id.write().await = Some(uid.clone());
            }
            if let Some(ref url) = baseurl {
                tracing::info!("个人微信登录成功，baseurl: {}", url);
            }

            // 持久化凭据到磁盘（下次自动加载）
            let saved_baseurl = baseurl.unwrap_or_else(|| ILINK_API.to_string());
            let creds = WeChatCredentials {
                bot_token: self.bot_token.read().await.clone().unwrap_or_default(),
                ilink_bot_id: self.ilink_bot_id.read().await.clone().unwrap_or_default(),
                ilink_user_id: self.ilink_user_id.read().await.clone().unwrap_or_default(),
                baseurl: saved_baseurl,
            };
            save_credentials_to_disk(&creds);

            // 注意：凭据可以持久化到配置文件中，方便下次自动登录
            tracing::info!("个人微信适配器：扫码登录成功");
        }

        // 设置 bot_info
        let bot_id = self
            .ilink_bot_id
            .read()
            .await
            .clone()
            .unwrap_or_else(|| "wechat_bot".to_string());
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
            let client = self.client().clone();
            let token = self.bot_token.read().await.clone().unwrap_or_default();
            let buf = self.updates_buf.read().await.clone().unwrap_or_default();
            let base_url = self
                .config
                .as_ref()
                .and_then(|c| c.base_url.clone())
                .unwrap_or_else(|| ILINK_API.to_string());

            let ctx_token = self.context_token.clone();
            let hb = self.heartbeat.clone();
            tokio::spawn(async move {
                longpoll_loop(client, token, buf, base_url, eb, cancel_rx, ctx_token, hb).await;
            });
        }

        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: self.bot_info.clone(),
        })
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
                .bot_token
                .try_read()
                .map(|g| g.is_some())
                .unwrap_or(false),
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
        let token = self.bot_token.read().await.clone().ok_or_else(|| {
            GatewayError::Internal("Not authenticated (no bot_token)".to_string())
        })?;
        let client = self.client();
        let url = format!("{}/ilink/bot/sendmessage", self.api_base_url());

        // iLink send API 完整格式：msg 包装 + context_token + 元数据
        let ctx_token = self.context_token.read().await.clone();
        let client_id = format!(
            "easybot:{}:{}",
            chrono::Utc::now().timestamp_millis(),
            uuid::Uuid::new_v4().as_simple()
        );

        let mut body = serde_json::json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": params.chat_id,
                "client_id": client_id,
                "message_type": 2,
                "message_state": 2,
                "item_list": [
                    {
                        "type": 1,
                        "text_item": {
                            "text": params.message.text,
                        }
                    }
                ]
            },
            "base_info": {
                "channel_version": "1.0.0"
            }
        });

        if let Some(ref ct) = ctx_token {
            body["msg"]["context_token"] = serde_json::Value::String(ct.clone());
        }

        tracing::debug!(
            "WeChat send request: to_user_id={}, has_ctx={}, text={}",
            params.chat_id,
            ctx_token.is_some(),
            &params.message.text[..params.message.text.len().min(100)]
        );

        let raw_resp = client
            .post(&url)
            .headers(self.auth_headers(&token))
            .json(&body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send failed: {}", e)))?;

        let status = raw_resp.status();
        let resp_text = raw_resp
            .text()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send read failed: {}", e)))?;

        tracing::debug!(
            "WeChat send response (status={}): {}",
            status,
            &resp_text[..resp_text.len().min(300)]
        );

        if !status.is_success() {
            self.errors.fetch_add(1, Ordering::Relaxed);
            let fail = SendResult::fail(
                format!(
                    "WeChat send HTTP {}: {}",
                    status.as_u16(),
                    &resp_text[..resp_text.len().min(200)]
                ),
                false,
            );
            publish_send_event(
                &self.event_bus,
                easybot_core::types::event::event_types::MESSAGE_FAILED,
                &params.chat_id,
                &fail,
            );
            return Ok(fail);
        }

        let resp: SendMessageResponse = serde_json::from_str(&resp_text).map_err(|e| {
            GatewayError::Internal(format!(
                "WeChat send parse failed: {} (body: {})",
                e,
                &resp_text[..resp_text.len().min(200)]
            ))
        })?;

        // ret 存在且非 0 时报告错误
        if let Some(ret) = resp.ret
            && ret != 0
        {
            self.errors.fetch_add(1, Ordering::Relaxed);
            let err_detail = resp.errmsg.as_deref().unwrap_or("unknown error");
            tracing::warn!("WeChat send API error: ret={}, errmsg={}", ret, err_detail);
            let fail = SendResult::fail(
                format!("WeChat API error (ret={}): {}", ret, err_detail),
                false,
            );
            publish_send_event(
                &self.event_bus,
                easybot_core::types::event::event_types::MESSAGE_FAILED,
                &params.chat_id,
                &fail,
            );
            return Ok(fail);
        }

        self.messages_out.fetch_add(1, Ordering::Relaxed);

        let msg_id = resp
            .message_id
            .or(resp.msg_id_str)
            .or(resp.msg_id.map(|id| id.to_string()))
            .or(resp.local_id)
            .or(resp.seq.map(|s| s.to_string()));

        let send_result = SendResult {
            success: true,
            message_id: msg_id,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            error: None,
            error_code: None,
            retryable: false,
        };
        publish_send_event(
            &self.event_bus,
            easybot_core::types::event::event_types::MESSAGE_SENT,
            &params.chat_id,
            &send_result,
        );
        Ok(send_result)
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        let token = self.bot_token.read().await.clone().ok_or_else(|| {
            GatewayError::Internal("Not authenticated (no bot_token)".to_string())
        })?;
        let client = self.client();

        // 将 MediaType 映射到 iLink media_type
        let (media_type, item_type) = match params.media.media_type {
            MediaType::Image => (MEDIA_TYPE_IMAGE, ITEM_TYPE_IMAGE),
            MediaType::Video => (MEDIA_TYPE_VIDEO, ITEM_TYPE_VIDEO),
            MediaType::Audio => (MEDIA_TYPE_VOICE, ITEM_TYPE_VOICE),
            MediaType::Document | MediaType::Sticker | MediaType::Animation => {
                (MEDIA_TYPE_FILE, ITEM_TYPE_FILE)
            }
        };

        // 上传到 CDN
        let upload_result = self
            .upload_media_to_cdn(&params.media, &params.chat_id, media_type)
            .await?;

        // 构建消息体
        let url = format!("{}/ilink/bot/sendmessage", self.api_base_url());
        let ctx_token = self.context_token.read().await.clone();
        let client_id = format!(
            "easybot:{}:{}",
            chrono::Utc::now().timestamp_millis(),
            uuid::Uuid::new_v4().as_simple()
        );

        // 构建对应类型的 item
        let item = match item_type {
            ITEM_TYPE_IMAGE => {
                let mut img_item = serde_json::json!({
                    "type": ITEM_TYPE_IMAGE,
                    "image_item": {
                        "media": upload_result["media"].clone(),
                        "mid_size": upload_result["mid_size"],
                    }
                });
                // 添加可选的文件名和尺寸
                if let Some(ref name) = params.media.filename {
                    img_item["image_item"]["file_name"] = serde_json::Value::String(name.clone());
                }
                if let Some(size) = params.media.file_size {
                    img_item["image_item"]["file_size"] = serde_json::json!(size);
                }
                img_item
            }
            ITEM_TYPE_VIDEO => {
                let mut vid_item = serde_json::json!({
                    "type": ITEM_TYPE_VIDEO,
                    "video_item": {
                        "media": upload_result["media"].clone(),
                        "video_size": upload_result["mid_size"],
                        "video_md5": upload_result["rawfilemd5"],
                        "play_length": params.media.duration.unwrap_or(0.0) as i64,
                    }
                });
                if let Some(ref name) = params.media.filename {
                    vid_item["video_item"]["file_name"] = serde_json::Value::String(name.clone());
                }
                vid_item
            }
            ITEM_TYPE_VOICE => {
                let voice_item = serde_json::json!({
                    "type": ITEM_TYPE_VOICE,
                    "voice_item": {
                        "media": upload_result["media"].clone(),
                        "encode_type": 6,
                        "bits_per_sample": 16,
                        "sample_rate": 24000,
                        "playtime": params.media.duration.unwrap_or(0.0) as i64,
                    }
                });
                voice_item
            }
            _ => {
                let file_item = serde_json::json!({
                    "type": ITEM_TYPE_FILE,
                    "file_item": {
                        "media": upload_result["media"].clone(),
                        "file_name": params.media.filename.as_deref().unwrap_or("file"),
                        "len": upload_result["rawsize"],
                    }
                });
                file_item
            }
        };

        let mut body = serde_json::json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": params.chat_id,
                "client_id": client_id,
                "message_type": 2,
                "message_state": 2,
                "item_list": [item],
            },
            "base_info": {
                "channel_version": "1.0.0"
            }
        });

        if let Some(ref ct) = ctx_token {
            body["msg"]["context_token"] = serde_json::Value::String(ct.clone());
        }

        tracing::debug!(
            "WeChat send_media: to_user_id={}, media_type={:?}, filename={:?}",
            params.chat_id,
            params.media.media_type,
            params.media.filename,
        );

        let raw_resp = client
            .post(&url)
            .headers(self.auth_headers(&token))
            .json(&body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send_media failed: {}", e)))?;

        let status = raw_resp.status();
        let resp_text = raw_resp
            .text()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send_media read failed: {}", e)))?;

        tracing::debug!(
            "WeChat send_media response (status={}): {}",
            status,
            &resp_text[..resp_text.len().min(300)]
        );

        if !status.is_success() {
            self.errors.fetch_add(1, Ordering::Relaxed);
            let fail = SendResult::fail(
                format!(
                    "WeChat send_media HTTP {}: {}",
                    status.as_u16(),
                    &resp_text[..resp_text.len().min(200)]
                ),
                false,
            );
            publish_send_event(
                &self.event_bus,
                easybot_core::types::event::event_types::MESSAGE_FAILED,
                &params.chat_id,
                &fail,
            );
            return Ok(fail);
        }

        let resp: SendMessageResponse = serde_json::from_str(&resp_text).map_err(|e| {
            GatewayError::Internal(format!(
                "WeChat send_media parse failed: {} (body: {})",
                e,
                &resp_text[..resp_text.len().min(200)]
            ))
        })?;

        if let Some(ret) = resp.ret
            && ret != 0
        {
            self.errors.fetch_add(1, Ordering::Relaxed);
            let err_detail = resp.errmsg.as_deref().unwrap_or("unknown error");
            tracing::warn!(
                "WeChat send_media API error: ret={}, errmsg={}",
                ret,
                err_detail
            );
            let fail = SendResult::fail(
                format!("WeChat API error (ret={}): {}", ret, err_detail),
                false,
            );
            publish_send_event(
                &self.event_bus,
                easybot_core::types::event::event_types::MESSAGE_FAILED,
                &params.chat_id,
                &fail,
            );
            return Ok(fail);
        }

        self.messages_out.fetch_add(1, Ordering::Relaxed);

        let msg_id = resp
            .message_id
            .or(resp.msg_id_str)
            .or(resp.msg_id.map(|id| id.to_string()))
            .or(resp.local_id)
            .or(resp.seq.map(|s| s.to_string()));

        let send_result = SendResult {
            success: true,
            message_id: msg_id,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            error: None,
            error_code: None,
            retryable: false,
        };
        publish_send_event(
            &self.event_bus,
            easybot_core::types::event::event_types::MESSAGE_SENT,
            &params.chat_id,
            &send_result,
        );
        Ok(send_result)
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
    fn default() -> Self {
        Self::new()
    }
}

// ── 长轮询后台任务 ──

/// 清除保存的凭据（检测到 session 过期时调用）
fn clear_credentials() {
    if let Some(path) = credential_path()
        && path.exists()
    {
        let _ = std::fs::remove_file(&path);
        tracing::warn!("个人微信凭据已清除（可能已过期），文件: {:?}", path);
    }
}

#[allow(clippy::too_many_arguments)]
async fn longpoll_loop(
    client: reqwest::Client,
    token: String,
    initial_buf: String,
    base_url: String,
    event_bus: Arc<EventBus>,
    mut cancel_rx: tokio::sync::broadcast::Receiver<()>,
    context_token: Arc<tokio::sync::RwLock<Option<String>>>,
    heartbeat: Heartbeat,
) {
    let url = format!("{}/ilink/bot/getupdates", base_url);
    let mut buf = initial_buf;
    let mut consecutive_failures: u32 = 0;

    loop {
        tokio::select! {
            _ = cancel_rx.recv() => {
                tracing::info!("个人微信长轮询已停止");
                break;
            }
            result = poll_messages(&client, &url, &token, &buf) => {
                match result {
                    Ok(Some((new_buf, msgs))) => {
                        heartbeat.beat(); // liveness: successful poll
                        buf = new_buf;
                        consecutive_failures = 0;
                        for msg in msgs {
                            if let Some(ref ct) = msg.context_token {
                                let mut ctx = context_token.write().await;
                                *ctx = Some(ct.clone());
                            }
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
                        heartbeat.beat(); // liveness: poll succeeded, just no messages
                        consecutive_failures = 0;
                    }
                    Err(e) => {
                        consecutive_failures += 1;
                        tracing::warn!("个人微信长轮询错误 (第{}次): {}", consecutive_failures, e);

                        // 连续 10 次失败，清除凭据并退出（session 可能已过期）
                        if consecutive_failures >= 10 {
                            tracing::error!("个人微信长轮询连续失败 {} 次，session 可能已过期，清除凭据", consecutive_failures);
                            clear_credentials();
                            break;
                        }

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

    let raw_resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", token))
        .header("AuthorizationType", "ilink_bot_token")
        .header(
            "X-Wechat-Uin",
            base64_encode_uin(uuid::Uuid::new_v4().as_u64_pair().0 as u32),
        )
        .json(&body)
        .timeout(Duration::from_secs(LONGPOLL_TIMEOUT + 10))
        .send()
        .await
        .map_err(|e| GatewayError::Internal(format!("Longpoll request failed: {}", e)))?;

    let resp_text = raw_resp
        .text()
        .await
        .map_err(|e| GatewayError::Internal(format!("Longpoll read body failed: {}", e)))?;

    tracing::debug!(
        "QQ getupdates raw response: {}",
        &resp_text[..resp_text.len().min(500)]
    );

    let resp: GetUpdatesResponse = serde_json::from_str(&resp_text).map_err(|e| {
        GatewayError::Internal(format!(
            "Longpoll parse failed: {} (body: {})",
            e,
            &resp_text[..resp_text.len().min(200)]
        ))
    })?;

    let new_buf = resp
        .get_updates_buf
        .or(resp.sync_buf)
        .unwrap_or_else(|| buf.to_string());
    if resp.msgs.is_empty() {
        Ok(None)
    } else {
        Ok(Some((new_buf, resp.msgs)))
    }
}

/// 将 iLink 消息转换为 InboundMessage
fn convert_message(msg: WeixinMessage) -> Option<InboundMessage> {
    let text = msg
        .item_list
        .first()
        .and_then(|item| match item.item_type {
            1 => item.text_item.as_ref().map(|t| t.text.clone()),
            2 => Some("[图片]".to_string()),
            3 => Some("[语音]".to_string()),
            4 => item
                .file_item
                .as_ref()
                .and_then(|f| f.file_name.clone())
                .unwrap_or_else(|| "[文件]".to_string())
                .into(),
            _ => Some("[未知消息类型]".to_string()),
        })
        .unwrap_or_default();

    let is_group = !msg.group_id.is_empty();

    let media: Option<Vec<MediaAttachment>> =
        msg.item_list.first().and_then(|item| match item.item_type {
            2 => item.image_item.as_ref().map(|img| {
                vec![MediaAttachment {
                    media_type: MediaType::Image,
                    url: img.file_url.clone(),
                    data: None,
                    mime_type: "image/jpeg".to_string(),
                    filename: img.file_name.clone(),
                    caption: None,
                    thumbnail_url: None,
                    file_size: img.file_size.map(|s| s as u64),
                    duration: None,
                }]
            }),
            4 => item.file_item.as_ref().map(|f| {
                vec![MediaAttachment {
                    media_type: MediaType::Document,
                    url: f.file_url.clone(),
                    data: None,
                    mime_type: "application/octet-stream".to_string(),
                    filename: f.file_name.clone(),
                    caption: None,
                    thumbnail_url: None,
                    file_size: f.file_size.map(|s| s as u64),
                    duration: None,
                }]
            }),
            _ => None,
        });

    let msg_id = msg.message_id.map(|id| id.to_string()).unwrap_or_default();

    Some(InboundMessage {
        id: msg_id,
        platform: "wechat".to_string(),
        chat_id: msg.from_user_id.clone(),
        chat_type: if is_group {
            ChatType::Group
        } else {
            ChatType::Dm
        },
        chat_name: None,
        text: Some(text),
        author: MessageAuthor {
            id: msg.from_user_id.clone(),
            name: Some(msg.from_user_id),
            is_bot: false,
        },
        timestamp: msg.create_time_ms,
        media,
        command: None,
        callback: None,
        reply_to: None,
        thread_id: None,
        mentioned: None,
        is_group,
        metadata: Some(serde_json::json!({
            "session_id": msg.session_id,
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
        assert!(caps.iter().any(|c| c.name == CapabilityName::Audio));
        assert!(caps.iter().any(|c| c.name == CapabilityName::Video));
        assert!(caps.iter().any(|c| c.name == CapabilityName::Document));
        // All media capabilities should be supported
        for cap in caps {
            assert!(cap.supported);
        }
    }

    #[tokio::test]
    async fn test_init() {
        let mut adapter = WeChatAdapter::new();
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
        assert!(result.ok);
    }

    #[tokio::test]
    async fn test_init_with_saved_credentials() {
        let mut adapter = WeChatAdapter::new();
        let result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: None,
                api_key: None,
                base_url: None,
                extra: serde_json::json!({
                    "bot_token": "saved_token",
                    "ilink_bot_id": "saved_bot",
                    "ilink_user_id": "saved_user",
                }),
            })
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(
            adapter.bot_token.read().await.clone(),
            Some("saved_token".to_string())
        );
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
            message_id: Some(12345),
            from_user_id: "user@im.wechat".to_string(),
            to_user_id: "bot@im.wechat".to_string(),
            message_type: 1,
            create_time_ms: 1700000000000,
            session_id: "session_abc".to_string(),
            group_id: "".to_string(),
            context_token: None,
            item_list: vec![WeixinMessageItem {
                item_type: 1,
                text_item: Some(WeixinTextItem {
                    text: "你好".to_string(),
                }),
                image_item: None,
                file_item: None,
            }],
        };

        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.id, "12345");
        assert_eq!(inbound.text.as_deref(), Some("你好"));
        assert_eq!(inbound.chat_type, ChatType::Dm);
        assert!(!inbound.is_group);
        assert_eq!(inbound.author.id, "user@im.wechat");
        assert_eq!(inbound.timestamp, 1700000000000);
        let meta = inbound.metadata.unwrap();
        assert_eq!(
            meta.get("session_id").and_then(|v| v.as_str()),
            Some("session_abc")
        );
    }

    #[test]
    fn test_convert_image_message() {
        let msg = WeixinMessage {
            message_id: Some(67890),
            from_user_id: "user@im.wechat".to_string(),
            to_user_id: "bot@im.wechat".to_string(),
            message_type: 2,
            create_time_ms: 1700000000000,
            session_id: "sess2".to_string(),
            group_id: "".to_string(),
            context_token: None,
            item_list: vec![WeixinMessageItem {
                item_type: 2,
                text_item: None,
                image_item: Some(WeixinImageItem {
                    md5sum: Some("abc".to_string()),
                    file_size: Some(1024),
                    file_name: Some("photo.jpg".to_string()),
                    aes_key: None,
                    file_url: Some("https://cdn.url/img".to_string()),
                    height: None,
                    width: None,
                }),
                file_item: None,
            }],
        };

        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.text.as_deref(), Some("[图片]"));
        assert!(inbound.media.is_some());
        let media_type = &inbound.media.as_ref().unwrap().first().unwrap().media_type;
        assert!(
            matches!(media_type, MediaType::Image),
            "expected Image media type, got {:?}",
            media_type
        );
    }

    #[test]
    fn test_convert_file_message() {
        let msg = WeixinMessage {
            message_id: Some(111),
            from_user_id: "user@im.wechat".to_string(),
            to_user_id: "bot@im.wechat".to_string(),
            message_type: 4,
            create_time_ms: 1700000000000,
            session_id: "".to_string(),
            group_id: "".to_string(),
            context_token: None,
            item_list: vec![WeixinMessageItem {
                item_type: 4,
                text_item: None,
                image_item: None,
                file_item: Some(WeixinFileItem {
                    md5sum: None,
                    file_size: Some(2048),
                    file_name: Some("report.pdf".to_string()),
                    aes_key: None,
                    file_url: Some("https://cdn.url/file".to_string()),
                }),
            }],
        };

        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.text.as_deref(), Some("report.pdf"));
        assert!(inbound.media.is_some());
        let media_type = &inbound.media.as_ref().unwrap().first().unwrap().media_type;
        assert!(
            matches!(media_type, MediaType::Document),
            "expected Document media type, got {:?}",
            media_type
        );
    }

    #[test]
    fn test_convert_group_message() {
        let msg = WeixinMessage {
            message_id: Some(222),
            from_user_id: "member@im.wechat".to_string(),
            to_user_id: "bot@im.wechat".to_string(),
            message_type: 1,
            create_time_ms: 1700000000000,
            session_id: "".to_string(),
            group_id: "group@im.wechat".to_string(),
            context_token: None,
            item_list: vec![WeixinMessageItem {
                item_type: 1,
                text_item: Some(WeixinTextItem {
                    text: "群聊消息".to_string(),
                }),
                image_item: None,
                file_item: None,
            }],
        };

        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.text.as_deref(), Some("群聊消息"));
        assert!(inbound.is_group);
        assert_eq!(inbound.chat_type, ChatType::Group);
    }

    #[tokio::test]
    async fn test_send_before_connect_errors() {
        let adapter = WeChatAdapter::new();
        let result = adapter
            .send(SendTextParams {
                chat_id: "user@im.wechat".to_string(),
                message: OutboundMessage {
                    text: "hi".to_string(),
                    parse_mode: ParseMode::None,
                },
                reply_to: None,
                metadata: None,
            })
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Not authenticated")
        );
    }

    #[tokio::test]
    async fn test_disconnect_idempotent() {
        let mut adapter = WeChatAdapter::new();
        assert!(adapter.disconnect().await.is_ok());
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_double_disconnect() {
        let mut adapter = WeChatAdapter::new();
        adapter.disconnect().await.unwrap();
        adapter.disconnect().await.unwrap();
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_health_before_init() {
        let adapter = WeChatAdapter::new();
        let health = adapter.health().await;
        assert_eq!(health.status, HealthStatus::Down);
        assert!(!health.connected);
    }

    #[tokio::test]
    async fn test_runtime_config_before_init() {
        let adapter = WeChatAdapter::new();
        let rc = adapter.runtime_config();
        assert!(!rc.enabled);
        assert!(!rc.token_configured);
    }

    #[tokio::test]
    async fn test_runtime_config_after_init() {
        let mut adapter = WeChatAdapter::new();
        adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: None,
                api_key: None,
                base_url: None,
                extra: serde_json::json!({"bot_token": "test"}),
            })
            .await
            .unwrap();
        let rc = adapter.runtime_config();
        assert!(rc.enabled);
        assert!(rc.token_configured);
    }

    #[tokio::test]
    async fn test_get_chat_info_always_dm() {
        let adapter = WeChatAdapter::new();
        let info = adapter.get_chat_info("user@im.wechat").await.unwrap();
        assert_eq!(info.chat_type, ChatType::Dm);
        assert_eq!(info.chat_id, "user@im.wechat");
    }

    #[tokio::test]
    async fn test_set_event_bus() {
        let bus = Arc::new(EventBus::new());
        let mut adapter = WeChatAdapter::new();
        adapter.set_event_bus(bus.clone());
        assert_eq!(adapter.platform_name(), "wechat");
    }

    #[test]
    fn test_convert_unknown_message_type() {
        let msg = WeixinMessage {
            message_id: Some(1),
            from_user_id: "u@im.wx".to_string(),
            to_user_id: "b@im.bot".to_string(),
            message_type: 99,
            create_time_ms: 1000,
            session_id: "".to_string(),
            group_id: "".to_string(),
            context_token: None,
            item_list: vec![],
        };
        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.text.as_deref(), Some(""));
    }

    #[test]
    fn test_convert_empty_item_list() {
        let msg = WeixinMessage {
            message_id: None,
            from_user_id: "u@im.wx".to_string(),
            to_user_id: "b@im.bot".to_string(),
            message_type: 1,
            create_time_ms: 2000,
            session_id: "".to_string(),
            group_id: "".to_string(),
            context_token: None,
            item_list: vec![],
        };
        let inbound = convert_message(msg).unwrap();
        assert_eq!(inbound.id, "");
        assert_eq!(inbound.text.as_deref(), Some(""));
    }

    #[test]
    fn test_deserialize_weixin_message_from_json() {
        let json = r#"{
            "message_id": 7472251148840494728,
            "from_user_id": "user@im.wechat",
            "to_user_id": "bot@im.bot",
            "message_type": 1,
            "create_time_ms": 1781523501518,
            "item_list": [{
                "type": 1,
                "text_item": { "text": "你好" }
            }]
        }"#;
        let msg: WeixinMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.from_user_id, "user@im.wechat");
        assert_eq!(msg.message_type, 1);
        assert_eq!(msg.item_list.len(), 1);
    }

    #[test]
    fn test_deserialize_image_weixin_message() {
        let json = r#"{
            "message_id": 123,
            "from_user_id": "user@im.wechat",
            "to_user_id": "bot@im.bot",
            "message_type": 2,
            "create_time_ms": 1000,
            "item_list": [{
                "type": 2,
                "image_item": {
                    "md5sum": "abc123",
                    "file_size": 2048,
                    "file_name": "photo.jpg",
                    "file_url": "https://cdn.url/photo"
                }
            }]
        }"#;
        let msg: WeixinMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.message_type, 2);
        let item = &msg.item_list[0];
        assert!(item.image_item.is_some());
        let img = item.image_item.as_ref().unwrap();
        assert_eq!(img.file_name.as_deref(), Some("photo.jpg"));
        assert_eq!(img.file_size, Some(2048));
    }

    #[test]
    fn test_deserialize_empty_getupdates_response() {
        let json = r#"{"msgs":[],"sync_buf":"CAAY","get_updates_buf":"CgkI"}"#;
        let resp: GetUpdatesResponse = serde_json::from_str(json).unwrap();
        assert!(resp.msgs.is_empty());
        assert_eq!(resp.get_updates_buf.as_deref(), Some("CgkI"));
        assert_eq!(resp.sync_buf.as_deref(), Some("CAAY"));
    }

    #[test]
    fn test_credentials_serialization_roundtrip() {
        let creds = WeChatCredentials {
            bot_token: "token123@im.bot:secret".to_string(),
            ilink_bot_id: "bot123@im.bot".to_string(),
            ilink_user_id: "user123@im.wechat".to_string(),
            baseurl: "https://ilinkai.weixin.qq.com".to_string(),
        };
        let json = serde_json::to_string(&creds).unwrap();
        let deserialized: WeChatCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.bot_token, creds.bot_token);
        assert_eq!(deserialized.ilink_bot_id, creds.ilink_bot_id);
        assert_eq!(deserialized.ilink_user_id, creds.ilink_user_id);
        assert_eq!(deserialized.baseurl, creds.baseurl);
    }

    #[test]
    fn test_base64_encode_uin_zero() {
        let encoded = base64_encode_uin(0);
        assert_eq!(encoded, "AAAAAA==");
    }

    #[test]
    fn test_base64_encode_uin_max() {
        let encoded = base64_encode_uin(u32::MAX);
        assert!(!encoded.is_empty());
        assert_eq!(encoded.len(), 8);
    }

    #[tokio::test]
    async fn test_send_media_before_connect_errors() {
        let adapter = WeChatAdapter::new();
        let result = adapter
            .send_media(SendMediaParams {
                chat_id: "user@im.wechat".to_string(),
                media: MediaAttachment {
                    media_type: MediaType::Image,
                    url: Some("https://example.com/img.jpg".to_string()),
                    data: None,
                    mime_type: "image/jpeg".to_string(),
                    filename: Some("test.jpg".to_string()),
                    caption: None,
                    thumbnail_url: None,
                    file_size: Some(1024),
                    duration: None,
                },
                reply_to: None,
                text: None,
            })
            .await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Not authenticated")
        );
    }

    // ── AES-128-ECB 加密工具函数测试 ──

    #[test]
    fn test_pkcs7_pad_empty() {
        let data = b"";
        let padded = pkcs7_pad(data, 16);
        assert_eq!(padded.len(), 16);
        assert!(padded.iter().all(|&b| b == 16));
    }

    #[test]
    fn test_pkcs7_pad_exact_block() {
        let data = b"1234567890123456"; // exactly 16 bytes
        let padded = pkcs7_pad(data, 16);
        assert_eq!(padded.len(), 32);
        assert_eq!(&padded[..16], data);
        assert!(padded[16..].iter().all(|&b| b == 16));
    }

    #[test]
    fn test_pkcs7_pad_partial_block() {
        let data = b"hello"; // 5 bytes
        let padded = pkcs7_pad(data, 16);
        assert_eq!(padded.len(), 16);
        assert_eq!(&padded[..5], data);
        assert!(padded[5..].iter().all(|&b| b == 11));
    }

    #[test]
    fn test_aes_128_ecb_encrypt_decrypt_roundtrip() {
        use aes::cipher::{BlockDecrypt, KeyInit, generic_array::GenericArray};

        let key: [u8; 16] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10,
        ];
        let plaintext = b"Hello, WeChat media encryption test!";

        let ciphertext = aes_128_ecb_encrypt(plaintext, &key);
        assert!(ciphertext.len() >= plaintext.len());
        assert_ne!(ciphertext, plaintext);

        // Decrypt and verify
        let cipher = aes::Aes128::new_from_slice(&key).unwrap();
        let mut decrypted = Vec::new();
        for chunk in ciphertext.chunks(16) {
            let mut block = GenericArray::clone_from_slice(chunk);
            cipher.decrypt_block(&mut block);
            decrypted.extend_from_slice(&block);
        }

        // Remove PKCS7 padding
        let pad_len = *decrypted.last().unwrap() as usize;
        assert!(pad_len > 0 && pad_len <= 16);
        let decrypted_len = decrypted.len() - pad_len;
        assert_eq!(&decrypted[..decrypted_len], plaintext);
    }

    #[test]
    fn test_aes_padded_size() {
        assert_eq!(aes_padded_size(0), 16);
        assert_eq!(aes_padded_size(1), 16);
        assert_eq!(aes_padded_size(15), 16);
        assert_eq!(aes_padded_size(16), 32);
        assert_eq!(aes_padded_size(100), 112);
    }

    #[test]
    fn test_encode_aes_key_for_api_format() {
        // The key encoding must be base64(hex_string_bytes), not base64(raw_bytes)
        let key: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let encoded = encode_aes_key_for_api(&key);
        // hex string: "000102030405060708090a0b0c0d0e0f"
        // base64 of that hex string
        assert!(!encoded.is_empty());
        // Verify it's valid base64
        use base64::Engine;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        // Decoded should be the 32-char hex string
        let hex_str = std::str::from_utf8(&decoded).unwrap();
        assert_eq!(hex_str.len(), 32);
        assert!(hex_str.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_encode_aes_key_for_api_decodable_by_wechat_client() {
        // Simulate WeChat client's decode chain:
        // base64_decode → 32 ASCII hex chars → bytes.fromhex() → 16-byte AES key
        use base64::Engine;
        let original_key: [u8; 16] = [
            0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        let encoded = encode_aes_key_for_api(&original_key);

        // Client side decode
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        let hex_str = std::str::from_utf8(&decoded).unwrap();
        let recovered_key: Vec<u8> = (0..hex_str.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex_str[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(recovered_key.len(), 16);
        assert_eq!(recovered_key.as_slice(), &original_key);
    }

    #[test]
    fn test_md5_hex() {
        let data = b"hello world";
        let hash = md5_hex(data);
        assert_eq!(hash.len(), 32);
        assert_eq!(hash, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn test_generate_filekey() {
        let fk1 = generate_filekey();
        let fk2 = generate_filekey();
        assert_eq!(fk1.len(), 32);
        assert_eq!(fk2.len(), 32);
        assert_ne!(fk1, fk2);
        assert!(fk1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_build_cdn_upload_url() {
        let url = build_cdn_upload_url(
            "https://novac2c.cdn.weixin.qq.com/c2c",
            "abc+def/ghi==",
            "0123456789abcdef0123456789abcdef",
        );
        assert!(url.starts_with("https://novac2c.cdn.weixin.qq.com/c2c/upload"));
        assert!(url.contains("encrypted_query_param="));
        assert!(url.contains("filekey=0123456789abcdef0123456789abcdef"));
        // + and / and = should be percent-encoded
        assert!(!url.contains("+"));
        assert!(!url.contains("=") || url.contains("%3D"));
    }

    #[test]
    fn test_url_encode_for_cdn() {
        let input = "abc+def/ghi==xyz?&";
        let encoded = url_encode_for_cdn(input);
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains("=="));
        assert!(!encoded.contains('?'));
        assert!(!encoded.contains('&'));
    }
}
