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
use std::sync::atomic::{AtomicU64, Ordering};
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
    pending: Arc<PendingMap>,
    next_id: AtomicU64,
    event_bus: Option<Arc<EventBus>>,
    call_timeout: Duration,
    /// 子进程是否已退出（reader 任务在 EOF 时置位）。
    child_exited: Arc<std::sync::atomic::AtomicBool>,
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
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            event_bus: None,
            call_timeout: DEFAULT_CALL_TIMEOUT,
            child_exited: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

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
                self.pending.lock().unwrap().remove(&id);
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
            self.pending.clone(),
            self.event_bus.clone(),
            self.platform.clone(),
            self.child_exited.clone(),
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
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
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
            } else if let Ok(note) = serde_json::from_value::<Notification>(value)
                && note.method == methods::NOTIFY_EVENT
                && let Some(bus) = &event_bus
            {
                let event_type = note
                    .params
                    .get("event")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let data = note.params.get("data").cloned().unwrap_or(Value::Null);
                bus.publish(GatewayEvent::new(event_type, platform.clone(), data));
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
            let _ = child.wait().await;
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
    fn health_status(&self) -> HealthStatus {
        if self.child_exited() {
            return HealthStatus::Down;
        }
        HealthStatus::Healthy
    }

    async fn health(&self) -> HealthReport {
        let connected = self.is_connected() && !self.child_exited();
        HealthReport {
            status: if connected {
                HealthStatus::Healthy
            } else {
                HealthStatus::Down
            },
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
