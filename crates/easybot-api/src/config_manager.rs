//! 配置管理器
//!
//! 提供线程安全的运行时配置存储和热重载能力。
//! 支持：
//! - 原子配置替换（通过 Arc swap）
//! - 文件变更轮询监听
//! - PUT /config 端点更新

use easybot_core::bus::EventBus;
use easybot_core::types::config::GatewayConfig;
use easybot_core::types::event::{GatewayEvent, event_types};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;

/// 配置管理器
///
/// 包装 GatewayConfig 并提供线程安全的读写。
/// 使用 Arc 使得读取方在获取引用后不受写入方影响。
///
/// 所有加载路径（new / with_path / new_shared / swap）都会自动应用
/// 运行时覆盖（如 EASYBOT_ADMIN_PASSWORD 环境变量），确保 API 返回
/// 的配置始终反映实际运行值。
#[derive(Clone)]
pub struct ConfigManager {
    current: Arc<RwLock<Arc<GatewayConfig>>>,
    config_path: Option<PathBuf>,
    last_mtime: Arc<RwLock<Option<SystemTime>>>,
}

impl ConfigManager {
    /// 将运行时环境变量覆盖应用到配置上
    ///
    /// 这些覆盖无法通过 YAML 的 ${VAR_NAME} 替换实现，因为它们依赖于
    /// 特殊优先级（env var 无条件覆盖配置文件）或运行时上下文。
    fn apply_runtime_overrides(config: &mut GatewayConfig) {
        // EASYBOT_ADMIN_PASSWORD 无条件覆盖 gateway.yaml 中的值
        if let Ok(password) = std::env::var("EASYBOT_ADMIN_PASSWORD")
            && !password.is_empty()
        {
            config.server.admin_password = password;
        }
    }

    /// 创建新的配置管理器
    pub fn new(mut config: GatewayConfig) -> Self {
        Self::apply_runtime_overrides(&mut config);
        Self {
            current: Arc::new(RwLock::new(Arc::new(config))),
            config_path: None,
            last_mtime: Arc::new(RwLock::new(None)),
        }
    }

    /// 从已存在的 Arc 创建配置管理器（与 AppState 共享）
    pub fn new_shared(config: Arc<GatewayConfig>) -> Self {
        let mut config = (*config).clone();
        Self::apply_runtime_overrides(&mut config);
        Self {
            current: Arc::new(RwLock::new(Arc::new(config))),
            config_path: None,
            last_mtime: Arc::new(RwLock::new(None)),
        }
    }

    /// 创建带文件路径的配置管理器（启用文件轮询）
    pub fn with_path(mut config: GatewayConfig, path: PathBuf) -> Self {
        Self::apply_runtime_overrides(&mut config);
        let mtime = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        Self {
            current: Arc::new(RwLock::new(Arc::new(config))),
            config_path: Some(path),
            last_mtime: Arc::new(RwLock::new(mtime)),
        }
    }

    /// 获取当前配置的 Arc 引用
    pub async fn get(&self) -> Arc<GatewayConfig> {
        self.current.read().await.clone()
    }

    /// 原子替换当前配置
    ///
    /// 返回旧配置。替换前自动应用运行时覆盖。
    pub async fn swap(&self, mut new_config: GatewayConfig) -> Arc<GatewayConfig> {
        Self::apply_runtime_overrides(&mut new_config);
        let mut current = self.current.write().await;

        std::mem::replace(&mut *current, Arc::new(new_config))
    }

    /// 检查配置文件是否有变更（轮询）
    ///
    /// 如果有变更则重新加载并替换配置，返回 true。
    pub async fn check_for_changes(&self, event_bus: &EventBus) -> bool {
        let path = match self.config_path.as_ref() {
            Some(p) => p.clone(),
            None => return false,
        };

        let current_mtime = match std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
        {
            Some(m) => m,
            None => return false,
        };

        let last = *self.last_mtime.read().await;
        if last == Some(current_mtime) {
            return false;
        }

        // 文件已变更，重新加载
        match easybot_core::config::load_config(&path).await {
            Ok(new_config) => {
                tracing::info!("Config file changed, reloading: {}", path.display());
                let _old = self.swap(new_config).await;

                // 发布配置变更事件
                event_bus.publish(GatewayEvent::new(
                    event_types::CONFIG_CHANGED,
                    "config",
                    serde_json::json!({"reload_type": "file_watch"}),
                ));

                *self.last_mtime.write().await = Some(current_mtime);
                true
            }
            Err(e) => {
                tracing::warn!("Failed to reload changed config: {}", e);
                false
            }
        }
    }
}

/// 启动配置文件轮询监听器
///
/// 每 `interval_secs` 秒检查配置文件是否有变更。
pub fn start_config_watcher(
    config_manager: ConfigManager,
    event_bus: Arc<EventBus>,
    interval_secs: u64,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            if config_manager.check_for_changes(&event_bus).await {
                tracing::info!("Configuration hot-reloaded via file watcher");
            }
        }
    });
}
