//! 配置模型
//!
//! 定义网关配置的结构体，与 YAML 配置文件对应。
//! 支持环境变量引用（${VAR_NAME}）和分层配置合并。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use std::collections::HashMap;
use crate::types::adapter::AdapterConfig;

/// 网关主配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub tls: TlsConfig,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_port() -> u16 {
    8080
}

/// TLS 配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TlsConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub cert_file: String,

    #[serde(default)]
    pub key_file: String,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cert_file: String::new(),
            key_file: String::new(),
        }
    }
}

/// API 配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApiConfig {
    #[serde(default = "default_api_base_path")]
    pub base_path: String,

    #[serde(default)]
    pub websocket: WebSocketConfig,
}

fn default_api_base_path() -> String {
    "/api/v1".to_string()
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_path: default_api_base_path(),
            websocket: WebSocketConfig::default(),
        }
    }
}

/// WebSocket 配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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

/// 存储配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StorageConfig {
    #[serde(default = "default_storage_type")]
    pub storage_type: String,

    #[serde(default)]
    pub path: String,

    #[serde(default)]
    pub connection_string: String,
}

fn default_storage_type() -> String {
    "sqlite".to_string()
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_type: "sqlite".to_string(),
            path: String::new(),
            connection_string: String::new(),
        }
    }
}

/// 日志配置
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
        }
    }
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            api: ApiConfig::default(),
            storage: StorageConfig::default(),
            logging: LoggingConfig::default(),
            adapters: HashMap::new(),
            webhooks: Vec::new(),
        }
    }
}
