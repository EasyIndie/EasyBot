//! 进程外插件**服务端运行时**。
//!
//! 把插件的 [`PlatformAdapter`] 实现暴露为 stdin/stdout 逐行 JSON 协议服务——
//! 插件开发者只需实现 `PlatformAdapter`，再在 `main` 里调用
//! [`run_plugin!`](crate::run_plugin) 即可。
//!
//! 与宿主侧 `easybot_core::plugin::IpcPluginAdapter` 配对（同一份
//! `easybot-plugin-protocol`）。

use std::sync::Arc;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use easybot_core::bus::EventBus;
use easybot_core::types::adapter::{AdapterConfig, PlatformAdapter};
use easybot_core::types::event::event_types;
use easybot_core::types::message::{MediaAttachment, MediaType, OutboundMessage, SendTextParams};
use easybot_plugin_protocol::{
    ConnectOutcome, ErrorKind, HandshakeResult, InitParams, Notification, PROTOCOL_VERSION,
    Request, Response, RetryTransportOutcome, SendMediaParams as WireSendMediaParams, SendOutcome,
    SendParams, methods,
};

/// 转发给宿主的事件类型（插件经本地 `EventBus` 发布的都会被转成 `event` 通知）。
const FORWARDED_EVENTS: &[&str] = &[
    event_types::MESSAGE_INBOUND,
    event_types::MESSAGE_SENT,
    event_types::MESSAGE_FAILED,
    event_types::CALLBACK_RECEIVED,
    event_types::ADAPTER_CONNECTED,
    event_types::ADAPTER_DISCONNECTED,
    event_types::ADAPTER_ERROR,
    event_types::ADAPTER_RECONNECTING,
    event_types::ADAPTER_RECONNECTED,
    event_types::ADAPTER_RECONNECT_FAILED,
];

/// 运行插件服务端（阻塞当前线程，直到 stdin 关闭或收到 `shutdown`）。
pub fn run_plugin<T>(adapter: T)
where
    T: PlatformAdapter + 'static,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    runtime.block_on(serve(adapter));
}

type SharedOut = Arc<tokio::sync::Mutex<tokio::io::Stdout>>;

async fn write_line(out: &SharedOut, value: &impl Serialize) {
    if let Ok(line) = serde_json::to_string(value) {
        let mut guard = out.lock().await;
        let _ = guard.write_all(line.as_bytes()).await;
        let _ = guard.write_all(b"\n").await;
        let _ = guard.flush().await;
    }
}

async fn serve<T>(mut adapter: T)
where
    T: PlatformAdapter + 'static,
{
    let out: SharedOut = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));

    // 本地事件总线：适配器发布的事件转成 `event` 通知回传宿主
    let bus = Arc::new(EventBus::new());
    adapter.set_event_bus(bus.clone());
    for event_type in FORWARDED_EVENTS {
        let mut rx = bus.subscribe(event_type);
        let out = out.clone();
        tokio::spawn(async move {
            while let Ok(event) = rx.recv().await {
                let note = Notification::event(&event.event_type, event.data);
                write_line(&out, &note).await;
            }
        });
    }

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 忽略无法解析的行
        let Ok(request) = serde_json::from_str::<Request>(line) else {
            continue;
        };
        let (response, shutdown) = handle(&mut adapter, request).await;
        write_line(&out, &response).await;
        if shutdown {
            break;
        }
    }
}

/// 处理单个请求，返回 `(响应, 是否请求退出)`。
async fn handle<T>(adapter: &mut T, request: Request) -> (Response, bool)
where
    T: PlatformAdapter,
{
    let id = request.id;
    match request.method.as_str() {
        methods::HANDSHAKE => {
            let capabilities = adapter
                .capabilities()
                .iter()
                .map(|c| format!("{:?}", c.name))
                .collect();
            let result = HandshakeResult {
                protocol: PROTOCOL_VERSION,
                platform: adapter.platform_name().to_string(),
                display_name: adapter.display_name().to_string(),
                sdk_version: 1,
                capabilities,
            };
            (Response::ok(id, json!(result)), false)
        }
        methods::INIT => {
            let config = serde_json::from_value::<InitParams>(request.params.clone())
                .ok()
                .and_then(|p| serde_json::from_value::<AdapterConfig>(p.config).ok())
                .unwrap_or_else(empty_adapter_config);
            match adapter.init(config).await {
                Ok(result) if result.ok => (Response::ok(id, json!({ "ok": true })), false),
                Ok(result) => (
                    Response::err(
                        id,
                        result.error.unwrap_or_else(|| "init failed".into()),
                        Some(ErrorKind::Permanent),
                    ),
                    false,
                ),
                Err(e) => (
                    Response::err(id, e.to_string(), Some(ErrorKind::Permanent)),
                    false,
                ),
            }
        }
        methods::CONNECT => match adapter.connect().await {
            Ok(result) => {
                let outcome = if result.ok {
                    ConnectOutcome::ok()
                } else {
                    ConnectOutcome::failed(
                        result.error.unwrap_or_default(),
                        result.error_kind.map(|k| match k {
                            easybot_core::ConnectErrorKind::Transient => ErrorKind::Transient,
                            easybot_core::ConnectErrorKind::Permanent => ErrorKind::Permanent,
                        }),
                    )
                };
                (Response::ok(id, json!(outcome)), false)
            }
            Err(e) => (
                Response::err(id, e.to_string(), Some(ErrorKind::Transient)),
                false,
            ),
        },
        methods::DISCONNECT => match adapter.disconnect().await {
            Ok(()) => (Response::ok(id, json!({})), false),
            Err(e) => (Response::err(id, e.to_string(), None), false),
        },
        methods::RETRY_TRANSPORT => match adapter.retry_transport().await {
            Ok(handled) => (
                Response::ok(id, json!(RetryTransportOutcome { handled })),
                false,
            ),
            Err(e) => (
                Response::err(id, e.to_string(), Some(ErrorKind::Transient)),
                false,
            ),
        },
        methods::SEND => match serde_json::from_value::<SendParams>(request.params.clone()) {
            Ok(params) => {
                let send = SendTextParams {
                    chat_id: params.chat_id,
                    message: OutboundMessage {
                        text: params.text,
                        parse_mode: parse_mode_from(&params.parse_mode),
                    },
                    reply_to: params.reply_to,
                    metadata: None,
                };
                match adapter.send(send).await {
                    Ok(result) => (
                        Response::ok(
                            id,
                            json!(SendOutcome {
                                success: result.success,
                                message_id: result.message_id,
                                error: result.error,
                            }),
                        ),
                        false,
                    ),
                    Err(e) => (Response::err(id, e.to_string(), None), false),
                }
            }
            Err(e) => (
                Response::err(
                    id,
                    format!("bad send params: {e}"),
                    Some(ErrorKind::Permanent),
                ),
                false,
            ),
        },
        methods::SEND_MEDIA => {
            match serde_json::from_value::<WireSendMediaParams>(request.params.clone()) {
                Ok(params) => {
                    let media = MediaAttachment {
                        media_type: media_type_from(&params.media_type),
                        // D1=A：宿主传路径，插件按路径读取
                        url: Some(params.path),
                        data: None,
                        mime_type: String::new(),
                        filename: params.filename,
                        caption: params.caption.clone(),
                        thumbnail_url: None,
                        file_size: None,
                        duration: None,
                    };
                    let send = easybot_core::SendMediaParams {
                        chat_id: params.chat_id,
                        media,
                        text: params.caption,
                        reply_to: params.reply_to,
                    };
                    match adapter.send_media(send).await {
                        Ok(result) => (
                            Response::ok(
                                id,
                                json!(SendOutcome {
                                    success: result.success,
                                    message_id: result.message_id,
                                    error: result.error,
                                }),
                            ),
                            false,
                        ),
                        Err(e) => (Response::err(id, e.to_string(), None), false),
                    }
                }
                Err(e) => (
                    Response::err(
                        id,
                        format!("bad send_media params: {e}"),
                        Some(ErrorKind::Permanent),
                    ),
                    false,
                ),
            }
        }
        methods::HEALTH => {
            let report = adapter.health().await;
            (Response::ok(id, json!(report)), false)
        }
        methods::SHUTDOWN => (Response::ok(id, json!({})), true),
        other => (
            Response::err(
                id,
                format!("method '{other}' not implemented by this plugin"),
                Some(ErrorKind::Unsupported),
            ),
            false,
        ),
    }
}

fn empty_adapter_config() -> AdapterConfig {
    AdapterConfig {
        enabled: None,
        token: None,
        api_key: None,
        base_url: None,
        extra: Value::Null,
    }
}

fn parse_mode_from(mode: &Option<String>) -> easybot_core::ParseMode {
    match mode.as_deref() {
        Some("markdown") => easybot_core::ParseMode::Markdown,
        Some("html") => easybot_core::ParseMode::Html,
        _ => easybot_core::ParseMode::None,
    }
}

fn media_type_from(media_type: &str) -> MediaType {
    match media_type {
        "audio" => MediaType::Audio,
        "video" => MediaType::Video,
        "document" => MediaType::Document,
        "sticker" => MediaType::Sticker,
        "animation" => MediaType::Animation,
        _ => MediaType::Image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use easybot_core::{
        AdapterRuntimeConfig, AdapterState, ChatInfo, ConnectResult, GatewayError, HealthReport,
        HealthStatus, InitResult, SendResult,
    };

    struct MockAdapter {
        state: AdapterState,
    }

    impl MockAdapter {
        fn new() -> Self {
            Self {
                state: AdapterState::Created,
            }
        }
    }

    #[crate::async_trait]
    impl PlatformAdapter for MockAdapter {
        fn platform_name(&self) -> &str {
            "sdk-mock"
        }
        fn display_name(&self) -> &str {
            "SDK Mock"
        }
        fn capabilities(&self) -> &[easybot_core::Capability] {
            &[]
        }
        async fn init(&mut self, _config: AdapterConfig) -> Result<InitResult, GatewayError> {
            self.state = AdapterState::Starting;
            Ok(InitResult {
                ok: true,
                error: None,
            })
        }
        async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
            self.state = AdapterState::Connected;
            Ok(ConnectResult::ok(None))
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
                status: HealthStatus::Healthy,
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
        async fn send(&self, _params: SendTextParams) -> Result<SendResult, GatewayError> {
            Ok(SendResult {
                success: true,
                message_id: Some("sdk-msg-1".into()),
                timestamp: None,
                error: None,
                error_code: None,
                retryable: false,
            })
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
        fn status_summary(&self) -> easybot_core::AdapterStatusSummary {
            easybot_core::AdapterStatusSummary {
                platform: "sdk-mock".into(),
                display_name: "SDK Mock".into(),
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

    async fn call(adapter: &mut MockAdapter, method: &str, params: Value) -> (Response, bool) {
        handle(adapter, Request::new(1, method, params)).await
    }

    #[tokio::test]
    async fn handshake_reports_platform() {
        let mut adapter = MockAdapter::new();
        let (resp, shutdown) = call(&mut adapter, methods::HANDSHAKE, Value::Null).await;
        assert!(!shutdown);
        assert!(resp.ok);
        let result = resp.result.expect("result");
        assert_eq!(result["platform"], "sdk-mock");
        assert_eq!(result["protocol"], PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn lifecycle_and_send() {
        let mut adapter = MockAdapter::new();
        let (init, _) = call(&mut adapter, methods::INIT, json!({ "config": {} })).await;
        assert!(init.ok);
        let (connect, _) = call(&mut adapter, methods::CONNECT, Value::Null).await;
        assert_eq!(connect.result.unwrap()["ok"], true);

        let (send, _) = call(
            &mut adapter,
            methods::SEND,
            json!({ "chat_id": "c1", "text": "hi" }),
        )
        .await;
        let outcome = send.result.expect("send result");
        assert_eq!(outcome["success"], true);
        assert_eq!(outcome["message_id"], "sdk-msg-1");
    }

    #[tokio::test]
    async fn shutdown_signals_exit() {
        let mut adapter = MockAdapter::new();
        let (resp, shutdown) = call(&mut adapter, methods::SHUTDOWN, Value::Null).await;
        assert!(resp.ok);
        assert!(shutdown, "shutdown 应要求退出循环");
    }

    #[tokio::test]
    async fn unknown_method_is_unsupported() {
        let mut adapter = MockAdapter::new();
        let (resp, _) = call(&mut adapter, "does.not.exist", Value::Null).await;
        assert!(!resp.ok);
        assert_eq!(
            resp.error.expect("error").kind,
            Some(ErrorKind::Unsupported)
        );
    }
}
