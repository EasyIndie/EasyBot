//! EasyBot API 服务层
//!
//! 提供 HTTP REST API 和 WebSocket 实时推送。
//! 基于 axum 框架实现。

use std::sync::Arc;
use easybot_core::bus::EventBus;
use easybot_core::adapter::AdapterManager;
use easybot_core::session::SessionManager;
use easybot_core::auth::ApiKeyManager;
use easybot_core::storage::MessageStore;
use easybot_core::types::config::GatewayConfig;
use crate::config_manager::ConfigManager;

pub mod config_manager;
pub mod metrics;
pub mod middleware;
pub mod server;
pub mod routes;
pub mod response;
pub mod openapi;

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
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(
        event_bus: Arc<EventBus>,
        adapter_manager: Arc<AdapterManager>,
        session_manager: Arc<SessionManager>,
        message_store: Arc<dyn MessageStore>,
        auth_manager: Arc<ApiKeyManager>,
        config: GatewayConfig,
        config_manager: ConfigManager,
        metrics: Option<Arc<metrics::MetricsRegistry>>,
    ) -> Self {
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
        }
    }
}
