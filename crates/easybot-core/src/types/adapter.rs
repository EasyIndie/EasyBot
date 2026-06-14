//! 平台适配器接口
//!
//! 定义 PlatformAdapter trait，所有 IM 平台连接器必须实现此接口。
//! 包含适配器生命周期、能力声明、消息发送、健康检查等。

use async_trait::async_trait;
use crate::types::message::*;
use crate::types::error::GatewayError;

/// 适配器能力名称
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Capability {
    pub name: CapabilityName,
    pub supported: bool,
    pub limits: Option<CapabilityLimits>,
}

/// 能力限制
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CapabilityLimits {
    pub max_text_length: Option<usize>,
    pub max_file_size: Option<u64>,
    pub max_buttons: Option<usize>,
}

/// 适配器状态
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum AdapterState {
    Created,
    Starting,
    Connected,
    Reconnecting,
    Failed,
    Stopped,
}

/// 健康状态
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Down,
}

/// 健康报告
#[derive(Debug, Clone, serde::Serialize)]
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
#[derive(Debug, Clone, serde::Serialize)]
pub struct BotInfo {
    pub name: String,
    pub username: Option<String>,
    pub id: String,
}

/// 适配器配置（来源自配置文件）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AdapterConfig {
    #[serde(default)]
    pub enabled: bool,
    pub token: Option<String>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub extra: serde_json::Value,
}

/// 适配器运行时配置状态
#[derive(Debug, Clone, serde::Serialize)]
pub struct AdapterRuntimeConfig {
    pub enabled: bool,
    pub token_configured: bool,
    pub extra: serde_json::Value,
}

/// 适配器状态摘要（对外 API 使用）
#[derive(Debug, Clone, serde::Serialize)]
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

    // ── 消息管理 ──

    /// 编辑消息（可选）
    async fn edit_message(
        &self,
        _params: EditMessageParams,
    ) -> Result<EditResult, GatewayError> {
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
    async fn list_chats(
        &self,
        _filter: Option<ChatFilter>,
    ) -> Result<Vec<ChatInfo>, GatewayError> {
        Err(GatewayError::capability_not_supported("list_chats"))
    }

    // ── 配置 ──

    /// 返回运行时配置状态
    fn runtime_config(&self) -> AdapterRuntimeConfig;

    // ── 状态 ──

    /// 返回适配器状态摘要（用于管理 API）
    fn status_summary(&self) -> AdapterStatusSummary;
}
