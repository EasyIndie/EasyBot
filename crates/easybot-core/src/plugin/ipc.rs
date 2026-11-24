//! 进程外插件代理（stdio JSON 协议）。
//!
//! 宿主 **spawn 一个插件可执行文件**，通过 stdin/stdout 逐行 JSON 通信——**不使用 dlopen**，
//! 因此可在 musl 静态宿主上工作（进程内 `.so` 插件做不到，见
//! `docs/other/rfc-out-of-process-plugins.md`）。
//!
//! 本模块是宿主侧对 [`PlatformAdapter`] 的实现：把 trait 方法映射为协议请求，
//! 并把插件推送的 `event` 通知发布到宿主 [`EventBus`]。
//!
//! 决策 D1=A：媒体**不内联**——宿主先把字节物化到专用临时目录，协议只传路径。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use easybot_plugin_protocol::{
    ConnectOutcome, ErrorKind, HandshakeResult, InitParams, Notification, PROTOCOL_VERSION,
    Request, Response, RetryTransportOutcome, SendMediaParams as WireSendMediaParams, SendOutcome,
    SendParams, methods,
};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::oneshot;

use crate::bus::EventBus;
use crate::types::adapter::{
    AdapterConfig, AdapterRuntimeConfig, AdapterState, AdapterStatusSummary, Capability,
    ConnectErrorKind, ConnectResult, HealthReport, HealthStatus, InitResult, PlatformAdapter,
};
use crate::types::error::GatewayError;
use crate::types::event::GatewayEvent;
use crate::types::message::{
    ChatInfo, MediaAttachment, MediaType, ParseMode, SendMediaParams, SendResult, SendTextParams,
};

/// 单次请求的默认超时（插件无响应时不无限挂起）。
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// 插件优雅退出的等待上限：超时后强杀，避免 `disable`/卸载被卡住。
const PLUGIN_EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// 单行 stdout 上限：超过视为插件失控（刷日志/死循环），断开并判故障。
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// 事件回传速率上限（每分钟）。插件无沙箱，刷事件会打满宿主 CPU/事件总线。
const MAX_EVENTS_PER_MINUTE: u32 = 6_000;

/// 插件子进程默认内存上限（512 MB）。`0` 表示不限制。
///
/// 仅在 Unix 生效（`RLIMIT_AS`）；Windows 需要 Job Object，尚未实现。
///
/// 可用 `EASYBOT_PLUGIN_MEMORY_LIMIT_MB` 覆盖（`0` = 关闭）。仅在 Unix 生效
/// （`RLIMIT_AS`）；Windows 需要 Job Object，尚未实现。
#[cfg(unix)]
const DEFAULT_PLUGIN_MEMORY_LIMIT_MB: u64 = 512;

/// 解析生效的插件内存上限（字节）；`0` 表示不限。
#[cfg(unix)]
fn plugin_memory_limit_bytes() -> u64 {
    let mb = std::env::var("EASYBOT_PLUGIN_MEMORY_LIMIT_MB")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_PLUGIN_MEMORY_LIMIT_MB);
    mb.saturating_mul(1024 * 1024)
}

/// 插件自述健康状态的编码：未上报（旧插件）。
const HEALTH_UNREPORTED: u8 = 0;
/// 插件自述健康。
const HEALTH_HEALTHY: u8 = 1;
/// 插件自述降级（正在重试）。
const HEALTH_DEGRADED: u8 = 2;
/// 插件自述失败。
const HEALTH_DOWN: u8 = 3;

/// 事件回传速率限制器（固定窗口，每分钟重置）。
///
/// 进程外插件无沙箱，一个刷事件的插件可以打满宿主 CPU 与事件总线；
/// 超限事件直接丢弃并计数（不影响响应/心跳通道）。
#[derive(Debug)]
struct EventRateLimiter {
    window_start: std::time::Instant,
    count: u32,
    limit: u32,
    dropped: u64,
}

impl EventRateLimiter {
    fn new(limit: u32) -> Self {
        Self {
            window_start: std::time::Instant::now(),
            count: 0,
            limit,
            dropped: 0,
        }
    }

    /// 是否允许该事件通过（超限则记一次丢弃并返回 false）。
    fn allow(&mut self) -> bool {
        self.allow_at(std::time::Instant::now())
    }

    fn allow_at(&mut self, now: std::time::Instant) -> bool {
        if now.duration_since(self.window_start) >= Duration::from_secs(60) {
            self.window_start = now;
            self.count = 0;
        }
        if self.limit != 0 && self.count >= self.limit {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.count = self.count.saturating_add(1);
        true
    }

    /// 累计丢弃数。
    fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// 计算进程外插件的健康状态。
///
/// 与内置适配器的语义对齐（`CLAUDE.md`「心跳表示后台任务存活且正在重试」）：
///
/// | 条件 | 结果 |
/// |---|---|
/// | 插件自述 `down` | `Down` |
/// | 插件自述 `degraded` | `Degraded` |
/// | 从未上报心跳（旧插件） | `Healthy`（宽容，避免升级即误伤） |
/// | 曾上报心跳但超过阈值未更新 | `Degraded` |
/// | 其它 | `Healthy` |
fn plugin_health_status(
    reported: u8,
    last_heartbeat_ms: i64,
    now_ms: i64,
    liveness_threshold_ms: i64,
) -> HealthStatus {
    match reported {
        HEALTH_DOWN => HealthStatus::Down,
        HEALTH_DEGRADED => HealthStatus::Degraded,
        _ => {
            if last_heartbeat_ms == 0 {
                // 旧插件：没有心跳通道，只能按「子进程存活」判断
                HealthStatus::Healthy
            } else if now_ms.saturating_sub(last_heartbeat_ms) > liveness_threshold_ms {
                // 曾经上报过，说明插件支持心跳；现在不再上报 → 后台任务卡住
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            }
        }
    }
}

/// 未决请求表：请求 id → 响应通道。
type PendingMap = Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>;

/// 进程外插件适配器。
pub struct IpcPluginAdapter {
    /// 插件可执行文件路径。
    plugin_path: PathBuf,
    /// 插件目录（作为子进程工作目录，并注入 `EASYBOT_PLUGIN_DIR`）。
    plugin_dir: Option<PathBuf>,
    /// EasyBot 配置根目录（注入 `EASYBOT_HOME`，供插件持久化凭据）。
    home: Option<PathBuf>,
    platform: String,
    display: String,
    capabilities: Vec<Capability>,
    state: Mutex<AdapterState>,
    child: tokio::sync::Mutex<Option<Child>>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    /// 未决请求表。每次 spawn 替换一代：已退出进程的 reader 只清理它自己那一代，
    /// 不会把过期错误投递给新进程的请求。
    pending: Mutex<Arc<PendingMap>>,
    next_id: AtomicU64,
    event_bus: Option<Arc<EventBus>>,
    call_timeout: Duration,
    /// 子进程是否已退出（reader 任务在 EOF 时置位）。
    child_exited: Arc<std::sync::atomic::AtomicBool>,
    /// 最近一次收到心跳的时间（epoch 毫秒；0 = 从未收到）。
    ///
    /// 旧版插件不发心跳，因此 0 必须按「宽容」处理（视为健康），
    /// 否则升级宿主会让所有现存插件立刻变成 `Degraded`。
    last_heartbeat_ms: Arc<AtomicI64>,
    /// 插件自述的健康状态（0 = 未上报 / healthy，1 = degraded，2 = down）。
    reported_health: Arc<AtomicU8>,
    /// 额外注入子进程的环境变量。
    extra_env: Vec<(String, String)>,
}

impl IpcPluginAdapter {
    /// 构造适配器（此时尚未 spawn）。
    ///
    /// - `plugin_path`：插件可执行文件路径。
    /// - `platform`：平台标识（须与注册到 `AdapterRegistry` 的键一致）。
    /// - `display`：默认显示名（握手后会被插件自述覆盖）。
    /// - `home`：EasyBot 配置根目录（可选，注入子进程 `EASYBOT_HOME`）。
    pub fn new(
        plugin_path: impl Into<PathBuf>,
        platform: impl Into<String>,
        display: impl Into<String>,
        home: Option<PathBuf>,
    ) -> Self {
        let plugin_path = plugin_path.into();
        let plugin_dir = plugin_path.parent().map(Path::to_path_buf);
        Self {
            plugin_path,
            plugin_dir,
            home,
            platform: platform.into(),
            display: display.into(),
            capabilities: Vec::new(),
            state: Mutex::new(AdapterState::Created),
            child: tokio::sync::Mutex::new(None),
            stdin: tokio::sync::Mutex::new(None),
            pending: Mutex::new(Arc::new(Mutex::new(HashMap::new()))),
            next_id: AtomicU64::new(1),
            event_bus: None,
            call_timeout: DEFAULT_CALL_TIMEOUT,
            child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_heartbeat_ms: Arc::new(AtomicI64::new(0)),
            reported_health: Arc::new(AtomicU8::new(HEALTH_UNREPORTED)),
            extra_env: Vec::new(),
        }
    }

    /// 覆盖单次请求超时（测试用）。
    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    /// 额外注入子进程的环境变量（测试／凭据传递用）。
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    /// 子进程是否已退出。
    pub fn child_exited(&self) -> bool {
        self.child_exited.load(Ordering::SeqCst)
    }

    /// 发起一次请求并等待响应（带超时）。
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let pending = self.pending.lock().unwrap().clone();
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(id, tx);

        let request = Request::new(id, method, params);
        {
            let mut guard = self.stdin.lock().await;
            let stdin = guard.as_mut().ok_or("plugin process not started")?;
            let mut line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
            line.push('\n');
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            stdin.flush().await.map_err(|e| e.to_string())?;
        }

        match tokio::time::timeout(self.call_timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(err))) => Err(err),
            Ok(Err(_)) => Err("plugin response channel closed".to_string()),
            Err(_) => {
                pending.lock().unwrap().remove(&id);
                Err(format!("plugin call '{method}' timed out"))
            }
        }
    }

    /// spawn 子进程并启动 reader 任务。
    async fn spawn_process(&self) -> Result<(), GatewayError> {
        let mut command = Command::new(&self.plugin_path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Some(dir) = &self.plugin_dir {
            command.current_dir(dir);
            command.env("EASYBOT_PLUGIN_DIR", dir);
        }
        if let Some(home) = &self.home {
            command.env("EASYBOT_HOME", home);
        }
        for (key, value) in &self.extra_env {
            command.env(key, value);
        }
        apply_memory_limit(&mut command);

        // 新一代 pending + 复位存活标志（重连/重启场景）
        let pending: Arc<PendingMap> = Arc::new(Mutex::new(HashMap::new()));
        *self.pending.lock().unwrap() = pending.clone();
        self.child_exited.store(false, Ordering::SeqCst);
        // 换代：新进程尚未上报心跳，回到「宽容」状态
        self.last_heartbeat_ms.store(0, Ordering::SeqCst);
        self.reported_health
            .store(HEALTH_UNREPORTED, Ordering::SeqCst);

        let mut child = command.spawn().map_err(|e| {
            GatewayError::Internal(format!(
                "failed to spawn plugin '{}': {e}",
                self.plugin_path.display()
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| GatewayError::Internal("plugin stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| GatewayError::Internal("plugin stdout unavailable".into()))?;

        *self.stdin.lock().await = Some(stdin);
        *self.child.lock().await = Some(child);

        spawn_reader(
            stdout,
            pending,
            self.event_bus.clone(),
            self.platform.clone(),
            self.child_exited.clone(),
            self.last_heartbeat_ms.clone(),
            self.reported_health.clone(),
        );
        Ok(())
    }
}

/// 后台读取插件 stdout：响应（有 `id`）派发到通道，事件通知发布到宿主事件总线。
fn spawn_reader(
    stdout: ChildStdout,
    pending: Arc<PendingMap>,
    event_bus: Option<Arc<EventBus>>,
    platform: String,
    child_exited: Arc<std::sync::atomic::AtomicBool>,
    last_heartbeat_ms: Arc<AtomicI64>,
    reported_health: Arc<AtomicU8>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        // 事件回传速率窗口（每分钟重置）。超限事件直接丢弃并计数，
        // 避免无沙箱插件把宿主 EventBus / CPU 打满。
        let mut event_limiter = EventRateLimiter::new(MAX_EVENTS_PER_MINUTE);

        loop {
            // 有界读取：`lines()` 不限制单行长度，插件写一行几百 MB 就能把宿主撑爆。
            let mut buf: Vec<u8> = Vec::new();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) => break, // EOF
                Ok(_) => {}
                Err(_) => break,
            }
            if buf.len() > MAX_LINE_BYTES {
                tracing::error!(
                    platform = %platform,
                    bytes = buf.len(),
                    limit = MAX_LINE_BYTES,
                    "plugin exceeded stdout line limit; treating as unhealthy"
                );
                break;
            }
            let line = String::from_utf8_lossy(&buf);
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };

            if value.get("id").is_some() {
                let Ok(resp) = serde_json::from_value::<Response>(value) else {
                    continue;
                };
                let outcome = if resp.ok {
                    Ok(resp.result.unwrap_or(Value::Null))
                } else {
                    Err(resp
                        .error
                        .map(|e| e.message)
                        .unwrap_or_else(|| "plugin error".to_string()))
                };
                if let Some(tx) = pending.lock().unwrap().remove(&resp.id) {
                    let _ = tx.send(outcome);
                }
            } else if let Ok(note) = serde_json::from_value::<Notification>(value) {
                match note.method.as_str() {
                    methods::NOTIFY_EVENT => {
                        if !event_limiter.allow() {
                            let dropped = event_limiter.dropped();
                            if dropped == 1 || dropped.is_multiple_of(1_000) {
                                tracing::warn!(
                                    platform = %platform,
                                    dropped,
                                    limit = MAX_EVENTS_PER_MINUTE,
                                    "plugin exceeds event rate limit; dropping events"
                                );
                            }
                            continue;
                        }
                        if let Some(bus) = &event_bus {
                            let event_type = note
                                .params
                                .get("event")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let data = note.params.get("data").cloned().unwrap_or(Value::Null);
                            bus.publish(GatewayEvent::new(event_type, platform.clone(), data));
                        }
                    }
                    methods::NOTIFY_HEARTBEAT => {
                        let status = note
                            .params
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let encoded = match status {
                            "down" => HEALTH_DOWN,
                            "degraded" => HEALTH_DEGRADED,
                            // 未识别的取值不升级故障等级：宁可保守地视为健康，
                            // 也不要因为插件笔误把适配器打成 Down。
                            _ => HEALTH_HEALTHY,
                        };
                        reported_health.store(encoded, Ordering::SeqCst);
                        last_heartbeat_ms
                            .store(chrono::Utc::now().timestamp_millis(), Ordering::SeqCst);
                    }
                    _ => {}
                }
            }
        }

        // 进程退出：置存活标志并唤醒所有未决请求，避免调用方永久挂起
        child_exited.store(true, Ordering::SeqCst);
        for (_, tx) in pending.lock().unwrap().drain() {
            let _ = tx.send(Err("plugin process exited".to_string()));
        }
    });
}

#[async_trait::async_trait]
impl PlatformAdapter for IpcPluginAdapter {
    fn platform_name(&self) -> &str {
        &self.platform
    }

    fn display_name(&self) -> &str {
        &self.display
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    fn set_event_bus(&mut self, bus: Arc<EventBus>) {
        self.event_bus = Some(bus);
    }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        // 幂等：宿主会先经 AdapterFactory、再由 `AdapterManager::start` 调用 `init`。
        // 子进程已在运行时不重复 spawn —— 否则第二个进程会替换第一个（`kill_on_drop`
        // 杀掉它），而旧进程的 reader 在 EOF 时会把「进程已退出」投递给新握手的请求。
        let already_running = {
            let child = self.child.lock().await;
            child.is_some() && !self.child_exited()
        };
        if already_running {
            return Ok(InitResult {
                ok: true,
                error: None,
            });
        }
        self.spawn_process().await?;

        // 1) 握手 + 协议版本校验
        let handshake = self
            .call(methods::HANDSHAKE, Value::Null)
            .await
            .map_err(|e| GatewayError::Internal(format!("plugin handshake failed: {e}")))?;
        let hs: HandshakeResult = serde_json::from_value(handshake)
            .map_err(|e| GatewayError::Internal(format!("invalid handshake result: {e}")))?;
        if hs.protocol != PROTOCOL_VERSION {
            return Err(GatewayError::Internal(format!(
                "plugin protocol version {} != host {PROTOCOL_VERSION}",
                hs.protocol
            )));
        }
        if !hs.display_name.is_empty() {
            self.display = hs.display_name;
        }
        // TODO(后续): 把 hs.capabilities 字符串映射为 CapabilityName。

        // 2) init（携带适配器配置与插件数据目录）
        let init_params = InitParams {
            config: serde_json::to_value(&config).unwrap_or(Value::Null),
            home: self
                .home
                .as_ref()
                .or(self.plugin_dir.as_ref())
                .map(|p| p.display().to_string()),
        };
        self.call(methods::INIT, serde_json::to_value(init_params).unwrap())
            .await
            .map_err(|e| GatewayError::Internal(format!("plugin init failed: {e}")))?;

        *self.state.lock().unwrap() = AdapterState::Starting;
        Ok(InitResult {
            ok: true,
            error: None,
        })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        let value = self
            .call(methods::CONNECT, Value::Null)
            .await
            .map_err(|e| GatewayError::Internal(format!("plugin connect failed: {e}")))?;
        let outcome: ConnectOutcome = serde_json::from_value(value)
            .map_err(|e| GatewayError::Internal(format!("invalid connect result: {e}")))?;

        if outcome.ok {
            *self.state.lock().unwrap() = AdapterState::Connected;
            Ok(ConnectResult::ok(None))
        } else {
            *self.state.lock().unwrap() = AdapterState::Failed;
            let kind = match outcome.error_kind {
                Some(ErrorKind::Transient) => Some(ConnectErrorKind::Transient),
                Some(ErrorKind::Permanent) => Some(ConnectErrorKind::Permanent),
                _ => None,
            };
            Ok(ConnectResult::failed(
                outcome.error.unwrap_or_default(),
                kind,
            ))
        }
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        // 尽力通知插件优雅退出；无论成败都回收子进程
        let _ = self.call(methods::DISCONNECT, Value::Null).await;
        let _ = self.call(methods::SHUTDOWN, Value::Null).await;
        if let Some(mut child) = self.child.lock().await.take() {
            // 有界等待：插件若忽略 `shutdown`（或卡在自身清理逻辑里），
            // 不能让宿主的 `plugin disable` / 卸载永久挂起 —— 超时后强杀。
            match tokio::time::timeout(PLUGIN_EXIT_TIMEOUT, child.wait()).await {
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!(
                        platform = %self.platform,
                        "plugin did not exit within {:?}; killing it",
                        PLUGIN_EXIT_TIMEOUT
                    );
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
            }
        }
        *self.stdin.lock().await = None;
        *self.state.lock().unwrap() = AdapterState::Stopped;
        Ok(())
    }

    async fn retry_transport(&mut self) -> Result<bool, GatewayError> {
        let value = self
            .call(methods::RETRY_TRANSPORT, Value::Null)
            .await
            .map_err(|e| GatewayError::Internal(format!("plugin retry_transport failed: {e}")))?;
        Ok(serde_json::from_value::<RetryTransportOutcome>(value)
            .map(|o| o.handled)
            .unwrap_or(false))
    }

    fn state(&self) -> AdapterState {
        self.state.lock().unwrap().clone()
    }

    /// 子进程退出后即使状态仍为 Connected 也报告 Down，驱动宿主健康监测器重连。
    ///
    /// 子进程存活时进一步参考插件的心跳上报（见 [`plugin_health_status`]）：
    /// 插件内部轮询失败、正在退避重试时应为 `Degraded`，而不是永远 `Healthy`。
    fn health_status(&self) -> HealthStatus {
        if self.child_exited() {
            return HealthStatus::Down;
        }
        plugin_health_status(
            self.reported_health.load(Ordering::SeqCst),
            self.last_heartbeat_ms.load(Ordering::SeqCst),
            chrono::Utc::now().timestamp_millis(),
            crate::types::adapter::DEFAULT_LIVENESS_THRESHOLD_MS,
        )
    }

    async fn health(&self) -> HealthReport {
        let connected = self.is_connected() && !self.child_exited();
        HealthReport {
            // 与 health_status() 保持一致：子进程存活但插件自述 degraded /
            // 心跳过期时，不能对外声称 Healthy。
            status: self.health_status(),
            connected,
            last_connected_at: None,
            last_error_at: None,
            last_error: None,
            messages_in: 0,
            messages_out: 0,
            errors: 0,
            uptime: None,
        }
    }

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        let wire = SendParams {
            chat_id: params.chat_id,
            text: params.message.text,
            parse_mode: parse_mode_str(&params.message.parse_mode),
            reply_to: params.reply_to,
        };
        self.send_wire(methods::SEND, serde_json::to_value(wire).unwrap())
            .await
    }

    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        // 决策 D1=A：宿主把媒体物化为本地文件，协议只传路径（不内联 base64）
        let path = materialize_media(&params.media).await?;
        let wire = WireSendMediaParams {
            chat_id: params.chat_id,
            path: path.display().to_string(),
            media_type: media_type_str(params.media.media_type).to_string(),
            filename: params.media.filename.clone(),
            caption: params.text.clone().or(params.media.caption.clone()),
            reply_to: params.reply_to,
        };
        self.send_wire(methods::SEND_MEDIA, serde_json::to_value(wire).unwrap())
            .await
    }

    async fn get_chat_info(&self, _chat_id: &str) -> Result<ChatInfo, GatewayError> {
        Err(GatewayError::capability_not_supported("get_chat_info"))
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: true,
            token_configured: false,
            extra: Value::Null,
        }
    }

    fn status_summary(&self) -> AdapterStatusSummary {
        AdapterStatusSummary {
            platform: self.platform.clone(),
            display_name: self.display.clone(),
            state: self.state(),
            connected: self.is_connected(),
            health: None,
            last_error: None,
            uptime: None,
            messages_in: 0,
            messages_out: 0,
        }
    }
}

impl IpcPluginAdapter {
    /// 发送类方法的共用收尾：请求 → 解析 [`SendOutcome`]。
    async fn send_wire(&self, method: &str, params: Value) -> Result<SendResult, GatewayError> {
        let value = self
            .call(method, params)
            .await
            .map_err(|e| GatewayError::Internal(format!("plugin {method} failed: {e}")))?;
        let outcome: SendOutcome = serde_json::from_value(value)
            .map_err(|e| GatewayError::Internal(format!("invalid {method} result: {e}")))?;
        Ok(SendResult {
            success: outcome.success,
            message_id: outcome.message_id,
            timestamp: None,
            error: outcome.error,
            error_code: None,
            retryable: false,
        })
    }
}

/// `ParseMode` → 协议字符串。
fn parse_mode_str(mode: &ParseMode) -> Option<String> {
    match mode {
        ParseMode::Markdown => Some("markdown".to_string()),
        ParseMode::Html => Some("html".to_string()),
        ParseMode::None => None,
    }
}

/// `MediaType` → 协议字符串。
fn media_type_str(media_type: MediaType) -> &'static str {
    match media_type {
        MediaType::Image => "image",
        MediaType::Audio => "audio",
        MediaType::Video => "video",
        MediaType::Document => "document",
        MediaType::Sticker => "sticker",
        MediaType::Animation => "animation",
    }
}

/// 媒体物化目录（宿主专用于跨进程传递媒体）。
fn media_temp_dir() -> PathBuf {
    std::env::temp_dir().join("easybot-media")
}

/// 把媒体附件物化为本地文件路径（决策 D1=A）。
///
/// 优先级：内联 base64 `data` > http(s) `url`（下载）> 本地路径 `url`。
async fn materialize_media(media: &MediaAttachment) -> Result<PathBuf, GatewayError> {
    let dir = media_temp_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        GatewayError::Internal(format!(
            "failed to create media temp dir {}: {e}",
            dir.display()
        ))
    })?;
    let filename = sanitize_filename(
        media
            .filename
            .clone()
            .unwrap_or_else(|| format!("media-{}", uuid::Uuid::new_v4())),
    );
    let path = dir.join(filename);

    if let Some(data) = &media.data {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| GatewayError::Internal(format!("failed to decode media base64: {e}")))?;
        tokio::fs::write(&path, bytes).await.map_err(|e| {
            GatewayError::Internal(format!("failed to write media {}: {e}", path.display()))
        })?;
        return Ok(path);
    }

    if let Some(url) = &media.url {
        if url.starts_with("http://") || url.starts_with("https://") {
            let response = reqwest::get(url)
                .await
                .map_err(|e| GatewayError::Transient(format!("failed to download media: {e}")))?;
            if !response.status().is_success() {
                return Err(GatewayError::Transient(format!(
                    "failed to download media: HTTP {}",
                    response.status()
                )));
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|e| GatewayError::Transient(format!("failed to read media body: {e}")))?;
            tokio::fs::write(&path, &bytes).await.map_err(|e| {
                GatewayError::Internal(format!("failed to write media {}: {e}", path.display()))
            })?;
            return Ok(path);
        }

        // 本地路径 / file:// URL
        let local = url.strip_prefix("file://").unwrap_or(url);
        let local = PathBuf::from(local);
        if local.exists() {
            return Ok(local);
        }
        return Err(GatewayError::Internal(format!(
            "media url is not an accessible local path: {url}"
        )));
    }

    Err(GatewayError::Internal(
        "media attachment has neither `data` nor `url`".to_string(),
    ))
}

/// 去除文件名中的路径成分，防止目录穿越。
fn sanitize_filename(name: String) -> String {
    let base = Path::new(&name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "media".to_string());
    let cleaned: String = base
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0'))
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "media".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_path_components() {
        assert_eq!(sanitize_filename("../../etc/passwd".into()), "passwd");
        assert_eq!(sanitize_filename("a/b/c.png".into()), "c.png");
        assert_eq!(sanitize_filename("..".into()), "media");
        assert_eq!(sanitize_filename("".into()), "media");
    }

    #[test]
    fn parse_mode_mapping() {
        assert_eq!(
            parse_mode_str(&ParseMode::Markdown).as_deref(),
            Some("markdown")
        );
        assert_eq!(parse_mode_str(&ParseMode::Html).as_deref(), Some("html"));
        assert_eq!(parse_mode_str(&ParseMode::None), None);
    }

    #[test]
    fn media_type_mapping() {
        assert_eq!(media_type_str(MediaType::Image), "image");
        assert_eq!(media_type_str(MediaType::Animation), "animation");
    }

    #[tokio::test]
    async fn materialize_inline_base64_to_file() {
        let media = MediaAttachment {
            media_type: MediaType::Image,
            url: None,
            data: Some(base64::engine::general_purpose::STANDARD.encode(b"hello")),
            mime_type: "text/plain".into(),
            filename: Some("note.txt".into()),
            caption: None,
            thumbnail_url: None,
            file_size: None,
            duration: None,
        };
        let path = materialize_media(&media).await.expect("materialize");
        assert!(path.exists());
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"hello");
        let _ = tokio::fs::remove_file(&path).await;
    }
}

/// 给插件子进程加内存上限（Unix `RLIMIT_AS`）。
///
/// 插件无沙箱：一个内存泄漏的插件会先触发系统 OOM killer，而被杀的往往不是
/// 它（Linux OOM score 偏袒小进程），结果是**宿主**被干掉。`RLIMIT_AS` 让
/// 超限只影响插件自身 —— 分配失败 → 插件崩溃 → 宿主判 `Down` → 既有重连逻辑。
///
/// Windows 用 Job Object 才能达到同等效果，尚未实现（`docs/18` 已注明）。
#[cfg(unix)]
#[allow(unsafe_code)] // SAFETY: pre_exec 中仅调用 async-signal-safe 的 setrlimit
fn apply_memory_limit(command: &mut Command) {
    let limit = plugin_memory_limit_bytes();
    if limit == 0 {
        return;
    }
    // SAFETY: 闭包在 fork 之后、exec 之前运行，只调用 `setrlimit`
    //（async-signal-safe）；失败不返回错误，只写 stderr —— 不能在 pre_exec
    // 里做内存分配或加锁（会死锁）。
    unsafe {
        command.pre_exec(move || {
            // rlim_t 在 64 位 Unix 上就是 u64；32 位目标上需要收窄（超限则饱和）。
            #[cfg(target_pointer_width = "64")]
            let limit_rlim: libc::rlim_t = limit;
            #[cfg(not(target_pointer_width = "64"))]
            let limit_rlim: libc::rlim_t =
                libc::rlim_t::try_from(limit).unwrap_or(libc::rlim_t::MAX);
            let rlim = libc::rlimit {
                rlim_cur: limit_rlim,
                rlim_max: limit_rlim,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
                // 用 write 而非 eprintln（后者可能分配）
                let msg = b"easybot: failed to set RLIMIT_AS for plugin\n";
                libc::write(2, msg.as_ptr().cast(), msg.len());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn apply_memory_limit(_command: &mut Command) {}

#[cfg(test)]
mod heartbeat_tests {
    use super::*;

    const THRESHOLD: i64 = 120_000;

    #[test]
    fn legacy_plugin_without_heartbeat_stays_healthy() {
        // 从未上报（旧插件）→ 宽容处理，不因升级宿主而被判故障
        assert_eq!(
            plugin_health_status(HEALTH_UNREPORTED, 0, 1_000_000, THRESHOLD),
            HealthStatus::Healthy
        );
    }

    #[test]
    fn fresh_heartbeat_is_healthy() {
        let now = 1_000_000;
        assert_eq!(
            plugin_health_status(HEALTH_HEALTHY, now - 1_000, now, THRESHOLD),
            HealthStatus::Healthy
        );
    }

    #[test]
    fn stale_heartbeat_is_degraded() {
        let now = 1_000_000;
        assert_eq!(
            plugin_health_status(HEALTH_HEALTHY, now - THRESHOLD - 1, now, THRESHOLD),
            HealthStatus::Degraded
        );
    }

    #[test]
    fn plugin_reported_degraded_and_down_are_respected() {
        let now = 1_000_000;
        assert_eq!(
            plugin_health_status(HEALTH_DEGRADED, now, now, THRESHOLD),
            HealthStatus::Degraded
        );
        assert_eq!(
            plugin_health_status(HEALTH_DOWN, now, now, THRESHOLD),
            HealthStatus::Down
        );
    }

    #[test]
    fn stale_heartbeat_does_not_override_reported_down() {
        let now = 1_000_000;
        assert_eq!(
            plugin_health_status(HEALTH_DOWN, now - THRESHOLD * 10, now, THRESHOLD),
            HealthStatus::Down
        );
    }

    #[test]
    fn clock_skew_does_not_panic() {
        // now 早于 last（系统时钟回拨）时应视为新鲜，而不是 panic/无意义结果
        let now = 1_000;
        assert_eq!(
            plugin_health_status(HEALTH_HEALTHY, now + 5_000, now, THRESHOLD),
            HealthStatus::Healthy
        );
    }
}

#[cfg(test)]
mod rate_limit_tests {
    use super::*;

    #[test]
    fn allows_up_to_limit_then_drops() {
        let mut limiter = EventRateLimiter::new(3);
        assert!(limiter.allow());
        assert!(limiter.allow());
        assert!(limiter.allow());
        assert!(!limiter.allow());
        assert!(!limiter.allow());
        assert_eq!(limiter.dropped(), 2);
    }

    #[test]
    fn window_resets_after_a_minute() {
        let mut limiter = EventRateLimiter::new(1);
        let now = std::time::Instant::now();
        assert!(limiter.allow_at(now));
        assert!(!limiter.allow_at(now));
        // 60s 后窗口重置
        assert!(limiter.allow_at(now + Duration::from_secs(60)));
    }

    #[test]
    fn zero_limit_disables_throttling() {
        let mut limiter = EventRateLimiter::new(0);
        for _ in 0..10_000 {
            assert!(limiter.allow());
        }
        assert_eq!(limiter.dropped(), 0);
    }
}

#[cfg(test)]
mod memory_limit_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn default_is_512mb() {
        // 不依赖环境变量的具体值（其它测试可能设置），只校验解析逻辑的量级：
        // 未设置时默认 512 MB —— 这里直接断言常量，避免测试间环境变量竞争。
        assert_eq!(DEFAULT_PLUGIN_MEMORY_LIMIT_MB, 512);
        assert_eq!(
            DEFAULT_PLUGIN_MEMORY_LIMIT_MB.saturating_mul(1024 * 1024),
            512 * 1024 * 1024
        );
    }

    #[cfg(unix)]
    #[test]
    fn applying_limit_does_not_break_command_construction() {
        // 只为覆盖 pre_exec 注册路径（真正的限制效果需要子进程配合验证，
        // 由集成测试用真实插件进程覆盖）。
        let mut cmd = Command::new("/bin/true");
        apply_memory_limit(&mut cmd);
    }
}
