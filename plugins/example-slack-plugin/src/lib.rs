//! Example: Slack Adapter Plugin
//!
//! Demonstrates how to build a custom IM adapter using the EasyBot Plugin SDK.
//! This is a reference implementation — not production-ready.
//!
//! Build:
//!   cd plugins/example-slack-plugin
//!   cargo build --release
//!
//! Install:
//!   cp target/release/libslack.{so,dylib} ~/.easybot/plugins/slack/
//!   cp plugin.yaml ~/.easybot/plugins/slack/
//!
//! Configure in gateway.yaml:
//!   adapters:
//!     slack:
//!       enabled: true
//!       token: "${SLACK_BOT_TOKEN}"

use easybot_plugin_sdk::prelude::*;

struct SlackAdapter {
    name: String,
    display: String,
    state: AdapterState,
    bot_token: Option<String>,
    event_bus: Option<std::sync::Arc<easybot_plugin_sdk::easybot_core::bus::EventBus>>,
}

impl SlackAdapter {
    fn new() -> Self {
        Self {
            name: "slack".into(),
            display: "Slack (Example Plugin)".into(),
            state: AdapterState::Created,
            bot_token: None,
            event_bus: None,
        }
    }
}

#[async_trait]
impl PlatformAdapter for SlackAdapter {
    fn platform_name(&self) -> &str {
        &self.name
    }

    fn display_name(&self) -> &str {
        &self.display
    }

    fn capabilities(&self) -> &[Capability] {
        &[Capability::text()]
    }

    fn set_event_bus(&mut self, bus: std::sync::Arc<easybot_plugin_sdk::easybot_core::bus::EventBus>) {
        self.event_bus = Some(bus);
    }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        self.bot_token = config.token.clone();
        if self.bot_token.is_none() {
            return Ok(InitResult {
                ok: false,
                error: Some("SLACK_BOT_TOKEN required".into()),
            });
        }
        self.state = AdapterState::Starting;
        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        // In production, establish a WebSocket (RTM API) or poll Events API
        self.state = AdapterState::Connected;
        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: None,
        })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        self.state = AdapterState::Stopped;
        self.bot_token = None;
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
            messages_in: 0,
            messages_out: 0,
            errors: 0,
            uptime: None,
        }
    }

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        let token = self.bot_token.as_ref()
            .ok_or_else(|| GatewayError::ConfigError("No token configured".into()))?;

        // Example: call Slack Web API to send a message
        let client = reqwest::Client::new();
        let resp = client
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(token)
            .json(&serde_json::json!({
                "channel": params.chat_id,
                "text": params.message.text,
            }))
            .send()
            .await
            .map_err(|e| GatewayError::SendError(e.to_string()))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| GatewayError::SendError(e.to_string()))?;

        if body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            Ok(SendResult {
                success: true,
                message_id: body.get("ts").and_then(|v| v.as_str().map(String::from)),
                timestamp: None,
                error: None,
                error_code: None,
                retryable: false,
            })
        } else {
            let error = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Err(GatewayError::SendError(error.to_string()))
        }
    }

    async fn get_chat_info(&self, _chat_id: &str) -> Result<ChatInfo, GatewayError> {
        // Use conversations.info API
        Err(GatewayError::capability_not_supported("get_chat_info"))
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: self.bot_token.is_some(),
            token_configured: self.bot_token.is_some(),
            extra: serde_json::Value::Null,
        }
    }

    fn status_summary(&self) -> AdapterStatusSummary {
        AdapterStatusSummary {
            platform: self.name.clone(),
            display_name: self.display.clone(),
            state: self.state.clone(),
            connected: self.state == AdapterState::Connected,
            health: None,
            last_error: None,
            uptime: None,
            messages_in: 0,
            messages_out: 0,
        }
    }
}

declare_plugin!(SlackAdapter, SlackAdapter::new);
