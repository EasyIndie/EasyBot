//! 配置模型
#![allow(missing_docs)]
//!
//! 定义网关配置的结构体，与 YAML 配置文件对应。
//! 支持环境变量引用（${VAR_NAME}）和分层配置合并。

use crate::types::adapter::AdapterConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// 网关主配置
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayConfig {
    /// 服务器配置
    #[serde(default)]
    pub server: ServerConfig,

    /// 外部 API 配置
    #[serde(default)]
    pub api: ApiConfig,

    /// 存储配置
    #[serde(default)]
    pub storage: StorageConfig,

    /// 日志配置
    #[serde(default)]
    pub logging: LoggingConfig,

    /// 适配器配置
    #[serde(default)]
    pub adapters: HashMap<String, AdapterConfig>,

    /// Webhook 配置
    #[serde(default)]
    pub webhooks: Vec<WebhookConfig>,
}

/// 服务器配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub tls: TlsConfig,

    /// CORS 允许的 Origins（生产环境白名单，debug 模式忽略）
    #[serde(default = "default_cors_origins")]
    pub cors_allowed_origins: Vec<String>,
}

fn default_cors_origins() -> Vec<String> {
    vec!["http://localhost:3000".into()]
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    8080
}

/// TLS 配置
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TlsConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub cert_file: String,

    #[serde(default)]
    pub key_file: String,
}

/// Prometheus 指标配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MetricsConfig {
    #[serde(default = "default_metrics_enabled")]
    pub enabled: bool,
    /// 指标端点路径
    #[serde(default = "default_metrics_path")]
    pub path: String,
}

fn default_metrics_enabled() -> bool {
    true
}
fn default_metrics_path() -> String {
    "/metrics".to_string()
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "/metrics".to_string(),
        }
    }
}

/// 速率限制配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitConfig {
    #[serde(default = "default_rl_enabled")]
    pub enabled: bool,
    /// 每分钟允许的请求数
    #[serde(default = "default_rl_requests")]
    pub requests_per_minute: u64,
    /// 突发大小（1 秒内允许的峰值请求）
    #[serde(default = "default_rl_burst")]
    pub burst_size: u32,
}

fn default_rl_enabled() -> bool {
    true
}
fn default_rl_requests() -> u64 {
    60
}
fn default_rl_burst() -> u32 {
    10
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_minute: 60,
            burst_size: 10,
        }
    }
}

/// API 配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiConfig {
    #[serde(default = "default_api_base_path")]
    pub base_path: String,

    #[serde(default)]
    pub websocket: WebSocketConfig,

    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    #[serde(default)]
    pub metrics: MetricsConfig,
}

fn default_api_base_path() -> String {
    "/api/v1".to_string()
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_path: default_api_base_path(),
            websocket: WebSocketConfig::default(),
            rate_limit: RateLimitConfig::default(),
            metrics: MetricsConfig::default(),
        }
    }
}

/// WebSocket 配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketConfig {
    #[serde(default = "default_ws_enabled")]
    pub enabled: bool,

    #[serde(default = "default_ws_max_clients")]
    pub max_clients: usize,

    #[serde(default = "default_ws_heartbeat")]
    pub heartbeat_interval_secs: u64,
}

fn default_ws_enabled() -> bool {
    true
}
fn default_ws_max_clients() -> usize {
    1000
}
fn default_ws_heartbeat() -> u64 {
    30
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_clients: 1000,
            heartbeat_interval_secs: 30,
        }
    }
}

/// TTL 保留策略配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RetentionConfig {
    /// 消息保留天数（默认 90）
    #[serde(default = "default_message_ttl_days")]
    pub message_ttl_days: u64,
    /// 会话保留天数（默认 365）
    #[serde(default = "default_session_ttl_days")]
    pub session_ttl_days: u64,
    /// 清理间隔秒数（默认 3600 = 1 小时）
    #[serde(default = "default_cleanup_interval_secs")]
    pub cleanup_interval_secs: u64,
}

fn default_message_ttl_days() -> u64 {
    90
}
fn default_session_ttl_days() -> u64 {
    365
}
fn default_cleanup_interval_secs() -> u64 {
    3600
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            message_ttl_days: 90,
            session_ttl_days: 365,
            cleanup_interval_secs: 3600,
        }
    }
}

/// 存储配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StorageConfig {
    #[serde(default = "default_storage_type", alias = "type")]
    pub storage_type: String,

    #[serde(default)]
    pub path: String,

    #[serde(default)]
    pub connection_string: String,

    /// PostgreSQL 连接池大小（仅 postgres 类型有效）
    #[serde(default = "default_pool_size")]
    pub pool_size: u32,

    /// PostgreSQL SSL 模式（仅 postgres 类型有效）
    #[serde(default = "default_ssl_mode")]
    pub ssl_mode: String,

    /// TTL 保留策略
    #[serde(default)]
    pub retention: RetentionConfig,
}

fn default_storage_type() -> String {
    "sqlite".to_string()
}
fn default_pool_size() -> u32 {
    10
}
fn default_ssl_mode() -> String {
    "prefer".to_string()
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_type: "sqlite".to_string(),
            path: String::new(),
            connection_string: String::new(),
            pool_size: 10,
            ssl_mode: "prefer".to_string(),
            retention: RetentionConfig::default(),
        }
    }
}

/// 日志配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,

    #[serde(default = "default_log_format")]
    pub format: String,

    #[serde(default = "default_log_output")]
    pub output: String,
}

fn default_log_level() -> String {
    "info".to_string()
}
fn default_log_format() -> String {
    "text".to_string()
}
fn default_log_output() -> String {
    "stdout".to_string()
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "text".to_string(),
            output: "stdout".to_string(),
        }
    }
}

/// Webhook 配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WebhookConfig {
    pub name: String,
    pub url: String,
    pub secret: Option<String>,
    pub events: Vec<String>,
    pub platforms: Option<Vec<String>>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            tls: TlsConfig::default(),
            cors_allowed_origins: default_cors_origins(),
        }
    }
}
