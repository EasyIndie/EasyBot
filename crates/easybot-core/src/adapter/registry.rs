//! 适配器注册表
//!
//! 管理适配器工厂的注册与发现。
//! 内置适配器和插件适配器都通过注册表统一管理。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::types::adapter::*;
use crate::types::error::BoxFuture;

/// 适配器工厂
///
/// 接收配置，返回适配器实例。
pub type AdapterFactory =
    Arc<dyn Fn(AdapterConfig) -> BoxFuture<'static, Result<Box<dyn PlatformAdapter>, String>> + Send + Sync>;

/// 适配器注册表
///
/// 存储适配器工厂函数，支持动态注册和按名创建。
pub struct AdapterRegistry {
    factories: RwLock<HashMap<String, RegistryEntry>>,
}

struct RegistryEntry {
    factory: AdapterFactory,
    display_name: String,
}

impl AdapterRegistry {
    /// 创建注册表
    pub fn new() -> Self {
        Self {
            factories: RwLock::new(HashMap::new()),
        }
    }

    /// 注册适配器工厂
    pub async fn register(
        &self,
        platform: &str,
        display_name: &str,
        factory: AdapterFactory,
    ) {
        let mut factories = self.factories.write().await;
        factories.insert(
            platform.to_string(),
            RegistryEntry {
                factory,
                display_name: display_name.to_string(),
            },
        );
    }

    /// 通过注册表创建适配器
    pub async fn create(
        &self,
        platform: &str,
        config: AdapterConfig,
    ) -> Result<Box<dyn PlatformAdapter>, String> {
        let factories = self.factories.read().await;
        let entry = factories
            .get(platform)
            .ok_or_else(|| format!("no factory registered for platform '{}'", platform))?;
        (entry.factory)(config).await
    }

    /// 检查平台是否有注册工厂
    pub async fn has_platform(&self, platform: &str) -> bool {
        let factories = self.factories.read().await;
        factories.contains_key(platform)
    }

    /// 列出所有已注册的平台
    pub async fn list_platforms(&self) -> Vec<(String, String)> {
        let factories = self.factories.read().await;
        factories
            .iter()
            .map(|(name, entry)| (name.clone(), entry.display_name.clone()))
            .collect()
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
