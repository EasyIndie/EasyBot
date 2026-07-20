//! Prometheus 指标收集
//!
//! 提供 HTTP 请求指标、业务指标和 `/metrics` 端点。
//! 通过 Tower 中间件自动记录请求数量、持续时间和状态码。

use axum::{
    extract::{Extension, State},
    response::IntoResponse,
};
use prometheus::{CounterVec, Encoder, GaugeVec, HistogramVec, Opts, Registry, TextEncoder};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
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
    /// 已认证 API 请求总数（标签: key_id, status_class）
    pub api_key_requests_total: CounterVec,
    /// API Key 认证失败总数
    pub api_auth_failures_total: CounterVec,
    /// Audit hash-chain integrity (1 valid, 0 invalid)
    pub audit_chain_integrity: GaugeVec,
    /// Durable ledger integrity by bounded ledger name (1 valid, 0 invalid).
    pub ledger_integrity: GaugeVec,
    /// Crash-left API key rotation transitions requiring operator action.
    pub pending_key_rotation_transitions: GaugeVec,
    /// Persistent outbound delivery backlog by bounded state.
    pub outbound_delivery_backlog: GaugeVec,
    last_integrity_check_ms: Arc<AtomicI64>,
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

        let api_key_requests_total = CounterVec::new(
            Opts::new(
                "api_key_requests_total",
                "Authenticated API requests by API key and HTTP status class",
            ),
            &["key_id", "status_class"],
        )
        .unwrap();

        let api_auth_failures_total = CounterVec::new(
            Opts::new(
                "api_auth_failures_total",
                "Failed API authentication attempts by reason",
            ),
            &["reason"],
        )
        .unwrap();
        let audit_chain_integrity = GaugeVec::new(
            Opts::new(
                "audit_chain_integrity",
                "Audit hash-chain integrity (1=valid)",
            ),
            &[],
        )
        .unwrap();
        let ledger_integrity = GaugeVec::new(
            Opts::new(
                "ledger_integrity",
                "Durable commercial ledger integrity (1=valid)",
            ),
            &["ledger"],
        )
        .unwrap();
        let pending_key_rotation_transitions = GaugeVec::new(
            Opts::new(
                "pending_key_rotation_transitions",
                "Durable API key rotation transitions requiring reconciliation",
            ),
            &[],
        )
        .unwrap();
        let outbound_delivery_backlog = GaugeVec::new(
            Opts::new(
                "outbound_delivery_backlog",
                "Persistent outbound deliveries requiring processing or reconciliation",
            ),
            &["state"],
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
        registry
            .register(Box::new(api_key_requests_total.clone()))
            .unwrap();
        registry
            .register(Box::new(api_auth_failures_total.clone()))
            .unwrap();
        registry
            .register(Box::new(audit_chain_integrity.clone()))
            .unwrap();
        registry
            .register(Box::new(ledger_integrity.clone()))
            .unwrap();
        registry
            .register(Box::new(pending_key_rotation_transitions.clone()))
            .unwrap();
        registry
            .register(Box::new(outbound_delivery_backlog.clone()))
            .unwrap();

        Self {
            registry,
            http_requests_total,
            http_request_duration_seconds,
            active_websocket_connections,
            messages_inbound_total,
            messages_outbound_total,
            adapter_status,
            api_key_requests_total,
            api_auth_failures_total,
            audit_chain_integrity,
            ledger_integrity,
            pending_key_rotation_transitions,
            outbound_delivery_backlog,
            last_integrity_check_ms: Arc::new(AtomicI64::new(0)),
        }
    }

    /// Record one authenticated request using only the server-generated key ID.
    pub fn record_api_key_request(&self, key_id: &str, status: u16) {
        let status_class = format!("{}xx", status / 100);
        self.api_key_requests_total
            .with_label_values(&[key_id, &status_class])
            .inc();
    }

    /// Record authentication failures with a bounded reason vocabulary.
    pub fn record_auth_failure(&self, reason: &'static str) {
        self.api_auth_failures_total
            .with_label_values(&[reason])
            .inc();
    }

    pub fn set_audit_integrity(&self, valid: bool) {
        self.audit_chain_integrity
            .with_label_values::<&str>(&[])
            .set(if valid { 1.0 } else { 0.0 });
    }

    pub fn set_ledger_integrity(&self, ledger: &'static str, valid: bool) {
        self.ledger_integrity
            .with_label_values(&[ledger])
            .set(if valid { 1.0 } else { 0.0 });
    }

    pub fn set_pending_key_rotations(&self, count: i64) {
        self.pending_key_rotation_transitions
            .with_label_values::<&str>(&[])
            .set(count as f64);
    }

    fn should_refresh_integrity(&self, now_ms: i64) -> bool {
        const REFRESH_MS: i64 = 5 * 60 * 1_000;
        let previous = self.last_integrity_check_ms.load(Ordering::Acquire);
        now_ms.saturating_sub(previous) >= REFRESH_MS
            && self
                .last_integrity_check_ms
                .compare_exchange(previous, now_ms, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    pub fn set_outbound_delivery_backlog(
        &self,
        stats: easybot_core::storage::OutboundDeliveryStats,
    ) {
        for (state, value) in [
            ("pending", stats.pending),
            ("stale_pending", stats.stale_pending),
            ("unpublished_event", stats.unpublished_events),
        ] {
            self.outbound_delivery_backlog
                .with_label_values(&[state])
                .set(value as f64);
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
pub async fn metrics_handler(
    State(state): State<AppState>,
    Extension(auth): Extension<easybot_core::auth::AuthInfo>,
) -> axum::response::Response {
    if let Err(error) = easybot_core::auth::permissions::require_permission(
        &auth,
        easybot_core::auth::permissions::Permission::MetricsRead,
    ) {
        return crate::response::ApiError(error).into_response();
    }
    match state.metrics {
        Some(ref registry) => {
            let now_ms = chrono::Utc::now().timestamp_millis();
            if registry.should_refresh_integrity(now_ms) {
                registry.set_audit_integrity(state.auth_manager.verify_audit_chain().await);
                let usage_valid = state
                    .auth_manager
                    .verify_usage_ledger_integrity()
                    .await
                    .unwrap_or_else(|error| {
                        tracing::error!(%error, "failed to verify usage ledger integrity");
                        false
                    });
                registry.set_ledger_integrity("usage", usage_valid);
                let billing_valid = state
                    .auth_manager
                    .verify_billing_ledger_integrity()
                    .await
                    .unwrap_or_else(|error| {
                        tracing::error!(%error, "failed to verify billing ledger integrity");
                        false
                    });
                registry.set_ledger_integrity("billing_events", billing_valid);
            }
            match state.auth_manager.rotation_transition_count().await {
                Ok(count) => registry.set_pending_key_rotations(count),
                Err(error) => {
                    tracing::error!(%error, "failed to collect pending key rotation transitions");
                    registry.set_pending_key_rotations(-1);
                }
            }
            let stale_before = chrono::Utc::now().timestamp_millis() - 5 * 60 * 1000;
            match state
                .message_store
                .outbound_delivery_stats(stale_before)
                .await
            {
                Ok(stats) => registry.set_outbound_delivery_backlog(stats),
                Err(error) => {
                    tracing::error!(%error, "failed to collect outbound delivery backlog metrics");
                }
            }
            registry.render().into_response()
        }
        None => "Metrics disabled".into_response(),
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
    metrics: Arc<MetricsRegistry>,
    event_bus: Arc<easybot_core::bus::EventBus>,
) -> tokio::task::JoinHandle<()> {
    use easybot_core::types::event::event_types;
    use futures::StreamExt;

    let mut stream = event_bus.subscribe_many(&[
        event_types::MESSAGE_INBOUND,
        event_types::ADAPTER_CONNECTED,
        event_types::ADAPTER_DISCONNECTED,
    ]);

    tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            match event.event_type.as_str() {
                event_types::MESSAGE_INBOUND => {
                    // 从 data 中提取 platform
                    if let Some(platform) = event.data.get("platform").and_then(|v| v.as_str()) {
                        metrics.record_inbound_message(platform);
                    }
                }
                event_types::ADAPTER_CONNECTED => {
                    if let Some(platform) = event.data.get("platform").and_then(|v| v.as_str()) {
                        metrics.set_adapter_connected(platform, true);
                    }
                }
                event_types::ADAPTER_DISCONNECTED => {
                    if let Some(platform) = event.data.get("platform").and_then(|v| v.as_str()) {
                        metrics.set_adapter_connected(platform, false);
                    }
                }
                _ => {}
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_usage_metrics_are_bounded_and_do_not_expose_secrets() {
        let metrics = MetricsRegistry::new();
        metrics.record_api_key_request("key-id-123", 200);
        metrics.record_api_key_request("key-id-123", 429);
        metrics.record_auth_failure("invalid_key");
        let rendered = metrics.render();

        assert!(
            rendered
                .contains("api_key_requests_total{key_id=\"key-id-123\",status_class=\"2xx\"} 1")
        );
        assert!(
            rendered
                .contains("api_key_requests_total{key_id=\"key-id-123\",status_class=\"4xx\"} 1")
        );
        assert!(rendered.contains("api_auth_failures_total{reason=\"invalid_key\"} 1"));
        assert!(!rendered.contains("eb_"));
    }

    #[test]
    fn outbound_delivery_backlog_has_bounded_labels() {
        let metrics = MetricsRegistry::new();
        metrics.set_outbound_delivery_backlog(easybot_core::storage::OutboundDeliveryStats {
            pending: 3,
            stale_pending: 2,
            unpublished_events: 1,
        });
        let rendered = metrics.render();
        assert!(rendered.contains("outbound_delivery_backlog{state=\"pending\"} 3"));
        assert!(rendered.contains("outbound_delivery_backlog{state=\"stale_pending\"} 2"));
        assert!(rendered.contains("outbound_delivery_backlog{state=\"unpublished_event\"} 1"));
    }

    #[test]
    fn commercial_ledger_integrity_metrics_are_bounded_and_refresh_is_throttled() {
        let metrics = MetricsRegistry::new();
        metrics.set_ledger_integrity("usage", true);
        metrics.set_ledger_integrity("billing_events", false);
        let rendered = metrics.render();
        assert!(rendered.contains("ledger_integrity{ledger=\"usage\"} 1"));
        assert!(rendered.contains("ledger_integrity{ledger=\"billing_events\"} 0"));

        assert!(metrics.should_refresh_integrity(300_000));
        assert!(!metrics.should_refresh_integrity(300_001));
        assert!(metrics.should_refresh_integrity(600_000));
    }

    #[test]
    fn pending_key_rotation_metric_has_no_dynamic_labels() {
        let metrics = MetricsRegistry::new();
        metrics.set_pending_key_rotations(2);
        assert!(
            metrics
                .render()
                .contains("pending_key_rotation_transitions 2")
        );
    }
}
