#![allow(missing_docs)]

//! EasyBot API 服务层
//!
//! 提供 HTTP REST API 和 WebSocket 实时推送。
//! 基于 axum 框架实现。

use crate::config_manager::ConfigManager;
use easybot_core::adapter::AdapterManager;
use easybot_core::auth::ApiKeyManager;
use easybot_core::bus::EventBus;
use easybot_core::session::SessionManager;
use easybot_core::storage::MessageStore;
use easybot_core::types::config::GatewayConfig;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub mod config_manager;
pub mod log_collector;
pub mod metrics;
pub mod middleware;
pub mod openapi;
pub mod response;
pub mod routes;
pub mod server;

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    pub event_bus: Arc<EventBus>,
    pub adapter_manager: Arc<AdapterManager>,
    pub session_manager: Arc<SessionManager>,
    pub message_store: Arc<dyn MessageStore>,
    pub auth_manager: Arc<ApiKeyManager>,
    /// 当前配置快照（热重载时通过 config_manager 更新）
    pub config: Arc<GatewayConfig>,
    /// 配置管理器（支持原子替换和文件监听）
    pub config_manager: ConfigManager,
    pub metrics: Option<Arc<metrics::MetricsRegistry>>,
    /// WebSocket 并发连接数信号量（基于 config.api.websocket.max_clients）
    pub ws_semaphore: Arc<Semaphore>,
    /// 进程启动时间
    pub started_at: std::time::Instant,
    /// 内存日志收集器（供管理后台日志查看使用）
    pub log_collector: Arc<log_collector::LogCollector>,
    /// 原始 API Key（供管理后台登录后返回给前端）
    pub dev_api_key: Option<String>,
    /// 管理后台登录密码
    pub admin_password: String,
}

impl AppState {
    /// 将序列化后的配置 JSON 与运行时实际值 reconcile
    ///
    /// config_manager 中保存的 GatewayConfig 反映了文件配置，
    /// 但某些字段在 main.rs 初始化时会被运行时值覆盖（如 admin_password 取 env var）。
    /// 此方法将这些运行时覆盖值写回 JSON，确保 API 返回的配置始终反映实际运行值。
    pub fn reconcile_config_json(&self, mut val: serde_json::Value) -> serde_json::Value {
        // 管理后台密码：使用 state.admin_password（已考虑 EASYBOT_ADMIN_PASSWORD 环境变量）
        if let Some(obj) = val.as_object_mut()
            && let Some(server) = obj.get_mut("server")
            && let Some(server_obj) = server.as_object_mut()
        {
            server_obj.insert(
                "admin_password".to_string(),
                serde_json::Value::String(self.admin_password.clone()),
            );
        }
        val
    }

    /// 创建新的应用状态
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_bus: Arc<EventBus>,
        adapter_manager: Arc<AdapterManager>,
        session_manager: Arc<SessionManager>,
        message_store: Arc<dyn MessageStore>,
        auth_manager: Arc<ApiKeyManager>,
        config: GatewayConfig,
        config_manager: ConfigManager,
        metrics: Option<Arc<metrics::MetricsRegistry>>,
        log_collector: Arc<log_collector::LogCollector>,
        dev_api_key: Option<String>,
        admin_password: String,
    ) -> Self {
        let max_clients = config.api.websocket.max_clients.max(1);
        let config_arc = Arc::new(config);
        Self {
            event_bus,
            adapter_manager,
            session_manager,
            message_store,
            auth_manager,
            config: config_arc,
            config_manager,
            metrics,
            ws_semaphore: Arc::new(Semaphore::new(max_clients)),
            started_at: std::time::Instant::now(),
            log_collector,
            dev_api_key,
            admin_password,
        }
    }
}
