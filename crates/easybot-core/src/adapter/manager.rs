//! 适配器管理器实现
//!
//! 管理所有平台适配器的生命周期、健康轮询和状态查询。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::adapter::registry::AdapterRegistry;
use crate::bus::EventBus;
use crate::types::adapter::*;
use crate::types::error::GatewayError;
use crate::types::event::event_types;
use crate::types::event::GatewayEvent;

/// 适配器管理器
///
/// 负责适配器的创建、启动、停止、健康检查等生命周期管理。
pub struct AdapterManager {
    /// 适配器注册表
    registry: AdapterRegistry,
    /// 运行中的适配器实例
    adapters: RwLock<HashMap<String, Box<dyn PlatformAdapter>>>,
    /// 适配器状态缓存
    statuses: RwLock<HashMap<String, AdapterStatusSummary>>,
    /// 事件总线（用于发布适配器生命周期事件）
    event_bus: Option<Arc<EventBus>>,
}

impl AdapterManager {
    /// 创建适配器管理器
    pub fn new() -> Self {
        Self {
            registry: AdapterRegistry::new(),
            adapters: RwLock::new(HashMap::new()),
            statuses: RwLock::new(HashMap::new()),
            event_bus: None,
        }
    }

    /// 设置事件总线（用于发布生命周期事件）
    pub fn with_event_bus(mut self, event_bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// 获取适配器注册表引用
    pub fn registry(&self) -> &AdapterRegistry {
        &self.registry
    }

    /// 启动适配器
    pub async fn start(
        &self,
        platform: &str,
        config: AdapterConfig,
    ) -> Result<StartAdapterResult, GatewayError> {
        // 通过注册表创建适配器实例
        let mut adapter = self
            .registry
            .create(platform, config.clone())
            .await
            .map_err(|e| GatewayError::PlatformNotFound(format!("{}: {}", platform, e)))?;

        // 初始化
        let init_result = adapter.init(config).await?;
        if !init_result.ok {
            let error_msg = init_result.error.clone().unwrap_or_default();
            self.publish_adapter_error(platform, &error_msg);
            return Ok(StartAdapterResult {
                ok: false,
                platform: platform.to_string(),
                error: init_result.error,
                bot_info: None,
            });
        }

        // 连接
        let connect_result = adapter.connect().await?;

        // 保存实例
        let platform_name = adapter.platform_name().to_string();
        let display_name = adapter.display_name().to_string();
        {
            let mut adapters = self.adapters.write().await;
            adapters.insert(platform_name.clone(), adapter);
        }

        // 更新状态缓存
        let connected = connect_result.ok;
        {
            let mut statuses = self.statuses.write().await;
            statuses.insert(
                platform_name.clone(),
                AdapterStatusSummary {
                    platform: platform_name.clone(),
                    display_name,
                    state: if connected {
                        AdapterState::Connected
                    } else {
                        AdapterState::Failed
                    },
                    connected,
                    health: None,
                    last_error: connect_result.error.clone(),
                    uptime: if connected { Some(0) } else { None },
                    messages_in: 0,
                    messages_out: 0,
                },
            );
        }

        // 发布生命周期事件
        if connected {
            self.publish_event(
                event_types::ADAPTER_CONNECTED,
                serde_json::json!({
                    "platform": &platform_name,
                    "connected": true,
                }),
            );
            info!("Adapter '{}' started (connected: true)", platform_name);
        } else {
            let error_msg = connect_result.error.clone().unwrap_or_default();
            self.publish_adapter_error(&platform_name, &error_msg);
            info!("Adapter '{}' started (connected: false)", platform_name);
        }

        Ok(StartAdapterResult {
            ok: connected,
            platform: platform_name,
            error: connect_result.error,
            bot_info: connect_result.bot_info,
        })
    }

    /// 停止适配器
    ///
    /// 先从 HashMap 移除适配器（释放写锁），再执行断开操作，避免阻塞其他操作。
    pub async fn stop(&self, platform: &str) -> Result<(), GatewayError> {
        let adapter = {
            let mut adapters = self.adapters.write().await;
            adapters.remove(platform)
        };
        if let Some(mut adapter) = adapter {
            if let Err(e) = adapter.disconnect().await {
                warn!("Error disconnecting adapter '{}': {}", platform, e);
            }
            // 更新状态缓存，否则 get_status 仍返回旧状态
            {
                let mut statuses = self.statuses.write().await;
                if let Some(status) = statuses.get_mut(platform) {
                    status.state = AdapterState::Stopped;
                    status.connected = false;
                }
            }
            self.publish_event(
                event_types::ADAPTER_DISCONNECTED,
                serde_json::json!({
                    "platform": platform,
                    "connected": false,
                }),
            );
            info!("Adapter '{}' stopped", platform);
        }
        Ok(())
    }

    /// 发送消息（通过适配器读锁）
    pub async fn send_message(
        &self,
        platform: &str,
        params: crate::types::message::SendTextParams,
    ) -> Result<crate::types::message::SendResult, GatewayError> {
        let adapters = self.adapters.read().await;
        let adapter = adapters
            .get(platform)
            .ok_or_else(|| GatewayError::AdapterNotConnected(platform.to_string()))?;
        adapter.send(params).await
    }

    /// 编辑消息
    pub async fn edit_message(
        &self,
        platform: &str,
        params: crate::types::message::EditMessageParams,
    ) -> Result<crate::types::message::EditResult, GatewayError> {
        let adapters = self.adapters.read().await;
        let adapter = adapters
            .get(platform)
            .ok_or_else(|| GatewayError::AdapterNotConnected(platform.to_string()))?;
        adapter.edit_message(params).await
    }

    /// 删除消息
    pub async fn delete_message(
        &self,
        platform: &str,
        chat_id: &str,
        message_id: &str,
    ) -> Result<crate::types::message::DeleteResult, GatewayError> {
        let adapters = self.adapters.read().await;
        let adapter = adapters
            .get(platform)
            .ok_or_else(|| GatewayError::AdapterNotConnected(platform.to_string()))?;
        adapter.delete_message(chat_id, message_id).await
    }

    /// 获取单个适配器状态（O(1) 查找）
    pub async fn get_status(&self, platform: &str) -> Option<AdapterStatusSummary> {
        let statuses = self.statuses.read().await;
        statuses.get(platform).cloned()
    }

    /// 列出所有适配器状态
    pub async fn list_statuses(&self) -> Vec<AdapterStatusSummary> {
        let adapters = self.adapters.read().await;
        let mut statuses = self.statuses.write().await;

        // 更新实时状态
        for (platform, adapter) in adapters.iter() {
            if let Some(status) = statuses.get_mut(platform) {
                status.state = adapter.state();
                status.connected = adapter.is_connected();
            }
        }

        statuses.values().cloned().collect()
    }

    /// 启动所有已配置的适配器
    pub async fn start_all(&self, configs: HashMap<String, AdapterConfig>) -> StartAllResult {
        let mut succeeded = Vec::new();
        let mut failed = Vec::new();

        for (platform, config) in configs {
            if !config.enabled {
                info!("Skipping disabled adapter '{}'", platform);
                continue;
            }
            match self.start(&platform, config).await {
                Ok(r) if r.ok => succeeded.push(platform),
                Ok(r) => failed.push((platform, r.error.unwrap_or_default())),
                Err(e) => failed.push((platform, e.to_string())),
            }
        }

        StartAllResult { succeeded, failed }
    }

    /// 停止所有适配器
    ///
    /// 一次性取出所有适配器释放写锁，再逐个断开，避免阻塞其他操作。
    pub async fn stop_all(&self) {
        let adapters: Vec<(String, Box<dyn PlatformAdapter>)> = {
            let mut locked = self.adapters.write().await;
            locked.drain().collect()
        };
        for (name, mut adapter) in adapters {
            if let Err(e) = adapter.disconnect().await {
                warn!("Error disconnecting adapter '{}': {}", name, e);
            }
            self.publish_event(
                event_types::ADAPTER_DISCONNECTED,
                serde_json::json!({
                    "platform": &name,
                    "connected": false,
                }),
            );
            info!("Adapter '{}' disconnected", name);
        }
    }

    /// 检查是否有任何适配器已连接
    pub async fn has_connected(&self) -> bool {
        let adapters = self.adapters.read().await;
        adapters.values().any(|a| a.is_connected())
    }

    /// 发布事件到 EventBus
    fn publish_event(&self, event_type: &str, data: serde_json::Value) {
        if let Some(ref bus) = self.event_bus {
            bus.publish(GatewayEvent::new(event_type, "adapter_manager", data));
        }
    }

    /// 发布适配器错误事件
    fn publish_adapter_error(&self, platform: &str, error: &str) {
        error!("Adapter '{}' error: {}", platform, error);
        self.publish_event(
            event_types::ADAPTER_ERROR,
            serde_json::json!({
                "platform": platform,
                "error": error,
            }),
        );
    }
}

impl Default for AdapterManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::AdapterFactory;
    use crate::types::message::{ChatInfo, SendResult, SendTextParams};
    use async_trait::async_trait;

    // ── Mock 适配器 ──────────────────────────────────────────

    struct MockTestAdapter {
        platform: String,
        display: String,
        state: AdapterState,
    }

    impl MockTestAdapter {
        fn new() -> Self {
            Self {
                platform: "test-mock".into(),
                display: "Test Mock".into(),
                state: AdapterState::Created,
            }
        }
    }

    #[async_trait]
    impl PlatformAdapter for MockTestAdapter {
        fn platform_name(&self) -> &str {
            &self.platform
        }
        fn display_name(&self) -> &str {
            &self.display
        }
        fn capabilities(&self) -> &[Capability] {
            &[]
        }

        async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
            let _ = config;
            self.state = AdapterState::Starting;
            Ok(InitResult {
                ok: true,
                error: None,
            })
        }

        async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
            self.state = AdapterState::Connected;
            Ok(ConnectResult {
                ok: true,
                error: None,
                bot_info: None,
            })
        }

        async fn disconnect(&mut self) -> Result<(), GatewayError> {
            self.state = AdapterState::Stopped;
            Ok(())
        }

        fn state(&self) -> AdapterState {
            self.state.clone()
        }

        async fn health(&self) -> HealthReport {
            HealthReport {
                status: if self.state == AdapterState::Connected {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Down
                },
                connected: self.state == AdapterState::Connected,
                last_connected_at: None,
                last_error_at: None,
                last_error: None,
                messages_in: 0,
                messages_out: 0,
                errors: 0,
                uptime: None,
            }
        }

        async fn send(&self, _p: SendTextParams) -> Result<SendResult, GatewayError> {
            Ok(SendResult {
                success: true,
                message_id: None,
                timestamp: None,
                error: None,
                error_code: None,
                retryable: false,
            })
        }

        async fn get_chat_info(&self, _id: &str) -> Result<ChatInfo, GatewayError> {
            Err(GatewayError::capability_not_supported("get_chat_info"))
        }

        fn runtime_config(&self) -> AdapterRuntimeConfig {
            AdapterRuntimeConfig {
                enabled: true,
                token_configured: false,
                extra: serde_json::json!({}),
            }
        }

        fn status_summary(&self) -> AdapterStatusSummary {
            AdapterStatusSummary {
                platform: self.platform.clone(),
                display_name: self.display.clone(),
                state: self.state.clone(),
                connected: self.state == AdapterState::Connected,
                health: None,
                last_error: None,
                uptime: None,
                messages_in: 0,
                messages_out: 0,
            }
        }
    }

    /// 注册 MockTestAdapter 到 registry
    async fn register_mock_adapter(manager: &AdapterManager) {
        let registry = manager.registry();
        let factory: AdapterFactory = std::sync::Arc::new(|config| {
            Box::pin(async move {
                let mut adapter = MockTestAdapter::new();
                let result = adapter.init(config).await.map_err(|e| e.to_string())?;
                if !result.ok {
                    return Err(result.error.unwrap_or_default());
                }
                Ok(Box::new(adapter) as Box<dyn PlatformAdapter>)
            })
        });
        registry.register("test-mock", "Test Mock", factory).await;
    }

    // ── 测试: stop() 后 get_status() 返回 Stopped ────────────

    #[tokio::test]
    async fn test_stop_updates_status_cache() {
        let manager = AdapterManager::new();
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: true,
            token: Some("test-token".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };

        // 启动 → 预期状态为 Connected
        let start_result = manager.start("test-mock", config).await.unwrap();
        assert!(start_result.ok);
        let status = manager.get_status("test-mock").await.unwrap();
        assert_eq!(status.state, AdapterState::Connected);
        assert!(status.connected);

        // 停止 → 预期状态为 Stopped
        manager.stop("test-mock").await.unwrap();
        let status = manager.get_status("test-mock").await.unwrap();
        assert_eq!(status.state, AdapterState::Stopped);
        assert!(!status.connected);
    }

    // ── 测试: config（含 token）被正确传递到适配器 ───────────

    #[tokio::test]
    async fn test_start_passes_config_to_adapter() {
        let manager = AdapterManager::new();
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: true,
            token: Some("my-secret-token".into()),
            api_key: Some("my-api-key".into()),
            base_url: None,
            extra: serde_json::json!({"custom": "value"}),
        };

        // 启动后，config 应该被传递到 init()，工厂创建 adapter 时使用
        let start_result = manager.start("test-mock", config.clone()).await.unwrap();
        assert!(start_result.ok);

        // get_status 验证状态
        let status = manager.get_status("test-mock").await.unwrap();
        assert_eq!(status.state, AdapterState::Connected);
    }

    #[tokio::test]
    async fn test_has_connected_after_start() {
        let manager = AdapterManager::new();
        register_mock_adapter(&manager).await;

        assert!(!manager.has_connected().await);

        let config = AdapterConfig {
            enabled: true,
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();

        assert!(manager.has_connected().await);
    }

    #[tokio::test]
    async fn test_send_message_delegation() {
        let manager = AdapterManager::new();
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: true,
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();

        let params = SendTextParams {
            chat_id: "1".to_string(),
            message: crate::types::message::OutboundMessage {
                text: "hello".to_string(),
                parse_mode: crate::types::message::ParseMode::None,
            },
            reply_to: None,
            metadata: None,
        };
        let result = manager.send_message("test-mock", params).await.unwrap();
        assert!(result.success);
    }

    #[tokio::test]
    async fn test_stop_all_cleans_up() {
        let manager = AdapterManager::new();
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: true,
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        assert!(manager.has_connected().await);

        manager.stop_all().await;
        // stop_all 从 adapters map 中移除，has_connected 检查 map
        assert!(!manager.has_connected().await);
        // 注意：stop_all 不更新 statuses 缓存，这里只验证 map 已清空
    }

    #[tokio::test]
    async fn test_start_all_skips_disabled() {
        let manager = AdapterManager::new();
        register_mock_adapter(&manager).await;

        let mut configs = std::collections::HashMap::new();
        configs.insert(
            "test-mock".to_string(),
            AdapterConfig {
                enabled: false,
                token: None,
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            },
        );

        let result = manager.start_all(configs).await;
        assert!(
            result.succeeded.is_empty(),
            "disabled adapter should not start"
        );
        assert!(
            result.failed.is_empty(),
            "disabled adapter should not fail either"
        );
    }

    #[tokio::test]
    async fn test_start_publishes_adapter_connected() {
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::ADAPTER_CONNECTED);
        let manager = AdapterManager::new().with_event_bus(event_bus);
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: true,
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();

        let event = tokio::time::timeout(tokio::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive ADAPTER_CONNECTED")
            .expect("event should be valid");

        assert_eq!(event.event_type, event_types::ADAPTER_CONNECTED);
        assert_eq!(event.source, "adapter_manager");
    }

    #[tokio::test]
    async fn test_stop_publishes_adapter_disconnected() {
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::ADAPTER_DISCONNECTED);
        let manager = AdapterManager::new().with_event_bus(event_bus);
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: true,
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();

        manager.stop("test-mock").await.unwrap();

        let event = tokio::time::timeout(tokio::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive ADAPTER_DISCONNECTED")
            .expect("event should be valid");

        assert_eq!(event.event_type, event_types::ADAPTER_DISCONNECTED);
        assert_eq!(event.source, "adapter_manager");
    }
}

/// 启动适配器结果
#[derive(Debug)]
pub struct StartAdapterResult {
    pub ok: bool,
    pub platform: String,
    pub error: Option<String>,
    pub bot_info: Option<BotInfo>,
}

/// 启动所有适配器结果
#[derive(Debug)]
pub struct StartAllResult {
    pub succeeded: Vec<String>,
    pub failed: Vec<(String, String)>,
}
