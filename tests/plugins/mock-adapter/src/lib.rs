#![allow(missing_docs)]

//! Mock 适配器（用于集成测试）
//!
//! 实现最小功能的 PlatformAdapter，通过 declare_plugin! 导出 C ABI。
//! 不作为正式工作区成员发布。

use easybot_plugin_sdk::prelude::*;

struct MockAdapter {
    name: String,
    display: String,
    state: AdapterState,
}

impl MockAdapter {
    fn new() -> Self {
        Self {
            name: "mock-test".into(),
            display: "Mock Test Adapter".into(),
            state: AdapterState::Created,
        }
    }
}

#[async_trait]
impl PlatformAdapter for MockAdapter {
    fn platform_name(&self) -> &str {
        &self.name
    }

    fn display_name(&self) -> &str {
        &self.display
    }

    fn capabilities(&self) -> &[Capability] {
        &[]
    }

    async fn init(&mut self, _config: AdapterConfig) -> Result<InitResult, GatewayError> {
        self.state = AdapterState::Starting;
        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        self.state = AdapterState::Connected;
        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: None,
        })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state.clone()
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            status: HealthStatus::Healthy,
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

    async fn send(&self, _params: SendTextParams) -> Result<SendResult, GatewayError> {
        Ok(SendResult {
            success: true,
            message_id: Some("mock-msg-1".into()),
            timestamp: None,
            error: None,
            error_code: None,
            retryable: false,
        })
    }

    async fn get_chat_info(&self, _chat_id: &str) -> Result<ChatInfo, GatewayError> {
        Err(GatewayError::capability_not_supported("get_chat_info"))
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: true,
            token_configured: false,
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

declare_plugin!(MockAdapter, MockAdapter::new);
