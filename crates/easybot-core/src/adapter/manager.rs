//! 适配器管理器实现
#![allow(missing_docs)]
//!
//! 管理所有平台适配器的生命周期、健康轮询和状态查询。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Weak;
use std::time::Instant;
use tokio::sync::RwLock;
use tokio::sync::broadcast;
use tokio::time::Duration;
use tracing::{error, info, warn};

use crate::adapter::registry::AdapterRegistry;
use crate::bus::EventBus;
use crate::types::adapter::*;
use crate::types::error::GatewayError;
use crate::types::event::GatewayEvent;
use crate::types::event::event_types;

/// 适配器管理器
///
/// 负责适配器的创建、启动、停止、健康检查等生命周期管理。
pub struct AdapterManager {
    /// 适配器注册表
    registry: AdapterRegistry,
    /// 运行中的适配器实例（已连接）
    adapters: RwLock<HashMap<String, Box<dyn PlatformAdapter>>>,
    /// 正在后台连接的适配器
    pending_connections: RwLock<HashSet<String>>,
    /// 适配器状态缓存
    statuses: RwLock<HashMap<String, AdapterStatusSummary>>,
    /// 事件总线（用于发布适配器生命周期事件）
    event_bus: Option<Arc<EventBus>>,
    /// Saved adapter configs, keyed by platform name.  Populated on every
    /// successful `start()` call so the health monitor can reconnect without
    /// external input.  Configs contain tokens — kept in memory only.
    configs: RwLock<HashMap<String, AdapterConfig>>,
    /// Cancel sender for the health monitor background task.
    monitor_cancel_tx: RwLock<Option<broadcast::Sender<()>>>,
    /// Weak self-reference for background tasks.  Initialised by calling
    /// `init_self_ref()` after wrapping in `Arc`.
    self_weak: RwLock<Option<Weak<AdapterManager>>>,
}

/// Per-platform reconnect state tracked by the health monitor.
///
/// Maximum total reconnection attempts before switching to slow retry mode.
/// After this many failures, the adapter transitions to 30-minute retry intervals.
const MAX_TOTAL_RECONNECT_ATTEMPTS: u32 = 20;

/// Maximum number of transport-only retries before escalating to full restart.
/// Transport retries skip re-authentication — only the background task is restarted.
const MAX_TRANSPORT_RETRIES: u32 = 5;

/// Backoff interval between transport-only retry attempts.
const TRANSPORT_RETRY_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
struct ReconnectState {
    consecutive_failures: u32,
    total_failures: u32,
    backoff_until: Option<Instant>,
    /// Number of transport-only retry attempts (resets on full reconnect success).
    transport_retries: u32,
    /// Whether the last health check was Healthy (used for hysteresis).
    was_healthy: bool,
    /// Set when a permanent failure (invalid credentials) is detected.
    /// Once set, the health monitor stops retrying for this platform.
    permanent_failure: bool,
}

/// Classifies a reconnect failure to decide the retry strategy.
#[derive(Debug)]
enum ReconnectFailure {
    /// Transient: network unavailable, DNS failure, timeout — should retry.
    Transient(String),
    /// Permanent: invalid credentials, token revoked, API returns 401/403 — no retry.
    Permanent(String),
}

/// Classify a [`GatewayError`] into [`ReconnectFailure`] based on error message heuristics.
fn classify_error(error: &GatewayError) -> ReconnectFailure {
    let msg = error.to_string().to_lowercase();
    // Auth-related errors → permanent (credentials are bad, retrying won't help)
    if msg.contains("unauthorized")
        || msg.contains(" 401 ")
        || msg.contains(" 403 ")
        || msg.contains("forbidden")
        || msg.contains("auth failed")
        || msg.contains("invalid token")
        || msg.contains("access token expired")
        || msg.contains("access token invalid")
        || msg.contains("invalid access token")
    {
        return ReconnectFailure::Permanent(error.to_string());
    }
    // Network-related errors → transient (will recover when network is back)
    if msg.contains("dns")
        || msg.contains("connection refused")
        || msg.contains("timeout")
        || msg.contains("network")
        || msg.contains("tcp connect")
        || msg.contains("resolve")
        || msg.contains("unreachable")
        || msg.contains("poll request failed")
        || msg.contains("connect failed")
        || msg.contains("fail to connect")
        || msg.contains("failed to connect")
    {
        return ReconnectFailure::Transient(error.to_string());
    }
    // Default: treat unknown errors as transient to avoid permanently
    // disabling an adapter from a one-off unexpected error.
    ReconnectFailure::Transient(error.to_string())
}

/// Exponential backoff: 5s → 10s → 30s → 60s → 120s, capped at 300s.
fn compute_backoff(consecutive_failures: u32) -> Duration {
    let secs = match consecutive_failures {
        0 => 5,
        1 => 10,
        2 => 30,
        3 => 60,
        4 => 120,
        _ => 300, // capped at 5 minutes
    };
    Duration::from_secs(secs)
}

impl AdapterManager {
    /// 创建适配器管理器
    pub fn new() -> Self {
        Self {
            registry: AdapterRegistry::new(),
            adapters: RwLock::new(HashMap::new()),
            pending_connections: RwLock::new(HashSet::new()),
            statuses: RwLock::new(HashMap::new()),
            event_bus: None,
            configs: RwLock::new(HashMap::new()),
            monitor_cancel_tx: RwLock::new(None),
            self_weak: RwLock::new(None),
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

    /// Initialise the weak self-reference so background tasks can obtain
    /// `Arc<Self>`.  Must be called once after wrapping in `Arc`, e.g.:
    ///
    /// ```ignore
    /// let mgr = Arc::new(AdapterManager::new());
    /// AdapterManager::init_self_ref(&mgr);
    /// ```
    pub async fn init_self_ref(self: &Arc<Self>) {
        *self.self_weak.write().await = Some(Arc::downgrade(self));
    }

    /// Obtain an `Arc<Self>` from the weak self-reference set by
    /// [`init_self_ref`](Self::init_self_ref).  Returns an error if the
    /// weak ref was never set or the last strong reference has been dropped.
    async fn ensure_self_ref(&self) -> Result<Arc<Self>, GatewayError> {
        let guard = self.self_weak.read().await;
        guard.as_ref().and_then(|w| w.upgrade()).ok_or_else(|| {
            GatewayError::Internal(
                "AdapterManager self-ref not set; wrap in Arc and call init_self_ref()".into(),
            )
        })
    }

    /// 启动适配器（非阻塞）
    ///
    /// 执行 init() 后立即返回，connect() 在后台任务中执行。
    /// 调用者可通过 `get_status()` 轮询状态变化（Connecting → Connected / Failed）。
    ///
    /// 注意：必须先通过 `init_self_ref()` 初始化弱引用，否则返回错误。
    pub async fn start(
        &self,
        platform: &str,
        mut config: AdapterConfig,
    ) -> Result<StartAdapterResult, GatewayError> {
        // 获取 Arc<Self> 用于后台任务
        let self_arc = self.ensure_self_ref().await?;

        // 注入凭据环境变量（如 token 未在 config 中设置，从 env var 读取）
        // 确保所有调用方（包括 API 手动启动）都获得凭据注入
        self.inject_credentials(platform, &mut config).await;

        // 通过注册表创建适配器实例
        let mut adapter = self
            .registry
            .create(platform, config.clone())
            .await
            .map_err(|e| GatewayError::PlatformNotFound(format!("{}: {}", platform, e)))?;

        // 在 init 前获取名称（init 失败后仍需用它们更新状态缓存）
        let platform_name = adapter.platform_name().to_string();
        let display_name = adapter.display_name().to_string();

        // 初始化（同步、快速）
        let init_result = adapter.init(config.clone()).await?;
        if !init_result.ok {
            let error_msg = init_result.error.clone().unwrap_or_default();
            self.publish_adapter_error(platform, &error_msg);
            // 更新状态为 Failed，使前端 /api/v1/adapters 能第一时间看到失败
            {
                let mut statuses = self.statuses.write().await;
                statuses.insert(
                    platform_name.clone(),
                    AdapterStatusSummary {
                        platform: platform_name.clone(),
                        display_name: display_name.clone(),
                        state: AdapterState::Failed,
                        connected: false,
                        health: None,
                        last_error: Some(error_msg.clone()),
                        uptime: None,
                        messages_in: 0,
                        messages_out: 0,
                    },
                );
            }
            return Ok(StartAdapterResult {
                ok: false,
                pending: false,
                platform: platform_name,
                error: init_result.error,
                bot_info: None,
            });
        }

        // 检查是否已在运行或连接中
        if self.adapters.read().await.contains_key(&platform_name) {
            return Err(GatewayError::Internal(format!(
                "Adapter '{}' is already running",
                platform_name
            )));
        }
        if self
            .pending_connections
            .read()
            .await
            .contains(&platform_name)
        {
            return Err(GatewayError::Internal(format!(
                "Adapter '{}' is already connecting",
                platform_name
            )));
        }

        // 设置 Connecting 状态（get_status / list_statuses 立即可见）
        {
            let mut statuses = self.statuses.write().await;
            statuses.insert(
                platform_name.clone(),
                AdapterStatusSummary {
                    platform: platform_name.clone(),
                    display_name: display_name.clone(),
                    state: AdapterState::Connecting,
                    connected: false,
                    health: None,
                    last_error: None,
                    uptime: None,
                    messages_in: 0,
                    messages_out: 0,
                },
            );
        }

        // 记录 pending connection + 保存 config（health monitor 据此跳过 / 重连）
        {
            let mut pending = self.pending_connections.write().await;
            pending.insert(platform_name.clone());
        }
        {
            let mut configs = self.configs.write().await;
            configs.insert(platform_name.clone(), config.clone());
        }

        // 后台执行 connect()
        let pname = platform_name.clone();
        let config_for_store = config.clone();
        tokio::spawn(async move {
            let connect_result = adapter.connect().await;

            // 原子检查：是否已被 stop() 取消
            let was_pending = self_arc.pending_connections.write().await.remove(&pname);
            if !was_pending {
                // 已被 stop() 移除 → 丢弃适配器实例
                return;
            }

            match connect_result {
                Ok(cr) if cr.ok => {
                    // 存入 adapters map
                    let health_status = adapter.health_status();
                    self_arc
                        .adapters
                        .write()
                        .await
                        .insert(pname.clone(), adapter);

                    // 更新状态
                    let mut statuses = self_arc.statuses.write().await;
                    if let Some(status) = statuses.get_mut(&pname) {
                        status.state = AdapterState::Connected;
                        status.connected = true;
                        status.last_error = None;
                        status.uptime = Some(0);
                    }

                    // 确保 config 已保存（可能被 reconnect 清除又重设）
                    self_arc
                        .configs
                        .write()
                        .await
                        .insert(pname.clone(), config_for_store);

                    self_arc.publish_event(
                        event_types::ADAPTER_CONNECTED,
                        serde_json::json!({
                            "platform": &pname,
                            "connected": true,
                            "health": health_status,
                        }),
                    );
                    info!("Adapter '{}' connected", pname);
                }
                _ => {
                    let error_msg = match &connect_result {
                        Ok(cr) => cr
                            .error
                            .clone()
                            .unwrap_or_else(|| "Unknown error".to_string()),
                        Err(e) => e.to_string(),
                    };

                    let mut statuses = self_arc.statuses.write().await;
                    if let Some(status) = statuses.get_mut(&pname) {
                        status.state = AdapterState::Failed;
                        status.connected = false;
                        status.last_error = Some(error_msg.clone());
                    }

                    // Keep config in place so the health monitor can retry
                    // reconnection. The 20-attempt cap in run_health_check
                    // prevents infinite retries; explicit stop() still removes it.

                    self_arc.publish_adapter_error(&pname, &error_msg);
                    error!("Adapter '{}' failed to connect: {}", pname, error_msg);
                }
            }
        });

        Ok(StartAdapterResult {
            ok: true,
            pending: true,
            platform: platform_name,
            error: None,
            bot_info: None,
        })
    }

    /// 停止适配器
    ///
    /// 同时处理已连接和正在后台连接的适配器。
    /// 对于 pending 连接：从 pending_connections 移除，后台任务检测到后自动丢弃。
    /// 对于已连接适配器：从 HashMap 移除后执行断开操作。
    pub async fn stop(&self, platform: &str) -> Result<(), GatewayError> {
        // 先检查 pending connection
        let was_pending = {
            let mut pending = self.pending_connections.write().await;
            pending.remove(platform)
        };

        if was_pending {
            // 从 configs 中移除，阻止 health monitor 重试
            {
                let mut configs = self.configs.write().await;
                configs.remove(platform);
            }
            // 更新状态缓存
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
            info!("Adapter '{}' stopped (was pending)", platform);
            return Ok(());
        }

        // 已连接适配器：从 map 移除后再断开
        let adapter = {
            let mut adapters = self.adapters.write().await;
            adapters.remove(platform)
        };
        // Clear saved config since adapter is intentionally stopped
        {
            let mut configs = self.configs.write().await;
            configs.remove(platform);
        }
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

    /// 发送媒体消息
    pub async fn send_media(
        &self,
        platform: &str,
        params: crate::types::message::SendMediaParams,
    ) -> Result<crate::types::message::SendResult, GatewayError> {
        let adapters = self.adapters.read().await;
        let adapter = adapters
            .get(platform)
            .ok_or_else(|| GatewayError::AdapterNotConnected(platform.to_string()))?;
        adapter.send_media(params).await
    }

    /// 发送交互式消息（含行内键盘）
    pub async fn send_interactive(
        &self,
        platform: &str,
        params: crate::types::message::SendInteractiveParams,
    ) -> Result<crate::types::message::SendResult, GatewayError> {
        let adapters = self.adapters.read().await;
        let adapter = adapters
            .get(platform)
            .ok_or_else(|| GatewayError::AdapterNotConnected(platform.to_string()))?;
        adapter.send_interactive(params).await
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

    /// 获取单个适配器状态（优先实时查询，已停止适配器回退缓存）
    pub async fn get_status(&self, platform: &str) -> Option<AdapterStatusSummary> {
        // 检查 pending connection（状态已在 start() 中写入 statuses）
        if self.pending_connections.read().await.contains(platform) {
            return self.statuses.read().await.get(platform).cloned();
        }
        // 检查已连接适配器（实时状态）
        let adapters = self.adapters.read().await;
        if let Some(adapter) = adapters.get(platform) {
            return Some(adapter.status_summary());
        }
        // 已停止/失败的适配器 — 回退到状态缓存
        self.statuses.read().await.get(platform).cloned()
    }

    /// 富化会话来源信息
    ///
    /// 通过适配器提供的 enrich_source 方法补充会话中的用户名、角色等信息。
    /// 如果适配器未连接或 enrich_source 返回 None，则返回 None。
    pub async fn enrich_session(
        &self,
        platform: &str,
        source: &crate::types::session::SessionSource,
    ) -> Option<crate::types::session::SessionSource> {
        let adapters = self.adapters.read().await;
        let adapter = adapters.get(platform)?;
        adapter.enrich_source(source).await
    }

    /// 列出所有适配器状态
    pub async fn list_statuses(&self) -> Vec<AdapterStatusSummary> {
        // 先从已连接适配器收集实时状态（读锁，不阻塞 statuses 的其他读取）
        let fresh_statuses: Vec<AdapterStatusSummary> = {
            let adapters = self.adapters.read().await;
            adapters.values().map(|a| a.status_summary()).collect()
        };

        // 合并到缓存时获取写锁。保留缓存中的终止状态（Failed/Stopped），
        // 避免健康监测器设置的永久失败状态被适配器自身报告覆盖。
        let mut statuses = self.statuses.write().await;
        for s in &fresh_statuses {
            let should_update = match statuses.get(&s.platform) {
                Some(cached)
                    if cached.state == AdapterState::Failed
                        || cached.state == AdapterState::Stopped =>
                {
                    // 健康监测器已将适配器标记为终止状态，不要用 live adapter
                    // 的 self-report 覆盖（可能在 transport retry 永久失败后
                    // adapter 实例尚未从 adapters map 中移除）
                    false
                }
                _ => true,
            };
            if should_update {
                statuses.insert(s.platform.clone(), s.clone());
            }
        }
        statuses.values().cloned().collect()
    }

    /// 启动所有适配器（基于注册表 + 凭据自动检测）
    ///
    /// 遍历所有已注册的适配器平台，根据配置和凭据环境变量决定是否启动：
    /// - `enabled: Some(false)` — 强制跳过，不启动
    /// - `enabled: Some(true)` — 强制启用，即使凭据未就绪
    /// - `enabled: None`（默认）— 自动检测：所有凭据环境变量已设置则启用
    ///
    /// 自动启用时，会将凭据环境变量的值注入 AdapterConfig：
    /// - 第一个凭据变量 → `config.token`
    /// - 所有凭据变量 → `config.extra`（key 为去掉平台前缀的小写名，如 FEISHU_APP_ID → app_id）
    pub async fn start_all(&self, configs: HashMap<String, AdapterConfig>) -> StartAllResult {
        let mut succeeded = Vec::new();
        let mut failed = Vec::new();

        // 遍历所有已注册的适配器平台（而非仅配置文件中的平台）
        let platforms = self.registry.list_platforms().await;

        for (platform, display_name) in platforms {
            // 从配置中获取覆盖值（如果存在），否则使用默认配置
            let config = configs
                .get(&platform)
                .cloned()
                .unwrap_or_else(|| AdapterConfig {
                    enabled: None,
                    token: None,
                    api_key: None,
                    base_url: None,
                    extra: serde_json::Value::default(),
                });

            // 解析 enabled 状态
            let effective_enabled = match config.enabled {
                Some(false) => {
                    info!(
                        "Skipping adapter '{}' ({}) — explicitly disabled in config",
                        platform, display_name
                    );
                    continue;
                }
                Some(true) => {
                    info!(
                        "Starting adapter '{}' ({}) — explicitly enabled",
                        platform, display_name
                    );
                    true
                }
                None => {
                    // 自动检测：检查凭据环境变量是否全部设置
                    let env_vars = self.registry.credential_env_vars(&platform).await;
                    if env_vars.is_empty() {
                        // 无凭据要求（例如个人微信扫码登录）— 自动启用
                        info!(
                            "Auto-enabling adapter '{}' ({}) — no credentials required",
                            platform, display_name
                        );
                        true
                    } else {
                        let all_set = env_vars
                            .iter()
                            .all(|v| std::env::var(v).map(|val| !val.is_empty()).unwrap_or(false));
                        if all_set {
                            info!(
                                "Auto-enabling adapter '{}' ({}) — credentials detected via env vars: {:?}",
                                platform, display_name, env_vars
                            );
                            true
                        } else {
                            info!(
                                "Skipping adapter '{}' ({}) — credentials not set (env vars: {:?})",
                                platform, display_name, env_vars
                            );
                            continue;
                        }
                    }
                }
            };

            if effective_enabled {
                // 凭据注入已由 start() 内部完成，无需在此处重复调用 inject_credentials
                match self.start(&platform, config).await {
                    Ok(r) if r.ok => succeeded.push(platform),
                    Ok(r) => failed.push((platform, r.error.unwrap_or_default())),
                    Err(e) => failed.push((platform, e.to_string())),
                }
            }
        }

        StartAllResult { succeeded, failed }
    }

    /// 将凭据环境变量注入 AdapterConfig
    ///
    /// - 若 `config.token` 未设置，从最后一个凭据环境变量读取（Secret/Token 惯例排在最后）
    /// - 所有凭据变量值写入 `config.extra`，key 为去掉平台前缀的小写名
    async fn inject_credentials(&self, platform: &str, config: &mut AdapterConfig) {
        let env_vars = self.registry.credential_env_vars(platform).await;
        if env_vars.is_empty() {
            return;
        }

        // 最后一个凭据变量 → token（惯例：ID 在前，Secret/Token 在后）
        if config.token.is_none()
            && let Some(last_var) = env_vars.last()
            && let Ok(val) = std::env::var(last_var)
            && !val.is_empty()
        {
            config.token = Some(val);
        }

        // 所有凭据变量 → extra（key: 去掉平台前缀，小写）
        let prefix = platform.to_uppercase() + "_";
        let mut extra_map = match config.extra.clone() {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        for var_name in &env_vars {
            let key = var_name
                .strip_prefix(&prefix)
                .unwrap_or(var_name)
                .to_lowercase();
            if let Ok(val) = std::env::var(var_name)
                && !val.is_empty()
            {
                extra_map
                    .entry(key)
                    .or_insert(serde_json::Value::String(val));
            }
        }
        config.extra = serde_json::Value::Object(extra_map);
    }

    /// 停止所有适配器
    ///
    /// 取消所有 pending connection 后，一次性取出已连接适配器再逐个断开。
    pub async fn stop_all(&self) {
        // Stop health monitor first so it doesn't try to reconnect
        // adapters while we're shutting them down.
        self.stop_health_monitor().await;

        // 取消所有 pending connection（后台任务检测到后自动丢弃）
        {
            let mut pending = self.pending_connections.write().await;
            pending.clear();
        }

        let adapters: Vec<(String, Box<dyn PlatformAdapter>)> = {
            let mut locked = self.adapters.write().await;
            locked.drain().collect()
        };
        // Clear all saved configs
        {
            let mut configs = self.configs.write().await;
            configs.clear();
        }
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

    /// Start the background health monitoring loop.
    ///
    /// Spawns a tokio task that periodically checks every running adapter's
    /// health and triggers reconnect when unhealthy.  The task is cancelled
    /// when `stop_all()` or `stop_health_monitor()` is called.
    ///
    /// Must be called with an `Arc<Self>` because the spawned task holds a
    /// clone of the Arc.
    pub async fn start_health_monitor(self: &Arc<Self>, interval: Duration) {
        let (cancel_tx, mut cancel_rx) = broadcast::channel(1);
        {
            let mut tx = self.monitor_cancel_tx.write().await;
            *tx = Some(cancel_tx);
        }

        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            mgr.health_monitor_loop(interval, &mut cancel_rx).await;
        });

        info!("Health monitor started (interval: {:?})", interval);
    }

    /// Stop the health monitoring loop (no-op if not running).
    pub async fn stop_health_monitor(&self) {
        if let Some(tx) = self.monitor_cancel_tx.write().await.take() {
            let _ = tx.send(());
            info!("Health monitor stop signal sent");
        }
    }

    /// Internal: the health monitor's main loop.
    async fn health_monitor_loop(
        self: Arc<Self>,
        interval: Duration,
        cancel_rx: &mut broadcast::Receiver<()>,
    ) {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await; // skip the immediate first tick

        // Per-platform reconnect state (exclusive to this task, no lock needed)
        let mut reconnect_state: HashMap<String, ReconnectState> = HashMap::new();

        loop {
            tokio::select! {
                _ = cancel_rx.recv() => {
                    info!("Health monitor stopped");
                    break;
                }
                _ = ticker.tick() => {
                    self.run_health_check(&mut reconnect_state).await;
                }
            }
        }
    }

    /// Internal: one iteration of the health check.
    async fn run_health_check(&self, reconnect_state: &mut HashMap<String, ReconnectState>) {
        // 先收集所有已知平台（只 clone 轻量的 String key，避免全量 config clone）
        let platforms: Vec<String> = { self.configs.read().await.keys().cloned().collect() };

        for platform in &platforms {
            let state = reconnect_state.entry(platform.clone()).or_default();

            // Respect backoff window
            if let Some(until) = state.backoff_until
                && Instant::now() < until
            {
                continue;
            }

            // Skip pending connections — they are already being handled
            if self.pending_connections.read().await.contains(platform) {
                continue;
            }

            // Check current adapter health.
            // Only trigger reconnect on Down (task appears dead).
            // Degraded means the task is alive but the message stream is
            // idle (e.g., no events during quiet periods). The frontend
            // displays Degraded, but no recovery is needed — the state
            // resolves naturally when messages start flowing again.
            let needs_reconnect = {
                let adapters = self.adapters.read().await;
                match adapters.get(platform) {
                    Some(adapter) => {
                        let health = adapter.health().await;
                        health.status == HealthStatus::Down
                    }
                    None => {
                        // Adapter was removed but config still exists — treat as unhealthy
                        true
                    }
                }
            };

            // Permanent failure guard: if the flag is set, check whether the
            // adapter has been manually restarted and is now healthy again.
            // This handles the case where a user stops and restarts an adapter
            // after a permanent failure — the flag lives in this task-local
            // HashMap and is not reset by stop()/start().
            if state.permanent_failure {
                let is_healthy_now = {
                    let adapters = self.adapters.read().await;
                    match adapters.get(platform) {
                        Some(a) => {
                            let h = a.health().await;
                            h.status == HealthStatus::Healthy
                        }
                        None => false,
                    }
                };
                if is_healthy_now {
                    state.permanent_failure = false;
                    // Fall through to run the normal health check.
                } else {
                    continue;
                }
            }

            // Slow retry guard for transient failures that exhausted the normal budget.
            if state.total_failures >= MAX_TOTAL_RECONNECT_ATTEMPTS
                && let Some(until) = state.backoff_until
                && Instant::now() < until
            {
                continue; // still cooling down
            }

            if needs_reconnect {
                // Check if the adapter instance still exists in the running map.
                let adapter_exists = self.adapters.read().await.contains_key(platform);

                // ── Tier 1: Transport-only retry (adapter exists, no re-auth) ──
                //
                // Adapters that need network-auth in connect() (Telegram, Discord,
                // Feishu) override retry_transport() to return Ok(true) and restart
                // just the background task without re-authentication.
                //
                // Adapters with built-in retry loops (QQ, WeChat) use the default
                // Ok(false) and fall through to Tier 2. During transient failures
                // their heartbeat stays fresh (beat on retry), so the health monitor
                // won't reach this code path. When heartbeat goes stale the internal
                // loop has exited — full reconnect is the correct recovery.
                if adapter_exists && state.transport_retries < MAX_TRANSPORT_RETRIES {
                    // Update status to Reconnecting so the admin panel reflects recovery in progress
                    {
                        let mut statuses = self.statuses.write().await;
                        if let Some(status) = statuses.get_mut(platform) {
                            status.state = AdapterState::Reconnecting;
                            status.connected = false;
                        }
                    }

                    match self.retry_transport(platform).await {
                        Ok(true) => {
                            // Transport restart succeeded — adapter is healthy again.
                            state.transport_retries += 1;
                            state.backoff_until = Some(Instant::now() + TRANSPORT_RETRY_BACKOFF);
                            // NOTE: do NOT reset total_failures — transport retries
                            // are cheap; we still want to track overall instability.
                            self.update_status_to_reconnecting(platform).await;
                            info!(
                                "Transport retry succeeded for '{}' (attempt {}/{})",
                                platform, state.transport_retries, MAX_TRANSPORT_RETRIES
                            );
                            continue;
                        }
                        Ok(false) => {
                            // Adapter doesn't support transport-only restart —
                            // fall through to full reconnect below.
                            info!(
                                "Adapter '{}' does not support transport retry, \
                                 falling back to full reconnect",
                                platform
                            );
                        }
                        Err(e) => {
                            // Transport retry failed — classify the error.
                            let failure = classify_error(&e);
                            match failure {
                                ReconnectFailure::Permanent(msg) => {
                                    error!(
                                        "Permanent failure during transport retry for '{}': {}",
                                        platform, msg
                                    );
                                    state.permanent_failure = true;
                                    // Remove the adapter from the running map and
                                    // disconnect it — preventing background tasks
                                    // from continuing to run after a permanent failure
                                    // (unlike Tier 2 full reconnect which calls stop()
                                    // → disconnect() through reconnect_adapter).
                                    if let Some(mut adapter) =
                                        self.adapters.write().await.remove(platform.as_str())
                                    {
                                        let _ = adapter.disconnect().await;
                                    }
                                    self.set_status_failed(platform, &msg).await;
                                    continue;
                                }
                                ReconnectFailure::Transient(msg) => {
                                    state.transport_retries += 1;
                                    state.backoff_until =
                                        Some(Instant::now() + TRANSPORT_RETRY_BACKOFF);
                                    warn!(
                                        "Transport retry failed for '{}' ({}/{}): {} — \
                                         next transport retry in {:?}",
                                        platform,
                                        state.transport_retries,
                                        MAX_TRANSPORT_RETRIES,
                                        msg,
                                        TRANSPORT_RETRY_BACKOFF,
                                    );
                                    continue;
                                }
                            }
                        }
                    }
                }

                // ── Tier 2: Full reconnect (stop + start with re-auth) ──
                // Update status to Reconnecting
                {
                    let mut statuses = self.statuses.write().await;
                    if let Some(status) = statuses.get_mut(platform) {
                        status.state = AdapterState::Reconnecting;
                        status.connected = false;
                    }
                }

                // Clone config for the reconnect attempt
                let config = { self.configs.read().await.get(platform).cloned() };
                let config = match config {
                    Some(c) => c,
                    None => continue,
                };

                match self.do_full_reconnect(platform, config).await {
                    Ok(()) => {
                        state.consecutive_failures = 0;
                        state.total_failures = 0;
                        state.transport_retries = 0;
                        state.backoff_until = None;
                        state.permanent_failure = false;
                        info!("Full reconnect succeeded for '{}'", platform);
                    }
                    Err(e) => {
                        let failure = classify_error(&e);
                        match failure {
                            ReconnectFailure::Permanent(msg) => {
                                // Auth failure — stop retrying immediately.
                                error!(
                                    "Permanent failure for '{}': {} — adapter disabled",
                                    platform, msg
                                );
                                state.permanent_failure = true;
                                self.set_status_failed(platform, &msg).await;
                            }
                            ReconnectFailure::Transient(_msg) => {
                                state.consecutive_failures += 1;
                                state.total_failures += 1;

                                let delay = if state.total_failures > MAX_TOTAL_RECONNECT_ATTEMPTS {
                                    Duration::from_secs(1800) // 30-min slow retry
                                } else {
                                    compute_backoff(state.consecutive_failures)
                                };
                                state.backoff_until = Some(Instant::now() + delay);

                                let attempt_label =
                                    if state.total_failures > MAX_TOTAL_RECONNECT_ATTEMPTS {
                                        format!(
                                            "slow retry #{} (every 30min)",
                                            state.total_failures - MAX_TOTAL_RECONNECT_ATTEMPTS
                                        )
                                    } else {
                                        format!(
                                            "attempt {}/{}",
                                            state.total_failures, MAX_TOTAL_RECONNECT_ATTEMPTS
                                        )
                                    };
                                warn!(
                                    "Reconnect failed for '{}' ({}): {} — next retry in {:?}",
                                    platform, attempt_label, e, delay,
                                );
                            }
                        }
                    }
                }
            } else {
                // All healthy — reset transient counters with hysteresis
                if state.was_healthy {
                    // Two consecutive healthy checks → fully reset
                    state.consecutive_failures = 0;
                    state.transport_retries = 0;
                    state.backoff_until = None;
                }
                state.was_healthy = true;
            }
        }
    }

    /// Stop + start an adapter with the given config.  Publishes lifecycle events.
    ///
    /// `start()` 现在是非阻塞的（connect 在后台执行），此方法通过轮询状态
    /// 最多等待 60 秒等待连接完成，以保持 reconnection 的语义不变。
    async fn reconnect_adapter(
        &self,
        platform: &str,
        config: AdapterConfig,
    ) -> Result<(), GatewayError> {
        // Publish reconnecting event
        self.publish_event(
            event_types::ADAPTER_RECONNECTING,
            serde_json::json!({"platform": platform}),
        );
        info!("Reconnecting adapter '{}'...", platform);

        // Step 1: stop (this removes from pending or adapters + disconnects)
        let _ = self.stop(platform).await;

        // Brief pause to let OS-level resources (sockets, TLS sessions) drain
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Step 2: start (non-blocking — connect runs in background)
        match self.start(platform, config).await {
            Ok(result) if result.ok => {
                // Step 3: wait for connection to complete (max 60s, with exponential backoff)
                info!("Waiting for adapter '{}' to connect...", platform);
                let deadline = Instant::now() + Duration::from_secs(60);
                let mut poll_delay = Duration::from_millis(100);
                while Instant::now() < deadline {
                    // Check if no longer pending (completed or failed)
                    if !self.pending_connections.read().await.contains(platform)
                        && let Some(status) = self.get_status(platform).await
                    {
                        if status.state == AdapterState::Connected {
                            self.publish_event(
                                event_types::ADAPTER_RECONNECTED,
                                serde_json::json!({
                                    "platform": platform,
                                    "connected": true,
                                    "health": status.health,
                                }),
                            );
                            info!("Reconnect succeeded for '{}'", platform);
                            return Ok(());
                        }
                        if status.state == AdapterState::Failed {
                            let err = status.last_error.unwrap_or_default();
                            self.publish_event(
                                event_types::ADAPTER_RECONNECT_FAILED,
                                serde_json::json!({"platform": platform, "error": &err}),
                            );
                            return Err(GatewayError::Internal(err));
                        }
                    }
                    tokio::time::sleep(poll_delay).await;
                    poll_delay = (poll_delay * 2).min(Duration::from_secs(5));
                }
                // Timeout
                let err = format!("Reconnect timeout for '{}'", platform);
                self.publish_event(
                    event_types::ADAPTER_RECONNECT_FAILED,
                    serde_json::json!({"platform": platform, "error": &err}),
                );
                Err(GatewayError::Internal(err))
            }
            Ok(result) => {
                // start() returned ok:false — init failed
                let err = result.error.unwrap_or_else(|| "unknown error".to_string());
                self.publish_event(
                    event_types::ADAPTER_RECONNECT_FAILED,
                    serde_json::json!({"platform": platform, "error": &err}),
                );
                Err(GatewayError::Internal(err))
            }
            Err(e) => {
                self.publish_event(
                    event_types::ADAPTER_RECONNECT_FAILED,
                    serde_json::json!({"platform": platform, "error": e.to_string()}),
                );
                Err(e)
            }
        }
    }

    /// Transport-only restart: delegates to the adapter's `retry_transport()`.
    ///
    /// This is a lightweight operation that cancels and restarts the background
    /// transport task without re-authentication. It is intended for transient
    /// network disruptions (WiFi drop, etc.).
    async fn retry_transport(&self, platform: &str) -> Result<bool, GatewayError> {
        let mut adapters = self.adapters.write().await;
        if let Some(adapter) = adapters.get_mut(platform) {
            adapter.retry_transport().await
        } else {
            Err(GatewayError::AdapterNotConnected(platform.to_string()))
        }
    }

    /// Full reconnect (stop + start with re-auth).
    ///
    /// Delegates to the existing [`reconnect_adapter`] which handles the full
    /// lifecycle, including publishing events.
    async fn do_full_reconnect(
        &self,
        platform: &str,
        config: AdapterConfig,
    ) -> Result<(), GatewayError> {
        self.reconnect_adapter(platform, config).await
    }

    /// Mark an adapter as permanently failed in the status cache.
    ///
    /// Uses in-place mutation to preserve accumulated message counters
    /// and uptime from the existing cache entry.  If no entry exists, a
    /// new one is created with default (zero) counters.
    async fn set_status_failed(&self, platform: &str, error_msg: &str) {
        let mut statuses = self.statuses.write().await;
        if let Some(status) = statuses.get_mut(platform) {
            // In-place mutation preserves messages_in, messages_out,
            // uptime, and display_name from the existing cache entry.
            status.state = AdapterState::Failed;
            status.connected = false;
            status.health = Some(HealthStatus::Down);
            status.last_error = Some(error_msg.to_string());
        } else {
            // No existing entry — create one with defaults.
            statuses.insert(
                platform.to_string(),
                AdapterStatusSummary {
                    platform: platform.to_string(),
                    display_name: platform.to_string(),
                    state: AdapterState::Failed,
                    connected: false,
                    health: Some(HealthStatus::Down),
                    last_error: Some(error_msg.to_string()),
                    uptime: None,
                    messages_in: 0,
                    messages_out: 0,
                },
            );
        }
        self.publish_adapter_error(platform, error_msg);
    }

    /// Update an existing status entry to Reconnecting state.
    async fn update_status_to_reconnecting(&self, platform: &str) {
        let mut statuses = self.statuses.write().await;
        if let Some(status) = statuses.get_mut(platform) {
            status.state = AdapterState::Reconnecting;
        }
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

    /// 创建一个测试用的 Arc<AdapterManager>（自动调用 init_self_ref）
    async fn new_manager() -> Arc<AdapterManager> {
        let mgr = Arc::new(AdapterManager::new());
        mgr.init_self_ref().await;
        mgr
    }

    /// 注册 MockTestAdapter 到 manager
    async fn register_mock_adapter(manager: &AdapterManager) {
        let registry = manager.registry();
        let factory: AdapterFactory = Arc::new(|config| {
            Box::pin(async move {
                let mut adapter = MockTestAdapter::new();
                let result = adapter.init(config).await.map_err(|e| e.to_string())?;
                if !result.ok {
                    return Err(result.error.unwrap_or_default());
                }
                let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
                Ok(boxed)
            })
        });
        registry
            .register("test-mock", "Test Mock", factory, &[])
            .await;
    }

    /// 等待适配器从 Connecting 变为 Connected（最长 2 秒）
    async fn wait_connected(manager: &AdapterManager, platform: &str) {
        for _ in 0..100 {
            if let Some(status) = manager.get_status(platform).await
                && status.state == AdapterState::Connected
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("Adapter '{}' did not connect within timeout", platform);
    }

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
                health: Some(self.health_status()),
                last_error: None,
                uptime: None,
                messages_in: 0,
                messages_out: 0,
            }
        }
    }

    // ── 测试: config（含 token）被正确传递到适配器 ───────────

    #[tokio::test]
    async fn test_start_passes_config_to_adapter() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("my-secret-token".into()),
            api_key: Some("my-api-key".into()),
            base_url: None,
            extra: serde_json::json!({"custom": "value"}),
        };

        // 启动后，config 应该被传递到 init()，工厂创建 adapter 时使用
        let start_result = manager.start("test-mock", config.clone()).await.unwrap();
        assert!(start_result.ok);
        assert!(start_result.pending);

        // 等待后台连接完成
        wait_connected(&manager, "test-mock").await;

        // get_status 验证状态
        let status = manager.get_status("test-mock").await.unwrap();
        assert_eq!(status.state, AdapterState::Connected);
    }

    #[tokio::test]
    async fn test_has_connected_after_start() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        assert!(!manager.has_connected().await);

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        assert!(manager.has_connected().await);
    }

    #[tokio::test]
    async fn test_send_message_delegation() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

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
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;
        assert!(manager.has_connected().await);

        manager.stop_all().await;
        // stop_all 清空 adapters map, 取消 pending_connections
        assert!(!manager.has_connected().await);
    }

    #[tokio::test]
    async fn test_start_all_skips_disabled() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let mut configs = HashMap::new();
        configs.insert(
            "test-mock".to_string(),
            AdapterConfig {
                enabled: Some(false),
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
    async fn test_start_all_disabled_by_config_overrides_env_vars() {
        // 模拟真实场景：.env 中已设置凭据（候选 auto-enable），
        // 但 gateway.local.yaml 中 enabled: false → 应跳过
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let registry = manager.registry();
        registry
            .register(
                "cred-mock",
                "Cred Mock",
                Arc::new(|config| {
                    Box::pin(async move {
                        let mut adapter = MockTestAdapter::new();
                        adapter.platform = "cred-mock".into();
                        let result = adapter.init(config).await.map_err(|e| e.to_string())?;
                        if !result.ok {
                            return Err(result.error.unwrap_or_default());
                        }
                        let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
                        Ok(boxed)
                    })
                }),
                &["CRED_MOCK_TOKEN"],
            )
            .await;

        // 设置凭据 — auto-enable 会因此尝试启动，但 enabled: false 优先
        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::set_var("CRED_MOCK_TOKEN", "some-token") };

        let mut configs = HashMap::new();
        configs.insert(
            "cred-mock".to_string(),
            AdapterConfig {
                enabled: Some(false),
                token: None,
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            },
        );

        let result = manager.start_all(configs).await;
        assert!(
            !result.succeeded.contains(&"cred-mock".to_string()),
            "cred-mock should NOT start when enabled:false in config \
             even though env vars are set for auto-enable; succeeded: {:?}",
            result.succeeded
        );

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("CRED_MOCK_TOKEN") };
    }

    #[tokio::test]
    async fn test_start_publishes_adapter_connected() {
        let event_bus = Arc::new(EventBus::new());
        let mut rx = event_bus.subscribe(event_types::ADAPTER_CONNECTED);
        let manager = Arc::new(AdapterManager::new().with_event_bus(event_bus));
        AdapterManager::init_self_ref(&manager).await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();

        // ADAPTER_CONNECTED 现在由后台任务发布
        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
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
        let manager = Arc::new(AdapterManager::new().with_event_bus(event_bus));
        AdapterManager::init_self_ref(&manager).await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        manager.stop("test-mock").await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive ADAPTER_DISCONNECTED")
            .expect("event should be valid");

        assert_eq!(event.event_type, event_types::ADAPTER_DISCONNECTED);
        assert_eq!(event.source, "adapter_manager");
    }

    // ── inject_credentials 测试 ───────────────────────────────

    #[tokio::test]
    async fn test_inject_credentials_populates_token_and_extra() {
        let manager = AdapterManager::new();
        let registry = manager.registry();
        let factory: AdapterFactory = Arc::new(|_config| {
            Box::pin(async move {
                let adapter = MockTestAdapter::new();
                let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
                Ok(boxed)
            })
        });
        // platform "test" → prefix "TEST_" → env vars must start with "TEST_"
        registry
            .register("test", "Test", factory, &["TEST_APP_ID", "TEST_APP_SECRET"])
            .await;

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::set_var("TEST_APP_ID", "app-id-123") };
        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::set_var("TEST_APP_SECRET", "secret-456") };

        let mut config = AdapterConfig {
            enabled: None,
            token: None,
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };

        manager.inject_credentials("test", &mut config).await;

        // token 应为最后一个凭据变量（SECRET）
        assert_eq!(config.token.as_deref(), Some("secret-456"));
        // extra: 去掉平台前缀 TEST_ 后的小写 key
        assert_eq!(config.extra["app_id"], "app-id-123");
        assert_eq!(config.extra["app_secret"], "secret-456");

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("TEST_APP_ID") };
        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("TEST_APP_SECRET") };
    }

    #[tokio::test]
    async fn test_inject_credentials_does_not_overwrite_existing_token() {
        let manager = AdapterManager::new();
        let registry = manager.registry();
        let factory: AdapterFactory = Arc::new(|_config| {
            Box::pin(async move {
                let adapter = MockTestAdapter::new();
                let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
                Ok(boxed)
            })
        });
        registry
            .register("toktest", "TokTest", factory, &["TOKTEST_TOKEN"])
            .await;

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::set_var("TOKTEST_TOKEN", "from-env") };

        let mut config = AdapterConfig {
            enabled: None,
            token: Some("explicit-token".to_string()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };

        manager.inject_credentials("toktest", &mut config).await;

        // 显式设置的 token 保持不变
        assert_eq!(config.token.as_deref(), Some("explicit-token"));
        // extra 仍会被填充，key 为去前缀后的小写: TOKTEST_TOKEN → strip TOKTEST_ → TOKEN → lowercase → token
        assert_eq!(config.extra["token"], "from-env");

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("TOKTEST_TOKEN") };
    }

    #[tokio::test]
    async fn test_start_all_auto_enables_with_injected_credentials() {
        let manager = new_manager().await;
        let registry = manager.registry();
        let factory: AdapterFactory = Arc::new(|config| {
            Box::pin(async move {
                let mut adapter = MockTestAdapter::new();
                // 验证 config 已被注入凭据
                let init_result = adapter.init(config).await.map_err(|e| e.to_string())?;
                if !init_result.ok {
                    return Err(init_result.error.unwrap_or_default());
                }
                let boxed: Box<dyn PlatformAdapter> = Box::new(adapter);
                Ok(boxed)
            })
        });
        registry
            .register("autotest", "AutoTest", factory, &["AUTOTEST_TOKEN"])
            .await;

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::set_var("AUTOTEST_TOKEN", "my-token") };

        // 不传入任何 config — start_all 应自动检测并注入凭据
        let result = manager.start_all(HashMap::new()).await;
        assert!(
            result.succeeded.contains(&"autotest".to_string()),
            "autotest should be auto-enabled: {:?}",
            result
        );

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("AUTOTEST_TOKEN") };
    }

    // ── compute_backoff 测试 ───────────────────────────────────

    #[test]
    fn test_compute_backoff_sequence() {
        assert_eq!(compute_backoff(0), Duration::from_secs(5));
        assert_eq!(compute_backoff(1), Duration::from_secs(10));
        assert_eq!(compute_backoff(2), Duration::from_secs(30));
        assert_eq!(compute_backoff(3), Duration::from_secs(60));
        assert_eq!(compute_backoff(4), Duration::from_secs(120));
    }

    #[test]
    fn test_compute_backoff_capped_at_300s() {
        assert_eq!(compute_backoff(5), Duration::from_secs(300));
        assert_eq!(compute_backoff(10), Duration::from_secs(300));
        assert_eq!(compute_backoff(100), Duration::from_secs(300));
    }

    // ── config storage 测试 ────────────────────────────────────

    #[tokio::test]
    async fn test_config_stored_on_start_cleared_on_stop() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("test-token".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({"key": "val"}),
        };

        // Start — config should be stored immediately by start()
        manager.start("test-mock", config.clone()).await.unwrap();
        wait_connected(&manager, "test-mock").await;
        {
            let configs = manager.configs.read().await;
            assert!(configs.contains_key("test-mock"));
            assert_eq!(
                configs.get("test-mock").unwrap().token.as_deref(),
                Some("test-token")
            );
            assert_eq!(configs.get("test-mock").unwrap().extra["key"], "val");
        }

        // Stop — config should be cleared
        manager.stop("test-mock").await.unwrap();
        {
            let configs = manager.configs.read().await;
            assert!(!configs.contains_key("test-mock"));
        }
    }

    #[tokio::test]
    async fn test_stop_all_clears_configs() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        // Verify config was stored
        assert!(!manager.configs.read().await.is_empty());

        manager.stop_all().await;

        // stop_all now clears configs as well
        assert!(manager.configs.read().await.is_empty());
    }

    #[tokio::test]
    async fn test_list_statuses_after_start() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        // Before start: statuses should be empty
        let statuses = manager.list_statuses().await;
        assert!(statuses.is_empty());

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        // After start: statuses should include the adapter
        let statuses = manager.list_statuses().await;
        assert!(!statuses.is_empty());
        let status = statuses.iter().find(|s| s.platform == "test-mock");
        assert!(status.is_some());
        assert!(status.unwrap().connected);
    }

    #[tokio::test]
    async fn test_list_statuses_after_stop() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        let _ = manager.stop("test-mock").await;

        // After stop: statuses should reflect disconnected state
        let statuses = manager.list_statuses().await;
        let status = statuses.iter().find(|s| s.platform == "test-mock");
        assert!(status.is_some());
        assert!(!status.unwrap().connected);
    }

    // ── health / status_summary 相关测试 ─────────────────────

    #[tokio::test]
    async fn test_list_statuses_preserves_terminal_failed_state() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        // Start adapter and wait for connection
        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        // Simulate the race condition: health monitor calls set_status_failed
        // during a transport retry while the adapter is still in the adapters
        // map (Tier 1 permanent failure path). The adapter instance hasn't
        // been removed yet, but the cache has the Failed state.
        manager
            .set_status_failed("test-mock", "permanent auth failure")
            .await;

        // list_statuses should preserve the Failed state from cache,
        // NOT overwrite it with the live adapter's self-reported Connected.
        let statuses = manager.list_statuses().await;
        let test_status = statuses.iter().find(|s| s.platform == "test-mock").unwrap();
        assert_eq!(
            test_status.state,
            AdapterState::Failed,
            "terminal Failed state should be preserved"
        );
        assert_eq!(
            test_status.last_error.as_deref(),
            Some("permanent auth failure")
        );
    }

    #[tokio::test]
    async fn test_set_status_failed_preserves_display_name() {
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        // First, populate the cache with a real display name via start
        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        // Verify the display name is correct
        {
            let statuses = manager.statuses.read().await;
            let s = statuses.get("test-mock").unwrap();
            assert_eq!(s.display_name, "Test Mock");
        }

        // Simulate set_status_failed
        manager
            .set_status_failed("test-mock", "permanent auth error")
            .await;

        // Verify display_name is preserved, not overwritten with "test-mock"
        let statuses = manager.statuses.read().await;
        let s = statuses.get("test-mock").unwrap();
        assert_eq!(s.state, AdapterState::Failed);
        assert_eq!(s.display_name, "Test Mock");
        assert_eq!(s.last_error.as_deref(), Some("permanent auth error"));
    }

    #[tokio::test]
    async fn test_health_status_computation() {
        // Test the default health_status() implementation on the trait.
        // MockTestAdapter uses the default health_status() which checks:
        //   - Connected + no heartbeat mechanism → Healthy
        //   - Connected + fresh heartbeat → Healthy
        //   - Connected + stale heartbeat → Degraded
        //   - Anything else → Down
        //
        // MockTestAdapter does NOT override heartbeat_age_ms() (returns None),
        // so when Connected, health_status() returns Healthy.
        // When Created, it returns Down.

        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        // Before start: state is Created → health_status() → Down
        let s = manager.get_status("test-mock").await;
        // No status yet (adapter never started)
        assert!(s.is_none());

        // Start and connect
        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        // After connect: state is Connected, no heartbeat → Healthy
        let adapters = manager.adapters.read().await;
        let adapter = adapters.get("test-mock").unwrap();
        let health = adapter.health().await;
        assert_eq!(health.status, HealthStatus::Healthy);
        assert!(health.connected);
    }

    #[tokio::test]
    async fn test_list_statuses_reflects_live_adapter_health() {
        // When an adapter is connected and healthy, list_statuses should
        // return the live status with correct health (not None).
        let manager = new_manager().await;
        register_mock_adapter(&manager).await;

        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("t".into()),
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        };
        manager.start("test-mock", config).await.unwrap();
        wait_connected(&manager, "test-mock").await;

        let statuses = manager.list_statuses().await;
        let s = statuses.iter().find(|s| s.platform == "test-mock").unwrap();
        assert_eq!(s.state, AdapterState::Connected);
        assert!(s.connected);
        // For the mock adapter (no heartbeat mechanism), Connected → Healthy
        assert_eq!(s.health, Some(HealthStatus::Healthy));
    }
}

/// 启动适配器结果
///
/// `ok: true` 表示 init 成功、连接已发起（后台进行中）。
/// `pending: true` 表示连接尚未完成（正在后台执行）。
/// 调用者可通过轮询 `get_status()` 等待状态变为 Connected 或 Failed。
#[derive(Debug)]
pub struct StartAdapterResult {
    pub ok: bool,
    pub pending: bool,
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
