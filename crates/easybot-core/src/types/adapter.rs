//! 平台适配器接口
//!
//! 定义 PlatformAdapter trait，所有 IM 平台连接器必须实现此接口。
//! 包含适配器生命周期、能力声明、消息发送、健康检查等。

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::types::error::GatewayError;
use crate::types::message::*;
use async_trait::async_trait;
use utoipa::ToSchema;

/// 适配器能力名称
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ToSchema, PartialEq)]
pub enum CapabilityName {
    Text,
    Image,
    Audio,
    Video,
    Document,
    Interactive,
    Streaming,
    Voice,
    Markdown,
    Html,
    CodeBlock,
    Thread,
    Topic,
    Group,
    ChatList,
    MessageEdit,
    MessageDelete,
    TypingIndicator,
}

/// 适配器能力声明
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ToSchema)]
pub struct Capability {
    pub name: CapabilityName,
    pub supported: bool,
    pub limits: Option<CapabilityLimits>,
}

/// 能力限制
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, ToSchema)]
pub struct CapabilityLimits {
    pub max_text_length: Option<usize>,
    pub max_file_size: Option<u64>,
    pub max_buttons: Option<usize>,
}

/// 适配器状态
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ToSchema, PartialEq)]
pub enum AdapterState {
    Created,
    Starting,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
    Stopped,
}

/// 健康状态
#[derive(Debug, Clone, serde::Serialize, ToSchema, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Down,
}

/// 健康报告
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
pub struct HealthReport {
    pub status: HealthStatus,
    pub connected: bool,
    pub last_connected_at: Option<i64>,
    pub last_error_at: Option<i64>,
    pub last_error: Option<String>,
    pub messages_in: u64,
    pub messages_out: u64,
    pub errors: u64,
    pub uptime: Option<u64>,
}

/// 连接结果
#[derive(Debug)]
pub struct ConnectResult {
    pub ok: bool,
    pub error: Option<String>,
    pub bot_info: Option<BotInfo>,
}

/// 初始化结果
#[derive(Debug)]
pub struct InitResult {
    pub ok: bool,
    pub error: Option<String>,
}

/// 机器人基本信息
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
pub struct BotInfo {
    pub name: String,
    pub username: Option<String>,
    pub id: String,
}

/// 适配器配置（来源自配置文件）
///
/// `enabled` 支持三态：
/// - `None`（默认）：自动检测 — 凭据环境变量已设置则启用
/// - `Some(true)`：强制启用，即使未检测到凭据
/// - `Some(false)`：强制禁用，即使凭据已设置
#[derive(Clone, serde::Serialize, serde::Deserialize, ToSchema)]
pub struct AdapterConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    pub token: Option<String>,
    pub api_key: Option<String>,
    /// 自定义 API 基础 URL（用于测试或代理场景，默认使用平台官方 API）
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub extra: serde_json::Value,
}

// SECURITY: Manual Debug impl that redacts credential fields
impl std::fmt::Debug for AdapterConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterConfig")
            .field("enabled", &self.enabled)
            .field(
                "token",
                &if self.token.is_some() {
                    Some("***REDACTED***")
                } else {
                    None
                },
            )
            .field(
                "api_key",
                &if self.api_key.is_some() {
                    Some("***REDACTED***")
                } else {
                    None
                },
            )
            .field("base_url", &self.base_url)
            .field("extra", &self.extra)
            .finish()
    }
}

impl AdapterConfig {
    /// 创建一个仅指定 enabled 状态的最小配置
    pub fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled: Some(enabled),
            token: None,
            api_key: None,
            base_url: None,
            extra: serde_json::Value::default(),
        }
    }
}

/// Default liveness threshold: if the background task hasn't emitted a
/// heartbeat in 120 seconds, the adapter is considered Degraded.
pub const DEFAULT_LIVENESS_THRESHOLD_MS: i64 = 120_000;

/// Utility for tracking background task liveness and message-stream health.
///
/// Maintains two independent signals:
///
/// | Signal | Method | Updated on | Consumer | Purpose |
/// |---|---|---|---|---|
/// | **Liveness** | [`beat`](Self::beat) | Every iteration (success, error, retry) | Health monitor | "Is the task alive?" |
/// | **Stream health** | [`beat_success`](Self::beat_success) | Only on successful message receipt | `health_status()` → frontend | "Is the message stream flowing?" |
///
/// This separation prevents the health monitor from killing a recovering
/// adapter (liveness stays fresh during retries) while still letting the
/// admin panel show `Degraded` when no messages are actually flowing
/// (stream-health goes stale during network outages).
///
/// Thread-safe and cheap to clone (wraps `Arc<AtomicI64>` internally).
#[derive(Clone, Debug)]
pub struct Heartbeat {
    last_beat_ms: Arc<AtomicI64>,
    /// Timestamp of the last *successful* message receipt / poll.  Used by
    /// `health_status()` to report transport-level health to the frontend.
    last_success_at_ms: Arc<AtomicI64>,
    started_at_ms: Arc<AtomicI64>,
    last_connected_at: Arc<AtomicI64>,
    last_error_at: Arc<AtomicI64>,
    last_error: Arc<std::sync::Mutex<Option<String>>>,
}

impl Heartbeat {
    /// Create a new heartbeat tracker, initialised to "now".
    pub fn new() -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            last_beat_ms: Arc::new(AtomicI64::new(now)),
            last_success_at_ms: Arc::new(AtomicI64::new(now)),
            started_at_ms: Arc::new(AtomicI64::new(now)),
            last_connected_at: Arc::new(AtomicI64::new(0)),
            last_error_at: Arc::new(AtomicI64::new(0)),
            last_error: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Record a **liveness** beat.  Call this from the background task on
    /// every iteration — success, error, or retry.  Keeps the health
    /// monitor from killing a recovering adapter.
    ///
    /// For stream-health tracking use [`beat_success`](Self::beat_success).
    pub fn beat(&self) {
        self.last_beat_ms
            .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
    }

    /// Record a **message-stream** beat.  Call this ONLY when the adapter
    /// successfully receives a message / completes a poll iteration without
    /// errors.  This is what `health_status()` uses to decide `Healthy` vs
    /// `Degraded` for the frontend.
    pub fn beat_success(&self) {
        let now = chrono::Utc::now().timestamp_millis();
        self.last_success_at_ms.store(now, Ordering::Relaxed);
        // A successful receive also implies liveness — update liveness beat
        // so callers don't need to call both methods.
        self.last_beat_ms.store(now, Ordering::Relaxed);
    }

    /// How many milliseconds have elapsed since the last liveness beat.
    /// Used by the health monitor to detect dead background tasks.
    pub fn age_ms(&self) -> i64 {
        let now = chrono::Utc::now().timestamp_millis();
        now.saturating_sub(self.last_beat_ms.load(Ordering::Relaxed))
    }

    /// How many milliseconds have elapsed since the last *successful*
    /// message receipt.  Used by `health_status()` to report transport
    /// health to the frontend.
    pub fn last_success_age_ms(&self) -> i64 {
        let now = chrono::Utc::now().timestamp_millis();
        now.saturating_sub(self.last_success_at_ms.load(Ordering::Relaxed))
    }

    /// Convenience: is the liveness heartbeat within a given threshold?
    pub fn is_fresh(&self, threshold_ms: i64) -> bool {
        self.age_ms() <= threshold_ms
    }

    /// Uptime in seconds since this Heartbeat was created.
    pub fn uptime_secs(&self) -> u64 {
        let now = chrono::Utc::now().timestamp_millis();
        let elapsed = now.saturating_sub(self.started_at_ms.load(Ordering::Relaxed));
        (elapsed as u64) / 1000
    }

    /// Record a successful connection (sets last_connected_at to now).
    pub fn record_connection(&self) {
        self.last_connected_at
            .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
    }

    /// Record an error (sets last_error_at and last_error).
    pub fn record_error(&self, error: impl Into<String>) {
        self.last_error_at
            .store(chrono::Utc::now().timestamp_millis(), Ordering::Relaxed);
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = Some(error.into());
        }
    }

    /// Get last successful connection timestamp (millis since epoch), if any.
    pub fn last_connected_at(&self) -> Option<i64> {
        let val = self.last_connected_at.load(Ordering::Relaxed);
        if val > 0 { Some(val) } else { None }
    }

    /// Get last error timestamp (millis since epoch), if any.
    pub fn last_error_at(&self) -> Option<i64> {
        let val = self.last_error_at.load(Ordering::Relaxed);
        if val > 0 { Some(val) } else { None }
    }

    /// Get last error message, if any.
    pub fn last_error_str(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

/// 适配器运行时配置状态
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
pub struct AdapterRuntimeConfig {
    pub enabled: bool,
    pub token_configured: bool,
    pub extra: serde_json::Value,
}

/// 适配器状态摘要（对外 API 使用）
#[derive(Debug, Clone, serde::Serialize, ToSchema)]
pub struct AdapterStatusSummary {
    pub platform: String,
    pub display_name: String,
    pub state: AdapterState,
    pub connected: bool,
    pub health: Option<HealthStatus>,
    pub last_error: Option<String>,
    pub uptime: Option<u64>,
    pub messages_in: u64,
    pub messages_out: u64,
}

/// 平台适配器 Trait
///
/// 所有 IM 平台连接器必须实现此 trait。
/// 所有方法均为异步，使用 tokio 运行时。
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    // ── 元数据 ──

    /// 平台唯一标识，如 "telegram"
    fn platform_name(&self) -> &str;

    /// 人类可读的平台显示名
    fn display_name(&self) -> &str;

    /// 能力列表
    fn capabilities(&self) -> &[Capability];

    /// 设置事件总线（在 init() 前由管理器调用）
    /// 默认实现为空操作；需要发布 IM 消息到总线的适配器应覆盖此方法。
    fn set_event_bus(&mut self, _bus: Arc<crate::bus::EventBus>) {}

    // ── 生命周期 ──

    /// 初始化适配器，检查配置但不建立网络连接
    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError>;

    /// 连接到 IM 平台并开始接收消息
    async fn connect(&mut self) -> Result<ConnectResult, GatewayError>;

    /// 断开连接，清理资源
    async fn disconnect(&mut self) -> Result<(), GatewayError>;

    /// 当前适配器状态
    fn state(&self) -> AdapterState;

    /// 返回是否已连接
    fn is_connected(&self) -> bool {
        self.state() == AdapterState::Connected
    }

    /// 仅重启后台传输任务（WebSocket / 长轮询），不重新鉴权。
    ///
    /// 健康监测器用于瞬态网络故障（WiFi 断连等）的快速恢复。
    /// 与 `disconnect()` + `connect()` 不同，此方法跳过鉴权步骤，
    /// 仅取消并重建后台传输任务。
    ///
    /// # 返回值语义
    /// - `Ok(true)`：传输重启成功，适配器已恢复正常
    /// - `Ok(false)`：适配器不支持轻量重启，健康监测器应回退到完整 stop+start
    /// - `Err(e)`：尝试重启但失败，健康监测器应根据错误类型决定下一步
    ///
    /// 默认返回 `Ok(false)`——有内置重试循环且心跳在错误时也更新的适配器无需覆盖。
    async fn retry_transport(&mut self) -> Result<bool, GatewayError> {
        Ok(false)
    }

    /// Returns the age of the last background liveness heartbeat in milliseconds.
    ///
    /// Returns `None` when the adapter does not support heartbeat tracking
    /// (the default).  Adapters that return `Some` should store a
    /// [`Heartbeat`] and forward to [`Heartbeat::age_ms`].
    ///
    /// This is used by the **health monitor** to decide whether the
    /// background task is still alive.  Adapters should call
    /// [`Heartbeat::beat`] on every iteration (including error/retry paths)
    /// to prevent false-positive kills.
    fn heartbeat_age_ms(&self) -> Option<i64> {
        None
    }

    /// Returns the age of the last *successful* message receipt in
    /// milliseconds.  Unlike [`heartbeat_age_ms`](Self::heartbeat_age_ms),
    /// this is only updated when the adapter actually receives a message
    /// (or completes a successful poll iteration).
    ///
    /// Used by [`health_status`](Self::health_status) to differentiate
    /// "task is alive but message stream is blocked" (e.g. WiFi down)
    /// from "everything is fine".  Falls back to `heartbeat_age_ms()` by
    /// default so existing adapters keep the old behaviour.
    fn heartbeat_success_age_ms(&self) -> Option<i64> {
        self.heartbeat_age_ms()
    }

    /// Compute the canonical health status from adapter state and an optional
    /// message-stream heartbeat.
    ///
    /// The default implementation checks:
    /// - `Connected` + recent successful message receipt => `Healthy`
    /// - `Connected` + stale message stream => `Degraded`
    /// - Anything else => `Down`
    ///
    /// Override this if your adapter needs custom health logic.
    fn health_status(&self) -> HealthStatus {
        if self.state() == AdapterState::Connected {
            // Check task liveness first: if the background task hasn't
            // sent a liveness heartbeat, the adapter is Down regardless
            // of stream health (e.g., WebSocket disconnected, polling loop
            // exited after exhausting internal retries).
            if let Some(age_ms) = self.heartbeat_age_ms()
                && age_ms > DEFAULT_LIVENESS_THRESHOLD_MS
            {
                return HealthStatus::Down;
            }
            // Task is alive — check message-stream health.
            if let Some(age_ms) = self.heartbeat_success_age_ms()
                && age_ms > DEFAULT_LIVENESS_THRESHOLD_MS
            {
                return HealthStatus::Degraded;
            }
            HealthStatus::Healthy
        } else {
            HealthStatus::Down
        }
    }

    /// 健康检查
    async fn health(&self) -> HealthReport;

    // ── 消息发送 ──

    /// 发送文本消息（必须实现）
    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError>;

    /// 发送媒体消息（可选）
    async fn send_media(&self, _params: SendMediaParams) -> Result<SendResult, GatewayError> {
        Err(GatewayError::capability_not_supported("send_media"))
    }

    /// 发送交互式消息（可选）
    async fn send_interactive(
        &self,
        _params: SendInteractiveParams,
    ) -> Result<SendResult, GatewayError> {
        Err(GatewayError::capability_not_supported("send_interactive"))
    }

    /// 发送输入指示器（可选）
    async fn send_typing(&self, _chat_id: &str) -> Result<(), GatewayError> {
        Err(GatewayError::capability_not_supported("send_typing"))
    }

    // ── 流式消息 ──

    /// 流式草稿发送（可选）
    ///
    /// 首次调用时不传 `message_id`，适配器发送初始消息并返回 `message_id`。
    /// 后续调用传入 `message_id` 更新草稿内容，实现流式输出效果。
    /// 流式结束后通过 `edit_message` 或再次调用 `send_draft` 定型最终内容。
    async fn send_draft(&self, _params: SendDraftParams) -> Result<DraftResult, GatewayError> {
        Err(GatewayError::capability_not_supported("send_draft"))
    }

    // ── 消息管理 ──

    /// 编辑消息（可选）
    async fn edit_message(&self, _params: EditMessageParams) -> Result<EditResult, GatewayError> {
        Err(GatewayError::capability_not_supported("edit_message"))
    }

    /// 删除消息（可选）
    async fn delete_message(
        &self,
        _chat_id: &str,
        _message_id: &str,
    ) -> Result<DeleteResult, GatewayError> {
        Err(GatewayError::capability_not_supported("delete_message"))
    }

    // ── 查询 ──

    /// 获取聊天信息
    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError>;

    /// 列出聊天列表（可选）
    async fn list_chats(&self, _filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        Err(GatewayError::capability_not_supported("list_chats"))
    }

    // ── 配置 ──

    /// 返回运行时配置状态
    fn runtime_config(&self) -> AdapterRuntimeConfig;

    // ── 状态 ──

    /// 返回适配器状态摘要（用于管理 API）
    fn status_summary(&self) -> AdapterStatusSummary;

    // ── 会话富化 ──

    /// 富化会话来源信息（可选）
    ///
    /// 会话创建后异步调用，适配器可通过此方法利用平台 API 补充
    /// 用户名、用户角色等 session source 信息。
    /// 默认实现直接返回 None（不进行富化）。
    async fn enrich_source(
        &self,
        _source: &crate::types::session::SessionSource,
    ) -> Option<crate::types::session::SessionSource> {
        None
    }
}

/// 简化能力声明宏
///
/// 统一使用元组语法 `(Name, supported)` 或 `(Name, supported, limits: { key: val })`。
///
/// ```ignore
/// use easybot_core::capabilities;
///
/// let caps = capabilities![
///     (Text, true),
///     (Image, true),
///     (Interactive, false),
///     (Document, true, limits: { max_file_size: 50 * 1024 * 1024 }),
/// ];
/// ```
#[macro_export]
macro_rules! capabilities {
    // (Name, supported, limits: { key: val, ... })
    ($(($name:ident, $supported:expr, limits: { $($limit_key:ident: $limit_val:expr),* $(,)? })),* $(,)?) => {{
        let mut caps = Vec::new();
        $(
            let mut limits = easybot_core::types::adapter::CapabilityLimits::default();
            $(limits.$limit_key = Some($limit_val);)*
            caps.push(easybot_core::types::adapter::Capability {
                name: easybot_core::types::adapter::CapabilityName::$name,
                supported: $supported,
                limits: Some(limits),
            });
        )*
        caps
    }};
    // (Name, supported)
    ($(($name:ident, $supported:expr)),* $(,)?) => {{
        let mut caps = Vec::new();
        $(
            caps.push(easybot_core::types::adapter::Capability {
                name: easybot_core::types::adapter::CapabilityName::$name,
                supported: $supported,
                limits: None,
            });
        )*
        caps
    }};
}
