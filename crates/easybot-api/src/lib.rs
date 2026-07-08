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
use easybot_core::types::event::event_types;
use std::sync::Arc;
use tokio::sync::{Semaphore, broadcast};

pub mod config_manager;
pub mod log_collector;
pub mod metrics;
pub mod middleware;
pub mod openapi;
pub mod response;
pub mod routes;
pub mod server;

/// 预序列化的 WebSocket 事件（一次序列化，广播给所有 WS 客户端）
#[derive(Clone)]
pub struct WsSerializedEvent {
    pub event_type: String,
    pub data: Arc<String>, // pre-serialized JSON data
    pub timestamp: i64,
}

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
    /// 预序列化 WS 事件广播器（一次序列化给所有客户端，避免 N 次独立序列化）
    pub ws_event_tx: broadcast::Sender<WsSerializedEvent>,
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
        let raw_payload_enabled = config_arc.api.raw_payload_enabled;

        // 启动 WS 预序列化广播器：所有 WS 客户端共享同一序列化结果
        let (ws_event_tx, _) = broadcast::channel(256);
        let ws_tx = ws_event_tx.clone();
        let ws_eb = event_bus.clone();
        tokio::spawn(async move {
            use futures::StreamExt;
            let mut stream = ws_eb.subscribe_many(event_types::all());
            while let Some(event) = stream.next().await {
                // metadata 处理（与 ws.rs 保持一致）
                let mut event_data = event.data;
                if !raw_payload_enabled {
                    if let Some(obj) = event_data.as_object_mut() {
                        obj.remove("metadata");
                    }
                } else if let Some(obj) = event_data.as_object_mut()
                    && let Some(serde_json::Value::String(s)) = obj.get("metadata")
                    && let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s)
                {
                    obj["metadata"] = parsed;
                }
                // 序列化 data 部分一次，广播给所有 WS 客户端
                let data_json = serde_json::to_string(&event_data).unwrap_or_default();
                let serialized = WsSerializedEvent {
                    event_type: event.event_type,
                    data: Arc::new(data_json),
                    timestamp: event.timestamp,
                };
                let _ = ws_tx.send(serialized);
            }
        });

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
            ws_event_tx,
            started_at: std::time::Instant::now(),
            log_collector,
            dev_api_key,
            admin_password,
        }
    }
}
