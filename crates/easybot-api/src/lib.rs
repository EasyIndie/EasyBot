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
    pub config: Arc<GatewayConfig>,
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
    ) -> Self {
        Self {
            event_bus,
            adapter_manager,
            session_manager,
            message_store,
            auth_manager,
            config: Arc::new(config),
        }
    }
}
