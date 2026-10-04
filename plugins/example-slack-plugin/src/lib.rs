//! Example: Slack Adapter Plugin（进程外插件的高级参考）
//!
//! Demonstrates how to build a custom IM adapter using the EasyBot Plugin SDK.
//! This is a reference implementation — not production-ready.
//!
//! 架构：本插件编译为**独立可执行文件**，宿主 EasyBot 以子进程方式启动，
//! 经 stdin/stdout JSON 协议通信（入口见 `src/main.rs` 的 `run_plugin!`）。
//!
//! Build:
//!   cd plugins/example-slack-plugin
//!   cargo build --release
//!
//! Install:
//!   mkdir -p ~/.easybot/plugins/slack
//!   cp target/release/easybot-example-slack-plugin ~/.easybot/plugins/slack/   # Windows: .exe
//!   cp plugin.yaml ~/.easybot/plugins/slack/
//!
//! Configure in gateway.yaml:
//!   adapters:
//!     slack:
//!       enabled: true
//!       token: "${SLACK_BOT_TOKEN}"

use std::sync::Arc;

use easybot_plugin_sdk::prelude::*;

/// Slack 适配器（示例）。
///
/// 真实适配器通常还持有 HTTP client、缓存（须带大小上限或 TTL，见
/// `docs/17 plugin-methodology.md`「缓存」一节）等字段。
pub struct SlackAdapter {
    name: String,
    display: String,
    state: AdapterState,
    bot_token: Option<String>,
    event_bus: Option<Arc<EventBus>>,
    client: reqwest::Client,
}

impl SlackAdapter {
    pub fn new() -> Self {
        Self {
            name: "slack".into(),
            display: "Slack (Example Plugin)".into(),
            state: AdapterState::Created,
            bot_token: None,
            event_bus: None,
            client: reqwest::Client::new(),
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
        &[Capability {
            name: CapabilityName::Text,
            supported: true,
            limits: None,
        }]
    }

    fn set_event_bus(&mut self, bus: Arc<EventBus>) {
        self.event_bus = Some(bus);
    }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        self.bot_token = config.token.clone();
        if self.bot_token.is_none() {
            // 缺凭据不算错误：返回 ok=false + 说明，宿主会跳过启动该适配器。
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
        // 真实实现：调 `auth.test` 校验 token，再建立 WebSocket（RTM）或轮询 Events API。
        match self.bot_token.as_deref() {
            None => Ok(ConnectResult::failed(
                "SLACK_BOT_TOKEN required".into(),
                None,
            )),
            Some(_) => {
                self.state = AdapterState::Connected;
                Ok(ConnectResult::ok(None))
            }
        }
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state.clone()
    }

    async fn health(&self) -> HealthReport {
        let connected = self.state == AdapterState::Connected;
        HealthReport {
            status: if connected {
                HealthStatus::Healthy
            } else {
                HealthStatus::Down
            },
            connected,
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
        let token = self
            .bot_token
            .as_ref()
            .ok_or_else(|| GatewayError::ConfigError("No token configured".into()))?;

        // 网络层失败必须用 `Transient` 包裹：宿主据此做退避重连（见
        // `docs/17 plugin-methodology.md`「错误分类」）。
        let resp = self
            .client
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(token)
            .json(&serde_json::json!({
                "channel": params.chat_id,
                "text": params.message.text,
            }))
            .send()
            .await
            .map_err(|e| GatewayError::Transient(e.to_string()))?;

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| GatewayError::Transient(e.to_string()))?;

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
            // 平台业务失败：返回 success=false（不是 Err——Err 表示调用本身失败）。
            let error = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            Ok(SendResult {
                success: false,
                message_id: None,
                timestamp: None,
                error: Some(error),
                error_code: None,
                retryable: false,
            })
        }
    }

    async fn get_chat_info(&self, _chat_id: &str) -> Result<ChatInfo, GatewayError> {
        // 真实实现走 conversations.info；示例返回「不支持」。
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

// 进程外插件入口见 `src/main.rs`：
//     easybot_plugin_sdk::run_plugin!(SlackAdapter, SlackAdapter::new);
