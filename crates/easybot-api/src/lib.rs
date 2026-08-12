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
use easybot_core::types::event::GatewayEvent;
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

async fn publish_outbound_events_once(
    store: &Arc<dyn MessageStore>,
    event_bus: &Arc<EventBus>,
) -> Result<usize, easybot_core::storage::StoreError> {
    let events = store.unpublished_outbound_events(100).await?;
    let count = events.len();
    for event in events {
        let mut data = event.result_json;
        if !data.is_object() {
            data = serde_json::json!({ "result": data });
        }
        if let Some(object) = data.as_object_mut() {
            object.insert(
                "delivery_id".into(),
                serde_json::Value::String(event.delivery_id.clone()),
            );
            object.insert("platform".into(), serde_json::Value::String(event.platform));
            object.insert("chat_id".into(), serde_json::Value::String(event.chat_id));
            object.insert(
                "delivery_state".into(),
                serde_json::to_value(event.state).unwrap_or_default(),
            );
        }
        let receivers = event_bus.publish(GatewayEvent::new(
            event_types::MESSAGE_SENT,
            "delivery.outbox",
            data,
        ));
        if receivers > 0 {
            store
                .mark_outbound_event_published(&event.delivery_id)
                .await?;
        }
    }
    Ok(count)
}

/// 预序列化的 WebSocket 事件（一次序列化，广播给所有 WS 客户端）
#[derive(Clone)]
pub struct WsSerializedEvent {
    pub event_type: String,
    pub data: Arc<String>, // pre-serialized JSON data
    pub timestamp: i64,
    pub platform: Option<String>,
    pub chat_id: Option<String>,
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
    /// 管理后台登录密码
    pub admin_password: String,
    /// 插件管理器（市场安装/管理；plugin-system 特性启用时可用）。
    /// `None` 表示插件系统未启用或尚未注入。
    #[cfg(feature = "plugin-system")]
    pub plugin_manager: Option<Arc<easybot_core::plugin::manager::PluginManager>>,
}

impl AppState {
    /// 创建新的应用状态
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
        admin_password: String,
    ) -> Self {
        let max_clients = config.api.websocket.max_clients.max(1);
        let config_arc = Arc::new(config);
        let raw_payload_enabled = config_arc.api.raw_payload_enabled;

        // 启动 WS 预序列化广播器：所有 WS 客户端共享同一序列化结果
        let (ws_event_tx, _) = broadcast::channel(256);
        let ws_tx = ws_event_tx.clone();
        // Establish subscriptions synchronously before the outbox can publish recovered events.
        let mut stream = event_bus.subscribe_many(event_types::all());
        tokio::spawn(async move {
            use futures::StreamExt;
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
                let platform = event_data
                    .get("platform")
                    .and_then(|value| value.as_str())
                    .map(str::to_ascii_lowercase);
                let chat_id = event_data
                    .get("chat_id")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                let data_json = serde_json::to_string(&event_data).unwrap_or_default();
                let serialized = WsSerializedEvent {
                    event_type: event.event_type,
                    data: Arc::new(data_json),
                    timestamp: event.timestamp,
                    platform,
                    chat_id,
                };
                let _ = ws_tx.send(serialized);
            }
        });

        // Transactional outbox publisher. A crash after delivery finalization but before
        // publication leaves event_published=false, so the next process resumes it.
        let outbox_store = message_store.clone();
        let outbox_bus = event_bus.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
            loop {
                interval.tick().await;
                if let Err(error) = publish_outbound_events_once(&outbox_store, &outbox_bus).await {
                    tracing::error!(%error, "failed to publish or acknowledge outbound event outbox");
                }
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
            admin_password,
            #[cfg(feature = "plugin-system")]
            plugin_manager: None,
        }
    }
}

#[cfg(test)]
mod outbox_tests {
    use super::*;
    use easybot_core::storage::sqlite::{SqliteMessageStore, run_migrations};
    use easybot_core::storage::{OutboundDelivery, OutboundDeliveryState, OutboundEvent};
    use futures::StreamExt;

    #[tokio::test]
    async fn committed_event_is_republished_and_acknowledged_after_restart() {
        let pool = sqlx::SqlitePool::connect(":memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        let concrete = SqliteMessageStore::new(pool);
        let delivery = OutboundDelivery {
            id: "recover-delivery-1".into(),
            actor_id: "customer-1".into(),
            idempotency_key: Some("request-123".into()),
            platform: "telegram".into(),
            chat_id: "chat-1".into(),
            request_json: serde_json::json!({"text": "hello"}),
            created_at: 1,
        };
        concrete.prepare_outbound_delivery(&delivery).await.unwrap();
        concrete
            .finalize_outbound_delivery(
                &delivery.id,
                OutboundDeliveryState::Succeeded,
                &serde_json::json!({"success": true}),
                None,
            )
            .await
            .unwrap();
        let store: Arc<dyn MessageStore> = Arc::new(concrete);
        let bus = Arc::new(EventBus::new());
        let mut events = bus.subscribe_many(&[event_types::MESSAGE_SENT]);

        assert_eq!(publish_outbound_events_once(&store, &bus).await.unwrap(), 1);
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event.source, "delivery.outbox");
        assert_eq!(event.data["delivery_id"], delivery.id);
        assert_eq!(event.data["delivery_state"], "succeeded");
        assert_eq!(publish_outbound_events_once(&store, &bus).await.unwrap(), 0);

        let none: Vec<OutboundEvent> = store.unpublished_outbound_events(10).await.unwrap();
        assert!(none.is_empty());

        drop(events);
        let no_subscriber = OutboundDelivery {
            id: "recover-delivery-no-subscriber".into(),
            actor_id: "customer-1".into(),
            idempotency_key: None,
            platform: "telegram".into(),
            chat_id: "chat-1".into(),
            request_json: serde_json::json!({"text": "retain me"}),
            created_at: 2,
        };
        store
            .prepare_outbound_delivery(&no_subscriber)
            .await
            .unwrap();
        store
            .finalize_outbound_delivery(
                &no_subscriber.id,
                OutboundDeliveryState::Succeeded,
                &serde_json::json!({"success": true}),
                None,
            )
            .await
            .unwrap();
        assert_eq!(publish_outbound_events_once(&store, &bus).await.unwrap(), 1);
        assert_eq!(
            store.unpublished_outbound_events(10).await.unwrap().len(),
            1,
            "an event must remain pending when no subscriber accepted it"
        );
    }
}
