//! 适配器管理器实现
//!
//! 管理所有平台适配器的生命周期、健康轮询和状态查询。

use std::collections::HashMap;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::types::adapter::*;
use crate::types::error::GatewayError;
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
}

impl AdapterManager {
    /// 创建适配器管理器
    pub fn new() -> Self {
        Self {
            registry: AdapterRegistry::new(),
            adapters: RwLock::new(HashMap::new()),
            statuses: RwLock::new(HashMap::new()),
        }
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
        {
            let mut statuses = self.statuses.write().await;
            statuses.insert(
                platform_name.clone(),
                AdapterStatusSummary {
                    platform: platform_name.clone(),
                    display_name,
                    state: if connect_result.ok {
                        AdapterState::Connected
                    } else {
                        AdapterState::Failed
                    },
                    connected: connect_result.ok,
                    health: None,
                    last_error: connect_result.error.clone(),
                    uptime: if connect_result.ok { Some(0) } else { None },
                    messages_in: 0,
                    messages_out: 0,
                },
            );
        }

        info!(
            "Adapter '{}' started (connected: {})",
            platform_name,
            connect_result.ok
        );

        Ok(StartAdapterResult {
            ok: connect_result.ok,
            platform: platform_name,
            error: connect_result.error,
            bot_info: connect_result.bot_info,
        })
    }

    /// 停止适配器
    pub async fn stop(&self, platform: &str) -> Result<(), GatewayError> {
        let mut adapters = self.adapters.write().await;
        if let Some(mut adapter) = adapters.remove(platform) {
            if let Err(e) = adapter.disconnect().await {
                warn!("Error disconnecting adapter '{}': {}", platform, e);
            }
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
    pub async fn stop_all(&self) {
        let mut adapters = self.adapters.write().await;
        for (name, mut adapter) in adapters.drain() {
            if let Err(e) = adapter.disconnect().await {
                warn!("Error disconnecting adapter '{}': {}", name, e);
            }
            info!("Adapter '{}' disconnected", name);
        }
    }

    /// 检查是否有任何适配器已连接
    pub async fn has_connected(&self) -> bool {
        let adapters = self.adapters.read().await;
        adapters.values().any(|a| a.is_connected())
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
