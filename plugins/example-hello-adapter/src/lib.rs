//! Hello Adapter — EasyBot 适配器插件（`easybot plugin new` 脚手架生成）
//!
//! 参考文档：
//!   - 快速上手：https://github.com/EasyIndie/EasyBot/blob/main/docs/plugin-quickstart.md
//!   - 完整指南：https://github.com/EasyIndie/EasyBot/blob/main/docs/plugin-guide.md
//!   - 方法论：  https://github.com/EasyIndie/EasyBot/blob/main/docs/plugin-methodology.md
//!
//! 构建 / 测试（本示例在仓库内以 path 依赖编译；对外发布的独立工程用 git 依赖）：
//!   cargo test                # 单元 + PluginTestHost 离线测试（无需启动真实网关）

// declare_plugin! 展开为 `#[unsafe(no_mangle)]` FFI 入口；workspace lint 默认 deny
// unsafe_code，此处豁免（对齐 easybot-plugin-sdk 自身的处理）。
#![allow(unsafe_code)]

use easybot_plugin_sdk::prelude::*;
use std::sync::Arc;

/// 适配器主体：持有状态与可选的事件总线。
///
/// TODO: 按平台需求添加字段——HTTP client、token、缓存（须带大小上限或 TTL，见
/// docs/plugin-methodology.md「缓存」一节）。
pub struct HelloAdapter {
    state: AdapterState,
    event_bus: Option<Arc<EventBus>>,
    // TODO: token: Option<String>,
    // TODO: client: Option<reqwest::Client>,
}

impl HelloAdapter {
    /// 构造器（`declare_plugin!` 的入口）。
    ///
    /// 若平台需要非默认初始化，可在此注入 HTTP client 等依赖——测试用 wiremock
    /// 替换（见 docs/plugin-methodology.md「传输可注入」）。
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for HelloAdapter {
    fn default() -> Self {
        Self {
            state: AdapterState::Created,
            event_bus: None,
        }
    }
}

#[async_trait]
impl PlatformAdapter for HelloAdapter {
    fn platform_name(&self) -> &str {
        "hello-adapter"
    }

    fn display_name(&self) -> &str {
        "Hello Adapter"
    }

    fn capabilities(&self) -> &[Capability] {
        // 声明平台能力：宿主据此路由消息与展示。
        // TODO: 按平台调整 supported / limits（CapabilityName 见 SDK prelude）。
        &[
            Capability {
                name: CapabilityName::Text,
                supported: true,
                limits: None,
            },
            Capability {
                name: CapabilityName::Interactive,
                supported: false,
                limits: None,
            },
        ]
    }

    fn set_event_bus(&mut self, bus: Arc<EventBus>) {
        // 入站事件（消息/回调）经总线发布给宿主；测试宿主（PluginTestHost）也用它断言。
        self.event_bus = Some(bus);
    }

    async fn init(&mut self, _config: AdapterConfig) -> Result<InitResult, GatewayError> {
        // 解析配置。凭据优先来自适配器 env 或 gateway.yaml 的 `adapters.<platform>` 段。
        // TODO: self.token = _config.token.clone();
        // if self.token.is_none() {
        //     return Ok(InitResult {
        //         ok: false,
        //         error: Some("hello-adapter token required".into()),
        //     });
        // }
        self.state = AdapterState::Starting;
        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        // TODO: 建立真实连接（WebSocket / 长轮询）。失败时分类标记：
        //   网络/临时失败 → ConnectResult::failed(msg, Some(ConnectErrorKind::Transient))
        //   凭据被拒     → ConnectResult::failed(msg, Some(ConnectErrorKind::Permanent))
        // 分类会穿透健康监测的重试/停用决策，见 docs/plugin-methodology.md。
        // 注意：后台轮询任务用 tokio::spawn 时须带 Semaphore/JoinSet 并发上限。
        self.state = AdapterState::Connected;
        Ok(ConnectResult::ok(Some(BotInfo {
            name: "Hello Adapter".into(),
            username: Some("hello-adapter".into()),
            id: "hello-adapter".into(),
        })))
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        // TODO: 停止后台任务、释放连接。
        self.state = AdapterState::Stopped;
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
        // TODO: 调用平台发送 API（reqwest）。网络层失败须用 GatewayError::Transient
        // 包裹，让重连路径按瞬态处理；业务失败用 GatewayError::SendError。
        tracing::info!(chat_id = %params.chat_id, text = %params.message.text, "send");

        // 演示事件总线：把发送文本回显为 message.inbound 事件（tests/host_test.rs 断言用）。
        if let Some(bus) = &self.event_bus {
            bus.publish(GatewayEvent::new(
                event_types::MESSAGE_INBOUND,
                self.platform_name(),
                serde_json::json!({
                    "chat_id": params.chat_id,
                    "text": params.message.text,
                }),
            ));
        }
        Ok(SendResult::ok(format!(
            "{}-{}",
            self.platform_name(),
            params.chat_id
        )))
    }

    async fn get_chat_info(&self, _chat_id: &str) -> Result<ChatInfo, GatewayError> {
        // TODO: 调用平台查询接口；暂未支持可返回 capability_not_supported。
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
            platform: self.platform_name().to_string(),
            display_name: self.display_name().to_string(),
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

// 声明插件入口点（FFI）。宿主经 `easybot_plugin_create` 创建适配器实例。
// 库文件名 = `lib{package-name}.{so|dylib|dll}`（Rust 用下划线连接 crate 名）。
declare_plugin!(HelloAdapter, HelloAdapter::new);
