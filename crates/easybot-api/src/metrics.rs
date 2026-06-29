//! Prometheus 指标收集
//!
//! 提供 HTTP 请求指标、业务指标和 `/metrics` 端点。
//! 通过 Tower 中间件自动记录请求数量、持续时间和状态码。

use axum::{extract::State, response::IntoResponse};
use prometheus::{CounterVec, Encoder, GaugeVec, HistogramVec, Opts, Registry, TextEncoder};
use tracing::warn;

use crate::AppState;

/// 指标注册表
///
/// 集中管理所有 Prometheus 指标，使用带标签的向量类型。
#[derive(Clone)]
pub struct MetricsRegistry {
    registry: Registry,
    /// HTTP 请求总数（标签: method, path, status）
    pub http_requests_total: CounterVec,
    /// HTTP 请求耗时（标签: method, path）
    pub http_request_duration_seconds: HistogramVec,
    /// 活跃 WebSocket 连接数
    pub active_websocket_connections: GaugeVec,
    /// 入站消息总数（标签: platform）
    pub messages_inbound_total: CounterVec,
    /// 出站消息总数（标签: platform）
    pub messages_outbound_total: CounterVec,
    /// 适配器状态（标签: platform，值: 1=connected, 0=disconnected）
    pub adapter_status: GaugeVec,
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRegistry {
    /// 创建新的指标注册表
    pub fn new() -> Self {
        let registry = Registry::new();

        let http_requests_total = CounterVec::new(
            Opts::new("http_requests_total", "Total number of HTTP requests"),
            &["method", "path", "status"],
        )
        .unwrap();

        let http_request_duration_seconds = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request duration in seconds",
            ),
            &["method", "path"],
        )
        .unwrap();

        let active_websocket_connections = GaugeVec::new(
            Opts::new(
                "active_websocket_connections",
                "Number of active WebSocket connections",
            ),
            &[],
        )
        .unwrap();

        let messages_inbound_total = CounterVec::new(
            Opts::new("messages_inbound_total", "Total number of inbound messages"),
            &["platform"],
        )
        .unwrap();

        let messages_outbound_total = CounterVec::new(
            Opts::new(
                "messages_outbound_total",
                "Total number of outbound messages",
            ),
            &["platform"],
        )
        .unwrap();

        let adapter_status = GaugeVec::new(
            Opts::new(
                "adapter_status",
                "Adapter status (1=connected, 0=disconnected)",
            ),
            &["platform"],
        )
        .unwrap();

        registry
            .register(Box::new(http_requests_total.clone()))
            .unwrap();
        registry
            .register(Box::new(http_request_duration_seconds.clone()))
            .unwrap();
        registry
            .register(Box::new(active_websocket_connections.clone()))
            .unwrap();
        registry
            .register(Box::new(messages_inbound_total.clone()))
            .unwrap();
        registry
            .register(Box::new(messages_outbound_total.clone()))
            .unwrap();
        registry.register(Box::new(adapter_status.clone())).unwrap();

        Self {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            active_websocket_connections,
            messages_inbound_total,
            messages_outbound_total,
            adapter_status,
        }
    }

    /// 记录一次 HTTP 请求
    pub fn record_http_request(&self, method: &str, path: &str, status: u16, duration_secs: f64) {
        let status_str = status.to_string();
        self.http_requests_total
            .with_label_values(&[method, path, &status_str])
            .inc();
        self.http_request_duration_seconds
            .with_label_values(&[method, path])
            .observe(duration_secs);
    }

    /// 记录一条入站消息
    pub fn record_inbound_message(&self, platform: &str) {
        self.messages_inbound_total
            .with_label_values(&[platform])
            .inc();
    }

    /// 记录一条出站消息
    pub fn record_outbound_message(&self, platform: &str) {
        self.messages_outbound_total
            .with_label_values(&[platform])
            .inc();
    }

    /// 更新适配器连接状态
    pub fn set_adapter_connected(&self, platform: &str, connected: bool) {
        let value: f64 = if connected { 1.0 } else { 0.0 };
        self.adapter_status
            .with_label_values(&[platform])
            .set(value);
    }

    /// 增加活跃 WebSocket 连接计数
    pub fn inc_websocket_connections(&self) {
        self.active_websocket_connections
            .with_label_values::<&str>(&[])
            .inc();
    }

    /// 减少活跃 WebSocket 连接计数
    pub fn dec_websocket_connections(&self) {
        self.active_websocket_connections
            .with_label_values::<&str>(&[])
            .dec();
    }

    /// 渲染 Prometheus 文本格式的指标
    pub fn render(&self) -> String {
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
            warn!("Failed to encode metrics: {}", e);
            return String::new();
        }
        String::from_utf8(buffer).unwrap_or_default()
    }
}

/// HTTP 指标收集中间件
///
/// 记录每个 HTTP 请求的 method、path、status 和耗时。
/// 动态路径段（如 UUID、数字 ID）会被归一化以避免标签基数爆炸。
pub async fn http_metrics_middleware(
    State(state): State<AppState>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> impl IntoResponse {
    let method = req.method().to_string();
    let path = normalize_path_for_metrics(req.uri().path());

    let start = std::time::Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16();

    if let Some(ref metrics) = state.metrics {
        metrics.record_http_request(&method, &path, status, duration);
    }

    response
}

/// 归一化路径中的动态段，避免标签基数爆炸
///
/// 将 UUID、数字 ID、timestamp 等替换为占位符。
fn normalize_path_for_metrics(path: &str) -> String {
    // 先按 `/` 分段处理
    path.split('/')
        .map(|segment| {
            if segment.is_empty() {
                return String::new();
            }
            // UUID 格式: 8-4-4-4-12 或 32 位 hex
            if is_uuid(segment) {
                return "{id}".to_string();
            }
            // session key 格式: "platform:chatId" 或 "platform:chatId:threadId"
            if segment.contains(':') {
                return "{key}".to_string();
            }
            // 纯数字 ID
            if segment.chars().all(|c| c.is_ascii_digit()) {
                return "{id}".to_string();
            }
            // 长 hex 字符串（如 message_id）
            if segment.len() >= 24 && segment.chars().all(|c| c.is_ascii_hexdigit()) {
                return "{id}".to_string();
            }
            segment.to_string()
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// 检查字符串是否为 UUID 格式
fn is_uuid(s: &str) -> bool {
    let len = s.len();
    // 标准 UUID: 8-4-4-4-12 = 36 chars
    if len == 36 {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 5
            && parts[0].len() == 8
            && parts[1].len() == 4
            && parts[2].len() == 4
            && parts[3].len() == 4
            && parts[4].len() == 12
        {
            return parts
                .iter()
                .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }
    // 无连字符 UUID: 32 chars
    if len == 32 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    false
}

/// `/metrics` 端点处理器
///
/// 从 AppState 中提取 MetricsRegistry 并渲染 Prometheus 格式。
pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    match state.metrics {
        Some(ref registry) => registry.render(),
        None => "Metrics disabled".to_string(),
    }
}

/// 启动事件总线监听器，自动更新消息和适配器指标
///
/// 在后台 tokio 任务中运行，监听 EventBus 事件：
/// - `message.inbound` → 增加入站消息计数
/// - `adapter.connected` / `adapter.disconnected` → 更新适配器状态 gauge
///
/// 返回 JoinHandle，可在优雅关闭时等待。
pub fn start_metrics_event_listener(
    metrics: std::sync::Arc<MetricsRegistry>,
    event_bus: std::sync::Arc<easybot_core::bus::EventBus>,
) -> tokio::task::JoinHandle<()> {
    use easybot_core::types::event::event_types;

    let mut rx = event_bus.subscribe_many(&[
        event_types::MESSAGE_INBOUND,
        event_types::ADAPTER_CONNECTED,
        event_types::ADAPTER_DISCONNECTED,
    ]);

    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => match event.event_type.as_str() {
                    event_types::MESSAGE_INBOUND => {
                        // 从 data 中提取 platform
                        if let Some(platform) = event.data.get("platform").and_then(|v| v.as_str())
                        {
                            metrics.record_inbound_message(platform);
                        }
                    }
                    event_types::ADAPTER_CONNECTED => {
                        if let Some(platform) = event.data.get("platform").and_then(|v| v.as_str())
                        {
                            metrics.set_adapter_connected(platform, true);
                        }
                    }
                    event_types::ADAPTER_DISCONNECTED => {
                        if let Some(platform) = event.data.get("platform").and_then(|v| v.as_str())
                        {
                            metrics.set_adapter_connected(platform, false);
                        }
                    }
                    _ => {}
                },
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Metrics event listener lagged by {} events", n);
                }
            }
        }
    })
}
