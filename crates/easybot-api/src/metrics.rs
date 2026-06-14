//! Prometheus 指标收集
//!
//! 提供 HTTP 请求指标、业务指标和 `/metrics` 端点。
//! 通过 Tower 中间件自动记录请求数量、持续时间和状态码。

use axum::{
    extract::State,
    response::IntoResponse,
};
use prometheus::{
    Counter, Gauge, Histogram, HistogramOpts, Registry,
    TextEncoder, Encoder,
};
use tracing::warn;

use crate::AppState;

/// 指标注册表
///
/// 集中管理所有 Prometheus 指标。
#[derive(Clone)]
pub struct MetricsRegistry {
    registry: Registry,
    /// HTTP 请求总数（按 method, path, status 标签）
    pub http_requests_total: Counter,
    /// HTTP 请求耗时（按 method, path 标签）
    pub http_request_duration_seconds: Histogram,
    /// 活跃 WebSocket 连接数
    pub active_websocket_connections: Gauge,
    /// 入站消息总数（按 platform 标签）
    pub messages_inbound_total: Counter,
    /// 出站消息总数（按 platform 标签）
    pub messages_outbound_total: Counter,
    /// 适配器状态（按 platform, state 标签）
    pub adapter_status: Gauge,
}

impl MetricsRegistry {
    /// 创建新的指标注册表
    pub fn new() -> Self {
        let registry = Registry::new();

        let http_requests_total = Counter::new(
            "http_requests_total",
            "Total number of HTTP requests",
        ).unwrap();
        let http_request_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "http_request_duration_seconds",
                "HTTP request duration in seconds",
            ),
        ).unwrap();
        let active_websocket_connections = Gauge::new(
            "active_websocket_connections",
            "Number of active WebSocket connections",
        ).unwrap();
        let messages_inbound_total = Counter::new(
            "messages_inbound_total",
            "Total number of inbound messages",
        ).unwrap();
        let messages_outbound_total = Counter::new(
            "messages_outbound_total",
            "Total number of outbound messages",
        ).unwrap();
        let adapter_status = Gauge::new(
            "adapter_status",
            "Adapter status (1=connected, 0=disconnected)",
        ).unwrap();

        registry.register(Box::new(http_requests_total.clone())).unwrap();
        registry.register(Box::new(http_request_duration_seconds.clone())).unwrap();
        registry.register(Box::new(active_websocket_connections.clone())).unwrap();
        registry.register(Box::new(messages_inbound_total.clone())).unwrap();
        registry.register(Box::new(messages_outbound_total.clone())).unwrap();
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

/// `/metrics` 端点处理器
///
/// 从 AppState 中提取 MetricsRegistry 并渲染 Prometheus 格式。
pub async fn metrics_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    match state.metrics {
        Some(ref registry) => registry.render(),
        None => "Metrics disabled".to_string(),
    }
}
