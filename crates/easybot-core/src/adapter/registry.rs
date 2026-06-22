//! 适配器注册表
//!
//! 管理适配器工厂的注册与发现。
//! 内置适配器和插件适配器都通过注册表统一管理。

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::types::BoxFuture;
use crate::types::adapter::*;

/// 适配器工厂
///
/// 接收配置，返回适配器实例。
pub type AdapterFactory = Arc<
    dyn Fn(AdapterConfig) -> BoxFuture<'static, Result<Box<dyn PlatformAdapter>, String>>
        + Send
        + Sync,
>;

/// 适配器注册表
///
/// 存储适配器工厂函数，支持动态注册和按名创建。
pub struct AdapterRegistry {
    factories: RwLock<HashMap<String, RegistryEntry>>,
}

struct RegistryEntry {
    factory: AdapterFactory,
    display_name: String,
    /// 凭据环境变量名列表（用于自动检测：所有变量均已设置时自动启用）
    credential_env_vars: Vec<String>,
}

impl AdapterRegistry {
    /// 创建注册表
    pub fn new() -> Self {
        Self {
            factories: RwLock::new(HashMap::new()),
        }
    }

    /// 注册适配器工厂
    ///
    /// `credential_env_vars` 是该适配器需要的凭据环境变量名列表。
    /// 启动时系统自动检测：所有变量均已设置则自动启用适配器。
    /// 例如 Telegram 传入 `&["TELEGRAM_BOT_TOKEN"]`。
    pub async fn register(
        &self,
        platform: &str,
        display_name: &str,
        factory: AdapterFactory,
        credential_env_vars: &[&str],
    ) {
        let mut factories = self.factories.write().await;
        factories.insert(
            platform.to_string(),
            RegistryEntry {
                factory,
                display_name: display_name.to_string(),
                credential_env_vars: credential_env_vars.iter().map(|s| s.to_string()).collect(),
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

    /// 获取指定平台的凭据环境变量名列表
    pub async fn credential_env_vars(&self, platform: &str) -> Vec<String> {
        let factories = self.factories.read().await;
        factories
            .get(platform)
            .map(|e| e.credential_env_vars.clone())
            .unwrap_or_default()
    }

    /// 列出所有已注册平台的凭据环境变量名
    pub async fn all_credential_env_vars(&self) -> HashMap<String, Vec<String>> {
        let factories = self.factories.read().await;
        factories
            .iter()
            .map(|(name, entry)| (name.clone(), entry.credential_env_vars.clone()))
            .collect()
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}
