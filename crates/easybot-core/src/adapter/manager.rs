//! 适配器管理器实现
//!
//! 管理所有平台适配器的生命周期、健康轮询和状态查询。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

use crate::bus::EventBus;
use crate::types::adapter::*;
use crate::types::error::GatewayError;
use crate::types::event::GatewayEvent;
use crate::types::event::event_types;
use crate::adapter::registry::AdapterRegistry;

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
            self.publish_event(event_types::ADAPTER_CONNECTED, serde_json::json!({
                "platform": &platform_name,
                "connected": true,
            }));
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
            self.publish_event(event_types::ADAPTER_DISCONNECTED, serde_json::json!({
                "platform": platform,
                "connected": false,
            }));
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
    pub async fn start_all(
        &self,
        configs: HashMap<String, AdapterConfig>,
    ) -> StartAllResult {
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
            self.publish_event(event_types::ADAPTER_DISCONNECTED, serde_json::json!({
                "platform": &name,
                "connected": false,
            }));
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
        self.publish_event(event_types::ADAPTER_ERROR, serde_json::json!({
            "platform": platform,
            "error": error,
        }));
    }
}

impl Default for AdapterManager {
    fn default() -> Self {
        Self::new()
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
