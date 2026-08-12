#![allow(missing_docs)]

//! 飞书/Lark 平台适配器
//!
//! 使用飞书开放平台 API 实现消息收发。
//! - 发送: HTTP REST API (im/v1/messages)
//! - 接收: 事件订阅 (WebSocket 长连接 / Webhook)

mod event;
mod types;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use easybot_core::bus::EventBus;
use easybot_core::capabilities;
use easybot_core::http::{CachedTokenStore, TokenFetcher};
use easybot_core::types::adapter::*;
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::event_types;
use easybot_core::types::message::*;
use larksuite_oapi_sdk_rs::{EventDispatcher, LarkClient};
use tokio::sync::broadcast;
use types::*;

/// 飞书开放平台 API 基础 URL
const FEISHU_API: &str = "https://open.feishu.cn/open-apis";

/// Token 刷新阈值（秒），在过期前提前刷新
const TOKEN_REFRESH_MARGIN: u64 = 300;

/// 角色缓存容量上限（防止无界增长，参考 QQ CHAT_TYPE_CACHE_LIMIT 模式）
const ROLE_CACHE_LIMIT: usize = 10_000;

/// 角色缓存 TTL（秒），过期后惰性淘汰 + 插入时主动淘汰
const ROLE_CACHE_TTL_SECS: u64 = 30;

/// 用户昵称缓存容量上限（防止无界增长，参考 ROLE_CACHE_LIMIT 模式）
const USER_NAME_CACHE_LIMIT: usize = 10_000;

/// 用户昵称缓存 TTL（秒）。
///
/// 把 contact/v3/users 查询从"每条消息一次"摊薄到 TTL 窗口（飞书频控
/// 1000 次/min、50 次/s）；失败/无名字结果也短缓存（负缓存）防失败风暴。
const USER_NAME_CACHE_TTL_SECS: u64 = 30;

/// 事件去重器：按 `header.event_id` 记录最近处理过的事件，TTL 内重复事件丢弃。
///
/// 平台在超时未收到 ACK 时会重推事件（慢 handler 触发），需幂等去重。
/// 有界：容量上限 + 插入时淘汰过期项，防止重推风暴导致无界增长。
struct EventDedup {
    inner: Mutex<HashMap<String, Instant>>,
}

impl EventDedup {
    /// 容量上限（参考 QQ CHAT_TYPE_CACHE_LIMIT 模式）
    const CAP: usize = 10_000;
    /// 重推窗口：飞书通常几分钟内重推，5 分钟足够覆盖
    const TTL: Duration = Duration::from_secs(300);

    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// 返回 `true` 表示事件应处理；`false` 表示重复事件（已处理过）。
    fn check_and_record(&self, event_id: &str) -> bool {
        if event_id.is_empty() {
            return true; // 无 event_id 不参与去重
        }
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        // 插入时主动淘汰过期项，避免惰性清理导致的突增
        map.retain(|_, t| t.elapsed() < Self::TTL);
        if map.contains_key(event_id) {
            return false;
        }
        if map.len() >= Self::CAP {
            // 容量上限：移除最旧的一项（近似 FIFO）
            if let Some(oldest) = map.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone()) {
                map.remove(&oldest);
            }
        }
        map.insert(event_id.to_string(), Instant::now());
        true
    }
}

/// 飞书 tenant_access_token 共享存储（`CachedTokenStore` 薄包装）
///
/// `inner: None` 直到 `connect()` 构建存储（此时冻结 app_id/app_secret）。
/// 适配器实例和 WebSocket 后台任务共用同一实例，单飞行刷新共享一次鉴权调用。
#[derive(Clone, Default)]
struct FeishuTokenStore {
    inner: Option<CachedTokenStore>,
}

impl FeishuTokenStore {
    fn new() -> Self {
        Self::default()
    }

    /// 返回 `Bearer {token}` 鉴权字符串；缺失/临界时自动刷新（单飞行）。
    async fn get(&self) -> Result<String, GatewayError> {
        let store = self.inner.as_ref().ok_or_else(|| {
            GatewayError::ConfigError(
                "Feishu token store not initialized (call connect() first)".to_string(),
            )
        })?;
        store.get().await.map(|t| format!("Bearer {t}"))
    }
}

/// 从飞书鉴权端点获取 tenant_access_token，返回 `(token, ttl_secs)`。
///
/// ★ 错误分类（2026 协议调研）：
/// - 网络/解析层失败 → 瞬态 `Transient`（重试安全）
/// - 凭据类错误（鉴权端点只会因 app_id/app_secret 问题返回 token 无效码）→ 永久 `AuthFailed`
/// - 其他非零 code → 保守按 `Internal` 处理
async fn feishu_fetch_access_token(
    client: &reqwest::Client,
    app_id: &str,
    app_secret: &str,
    base_url: &str,
) -> Result<(String, u64), GatewayError> {
    let url = format!("{}/auth/v3/tenant_access_token/internal", base_url);

    let resp: FeishuTokenResponse = client
        .post(&url)
        .json(&serde_json::json!({
            "app_id": app_id,
            "app_secret": app_secret,
        }))
        .send()
        .await
        // 网络层失败（DNS/超时/拒连）是瞬态错误——健康监控应退避重试。
        .map_err(|e| GatewayError::Transient(format!("Feishu token refresh failed: {}", e)))?
        .json()
        .await
        .map_err(|e| {
            GatewayError::Transient(format!("Feishu token refresh parse failed: {}", e))
        })?;

    if resp.code != 0 {
        // 鉴权端点返回 token 无效码 = 凭据被拒 → 永久停用（健康监控不应无限重试）。
        const CREDENTIAL_CODES: &[i64] = &[99991663, 99991664, 99991665, 20013, 20005, 4001];
        let msg = resp.msg.unwrap_or_default();
        if CREDENTIAL_CODES.contains(&resp.code) {
            return Err(GatewayError::AuthFailed(format!(
                "Feishu auth rejected (code {}): {}",
                resp.code, msg
            )));
        }
        // 非凭据错误码（系统繁忙 99999、限流等）→ 瞬态错误，健康监控应退避重试。
        // 消息刻意不含 "auth failed" 字样，避免启发式分类误判为永久凭据错误。
        return Err(GatewayError::Transient(format!(
            "Feishu token endpoint error (code {}): {}",
            resp.code, msg
        )));
    }

    let token = resp.tenant_access_token.ok_or_else(|| {
        // 成功 code 却缺 token —— 凭据/响应问题，保守按永久。
        GatewayError::AuthFailed("Feishu token refresh: no tenant_access_token".to_string())
    })?;
    let expire = resp.expire.unwrap_or(7200);

    Ok((token, expire))
}

/// ★ 飞书 token 失效检测（2026 协议调研，官方通用错误码表）。
///
/// 命中即"token 无效/过期" → 强制刷新后重试一次：
/// - `99991663`：tenant token 过期 / 该 API 不支持 tenant token
/// - `99991665`：tenant_access_token 非法
/// - `20013`：tenant access token 无效
/// - `20005`：access_token 无效/过期（通用）
///
/// 明确排除（不触发重试）：
/// - `99991661`：缺少 token（header 缺失，代码 bug）
/// - `99991664`：是 **app_access_token** 非法，不是 tenant
fn is_feishu_token_invalid_code(code: i64) -> bool {
    matches!(code, 99991663 | 99991665 | 20013 | 20005)
}

/// 适配 `send_with_token_retry` 的谓词：飞书 token 被拒信号统一用
/// `GatewayError::Unauthorized`（→ HTTP 401）表示。
fn is_feishu_token_invalid(e: &GatewayError) -> bool {
    matches!(e, GatewayError::Unauthorized(_))
}

/// 飞书适配器
pub struct FeishuAdapter {
    platform_name: String,
    display_name: String,
    config: Option<AdapterConfig>,
    state: AdapterState,
    bot_info: Option<BotInfo>,
    capabilities: Vec<Capability>,
    messages_in: Arc<AtomicU64>,
    messages_out: AtomicU64,
    errors: AtomicU64,
    event_bus: Option<Arc<EventBus>>,
    cancel_tx: Option<broadcast::Sender<()>>,
    /// Background liveness heartbeat (updated by the WebSocket task)
    heartbeat: Heartbeat,
    /// 缓存的 HTTP 客户端（OnceLock 延迟初始化，与 Telegram 适配器模式一致）
    http_client: std::sync::OnceLock<reqwest::Client>,
    /// 共享的 access token 存储（适配器 + WebSocket 后台任务共用）
    token_store: FeishuTokenStore,
    /// open_id → 昵称 缓存（含负缓存）。
    /// 把 contact/v3/users 调用摊薄到 TTL 窗口，避免 p2p/群消息每条都打联系人 API。
    user_name_cache: Mutex<HashMap<String, (Option<String>, Instant)>>,
}

impl FeishuAdapter {
    pub fn new() -> Self {
        Self {
            platform_name: "feishu".to_string(),
            display_name: "飞书".to_string(),
            config: None,
            state: AdapterState::Created,
            bot_info: None,
            capabilities: capabilities![
                (Text, true),
                (Image, true),
                (Audio, true),
                (Video, true),
                (Document, true),
                (Interactive, true),
                (Markdown, true),
                (Group, true),
                (MessageEdit, true),
                (MessageDelete, true),
            ],
            messages_in: Arc::new(AtomicU64::new(0)),
            messages_out: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            event_bus: None,
            cancel_tx: None,
            heartbeat: Heartbeat::new(),
            http_client: std::sync::OnceLock::new(),
            token_store: FeishuTokenStore::new(),
            user_name_cache: Mutex::new(HashMap::new()),
        }
    }

    /// 设置事件总线（在 init 之前调用）
    pub fn set_event_bus(&mut self, event_bus: Arc<EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// 获取或创建 HTTP 客户端（OnceLock 延迟初始化，首次调用时创建）
    fn client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap_or_else(|e| {
                    tracing::error!("Failed to create Feishu HTTP client: {}", e);
                    // Fallback to default client — connection will fail later
                    // with a more descriptive error rather than panicking
                    reqwest::Client::new()
                })
        })
    }

    /// 返回 API 基础 URL（支持通过 config.base_url 覆盖）
    fn api_base_url(&self) -> &str {
        self.config
            .as_ref()
            .and_then(|c| c.base_url.as_deref())
            .unwrap_or(FEISHU_API)
    }

    /// 返回 SDK（larksuite-oapi-sdk-rs）使用的基础 URL。
    ///
    /// SDK 约定 base_url 不含 `/open-apis` 后缀（HTTP 路径自带 /open-apis，
    /// WS 端点为 `{domain}/callback/ws/endpoint`），而 `api_base_url()` 含
    /// `/open-apis`，因此需要剥离尾缀。
    fn sdk_base_url(&self) -> String {
        self.api_base_url()
            .trim_end_matches('/')
            .trim_end_matches("/open-apis")
            .to_string()
    }

    /// 统一的飞书 API 请求方法。
    ///
    /// 通过 `send_with_token_retry` 包装：token 缺失/临界时单飞行刷新；
    /// 若 API 返回 token 失效信号（HTTP 401 / code 99991663 等）→ 强制刷新一次再重试。
    async fn send_api_request<T: serde::de::DeserializeOwned + Send>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, GatewayError> {
        let store = self.token_store.inner.as_ref().ok_or_else(|| {
            GatewayError::ConfigError(
                "Feishu token store not initialized (call connect() first)".to_string(),
            )
        })?;
        easybot_core::http::send_with_token_retry(store, is_feishu_token_invalid, |token| {
            let method = method.clone();
            let auth = format!("Bearer {token}");
            Box::pin(async move { self.send_api_request_raw(&auth, method, path, body).await })
        })
        .await
    }

    /// 实际的飞书 API 请求（不管理 token）。token 被拒信号在 raw 层统一为
    /// `GatewayError::Unauthorized`，由上层谓词触发刷新重试。
    async fn send_api_request_raw<T: serde::de::DeserializeOwned>(
        &self,
        auth: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<T, GatewayError> {
        let client = self.client();
        let url = format!("{}{}", self.api_base_url(), path);

        let mut req = client
            .request(method.clone(), &url)
            .header("Authorization", auth);
        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| GatewayError::Transient(format!("Feishu {} failed: {}", method, e)))?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(GatewayError::Unauthorized(format!(
                "Feishu {} {}: 401 Unauthorized",
                method, path
            )));
        }

        let result: FeishuApiResponse<T> = resp.json().await.map_err(|e| {
            GatewayError::Internal(format!("Feishu {} parse failed: {}", method, e))
        })?;

        if result.code != 0 {
            if is_feishu_token_invalid_code(result.code) {
                return Err(GatewayError::Unauthorized(format!(
                    "Feishu API error ({} {}): {} (code {})",
                    method,
                    path,
                    result.msg.unwrap_or_default(),
                    result.code
                )));
            }
            return Err(GatewayError::Internal(format!(
                "Feishu API error ({} {}): {} (code {})",
                method,
                path,
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        result.data.ok_or_else(|| {
            GatewayError::Internal(format!(
                "Feishu API returned no data for {} {}",
                method, path
            ))
        })
    }

    /// 飞书 API GET 请求
    async fn api_get<T: serde::de::DeserializeOwned + Send>(
        &self,
        path: &str,
    ) -> Result<T, GatewayError> {
        self.send_api_request::<T>(reqwest::Method::GET, path, None)
            .await
    }

    /// 飞书 API POST 请求
    async fn api_post<T: serde::de::DeserializeOwned + Send>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        self.send_api_request::<T>(reqwest::Method::POST, path, Some(body))
            .await
    }

    /// 飞书 API PUT 请求
    async fn api_put<T: serde::de::DeserializeOwned + Send>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GatewayError> {
        self.send_api_request::<T>(reqwest::Method::PUT, path, Some(body))
            .await
    }

    /// 飞书 API DELETE 请求（不要求 data 字段）。
    ///
    /// 与 `send_api_request` 同款 token 刷新重试包装。
    async fn api_delete(&self, path: &str) -> Result<(), GatewayError> {
        let store = self.token_store.inner.as_ref().ok_or_else(|| {
            GatewayError::ConfigError(
                "Feishu token store not initialized (call connect() first)".to_string(),
            )
        })?;
        easybot_core::http::send_with_token_retry(store, is_feishu_token_invalid, |token| {
            let auth = format!("Bearer {token}");
            Box::pin(async move { self.api_delete_raw(&auth, path).await })
        })
        .await
    }

    /// 实际的飞书 API DELETE 请求（不管理 token）。
    async fn api_delete_raw(&self, auth: &str, path: &str) -> Result<(), GatewayError> {
        let client = self.client();
        let url = format!("{}{}", self.api_base_url(), path);

        let resp = client
            .delete(&url)
            .header("Authorization", auth)
            .send()
            .await
            .map_err(|e| GatewayError::Transient(format!("Feishu DELETE failed: {}", e)))?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(GatewayError::Unauthorized(format!(
                "Feishu DELETE {}: 401 Unauthorized",
                path
            )));
        }

        let result: FeishuApiResponse<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu DELETE parse failed: {}", e)))?;

        if result.code != 0 {
            if is_feishu_token_invalid_code(result.code) {
                return Err(GatewayError::Unauthorized(format!(
                    "Feishu API error (DELETE {}): {} (code {})",
                    path,
                    result.msg.unwrap_or_default(),
                    result.code
                )));
            }
            return Err(GatewayError::Internal(format!(
                "Feishu API error (DELETE {}): {} (code {})",
                path,
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        Ok(())
    }
}
#[async_trait]
impl PlatformAdapter for FeishuAdapter {
    fn platform_name(&self) -> &str {
        &self.platform_name
    }

    fn display_name(&self) -> &str {
        &self.display_name
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        let extra = &config.extra;
        let app_id = extra.get("app_id").and_then(|v| v.as_str());
        let app_secret = config.token.as_deref();

        if app_id.is_none() || app_secret.is_none() {
            return Ok(InitResult {
                ok: false,
                error: Some("飞书适配器需要配置 extra.app_id 和 token (app_secret)".to_string()),
            });
        }

        self.config = Some(config);
        self.state = AdapterState::Starting;

        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        // 1. 获取配置并构建 token store（fetch 闭包在此时冻结 app_id/app_secret/base_url）
        let config = self.config.as_ref().ok_or_else(|| {
            GatewayError::Internal("connect() called before init() — config not set".into())
        })?;
        let app_id = config
            .extra
            .get("app_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                GatewayError::ConfigError("Missing 'app_id' in feishu config.extra".to_string())
            })?;
        let app_secret = config.token.as_deref().ok_or_else(|| {
            GatewayError::ConfigError("Missing 'token' (app_secret) for feishu".to_string())
        })?;
        let base_url = self.api_base_url();

        let client = self.client().clone();
        let app_id_owned = app_id.to_string();
        let app_secret_owned = app_secret.to_string();
        let base_url_owned = base_url.to_string();
        let fetch: TokenFetcher = Box::new(move || {
            // 每次调用 clone 凭据供 async move 使用（刷新低频，clone 开销可忽略）
            let client = client.clone();
            let app_id = app_id_owned.clone();
            let app_secret = app_secret_owned.clone();
            let base_url = base_url_owned.clone();
            Box::pin(async move {
                feishu_fetch_access_token(&client, &app_id, &app_secret, &base_url).await
            })
        });
        self.token_store.inner = Some(CachedTokenStore::new(
            fetch,
            Duration::from_secs(TOKEN_REFRESH_MARGIN),
        ));

        // 获取 access token 验证凭证（失败时 store 仍保留，connect 重试可复用）
        self.token_store.get().await?;

        self.state = AdapterState::Connected;
        self.bot_info = Some(BotInfo {
            name: app_id.to_string(),
            username: Some("feishu_bot".to_string()),
            id: app_id.to_string(),
        });

        tracing::info!("飞书适配器已连接");
        self.heartbeat.record_connection();

        // 3. 如果配置了 EventBus，启动 WebSocket 事件订阅
        self.cancel_and_respawn_ws().await;

        Ok(ConnectResult::ok(self.bot_info.clone()))
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        // 发送取消信号停止 WebSocket 事件订阅
        if let Some(cancel_tx) = &self.cancel_tx {
            let _ = cancel_tx.send(());
            tracing::info!("飞书 WebSocket 事件订阅停止信号已发送");
        }
        self.cancel_tx = None;
        self.state = AdapterState::Stopped;
        tracing::info!("飞书适配器已断开");
        Ok(())
    }

    /// 传输层重试：取消旧 WebSocket 任务并启动新任务，不重新鉴权。
    ///
    /// 跳过 token 验证（token_store 中已有有效 token），仅重建
    /// SDK Client 和 WebSocket 连接，比完整 stop+start 更轻量。
    async fn retry_transport(&mut self) -> Result<bool, GatewayError> {
        if self.event_bus.is_none() {
            return Ok(false);
        }
        // Cancel old WebSocket task and spawn a new one, reusing the
        // cached token_store (no re-auth network call needed).
        self.cancel_and_respawn_ws().await;
        tracing::info!("飞书 transport retry: WebSocket 任务已重启");
        Ok(true)
    }

    fn state(&self) -> AdapterState {
        self.state.clone()
    }

    fn heartbeat_age_ms(&self) -> Option<i64> {
        Some(self.heartbeat.age_ms())
    }

    fn heartbeat_success_age_ms(&self) -> Option<i64> {
        Some(self.heartbeat.last_success_age_ms())
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: self
                .config
                .as_ref()
                .map(|c| c.enabled != Some(false))
                .unwrap_or(false),
            token_configured: self
                .config
                .as_ref()
                .and_then(|c| c.token.as_ref())
                .is_some(),
            extra: self
                .config
                .as_ref()
                .map(|c| c.extra.clone())
                .unwrap_or_default(),
        }
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            status: self.health_status(),
            connected: self.state == AdapterState::Connected,
            last_connected_at: None,
            last_error_at: None,
            last_error: None,
            messages_in: self.messages_in.load(Ordering::Relaxed),
            messages_out: self.messages_out.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            uptime: self.heartbeat.uptime_secs().into(),
        }
    }

    fn status_summary(&self) -> AdapterStatusSummary {
        AdapterStatusSummary {
            platform: self.platform_name.clone(),
            display_name: self.display_name.clone(),
            state: self.state.clone(),
            connected: self.state == AdapterState::Connected,
            health: Some(self.health_status()),
            last_error: self.heartbeat.last_error_str(),
            uptime: self.heartbeat.uptime_secs().into(),
            messages_in: self.messages_in.load(Ordering::Relaxed),
            messages_out: self.messages_out.load(Ordering::Relaxed),
        }
    }

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        let content = serde_json::json!({
            "text": params.message.text,
        });

        // 飞书 send message API: POST /open-apis/im/v1/messages
        // Query param: receive_id_type=chat_id
        let path = "/im/v1/messages?receive_id_type=chat_id".to_string();

        let body = serde_json::json!({
            "receive_id": params.chat_id,
            "msg_type": "text",
            "content": content.to_string(),
        });

        let send_result = match self.api_post::<FeishuSendMessageData>(&path, &body).await {
            Ok(data) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(data.message_id),
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    error: None,
                    error_code: None,
                    retryable: false,
                }
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                SendResult::fail(e.to_string(), true)
            }
        };
        if let Some(bus) = &self.event_bus {
            bus.publish_send_result(
                if send_result.success {
                    event_types::MESSAGE_SENT
                } else {
                    event_types::MESSAGE_FAILED
                },
                "feishu",
                &params.chat_id,
                &send_result,
            );
        }
        Ok(send_result)
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        // 飞书媒体发送协议（2026 核对官方文档）：
        // - 图片/动图 → POST /im/v1/images（image_type=message）→ image_key，
        //   消息 content = {"image_key": ...}
        // - 音频 → POST /im/v1/files（file_type=opus）→ file_key，
        //   消息 content = {"file_key": ..., "duration": ...}
        // - 视频 → POST /im/v1/files（file_type=mp4）→ file_key，
        //   消息 content = {"file_key": ...}（封面 image_key 官方为可选，不额外上传）
        // - 文件 → POST /im/v1/files（file_type 按扩展名映射 pdf/doc/xls/ppt/stream）→ file_key，
        //   消息 content = {"file_key": ..., "file_name": ...}
        // - 贴纸 → 飞书无官方上传通道（/im/v1/files 无 sticker file_type），明确拒绝。
        let (content, msg_type) = match params.media.media_type {
            MediaType::Image | MediaType::Animation => {
                let image_key = self.upload_image(&params.media).await?;
                (serde_json::json!({ "image_key": image_key }), "image")
            }
            MediaType::Audio => {
                let file_key = self.upload_file(&params.media, "opus").await?;
                let mut content = serde_json::json!({ "file_key": file_key });
                if let Some(dur) = params.media.duration {
                    // duration 单位为毫秒
                    content["duration"] = serde_json::json!((dur * 1000.0) as i64);
                }
                (content, "audio")
            }
            MediaType::Video => {
                let file_key = self.upload_file(&params.media, "mp4").await?;
                (serde_json::json!({ "file_key": file_key }), "media")
            }
            MediaType::Document => {
                let file_type =
                    Self::feishu_file_type(params.media.filename.as_deref().unwrap_or("file"));
                let file_key = self.upload_file(&params.media, file_type).await?;
                (
                    serde_json::json!({
                        "file_key": file_key,
                        "file_name": params.media.filename.clone().unwrap_or_default(),
                    }),
                    "file",
                )
            }
            MediaType::Sticker => {
                // 贴纸：无官方上传方式，明确以 capability 拒绝（不声明 Sticker 能力）。
                return Err(GatewayError::capability_not_supported(
                    "feishu send_media sticker",
                ));
            }
        };

        let path = "/im/v1/messages?receive_id_type=chat_id".to_string();

        let body = serde_json::json!({
            "receive_id": params.chat_id,
            "msg_type": msg_type,
            "content": content.to_string(),
        });

        let send_result = match self.api_post::<FeishuSendMessageData>(&path, &body).await {
            Ok(data) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(data.message_id),
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    error: None,
                    error_code: None,
                    retryable: false,
                }
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                SendResult::fail(e.to_string(), true)
            }
        };
        if let Some(bus) = &self.event_bus {
            bus.publish_send_result(
                if send_result.success {
                    event_types::MESSAGE_SENT
                } else {
                    event_types::MESSAGE_FAILED
                },
                "feishu",
                &params.chat_id,
                &send_result,
            );
        }
        Ok(send_result)
    }

    async fn send_interactive(
        &self,
        params: SendInteractiveParams,
    ) -> Result<SendResult, GatewayError> {
        let content = serde_json::json!({
            "elements": params.keyboard.rows.iter().map(|row| {
                let buttons: Vec<serde_json::Value> = row.buttons.iter().map(|b| {
                    let text = b.text.clone();
                    let value = b.callback_data.as_ref().map(|d| serde_json::json!({"data": d})).unwrap_or_default();
                    serde_json::json!({
                        "tag": "button",
                        "text": {
                            "tag": "plain_text",
                            "content": text,
                        },
                        "value": value,
                    })
                }).collect();
                if buttons.len() == 1 {
                    buttons.into_iter().next().unwrap()
                } else {
                    serde_json::json!({
                        "tag": "action",
                        "actions": buttons,
                    })
                }
            }).collect::<Vec<_>>(),
        });

        let path = "/im/v1/messages?receive_id_type=chat_id".to_string();
        let body = serde_json::json!({
            "receive_id": params.chat_id,
            "msg_type": "interactive",
            "content": serde_json::to_string(&content).unwrap_or_default(),
        });

        let send_result = match self.api_post::<FeishuSendMessageData>(&path, &body).await {
            Ok(data) => {
                self.messages_out.fetch_add(1, Ordering::Relaxed);
                SendResult {
                    success: true,
                    message_id: Some(data.message_id),
                    timestamp: Some(chrono::Utc::now().timestamp_millis()),
                    error: None,
                    error_code: None,
                    retryable: false,
                }
            }
            Err(e) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                SendResult::fail(e.to_string(), true)
            }
        };
        if let Some(bus) = &self.event_bus {
            bus.publish_send_result(
                if send_result.success {
                    event_types::MESSAGE_SENT
                } else {
                    event_types::MESSAGE_FAILED
                },
                "feishu",
                &params.chat_id,
                &send_result,
            );
        }
        Ok(send_result)
    }

    async fn edit_message(&self, params: EditMessageParams) -> Result<EditResult, GatewayError> {
        // 飞书 open-api: PUT /im/v1/messages/{message_id} (文本编辑)
        // 消息卡片编辑用 PATCH，文本编辑用 PUT
        let path = format!("/im/v1/messages/{}", params.message_id);
        let content = serde_json::json!({
            "text": params.message.text,
        });
        let body = serde_json::json!({
            "content": content.to_string(),
            "msg_type": "text",
        });

        let _: serde_json::Value = self.api_put(&path, &body).await?;

        Ok(EditResult {
            success: true,
            updated_at: Some(chrono::Utc::now().timestamp_millis()),
            error: None,
        })
    }

    async fn delete_message(
        &self,
        _chat_id: &str,
        message_id: &str,
    ) -> Result<DeleteResult, GatewayError> {
        // 飞书开放平台支持撤回自己的消息（24h 内）
        // DELETE /im/v1/messages/{message_id}
        let path = format!("/im/v1/messages/{}", message_id);
        match self.api_delete(&path).await {
            Ok(()) => Ok(DeleteResult {
                success: true,
                error: None,
            }),
            Err(e) => Ok(DeleteResult {
                success: false,
                error: Some(e.to_string()),
            }),
        }
    }

    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError> {
        let path = format!("/im/v1/chats/{}", chat_id);
        let data: FeishuChatInfo = self.api_get(&path).await?;

        Ok(ChatInfo {
            chat_id: chat_id.to_string(),
            name: Some(data.name),
            chat_type: if data.chat_type == "group" {
                ChatType::Group
            } else {
                ChatType::Dm
            },
            member_count: Some(data.member_count as u32),
        })
    }

    async fn list_chats(&self, _filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        let data: FeishuListChatData = self.api_get("/im/v1/chats?page_size=50").await?;

        Ok(data
            .items
            .into_iter()
            .map(|item| ChatInfo {
                chat_id: item.chat_id,
                name: Some(item.name),
                chat_type: if item.chat_type == "group" {
                    ChatType::Group
                } else {
                    ChatType::Dm
                },
                member_count: Some(item.member_count as u32),
            })
            .collect())
    }

    // ── 会话富化 ──

    async fn enrich_source(
        &self,
        source: &easybot_core::types::session::SessionSource,
    ) -> Option<easybot_core::types::session::SessionSource> {
        let mut enriched = source.clone();
        let is_p2p = source.chat_type == ChatType::Dm;

        // 群名：通过飞书群信息 API 获取。
        // p2p 单聊的 `/im/v1/chats/{chat_id}` 不返回 name、也不含对端信息（单聊无群名），
        // 跳过该调用省一半请求；p2p 会话名由下方对端用户昵称填充。
        // 注意：api_base_url() 已含 /open-apis 前缀，path 不能重复带前缀
        if !is_p2p {
            let chat_id = &source.chat_id;
            let chat_path = format!("/im/v1/chats/{}", chat_id);
            if let Ok(chat_info) = self.api_get::<serde_json::Value>(&chat_path).await
                && let Some(name) = chat_info
                    .get("data")
                    .and_then(|d| d.get("name"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
            {
                enriched.chat_name = Some(name);
            }
        }

        // 用户昵称：通过飞书联系人 API 查询（p2p 时 user_id = 对端用户 open_id）。
        // 命中缓存（含负缓存）直接复用，避免每条消息都打联系人 API。
        if let Some(user_id) = &source.user_id {
            let cached = self.user_name_cached(user_id);
            let name = match cached {
                Some(name) => name,
                None => {
                    let user_path = format!("/contact/v3/users/{}", user_id);
                    let name = match self.api_get::<serde_json::Value>(&user_path).await {
                        Ok(user_info) => user_info
                            .get("data")
                            .and_then(|d| d.get("user"))
                            .and_then(|u| u.get("name"))
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string()),
                        Err(_) => None,
                    };
                    // 缓存结果（失败/无名字也短缓存，负缓存防失败风暴）
                    self.cache_user_name(user_id, name.clone());
                    name
                }
            };
            if let Some(name) = &name {
                enriched.user_name = Some(name.clone());
                if is_p2p {
                    // DM 用对端用户名作为会话名，让只读 chat_name 的消费方（如 /chats 端点）也能展示
                    enriched.chat_name = Some(name.clone());
                }
            }
        }

        Some(enriched)
    }
}

// ── 辅助方法 ──

impl FeishuAdapter {
    /// 查询用户昵称缓存。返回 `Some(name)` 表示命中（`name=None` 为负缓存：最近查询失败/无名字）；
    /// `None` 表示未缓存，需要调 API。
    fn user_name_cached(&self, user_id: &str) -> Option<Option<String>> {
        let cache = self
            .user_name_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some((name, expiry)) = cache.get(user_id)
            && expiry.elapsed() < Duration::from_secs(USER_NAME_CACHE_TTL_SECS)
        {
            return Some(name.clone());
        }
        None
    }

    /// 写入用户昵称缓存。超限时先清过期条目，仍超限则逐出最早过期的一项（近似 FIFO），
    /// 保证容量有界（对齐 CLAUDE.md 缓存约束）。
    fn cache_user_name(&self, user_id: &str, name: Option<String>) {
        let mut cache = self
            .user_name_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        cache.insert(user_id.to_string(), (name, Instant::now()));
        if cache.len() > USER_NAME_CACHE_LIMIT {
            cache.retain(|_, (_, t)| t.elapsed() < Duration::from_secs(USER_NAME_CACHE_TTL_SECS));
        }
        if cache.len() > USER_NAME_CACHE_LIMIT
            && let Some(oldest) = cache
                .iter()
                .min_by(|a, b| a.1.1.cmp(&b.1.1))
                .map(|(k, _)| k.clone())
        {
            cache.remove(&oldest);
        }
    }

    /// Cancel the old WebSocket task (if any) and spawn a new one.
    ///
    /// Used by both `connect()` (first connection) and `retry_transport()`
    /// (lightweight reconnect without re-auth).  The token is expected to
    /// already be cached in `self.token_store` — no network call is made
    /// to refresh it.
    async fn cancel_and_respawn_ws(&mut self) {
        let event_bus = match self.event_bus.as_ref() {
            Some(eb) => eb.clone(),
            None => return,
        };

        // Cancel old WebSocket task first
        if let Some(cancel_tx) = self.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }

        let config = match self.config.as_ref() {
            Some(c) => c,
            None => {
                tracing::error!("飞书 cancel_and_respawn_ws: config not set");
                return;
            }
        };
        let app_id_owned = match config.extra.get("app_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => {
                tracing::error!("飞书 cancel_and_respawn_ws: app_id not in config");
                return;
            }
        };
        let app_secret = match config.token.clone() {
            Some(s) => s,
            None => {
                tracing::error!("飞书 cancel_and_respawn_ws: token not in config");
                return;
            }
        };

        let (cancel_tx, mut cancel_rx) = broadcast::channel(1);
        self.cancel_tx = Some(cancel_tx);

        // 角色解析 HTTP 客户端：必须带超时，否则慢请求会阻塞事件 handler
        // （SDK 等待 handler 完成才回 ACK → 拖垮整个 WS 事件流）。
        let feishu_http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|e| {
                tracing::error!("飞书角色解析 HTTP 客户端创建失败: {}", e);
                reqwest::Client::new()
            });
        let feishu_base_url = self.api_base_url().to_string();
        // SDK 约定 base_url 不含 /open-apis 后缀（WS 端点为 {domain}/callback/ws/endpoint）
        let sdk_base_url = self.sdk_base_url();
        let mi = self.messages_in.clone();

        // Create SDK Client (no network I/O — token is fetched lazily)
        // F6: 传入 base_url，使 WS 长连接与私有化/代理/mock 场景对齐。
        let sdk_client = match LarkClient::builder(&app_id_owned, &app_secret)
            .base_url(sdk_base_url.clone())
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("飞书 SDK 客户端创建失败: {}", e);
                return;
            }
        };

        // Role cache shared across event handlers
        let role_cache: Arc<Mutex<HashMap<String, (SenderRole, Instant)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let shared_token_store = self.token_store.clone();
        // 事件去重：按 header.event_id 丢弃平台重推的重复事件
        let event_dedup = Arc::new(EventDedup::new());
        // WS 连接状态（on_ready / on_disconnected 维护），用于门控存活心跳
        let ws_connected = Arc::new(AtomicBool::new(false));

        // Event verification config
        let env_verify_token = std::env::var("FEISHU_VERIFICATION_TOKEN").ok();
        let env_encrypt_key = std::env::var("FEISHU_ENCRYPT_KEY").ok();
        let verify_token = config
            .extra
            .get("verification_token")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| env_verify_token.as_deref().filter(|s| !s.is_empty()))
            .unwrap_or("");
        let encrypt_key = config
            .extra
            .get("encrypt_key")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| env_encrypt_key.as_deref().filter(|s| !s.is_empty()))
            .unwrap_or("");

        let dispatcher = if verify_token.is_empty() && encrypt_key.is_empty() {
            tracing::info!(
                "飞书事件签名验证未配置（WebSocket 连接使用 OAuth 鉴权），跳过 per-event 验证"
            );
            EventDispatcher::new("", "").skip_sign_verify()
        } else {
            EventDispatcher::new(verify_token, encrypt_key)
        };

        let hb_for_events = self.heartbeat.clone();
        let dispatcher = dispatcher
            .on_customized_event(EVENT_MESSAGE_RECEIVE_V1, {
                let rc = role_cache.clone();
                let hb = hb_for_events.clone();
                let eb = event_bus.clone();
                let bot_id = app_id_owned.clone();
                let hc = feishu_http.clone();
                let bu = feishu_base_url.clone();
                let ts = shared_token_store.clone();
                let mi = mi.clone();
                let dedup = event_dedup.clone();
                move |_req: larksuite_oapi_sdk_rs::EventReq,
                      body: larksuite_oapi_sdk_rs::EventV2Body| {
                    let eb = eb.clone();
                    let bot_id = bot_id.clone();
                    let hc = hc.clone();
                    let bu = bu.clone();
                    let ts = ts.clone();
                    let rc = rc.clone();
                    let mi = mi.clone();
                    let hb = hb.clone();
                    let dedup = dedup.clone();
                    async move {
                        // 按 header.event_id 去重（平台超时未 ACK 会重推事件）
                        let event_id = body
                            .header
                            .as_ref()
                            .map(|h| h.event_id.clone())
                            .unwrap_or_default();
                        if !dedup.check_and_record(&event_id) {
                            tracing::debug!("飞书重复事件已丢弃: {}", event_id);
                            return Ok(());
                        }
                        let event_data = body.event.into_value();
                        let sender_role =
                            Self::resolve_feishu_role(&event_data, &hc, &bu, &ts, &rc).await;
                        event::handle_message_receive(event_data, &eb, &bot_id, sender_role).await;
                        mi.fetch_add(1, Ordering::Relaxed);
                        hb.beat_success();
                        Ok(())
                    }
                }
            })
            .on_event("im.chat.updated_v1", {
                let rc = role_cache.clone();
                let hb = hb_for_events.clone();
                move |event_data| {
                    let rc = rc.clone();
                    let hb = hb.clone();
                    async move {
                        if let Some(chat_id) =
                            event_data.pointer("/chat_id").and_then(|v| v.as_str())
                        {
                            if let Ok(mut cache) = rc.lock() {
                                cache.retain(|key, _| !key.starts_with(&format!("{}:", chat_id)));
                            }
                            tracing::info!("飞书群配置变更，已清除群 {} 的角色缓存", chat_id);
                        }
                        hb.beat();
                        Ok(())
                    }
                }
            });

        // F2: 移除"无条件 60s 定时器 beat"。改为连接状态门控：
        // - on_ready（每次连接建立）→ 置位连接标志 + beat
        // - on_disconnected（每次断连）→ 清位连接标志，不再 beat
        // 门控定时器仅在 WS 已连接时 beat → 断连后心跳过期 → health 转
        // Degraded/Down → 健康监控介入（retry_transport / reconnect）。
        let ws_client = sdk_client.ws_client(dispatcher);
        let ws_client = ws_client
            .domain(sdk_base_url)
            .log_level(tracing::Level::WARN)
            .on_ready({
                let connected = ws_connected.clone();
                let hb = hb_for_events.clone();
                move || {
                    connected.store(true, Ordering::Relaxed);
                    hb.beat(); // 连接建立 → 心跳新鲜
                }
            })
            .on_disconnected({
                let connected = ws_connected.clone();
                move || {
                    // 断连后不再 beat：让心跳过期，健康监控能看见死连接
                    connected.store(false, Ordering::Relaxed);
                }
            });

        // Initial heartbeat beat to avoid false Degraded signal during the
        // 120 s window before the first inbound event arrives.
        let hb = self.heartbeat.clone();
        hb.beat();
        tokio::spawn(async move {
            // Liveness heartbeat: fires every 60s ONLY while the WebSocket
            // connection is established (ws_connected set by on_ready).
            // When the connection drops, on_disconnected clears the flag
            // and the ticker stops beating → heartbeat expires.
            let mut liveness = tokio::time::interval(Duration::from_secs(60));
            liveness.tick().await; // skip the immediate first tick

            let ws_fut = ws_client.start();
            tokio::pin!(ws_fut);

            loop {
                tokio::select! {
                    _ = cancel_rx.recv() => {
                        tracing::info!("飞书 WebSocket 事件订阅已停止");
                        break;
                    }
                    _ = liveness.tick() => {
                        if ws_connected.load(Ordering::Relaxed) {
                            hb.beat(); // WS 连接存活 — 维持 liveness 新鲜
                        }
                    }
                    result = &mut ws_fut => {
                        match result {
                            Ok(()) => tracing::info!("飞书 WebSocket 连接正常关闭"),
                            Err(e) => tracing::error!("飞书 WebSocket 连接异常: {}", e),
                        }
                        break;
                    }
                }
            }
        });
    }

    /// 根据文件名后缀映射飞书 `/im/v1/files` 的 file_type（官方合法值：
    /// `opus`/`mp4`/`pdf`/`doc`/`xls`/`ppt`/`stream`，未匹配 → stream）。
    fn feishu_file_type(file_name: &str) -> &'static str {
        let ext = file_name
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "pdf" => "pdf",
            "doc" | "docx" => "doc",
            "xls" | "xlsx" => "xls",
            "ppt" | "pptx" => "ppt",
            _ => "stream",
        }
    }

    /// 获取媒体文件字节：优先下载 URL（带大小上限），否则解码 base64。
    ///
    /// SECURITY: 下载采用流式读取并计数，超过上限即拒绝——chunked 或无
    /// Content-Length 头的响应同样受限，避免无界读入内存。
    async fn media_bytes(&self, media: &MediaAttachment) -> Result<Vec<u8>, GatewayError> {
        const MAX_DOWNLOAD_BYTES: u64 = 25 * 1024 * 1024; // 25MB

        if let Some(ref url) = media.url {
            let client = self.client();
            let resp =
                client.get(url).send().await.map_err(|e| {
                    GatewayError::Transient(format!("Download media failed: {}", e))
                })?;
            let mut resp = resp;
            let mut total: u64 = 0;
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|e| GatewayError::Internal(format!("Read media bytes failed: {}", e)))?
            {
                total += chunk.len() as u64;
                if total > MAX_DOWNLOAD_BYTES {
                    return Err(GatewayError::Internal(format!(
                        "Rejected media download: exceeds {} limit",
                        MAX_DOWNLOAD_BYTES
                    )));
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(buf)
        } else if let Some(ref base64_data) = media.data {
            // 解码 base64 数据作为文件内容
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(base64_data)
                .map_err(|e| GatewayError::Internal(format!("Base64 decode failed: {}", e)))
        } else {
            Err(GatewayError::InvalidRequest(
                "No media data or URL provided".to_string(),
            ))
        }
    }

    /// 上传图片到飞书，返回 image_key。
    ///
    /// 图片走 `/im/v1/images`（image_type=message），与普通文件上传通道分离。
    async fn upload_image(&self, media: &MediaAttachment) -> Result<String, GatewayError> {
        let store = self.token_store.inner.as_ref().ok_or_else(|| {
            GatewayError::ConfigError(
                "Feishu token store not initialized (call connect() first)".to_string(),
            )
        })?;
        let url = format!("{}/im/v1/images", self.api_base_url());
        let file_data = self.media_bytes(media).await?;
        let file_name = media
            .filename
            .clone()
            .unwrap_or_else(|| "image".to_string());
        let mime_type = media.mime_type.clone();

        easybot_core::http::send_with_token_retry(store, is_feishu_token_invalid, move |token| {
            // Part/Form 不可 Clone：每次尝试重建 multipart（重试仅在
            // token 被拒时发生，克隆文件内容开销可接受）
            let url = url.clone();
            let file_data = file_data.clone();
            let file_name = file_name.clone();
            let mime_type = mime_type.clone();
            let auth = format!("Bearer {token}");
            Box::pin(async move {
                self.upload_image_raw(&url, &auth, file_data, file_name, &mime_type)
                    .await
            })
        })
        .await
    }

    /// 实际的飞书图片上传请求（不管理 token）。token 被拒信号同 raw 层
    /// 统一为 `GatewayError::Unauthorized`。
    async fn upload_image_raw(
        &self,
        url: &str,
        auth: &str,
        file_data: Vec<u8>,
        file_name: String,
        mime_type: &str,
    ) -> Result<String, GatewayError> {
        // 使用 multipart 上传（image_type 固定为 message，用于发送消息）
        let image_part = reqwest::multipart::Part::bytes(file_data)
            .file_name(file_name)
            .mime_str(mime_type)
            .map_err(|e| GatewayError::Internal(format!("Invalid mime type: {}", e)))?;
        let form = reqwest::multipart::Form::new()
            .part("image", image_part)
            .text("image_type", "message");

        let client = self.client();
        let resp = client
            .post(url)
            .header("Authorization", auth)
            .multipart(form)
            .send()
            .await
            .map_err(|e| GatewayError::Transient(format!("Feishu image upload failed: {}", e)))?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(GatewayError::Unauthorized(
                "Feishu image upload: 401 Unauthorized".to_string(),
            ));
        }

        let result: FeishuApiResponse<FeishuImageUploadData> = resp.json().await.map_err(|e| {
            GatewayError::Internal(format!("Feishu image upload parse failed: {}", e))
        })?;

        if result.code != 0 {
            if is_feishu_token_invalid_code(result.code) {
                return Err(GatewayError::Unauthorized(format!(
                    "Feishu image upload error: {} (code {})",
                    result.msg.unwrap_or_default(),
                    result.code
                )));
            }
            return Err(GatewayError::Internal(format!(
                "Feishu image upload error: {} (code {})",
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        result
            .data
            .and_then(|d| d.image_key)
            .ok_or_else(|| GatewayError::Internal("No image_key in upload response".to_string()))
    }

    /// 上传文件到飞书，返回 file_key。
    ///
    /// 走 `/im/v1/files`，file_type 必须是官方合法值
    /// （`opus`/`mp4`/`pdf`/`doc`/`xls`/`ppt`/`stream`）。
    async fn upload_file(
        &self,
        media: &MediaAttachment,
        file_type: &str,
    ) -> Result<String, GatewayError> {
        let store = self.token_store.inner.as_ref().ok_or_else(|| {
            GatewayError::ConfigError(
                "Feishu token store not initialized (call connect() first)".to_string(),
            )
        })?;
        let url = format!("{}/im/v1/files", self.api_base_url());
        let file_data = self.media_bytes(media).await?;
        let file_name = media.filename.clone().unwrap_or_else(|| "file".to_string());
        let mime_type = media.mime_type.clone();
        let file_type = file_type.to_string();
        // duration 单位为毫秒（飞书用于音频/视频时长显示）
        let duration_ms = media.duration.map(|d| (d * 1000.0) as i64);

        easybot_core::http::send_with_token_retry(store, is_feishu_token_invalid, move |token| {
            // Part/Form 不可 Clone：每次尝试重建 multipart（重试仅在
            // token 被拒时发生，克隆文件内容开销可接受）
            let url = url.clone();
            let file_data = file_data.clone();
            let file_type = file_type.clone();
            let file_name = file_name.clone();
            let mime_type = mime_type.clone();
            let auth = format!("Bearer {token}");
            Box::pin(async move {
                self.upload_file_raw(
                    &url,
                    &auth,
                    file_data,
                    &file_type,
                    &file_name,
                    &mime_type,
                    duration_ms,
                )
                .await
            })
        })
        .await
    }

    /// 实际的飞书文件上传请求（不管理 token）。token 被拒信号同 raw 层
    /// 统一为 `GatewayError::Unauthorized`。
    #[allow(clippy::too_many_arguments)]
    async fn upload_file_raw(
        &self,
        url: &str,
        auth: &str,
        file_data: Vec<u8>,
        file_type: &str,
        file_name: &str,
        mime_type: &str,
        duration_ms: Option<i64>,
    ) -> Result<String, GatewayError> {
        // 使用 multipart 上传
        let file_part = reqwest::multipart::Part::bytes(file_data)
            .file_name(file_name.to_string())
            .mime_str(mime_type)
            .map_err(|e| GatewayError::Internal(format!("Invalid mime type: {}", e)))?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("file_type", file_type.to_string())
            .text("file_name", file_name.to_string());
        if let Some(dur) = duration_ms {
            form = form.text("duration", dur.to_string());
        }

        let client = self.client();
        let resp = client
            .post(url)
            .header("Authorization", auth)
            .multipart(form)
            .send()
            .await
            .map_err(|e| GatewayError::Transient(format!("Feishu upload failed: {}", e)))?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(GatewayError::Unauthorized(
                "Feishu upload: 401 Unauthorized".to_string(),
            ));
        }

        let result: FeishuApiResponse<FeishuUploadData> = resp
            .json()
            .await
            .map_err(|e| GatewayError::Internal(format!("Feishu upload parse failed: {}", e)))?;

        if result.code != 0 {
            if is_feishu_token_invalid_code(result.code) {
                return Err(GatewayError::Unauthorized(format!(
                    "Feishu upload error: {} (code {})",
                    result.msg.unwrap_or_default(),
                    result.code
                )));
            }
            return Err(GatewayError::Internal(format!(
                "Feishu upload error: {} (code {})",
                result.msg.unwrap_or_default(),
                result.code
            )));
        }

        result
            .data
            .and_then(|d| d.file_key)
            .ok_or_else(|| GatewayError::Internal("No file_key in upload response".to_string()))
    }

    // ── 角色解析（WebSocket 事件用） ──

    /// 解析群消息发送者在飞书群内的角色。
    /// 对非群消息直接返回 None，群消息调用飞书 API 查询后缓存 30 秒。
    pub(crate) async fn resolve_feishu_role(
        event_data: &serde_json::Value,
        client: &reqwest::Client,
        base_url: &str,
        token_store: &FeishuTokenStore,
        role_cache: &Mutex<HashMap<String, (SenderRole, Instant)>>,
    ) -> Option<SenderRole> {
        // 只处理群聊消息
        let chat_type = event_data
            .pointer("/message/chat_type")
            .and_then(|v| v.as_str())?;
        if chat_type != "group" {
            return None;
        }
        let chat_id = event_data
            .pointer("/message/chat_id")
            .and_then(|v| v.as_str())?;
        let sender_id = event_data
            .pointer("/sender/sender_id/open_id")
            .and_then(|v| v.as_str())?;

        let cache_key = format!("{}:{}", chat_id, sender_id);

        // 检查缓存（TTL 过期后重新查询）
        {
            let mut cache = role_cache.lock().ok()?;
            if let Some((role, expiry)) = cache.get(&cache_key) {
                if expiry.elapsed() < Duration::from_secs(ROLE_CACHE_TTL_SECS) {
                    return Some(role.clone());
                }
                // 缓存过期，移除
                cache.remove(&cache_key);
            }
        }

        // 获取 token（通过共享 token store，与适配器实例共用缓存，自动刷新）
        let auth = token_store.get().await.ok()?;

        // 使用获取群信息 API 查询群主和管理员列表（无需分页，一次调用即可）
        let chat_url = format!("{}/im/v1/chats/{}", base_url.trim_end_matches('/'), chat_id);
        let resp = client
            .get(&chat_url)
            .header("Authorization", auth)
            .send()
            .await
            .ok()?;

        if !resp.status().is_success() {
            tracing::warn!("飞书群信息 API 返回 {}", resp.status());
            return None;
        }

        let body: serde_json::Value = resp.json().await.ok()?;
        let data = body.get("data")?;

        // 获取群主 ID
        let owner_id = data.get("owner_id").and_then(|v| v.as_str());
        // 获取管理员 ID 列表
        let admin_ids: Vec<&str> = data
            .get("user_manager_id_list")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        // 判断角色
        let role = if owner_id == Some(sender_id) {
            SenderRole::Owner
        } else if admin_ids.contains(&sender_id) {
            SenderRole::Admin
        } else {
            SenderRole::Member
        };

        // 写入缓存（TTL + im.chat.updated_v1 事件双重失效机制）
        if let Ok(mut cache) = role_cache.lock() {
            // 插入前主动淘汰过期项，避免惰性清理导致短时间突增
            cache.retain(|_, (_, expiry)| {
                expiry.elapsed() < Duration::from_secs(ROLE_CACHE_TTL_SECS)
            });
            // 容量上限：仍超限则移除最旧的一项（近似 FIFO）
            if cache.len() >= ROLE_CACHE_LIMIT
                && let Some(oldest) = cache
                    .iter()
                    .min_by_key(|(_, (_, expiry))| *expiry)
                    .map(|(k, _)| k.clone())
            {
                cache.remove(&oldest);
            }
            cache.insert(cache_key, (role.clone(), Instant::now()));
        }

        Some(role)
    }
}

impl Default for FeishuAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ── 单元测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_adapter() {
        let adapter = FeishuAdapter::new();
        assert_eq!(adapter.platform_name(), "feishu");
        assert_eq!(adapter.state(), AdapterState::Created);
        assert!(!adapter.capabilities.is_empty());
    }

    #[tokio::test]
    async fn test_init_missing_config() {
        let mut adapter = FeishuAdapter::new();
        let result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: None,
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert!(!result.ok); // 应该失败，缺少配置
    }

    #[tokio::test]
    async fn test_init_missing_app_id() {
        let mut adapter = FeishuAdapter::new();
        let result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("test_secret".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert!(!result.ok); // 缺少 app_id
    }

    #[tokio::test]
    async fn test_init_valid_config() {
        let mut adapter = FeishuAdapter::new();
        let result = adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("test_secret".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({"app_id": "cli_xxxxx"}),
            })
            .await
            .unwrap();
        assert!(result.ok);
    }

    #[test]
    fn test_capabilities() {
        let adapter = FeishuAdapter::new();
        let caps = adapter.capabilities();
        assert!(
            caps.iter()
                .any(|c| c.name == CapabilityName::Text && c.supported)
        );
        assert!(
            caps.iter()
                .any(|c| c.name == CapabilityName::Group && c.supported)
        );
        assert!(
            caps.iter()
                .any(|c| c.name == CapabilityName::MessageEdit && c.supported)
        );
    }

    #[test]
    fn test_status_summary() {
        let adapter = FeishuAdapter::new();
        let status = adapter.status_summary();
        assert_eq!(status.platform, "feishu");
        assert_eq!(status.display_name, "飞书");
        assert!(!status.connected);
        // health should be present (not None) — frontend relies on this
        assert!(status.health.is_some(), "health should not be None");
        assert_eq!(status.health.unwrap(), HealthStatus::Down);
    }

    #[tokio::test]
    async fn test_runtime_config_before_init() {
        let adapter = FeishuAdapter::new();
        let rc = adapter.runtime_config();
        assert!(!rc.enabled);
        assert!(!rc.token_configured);
        // 未初始化时 extra 应为 null 或空对象
        assert!(rc.extra.is_object() || rc.extra.is_null());
    }

    #[tokio::test]
    async fn test_runtime_config_after_init() {
        let mut adapter = FeishuAdapter::new();
        adapter
            .init(AdapterConfig {
                enabled: Some(true),
                token: Some("secret".to_string()),
                api_key: None,
                base_url: None,
                extra: serde_json::json!({"app_id": "cli_xxx"}),
            })
            .await
            .unwrap();
        let rc = adapter.runtime_config();
        assert!(rc.enabled);
        assert!(rc.token_configured);
        assert_eq!(
            rc.extra.get("app_id").and_then(|v| v.as_str()),
            Some("cli_xxx")
        );
    }

    #[tokio::test]
    async fn test_health_before_init() {
        let adapter = FeishuAdapter::new();
        let health = adapter.health().await;
        assert_eq!(health.status, HealthStatus::Down);
        assert!(!health.connected);
    }

    #[tokio::test]
    async fn test_disconnect_idempotent() {
        let mut adapter = FeishuAdapter::new();
        // 在 connect 之前调用 disconnect 不应 panic
        let result = adapter.disconnect().await;
        assert!(result.is_ok());
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_double_disconnect() {
        let mut adapter = FeishuAdapter::new();
        adapter.disconnect().await.unwrap();
        adapter.disconnect().await.unwrap(); // 连续两次 disconnect
        assert_eq!(adapter.state(), AdapterState::Stopped);
    }

    #[tokio::test]
    async fn test_get_chat_info_network_error() {
        let adapter = FeishuAdapter::new();
        // 未初始化时调用应返回错误
        let result = adapter.get_chat_info("oc_test").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_adapter_default() {
        let adapter = FeishuAdapter::default();
        assert_eq!(adapter.platform_name(), "feishu");
    }

    #[tokio::test]
    async fn test_send_before_connect() {
        let adapter = FeishuAdapter::new();
        let result = adapter
            .send(SendTextParams {
                chat_id: "oc_test".to_string(),
                message: OutboundMessage {
                    text: "hello".to_string(),
                    parse_mode: ParseMode::None,
                },
                reply_to: None,
                metadata: None,
            })
            .await;
        // 未初始化时发送应返回错误
        assert!(result.unwrap().error.is_some());
    }

    #[tokio::test]
    async fn test_delete_message_before_connect() {
        let adapter = FeishuAdapter::new();
        let result = adapter.delete_message("oc_test", "om_test").await.unwrap();
        // 未初始化时，删除应返回失败（token refresh 失败）
        assert!(!result.success);
    }

    #[test]
    fn test_capabilities_all_expected() {
        let adapter = FeishuAdapter::new();
        let caps = adapter.capabilities();
        let expected = vec![
            CapabilityName::Text,
            CapabilityName::Image,
            CapabilityName::Audio,
            CapabilityName::Video,
            CapabilityName::Document,
            CapabilityName::Interactive,
            CapabilityName::Markdown,
            CapabilityName::Group,
            CapabilityName::MessageEdit,
            CapabilityName::MessageDelete,
        ];
        for name in expected {
            assert!(
                caps.iter().any(|c| c.name == name && c.supported),
                "capability {:?} should be supported",
                name
            );
        }
    }
}
