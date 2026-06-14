//! Telegram 平台适配器
//!
//! 使用 Telegram Bot API 实现消息收发。
//! Phase 1 实现基本的文本消息发送。

use async_trait::async_trait;
use easybot_core::types::adapter::*;
use easybot_core::types::message::*;
use easybot_core::types::error::GatewayError;

/// Telegram 适配器
pub struct TelegramAdapter {
    platform_name: String,
    display_name: String,
    config: Option<AdapterConfig>,
    state: AdapterState,
    bot_info: Option<BotInfo>,
    capabilities: Vec<Capability>,
    messages_in: u64,
    messages_out: u64,
    errors: u64,
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
                Capability { name: CapabilityName::Text, supported: true, limits: None },
                Capability { name: CapabilityName::Image, supported: true, limits: None },
                Capability { name: CapabilityName::Audio, supported: true, limits: None },
                Capability { name: CapabilityName::Video, supported: true, limits: None },
                Capability { name: CapabilityName::Document, supported: true, limits: None },
                Capability { name: CapabilityName::Interactive, supported: true, limits: None },
                Capability { name: CapabilityName::Markdown, supported: true, limits: None },
                Capability { name: CapabilityName::Html, supported: true, limits: None },
                Capability { name: CapabilityName::Group, supported: true, limits: None },
                Capability { name: CapabilityName::TypingIndicator, supported: true, limits: None },
                Capability { name: CapabilityName::MessageEdit, supported: true, limits: None },
                Capability { name: CapabilityName::MessageDelete, supported: true, limits: None },
                Capability { name: CapabilityName::ChatList, supported: false, limits: None },
                Capability { name: CapabilityName::Streaming, supported: false, limits: None },
            ],
            messages_in: 0,
            messages_out: 0,
            errors: 0,
        }
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
        if config.token.is_none() || config.token.as_ref().map_or(true, |t| t.is_empty()) {
            return Ok(InitResult {
                ok: false,
                error: Some("Telegram bot token is required".to_string()),
            });
        }
        self.config = Some(config);
        self.state = AdapterState::Created;
        Ok(InitResult { ok: true, error: None })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        let token = self.config.as_ref()
            .and_then(|c| c.token.clone())
            .unwrap_or_default();

        // Phase 1: 模拟连接成功，验证 token 格式
        // Phase 2+: 使用 Telegram Bot API 验证 token 并获取 bot 信息
        if token.len() < 10 {
            return Ok(ConnectResult {
                ok: false,
                error: Some("Invalid token format".to_string()),
                bot_info: None,
            });
        }

        self.state = AdapterState::Connected;
        self.bot_info = Some(BotInfo {
            name: "EasyBot".to_string(),
            username: Some("easybot".to_string()),
            id: token.split(':').next().unwrap_or("0").to_string(),
        });

        tracing::info!("Telegram adapter connected (bot id: {})", self.bot_info.as_ref().unwrap().id);

        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: self.bot_info.clone(),
        })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        self.state = AdapterState::Stopped;
        tracing::info!("Telegram adapter disconnected");
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
            messages_in: self.messages_in,
            messages_out: self.messages_out,
            errors: self.errors,
            uptime: None,
        }
    }

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        let token = self.config.as_ref()
            .and_then(|c| c.token.clone())
            .ok_or_else(|| GatewayError::ConfigError("Telegram token not configured".to_string()))?;

        // Phase 1: 使用 reqwest 调用 Telegram Bot API 的 sendMessage
        // Phase 1 暂不引入 HTTP 客户端依赖，返回模拟成功
        // 实际实现将在引入 reqwest 后使用:
        //
        // let client = reqwest::Client::new();
        // let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
        // let resp = client.post(&url)
        //     .json(&serde_json::json!({
        //         "chat_id": params.chat_id,
        //         "text": params.message.text,
        //         "parse_mode": match params.message.parse_mode {
        //             ParseMode::Markdown => "MarkdownV2",
        //             ParseMode::Html => "HTML",
        //             ParseMode::None => "",
        //         },
        //     }))
        //     .send()
        //     .await
        //     .map_err(|e| GatewayError::Internal(e.to_string()))?;

        tracing::info!(
            "[Telegram] Sending message to {}: {}",
            params.chat_id,
            &params.message.text[..params.message.text.len().min(50)]
        );

        Ok(SendResult::ok(format!("sim_msg_{}", chrono::Utc::now().timestamp_millis())))
    }

    async fn send_typing(&self, _chat_id: &str) -> Result<(), GatewayError> {
        // Telegram Bot API: sendChatAction with "typing"
        Ok(())
    }

    async fn get_chat_info(&self, _chat_id: &str) -> Result<ChatInfo, GatewayError> {
        Ok(ChatInfo {
            chat_id: _chat_id.to_string(),
            name: Some("Unknown".to_string()),
            chat_type: ChatType::Dm,
            member_count: None,
        })
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: self.config.as_ref().map_or(false, |c| c.enabled),
            token_configured: self.config.as_ref().and_then(|c| c.token.as_ref()).map_or(false, |t| !t.is_empty()),
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
            messages_in: self.messages_in,
            messages_out: self.messages_out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_without_token() {
        let mut adapter = TelegramAdapter::new();
        let result = adapter.init(AdapterConfig {
            enabled: true,
            token: None,
            api_key: None,
            extra: serde_json::json!({}),
        }).await.unwrap();
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn test_init_and_connect() {
        let mut adapter = TelegramAdapter::new();
        adapter.init(AdapterConfig {
            enabled: true,
            token: Some("123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11".to_string()),
            api_key: None,
            extra: serde_json::json!({}),
        }).await.unwrap();
        let result = adapter.connect().await.unwrap();
        assert!(result.ok);
    }

    #[tokio::test]
    async fn test_send_message() {
        let mut adapter = TelegramAdapter::new();
        adapter.init(AdapterConfig {
            enabled: true,
            token: Some("123456:valid_token_format".to_string()),
            api_key: None,
            extra: serde_json::json!({}),
        }).await.unwrap();
        adapter.connect().await.unwrap();

        let result = adapter.send(SendTextParams {
            chat_id: "123456789".to_string(),
            message: OutboundMessage {
                text: "Hello, World!".to_string(),
                parse_mode: ParseMode::Markdown,
            },
            reply_to: None,
            metadata: None,
        }).await.unwrap();

        assert!(result.success);
    }
}
