//! 错误类型
//!
//! 定义网关的统⼀错误模型。所有错误都有错误码、HTTP 状态码映射、
//! 和人类可读的消息。遵循架构设计中定义的错误码规范。

use serde::Serialize;
use utoipa::ToSchema;

/// 网关统⼀错误类型
#[derive(Debug, Clone, thiserror::Error)]
pub enum GatewayError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Platform '{0}' not found or not configured")]
    PlatformNotFound(String),

    #[error("Chat '{0}' not found")]
    ChatNotFound(String),

    #[error("Adapter not connected: {0}")]
    AdapterNotConnected(String),

    #[error("Message too long: {current} > {max}")]
    MessageTooLong { current: usize, max: usize },

    #[error("Rate limited by platform")]
    RateLimited { retry_after_ms: u64 },

    #[error("Capability not supported: {0}")]
    CapabilityNotSupported(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl GatewayError {
    /// 构造 "能力不支持" 错误
    pub fn capability_not_supported(name: &str) -> Self {
        GatewayError::CapabilityNotSupported(name.to_string())
    }

    /// 获取标准错误码
    pub fn error_code(&self) -> &'static str {
        match self {
            GatewayError::InvalidRequest(_) => "INVALID_REQUEST",
            GatewayError::PlatformNotFound(_) => "PLATFORM_NOT_FOUND",
            GatewayError::ChatNotFound(_) => "CHAT_NOT_FOUND",
            GatewayError::AdapterNotConnected(_) => "ADAPTER_NOT_CONNECTED",
            GatewayError::MessageTooLong { .. } => "MESSAGE_TOO_LONG",
            GatewayError::RateLimited { .. } => "RATE_LIMITED",
            GatewayError::CapabilityNotSupported(_) => "CAPABILITY_NOT_SUPPORTED",
            GatewayError::AuthFailed(_) => "AUTH_FAILED",
            GatewayError::Unauthorized(_) => "UNAUTHORIZED",
            GatewayError::ConfigError(_) => "CONFIG_ERROR",
            GatewayError::StorageError(_) => "STORAGE_ERROR",
            GatewayError::Internal(_) => "INTERNAL_ERROR",
        }
    }

    /// 获取对应的 HTTP 状态码
    pub fn http_status(&self) -> u16 {
        match self {
            GatewayError::InvalidRequest(_)
            | GatewayError::MessageTooLong { .. }
            | GatewayError::CapabilityNotSupported(_) => 400,
            GatewayError::AuthFailed(_) | GatewayError::Unauthorized(_) => 401,
            GatewayError::PlatformNotFound(_) | GatewayError::ChatNotFound(_) => 404,
            GatewayError::RateLimited { .. } => 429,
            GatewayError::AdapterNotConnected(_) => 503,
            GatewayError::ConfigError(_)
            | GatewayError::StorageError(_)
            | GatewayError::Internal(_) => 500,
        }
    }

    /// 返回对外安全的消息文本
    ///
    /// 内部错误（Internal / StorageError / ConfigError）可能包含文件路径、
    /// SQL 错误等敏感信息，对外返回通用消息。
    /// 其他变体的消息是用户交互中产生的，可安全返回。
    pub fn external_message(&self) -> String {
        match self {
            Self::Internal(_) => "Internal server error".to_string(),
            Self::StorageError(_) => "Storage error".to_string(),
            Self::ConfigError(_) => "Configuration error".to_string(),
            // 以下变体的消息是用户交互中产生的，可安全返回
            Self::InvalidRequest(msg) => format!("Invalid request: {}", msg),
            Self::PlatformNotFound(msg) => format!("Platform '{}' not found", msg),
            Self::ChatNotFound(msg) => format!("Chat '{}' not found", msg),
            Self::AdapterNotConnected(msg) => format!("Adapter not connected: {}", msg),
            Self::MessageTooLong { current, max } => {
                format!("Message too long: {current} > {max}")
            }
            Self::RateLimited { retry_after_ms } => {
                format!("Rate limited by platform, retry after {retry_after_ms}ms")
            }
            Self::CapabilityNotSupported(msg) => format!("Capability not supported: {msg}"),
            Self::AuthFailed(msg) => format!("Authentication failed: {msg}"),
            Self::Unauthorized(msg) => format!("Unauthorized: {msg}"),
        }
    }

    /// 是否为内部错误（不应暴露详情给客户端）
    pub fn is_internal_error(&self) -> bool {
        matches!(
            self,
            Self::Internal(_) | Self::StorageError(_) | Self::ConfigError(_)
        )
    }

    /// 序列化为 API 错误响应格式
    ///
    /// 内部错误的详情不会暴露给客户端，改用通用消息。
    /// 完整错误信息通过 tracing 日志记录。
    pub fn to_api_error(&self) -> ApiErrorResponse {
        let message = self.external_message();

        // 内部错误：将完整信息写入日志，API 响应仅返回通用消息
        if self.is_internal_error() {
            tracing::error!(code = self.error_code(), detail = %self, "内部错误已脱敏返回");
        }

        ApiErrorResponse {
            error: ApiErrorDetail {
                code: self.error_code().to_string(),
                message,
                details: None,
            },
        }
    }
}

/// API 错误响应格式
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApiErrorResponse {
    pub error: ApiErrorDetail,
}

/// API 错误详情
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
