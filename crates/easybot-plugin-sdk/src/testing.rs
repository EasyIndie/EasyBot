//! 插件测试宿主（`#[cfg(feature = "testing")]`）
//!
//! 在内存里模拟 EasyBot 宿主，让插件作者**不启动真实网关**也能跑通
//! `init → connect → send → disconnect` 与事件发布等核心路径
//! （DX-4 测试金字塔第 2 层）。
//!
//! # 能力
//!
//! - 事件总线：复用 core 的 [`EventBus`]（tokio broadcast，镜像真实语义），
//!   插件通过 `set_event_bus` 注入，测试用 `subscribe`/`recv_event` 断言事件流。
//! - 生命周期驱动：`attach`（注入事件总线）→ `init` → `connect` → `disconnect`。
//! - 消息构造器：`inbound_text` / `inbound_media` / `inbound_callback`，
//!   免手写 `InboundMessage` 的全部字段。
//! - 发送记录：`send` 包装 `PlatformAdapter::send` 并把请求参数记入
//!   `send_log`，供断言插件收到的发送请求形态正确。
//!
//! # 传输注入（方法论）
//!
//! 真实平台适配器的 `send`/轮询会发 HTTP。**测试不应依赖网络**：
//! 插件把 HTTP client 通过 `init(config)`（如 `base_url` 指向 wiremock）或
//! 构造器注入，测试用 wiremock 替换为内存 mock。宿主只负责宿主侧语义，
//! 不 mock 平台 HTTP——那是 wiremock 的职责（测试金字塔第 3 层）。
//!
//! # 示例
//!
//! ```ignore
//! let mut host = PluginTestHost::new();
//! let mut adapter = MyAdapter::default();
//! host.attach(&mut adapter);
//! host.init(&mut adapter, PluginTestHost::config()).await.unwrap();
//! assert!(host.connect(&mut adapter).await.unwrap().ok);
//!
//! let mut rx = host.subscribe(event_types::MESSAGE_INBOUND);
//! let result = host.send(&mut adapter, SendTextParams {
//!     chat_id: "c1".into(),
//!     message: OutboundMessage { text: "hi".into(), parse_mode: ParseMode::None },
//!     reply_to: None, metadata: None,
//! }).await.unwrap();
//! assert!(result.success);
//! assert_eq!(host.send_log().len(), 1);
//! let ev = recv_event(&mut rx, Duration::from_secs(1)).await.unwrap();
//! ```

use easybot_core::bus::EventBus;
use easybot_core::types::event::{GatewayEvent, event_types};
use easybot_core::types::message::{
    CallbackData, ChatType, InboundMessage, MediaAttachment, MessageSender, MessageType,
    SendResult, SendTextParams,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

use crate::{AdapterConfig, GatewayError, PlatformAdapter};

/// 当前时间戳（毫秒）。避免为宿主引入 chrono 依赖。
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 等待事件总线上的下一条事件，超时返回 `None`。
///
/// 宿主与测试共享 tokio 运行时（`#[tokio::test]`），bus 事件与真实网关
/// 一样经 broadcast 通道投递，慢消费者会 `Lagged`（此处按空处理）。
pub async fn recv_event(
    rx: &mut broadcast::Receiver<GatewayEvent>,
    timeout: Duration,
) -> Option<GatewayEvent> {
    match tokio::time::timeout(timeout, rx.recv()).await {
        Ok(Ok(event)) => Some(event),
        _ => None,
    }
}

/// 内存版 EasyBot 宿主，供插件离线测试。
///
/// 持有事件总线与发送记录；不启动真实网络、不做健康监测。
pub struct PluginTestHost {
    bus: Arc<EventBus>,
    sends: Arc<Mutex<Vec<SendTextParams>>>,
}

impl Default for PluginTestHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginTestHost {
    /// 创建新宿主（事件总线容量与真实网关一致）。
    pub fn new() -> Self {
        Self {
            bus: Arc::new(EventBus::new()),
            sends: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 事件总线引用（可用于 `subscribe`，或让插件代码直接发布事件）。
    pub fn bus(&self) -> Arc<EventBus> {
        self.bus.clone()
    }

    /// 把事件总线注入适配器（等价于真实宿主的 `adapter.set_event_bus`）。
    ///
    /// 必须在 `connect()` 之前调用，否则插件后台任务拿不到总线。
    pub fn attach(&self, adapter: &mut dyn PlatformAdapter) {
        adapter.set_event_bus(self.bus.clone());
    }

    /// 调用 `adapter.init(config)`。
    pub async fn init(
        &self,
        adapter: &mut dyn PlatformAdapter,
        config: AdapterConfig,
    ) -> Result<crate::InitResult, GatewayError> {
        adapter.init(config).await
    }

    /// 调用 `adapter.connect()`。
    pub async fn connect(
        &self,
        adapter: &mut dyn PlatformAdapter,
    ) -> Result<crate::ConnectResult, GatewayError> {
        adapter.connect().await
    }

    /// 调用 `adapter.disconnect()`。
    pub async fn disconnect(&self, adapter: &mut dyn PlatformAdapter) -> Result<(), GatewayError> {
        adapter.disconnect().await
    }

    /// 驱动一次发送，并把请求参数记入 [`send_log`](Self::send_log) 供断言。
    pub async fn send(
        &self,
        adapter: &mut dyn PlatformAdapter,
        params: SendTextParams,
    ) -> Result<SendResult, GatewayError> {
        self.sends.lock().unwrap().push(params.clone());
        adapter.send(params).await
    }

    /// 到目前为止通过 [`send`](Self::send) 驱动的发送请求（克隆返回）。
    pub fn send_log(&self) -> Vec<SendTextParams> {
        self.sends.lock().unwrap().clone()
    }

    /// 订阅指定事件类型（事件类型常量见 [`event_types`]）。
    pub fn subscribe(&self, event_type: &str) -> broadcast::Receiver<GatewayEvent> {
        self.bus.subscribe(event_type)
    }

    /// 向总线发布事件（模拟宿主/其它组件投递插件关心的入站事件）。
    pub fn publish(&self, event: GatewayEvent) {
        self.bus.publish(event);
    }

    /// 生成最小可用 `AdapterConfig`（`enabled: true`，无凭据）。
    ///
    /// 测试可再直接改写字段（如 `base_url` 指向 wiremock）。
    pub fn config() -> AdapterConfig {
        AdapterConfig::with_enabled(true)
    }

    /// 生成指向自定义 base_url 的 `AdapterConfig`（HTTP 插件 wiremock 测试用）。
    pub fn config_with_base_url(base_url: impl Into<String>) -> AdapterConfig {
        let mut config = Self::config();
        config.base_url = Some(base_url.into());
        config
    }

    /// 构造发送者（ID + 可选显示名），其余字段取安全默认值。
    pub fn sender(id: impl Into<String>, name: Option<&str>) -> MessageSender {
        MessageSender {
            id: id.into(),
            name: name.map(Into::into),
            username: None,
            avatar_url: None,
            is_bot: false,
            role: None,
            language_code: None,
        }
    }

    /// 构造纯文本入站消息。
    pub fn inbound_text(
        platform: impl Into<String>,
        chat_id: impl Into<String>,
        text: impl Into<String>,
    ) -> InboundMessage {
        let platform = platform.into();
        let chat_id = chat_id.into();
        InboundMessage {
            id: format!("in-{}", chat_id),
            platform: std::borrow::Cow::Owned(platform),
            msg_type: MessageType::Text,
            text: Some(text.into()),
            media: None,
            sender: Self::sender(format!("u-{chat_id}"), Some("Tester")),
            recipient: None,
            chat_id,
            chat_name: None,
            chat_type: ChatType::Dm,
            guild_id: None,
            thread_id: None,
            root_id: None,
            timestamp: now_millis(),
            command: None,
            callback: None,
            reply_to: None,
            mentions: None,
            mentioned: None,
            metadata: None,
        }
    }

    /// 构造带媒体附件的入站消息。
    pub fn inbound_media(
        platform: impl Into<String>,
        chat_id: impl Into<String>,
        media: MediaAttachment,
    ) -> InboundMessage {
        let mut message = Self::inbound_text(platform, chat_id, "");
        message.msg_type = MessageType::Image;
        message.text = None;
        message.media = Some(vec![media]);
        message
    }

    /// 构造按钮回调入站消息。
    pub fn inbound_callback(
        platform: impl Into<String>,
        chat_id: impl Into<String>,
        data: CallbackData,
    ) -> InboundMessage {
        let mut message = Self::inbound_text(platform, chat_id, "");
        message.msg_type = MessageType::Interactive;
        message.text = None;
        message.callback = Some(data);
        message
    }

    /// 构造「收到回调」事件（`callback.received`），供宿主侧断言。
    pub fn callback_event(
        platform: impl Into<String>,
        chat_id: impl Into<String>,
        data: &CallbackData,
    ) -> GatewayEvent {
        let platform = platform.into();
        let chat_id = chat_id.into();
        GatewayEvent::new(
            event_types::CALLBACK_RECEIVED,
            platform.clone(),
            serde_json::json!({
                "id": format!("cb-{chat_id}"),
                "platform": platform,
                "chat_id": chat_id,
                "user_id": format!("u-{chat_id}"),
                "data": data.data,
                "message_id": data.message_id,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AdapterRuntimeConfig, AdapterState, AdapterStatusSummary, BotInfo, Capability,
        CapabilityName, ConnectErrorKind, ConnectResult, HealthReport, HealthStatus, InitResult,
    };
    use easybot_core::types::message::{MediaType, OutboundMessage, ParseMode, SendResult};

    /// 最小可用插件：把收到的文本原样发布为 `message.inbound` 事件。
    #[derive(Default)]
    struct EchoAdapter {
        bus: Option<Arc<EventBus>>,
        connected: bool,
    }

    #[async_trait::async_trait]
    impl PlatformAdapter for EchoAdapter {
        fn platform_name(&self) -> &str {
            "echo"
        }
        fn display_name(&self) -> &str {
            "Echo"
        }
        fn capabilities(&self) -> &[Capability] {
            &[Capability {
                name: CapabilityName::Text,
                supported: true,
                limits: None,
            }]
        }
        fn set_event_bus(&mut self, bus: Arc<EventBus>) {
            self.bus = Some(bus);
        }
        async fn init(&mut self, _config: AdapterConfig) -> Result<InitResult, GatewayError> {
            Ok(InitResult {
                ok: true,
                error: None,
            })
        }
        async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
            self.connected = true;
            Ok(ConnectResult::ok(Some(BotInfo {
                name: "echo".into(),
                username: Some("echo_bot".into()),
                id: "echo-1".into(),
            })))
        }
        async fn disconnect(&mut self) -> Result<(), GatewayError> {
            self.connected = false;
            Ok(())
        }
        fn state(&self) -> AdapterState {
            if self.connected {
                AdapterState::Connected
            } else {
                AdapterState::Created
            }
        }
        async fn health(&self) -> HealthReport {
            HealthReport {
                status: HealthStatus::Healthy,
                connected: self.connected,
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
            let message =
                PluginTestHost::inbound_text("echo", &params.chat_id, &params.message.text);
            if let Some(bus) = &self.bus {
                bus.publish(GatewayEvent::new(
                    event_types::MESSAGE_INBOUND,
                    "echo",
                    serde_json::to_value(message).unwrap_or_default(),
                ));
            }
            Ok(SendResult::ok(format!("echo-{}", params.chat_id)))
        }
        async fn get_chat_info(
            &self,
            chat_id: &str,
        ) -> Result<easybot_core::types::message::ChatInfo, GatewayError> {
            Ok(easybot_core::types::message::ChatInfo {
                chat_id: chat_id.to_string(),
                name: Some("Test Chat".into()),
                chat_type: ChatType::Dm,
                member_count: None,
            })
        }
        fn runtime_config(&self) -> AdapterRuntimeConfig {
            AdapterRuntimeConfig {
                enabled: true,
                token_configured: false,
                extra: serde_json::Value::default(),
            }
        }
        fn status_summary(&self) -> AdapterStatusSummary {
            AdapterStatusSummary {
                platform: "echo".into(),
                display_name: "Echo".into(),
                state: self.state(),
                connected: self.connected,
                health: Some(HealthStatus::Healthy),
                last_error: None,
                uptime: Some(0),
                messages_in: 0,
                messages_out: 0,
            }
        }
    }

    #[tokio::test]
    async fn lifecycle_and_send_roundtrip() {
        let host = PluginTestHost::new();
        let mut adapter = EchoAdapter::default();
        host.attach(&mut adapter);
        host.init(&mut adapter, PluginTestHost::config())
            .await
            .unwrap();

        let mut rx = host.subscribe(event_types::MESSAGE_INBOUND);
        let conn = host.connect(&mut adapter).await.unwrap();
        assert!(conn.ok);
        assert_eq!(
            conn.bot_info.as_ref().map(|b| b.username.as_deref()),
            Some(Some("echo_bot"))
        );

        let result = host
            .send(
                &mut adapter,
                SendTextParams {
                    chat_id: "c1".into(),
                    message: OutboundMessage {
                        text: "hi".into(),
                        parse_mode: ParseMode::None,
                    },
                    reply_to: None,
                    metadata: None,
                },
            )
            .await
            .unwrap();
        assert!(result.success);

        // 发送请求被记录
        assert_eq!(host.send_log().len(), 1);
        assert_eq!(host.send_log()[0].message.text, "hi");

        // 插件发布了 message.inbound 事件，且文本回显
        let event = recv_event(&mut rx, Duration::from_secs(1))
            .await
            .expect("no event");
        assert_eq!(event.event_type, event_types::MESSAGE_INBOUND);
        assert_eq!(event.source, "echo");
        assert_eq!(event.data["text"], "hi");

        host.disconnect(&mut adapter).await.unwrap();
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[tokio::test]
    async fn constructors_build_well_formed_messages() {
        let text = PluginTestHost::inbound_text("echo", "c1", "hello");
        assert_eq!(text.msg_type, MessageType::Text);
        assert_eq!(text.text.as_deref(), Some("hello"));
        assert_eq!(text.chat_id, "c1");
        assert_eq!(text.platform, "echo");

        let media = PluginTestHost::inbound_media(
            "echo",
            "c2",
            MediaAttachment {
                media_type: MediaType::Image,
                url: Some("https://example.com/a.png".into()),
                data: None,
                mime_type: "image/png".into(),
                filename: None,
                caption: None,
                thumbnail_url: None,
                file_size: None,
                duration: None,
            },
        );
        assert_eq!(media.msg_type, MessageType::Image);
        assert!(
            media
                .media
                .as_ref()
                .is_some_and(|m| m[0].url.as_deref() == Some("https://example.com/a.png"))
        );

        let data = CallbackData {
            data: "btn-1".into(),
            message_id: "m1".into(),
        };
        let cb = PluginTestHost::inbound_callback("echo", "c3", data.clone());
        assert_eq!(cb.msg_type, MessageType::Interactive);
        assert_eq!(cb.callback.as_ref().map(|c| c.data.as_str()), Some("btn-1"));

        let event = PluginTestHost::callback_event("echo", "c3", &data);
        assert_eq!(event.event_type, event_types::CALLBACK_RECEIVED);
        assert_eq!(event.data["data"], "btn-1");
    }

    #[tokio::test]
    async fn connect_failure_kind_passthrough() {
        let host = PluginTestHost::new();
        let mut adapter = ConnectFailAdapter;
        host.attach(&mut adapter);
        let result = host.connect(&mut adapter).await.unwrap();
        assert!(!result.ok);
        assert_eq!(result.error_kind, Some(ConnectErrorKind::Transient));
    }

    /// 固定返回瞬态连接失败的适配器（验证 ConnectResult 分类穿透）。
    struct ConnectFailAdapter;

    #[async_trait::async_trait]
    impl PlatformAdapter for ConnectFailAdapter {
        fn platform_name(&self) -> &str {
            "fail"
        }
        fn display_name(&self) -> &str {
            "Fail"
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }
        async fn init(&mut self, _c: AdapterConfig) -> Result<InitResult, GatewayError> {
            Ok(InitResult {
                ok: true,
                error: None,
            })
        }
        async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
            Ok(ConnectResult::failed(
                "network timeout".into(),
                Some(ConnectErrorKind::Transient),
            ))
        }
        async fn disconnect(&mut self) -> Result<(), GatewayError> {
            Ok(())
        }
        fn state(&self) -> AdapterState {
            AdapterState::Failed
        }
        async fn health(&self) -> HealthReport {
            HealthReport {
                status: HealthStatus::Down,
                connected: false,
                last_connected_at: None,
                last_error_at: None,
                last_error: Some("network timeout".into()),
                messages_in: 0,
                messages_out: 0,
                errors: 1,
                uptime: None,
            }
        }
        async fn send(&self, _p: SendTextParams) -> Result<SendResult, GatewayError> {
            Ok(SendResult::ok("noop".into()))
        }
        async fn get_chat_info(
            &self,
            chat_id: &str,
        ) -> Result<easybot_core::types::message::ChatInfo, GatewayError> {
            Ok(easybot_core::types::message::ChatInfo {
                chat_id: chat_id.to_string(),
                name: None,
                chat_type: ChatType::Dm,
                member_count: None,
            })
        }
        fn runtime_config(&self) -> AdapterRuntimeConfig {
            AdapterRuntimeConfig {
                enabled: true,
                token_configured: false,
                extra: serde_json::Value::default(),
            }
        }
        fn status_summary(&self) -> AdapterStatusSummary {
            AdapterStatusSummary {
                platform: "fail".into(),
                display_name: "Fail".into(),
                state: self.state(),
                connected: false,
                health: Some(HealthStatus::Down),
                last_error: Some("network timeout".into()),
                uptime: Some(0),
                messages_in: 0,
                messages_out: 0,
            }
        }
    }
}
