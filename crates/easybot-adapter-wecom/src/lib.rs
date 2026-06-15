//! 企业微信 (WeCom) 平台适配器
//!
//! 使用企业微信服务端 API 实现消息收发。
//! - 发送：应用消息推送 (cgi-bin/message/send) + 群机器人 Webhook
//! - 接收：回调 URL 模式（预留）
//!
//! # 配置
//! ```yaml
//! wecom:
//!   enabled: true
//!   token: "<corp_secret>"           # 企业微信应用 Secret
//!   extra:
//!     corpid: "<your_corp_id>"        # 企业 ID
//!     agentid: 1000001                # 应用 AgentId
//!     webhook_url: "..."              # 可选：群机器人 Webhook URL
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::types::adapter::*;
use easybot_core::types::message::*;
use easybot_core::types::error::GatewayError;


/// 企业微信 API 基础 URL
const WECOM_API: &str = "https://qyapi.weixin.qq.com/cgi-bin";

/// Token 刷新余量（秒）
const TOKEN_REFRESH_MARGIN: u64 = 300;

/// 企业微信适配器（应用消息模式）
pub struct WeComAdapter {
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
    /// 缓存的 access_token
    access_token: tokio::sync::RwLock<Option<String>>,
    token_expires_at: tokio::sync::RwLock<i64>,
}

impl WeComAdapter {
    pub fn new() -> Self {
        Self {
            platform_name: "wecom".to_string(),
            display_name: "企业微信".to_string(),
            config: None,
            state: AdapterState::Created,
            bot_info: None,
            capabilities: vec![
                Capability { name: CapabilityName::Text, supported: true, limits: None },
                Capability { name: CapabilityName::Markdown, supported: true, limits: None },
                Capability { name: CapabilityName::Image, supported: true, limits: None },
                Capability { name: CapabilityName::Document, supported: true, limits: None },
                Capability { name: CapabilityName::Group, supported: true, limits: None },
            ],
            messages_in: AtomicU64::new(0),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            http_client: None,
            access_token: tokio::sync::RwLock::new(None),
            token_expires_at: tokio::sync::RwLock::new(0),
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

    /// 获取 corpid 和 corpsecret
    fn creds(&self) -> Result<(String, String), GatewayError> {
        let config = self.config.as_ref()
            .ok_or_else(|| GatewayError::ConfigError("Adapter not initialized".to_string()))?;
        let corpid = config.extra.get("corpid")
            .and_then(|v| v.as_str())
            .ok_or_else(|| GatewayError::ConfigError("Missing 'corpid' in wecom config.extra".to_string()))?;
        let secret = config.token.as_deref()
            .ok_or_else(|| GatewayError::ConfigError("Missing 'token' (corp_secret) for wecom".to_string()))?;
        Ok((corpid.to_string(), secret.to_string()))
    }

    /// 获取 agentid
    fn agent_id(&self) -> Result<i64, GatewayError> {
        let config = self.config.as_ref()
            .ok_or_else(|| GatewayError::ConfigError("Adapter not initialized".to_string()))?;
        config.extra.get("agentid")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| GatewayError::ConfigError("Missing 'agentid' in wecom config.extra".to_string()))
    }

    /// 获取 webhook URL（可选）
    fn webhook_url(&self) -> Option<String> {
        self.config.as_ref()
            .and_then(|c| c.extra.get("webhook_url"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// 确保 access_token 有效
    async fn ensure_token(&self) -> Result<String, GatewayError> {
        let expires_at = *self.token_expires_at.read().await;
        let now_ms = chrono::Utc::now().timestamp_millis();

        if expires_at > now_ms + (TOKEN_REFRESH_MARGIN as i64 * 1000) {
            if let Some(token) = self.access_token.read().await.clone() {
                return Ok(token);
            }
        }
        self.refresh_token().await
    }

    /// 获取 access_token
    async fn refresh_token(&self) -> Result<String, GatewayError> {
        let (corpid, corpsecret) = self.creds()?;
        let client = self.client()?;
        let url = format!("{}/gettoken?corpid={}&corpsecret={}", WECOM_API, corpid, corpsecret);

        let resp: WeChatTokenResponse = client.get(&url)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat gettoken failed: {}", e)))?
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat gettoken parse failed: {}", e)))?;

        if resp.errcode != 0 {
            return Err(GatewayError::Internal(format!(
                "WeChat auth failed: {} (code {})", resp.errmsg, resp.errcode
            )));
        }

        let token = resp.access_token
            .ok_or_else(|| GatewayError::Internal("No access_token in wecom response".to_string()))?;

        *self.access_token.write().await = Some(token.clone());
        *self.token_expires_at.write().await = chrono::Utc::now().timestamp_millis() + 7_100_000; // 接近2小时

        Ok(token)
    }

    /// 应用消息 - 发送文本
    async fn send_app_text(&self, chat_id: &str, text: &str) -> Result<WeChatSendResult, GatewayError> {
        let token = self.ensure_token().await?;
        let agent_id = self.agent_id()?;
        let client = self.client()?;
        let url = format!("{}/message/send?access_token={}", WECOM_API, token);

        let body = serde_json::json!({
            "touser": chat_id,
            "msgtype": "text",
            "agentid": agent_id,
            "text": {
                "content": text,
            },
        });

        let resp: WeChatSendResult = client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send failed: {}", e)))?
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send parse failed: {}", e)))?;

        Ok(resp)
    }

    /// 应用消息 - 发送 Markdown
    async fn send_app_markdown(&self, chat_id: &str, text: &str) -> Result<WeChatSendResult, GatewayError> {
        let token = self.ensure_token().await?;
        let agent_id = self.agent_id()?;
        let client = self.client()?;
        let url = format!("{}/message/send?access_token={}", WECOM_API, token);

        let body = serde_json::json!({
            "touser": chat_id,
            "msgtype": "markdown",
            "agentid": agent_id,
            "markdown": {
                "content": text,
            },
        });

        let resp: WeChatSendResult = client.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat markdown send failed: {}", e)))?
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat markdown send parse failed: {}", e)))?;

        Ok(resp)
    }

    /// Webhook 模式发送文本
    async fn send_webhook_text(url: &str, text: &str) -> Result<(), GatewayError> {
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "msgtype": "text",
            "text": {
                "content": text,
            },
        });

        let resp = client.post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat webhook send failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(GatewayError::Internal(format!(
                "WeChat webhook error ({}): {}", status, body
            )));
        }

        Ok(())
    }
}

#[async_trait]
impl PlatformAdapter for WeComAdapter {
    fn platform_name(&self) -> &str { &self.platform_name }
    fn display_name(&self) -> &str { &self.display_name }
    fn capabilities(&self) -> &[Capability] { &self.capabilities }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        let has_corpid = config.extra.get("corpid").and_then(|v| v.as_str()).is_some();
        let has_secret = config.token.is_some();
        let has_webhook = config.extra.get("webhook_url").and_then(|v| v.as_str()).is_some();

        if (!has_corpid || !has_secret)
            && !has_webhook {
                return Ok(InitResult {
                    ok: false,
                    error: Some("企业微信适配器需要配置 token(corp_secret) + extra.corpid，或 extra.webhook_url".to_string()),
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
        // 如果是 webhook 模式，直接连接
        if self.webhook_url().is_some() {
            self.state = AdapterState::Connected;
            self.bot_info = Some(BotInfo {
                name: "企业微信(Webhook)".to_string(),
                username: Some("wecom_webhook".to_string()),
                id: "webhook".to_string(),
            });
            tracing::info!("企业微信适配器已连接（Webhook 模式）");
            return Ok(ConnectResult { ok: true, error: None, bot_info: self.bot_info.clone() });
        }

        // 否则验证 token
        match self.refresh_token().await {
            Ok(_) => {
                self.state = AdapterState::Connected;
                self.bot_info = Some(BotInfo {
                    name: "企业微信".to_string(),
                    username: None,
                    id: self.creds().map(|c| c.0).unwrap_or_default(),
                });
                tracing::info!("企业微信适配器已连接（应用消息模式）");
                Ok(ConnectResult { ok: true, error: None, bot_info: self.bot_info.clone() })
            }
            Err(e) => Ok(ConnectResult {
                ok: false,
                error: Some(format!("WeChat auth failed: {}", e)),
                bot_info: None,
            }),
        }
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        self.state = AdapterState::Stopped;
        tracing::info!("企业微信适配器已断开");
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
        // 优先使用 Webhook 模式
        if let Some(ref wh_url) = self.webhook_url() {
            return match Self::send_webhook_text(wh_url, &params.message.text).await {
                Ok(()) => {
                    self.messages_out.fetch_add(1, Ordering::Relaxed);
                    Ok(SendResult {
                        success: true,
                        message_id: Some(format!("webhook_{}", chrono::Utc::now().timestamp_millis())),
                        timestamp: Some(chrono::Utc::now().timestamp_millis()),
                        error: None, error_code: None, retryable: false,
                    })
                }
                Err(e) => {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                    Ok(SendResult::fail(e.to_string(), true))
                }
            };
        }

        // 应用消息模式
        let is_markdown = params.message.parse_mode == easybot_core::types::message::ParseMode::Markdown;

        let result = if is_markdown {
            self.send_app_markdown(&params.chat_id, &params.message.text).await
        } else {
            self.send_app_text(&params.chat_id, &params.message.text).await
        };

        match result {
            Ok(resp) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                Ok(SendResult {
                    success: resp.errcode == 0,
                    message_id: Some(resp.msgid.unwrap_or_default()),
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    error: if resp.errcode != 0 { Some(resp.errmsg) } else { None },
                    error_code: if resp.errcode != 0 { Some(resp.errcode.to_string()) } else { None },
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
        // 企业微信支持图片上传后发送
        // 简化处理：先上传临时素材，再发送
        let token = self.ensure_token().await?;
        let agent_id = self.agent_id()?;
        let client = self.client()?;

        // 上传图片获取 media_id
        let upload_url = format!("{}/media/upload?access_token={}&type=image", WECOM_API, token);
        let file_data = if let Some(ref url) = params.media.url {
            let resp = client.get(url)
                .send().await
                .map_err(|e| GatewayError::Internal(format!("Download media failed: {}", e)))?;
            resp.bytes().await
                .map_err(|e| GatewayError::Internal(format!("Read media failed: {}", e)))?
                .to_vec()
        } else if let Some(ref data) = params.media.data {
            data.as_bytes().to_vec()
        } else {
            return Err(GatewayError::InvalidRequest("No media data".to_string()));
        };

        let file_part = reqwest::multipart::Part::bytes(file_data)
            .file_name(params.media.filename.unwrap_or_else(|| "file".to_string()))
            .mime_str(&params.media.mime_type)
            .map_err(|e| GatewayError::Internal(format!("Mime error: {}", e)))?;

        let form = reqwest::multipart::Form::new().part("media", file_part);

        let upload_resp: WeChatUploadResult = client.post(&upload_url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat upload failed: {}", e)))?
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat upload parse failed: {}", e)))?;

        if upload_resp.errcode != 0 {
            return Ok(SendResult::fail(format!("WeChat upload error: {}", upload_resp.errmsg), false));
        }

        let media_id = upload_resp.media_id
            .ok_or_else(|| GatewayError::Internal("No media_id in upload response".to_string()))?;

        // 发送图片消息
        let send_url = format!("{}/message/send?access_token={}", WECOM_API, token);
        let send_body = serde_json::json!({
            "touser": params.chat_id,
            "msgtype": "image",
            "agentid": agent_id,
            "image": { "media_id": media_id },
        });

        let resp: WeChatSendResult = client.post(&send_url)
            .json(&send_body)
            .send()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send media failed: {}", e)))?
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("WeChat send media parse failed: {}", e)))?;

        self.messages_out.fetch_add(1, Ordering::Relaxed);
        Ok(SendResult {
            success: resp.errcode == 0,
            message_id: Some(resp.msgid.unwrap_or_default()),
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            error: if resp.errcode != 0 { Some(resp.errmsg) } else { None },
            error_code: None,
            retryable: false,
        })
    }

    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError> {
        // 企业微信没有简单的查询用户/群信息 API（需要复杂权限）
        // 返回基本信息
        Ok(ChatInfo {
            chat_id: chat_id.to_string(),
            name: None,
            chat_type: if chat_id.starts_with('@') || chat_id.starts_with("ww") {
                ChatType::Dm
            } else {
                ChatType::Group
            },
            member_count: None,
        })
    }

    async fn list_chats(&self, _filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        Ok(Vec::new()) // 企业微信不提供群列表 API
    }
}

impl Default for WeComAdapter {
    fn default() -> Self { Self::new() }
}

// ── API 响应类型 ──

/// Token 响应
#[derive(Debug, serde::Deserialize)]
struct WeChatTokenResponse {
    errcode: i64,
    errmsg: String,
    access_token: Option<String>,
    #[allow(dead_code)]
    expires_in: Option<u64>,
}

/// 发送消息结果
#[derive(Debug, serde::Deserialize)]
struct WeChatSendResult {
    errcode: i64,
    errmsg: String,
    msgid: Option<String>,
}

/// 上传结果
#[derive(Debug, serde::Deserialize)]
struct WeChatUploadResult {
    errcode: i64,
    errmsg: String,
    #[allow(dead_code)]
    #[serde(rename = "type")]
    media_type: Option<String>,
    media_id: Option<String>,
    #[allow(dead_code)]
    created_at: Option<String>,
}

// ── 单元测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_adapter() {
        let adapter = WeComAdapter::new();
        assert_eq!(adapter.platform_name(), "wecom");
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[test]
    fn test_capabilities() {
        let adapter = WeComAdapter::new();
        assert!(adapter.capabilities().iter().any(|c| c.name == CapabilityName::Text));
    }

    #[tokio::test]
    async fn test_init_missing_config() {
        let mut adapter = WeComAdapter::new();
        let r = adapter.init(AdapterConfig {
            enabled: true, token: None, api_key: None, extra: serde_json::json!({}),
        }).await.unwrap();
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn test_init_webhook_only() {
        let mut adapter = WeComAdapter::new();
        let r = adapter.init(AdapterConfig {
            enabled: true, token: None, api_key: None,
            extra: serde_json::json!({"webhook_url": "https://qyapi.weixin.qq.com/webhook/send?key=xxx"}),
        }).await.unwrap();
        assert!(r.ok);
    }

    #[tokio::test]
    async fn test_init_app_config() {
        let mut adapter = WeComAdapter::new();
        let r = adapter.init(AdapterConfig {
            enabled: true, token: Some("secret".into()), api_key: None,
            extra: serde_json::json!({"corpid": "ww12345", "agentid": 1000001}),
        }).await.unwrap();
        assert!(r.ok);
    }

    #[test]
    fn test_status_summary() {
        let adapter = WeComAdapter::new();
        let s = adapter.status_summary();
        assert_eq!(s.platform, "wecom");
        assert_eq!(s.display_name, "企业微信");
    }

    #[test]
    fn test_agent_id() {
        let adapter = WeComAdapter {
            config: Some(AdapterConfig {
                enabled: true, token: Some("s".into()), api_key: None,
                extra: serde_json::json!({"agentid": 1000002}),
            }),
            ..WeComAdapter::new()
        };
        assert_eq!(adapter.agent_id().unwrap(), 1000002);
    }
}
