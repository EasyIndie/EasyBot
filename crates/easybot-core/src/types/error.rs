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
            GatewayError::ConfigError(_) | GatewayError::StorageError(_) | GatewayError::Internal(_) => {
                500
            }
        }
    }

    /// 序列化为 API 错误响应格式
    pub fn to_api_error(&self) -> ApiErrorResponse {
        ApiErrorResponse {
            error: ApiErrorDetail {
                code: self.error_code().to_string(),
                message: self.to_string(),
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
