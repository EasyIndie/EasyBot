//! 健康检查路由

use crate::AppState;
use axum::{Json, extract::State, http::StatusCode};
use serde::Serialize;
use utoipa::ToSchema;

/// 健康检查响应
#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    #[schema(example = "healthy")]
    pub status: String,
    #[schema(example = "0.1.0")]
    pub version: String,
    /// 数据库 schema 版本
    pub schema_version: i64,
    /// 服务运行时间（秒）
    pub uptime: i64,
    pub adapters: AdapterSummary,
    pub sessions: SessionSummary,
}

/// 适配器摘要
#[derive(Serialize, ToSchema)]
pub struct AdapterSummary {
    pub total: usize,
    pub connected: usize,
}

/// 会话摘要
#[derive(Serialize, ToSchema)]
pub struct SessionSummary {
    pub active: usize,
}

#[derive(Serialize, ToSchema)]
pub struct ProbeResponse {
    pub status: String,
    pub version: String,
    pub uptime: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ReadinessResponse {
    pub status: String,
    /// API key, audit, and billing database.
    pub storage: String,
    pub auth_schema_version: i64,
    /// Message and session history database.
    pub message_storage: String,
    pub metering: String,
    pub quota: String,
    pub key_rotations: String,
    pub pending_key_rotations: u64,
    pub delivery_journal: String,
    pub pending_deliveries: u64,
    pub stale_pending_deliveries: u64,
    pub unpublished_delivery_events: u64,
    pub adapters_total: usize,
    pub adapters_connected: usize,
}

/// Process liveness only. Never checks external platforms.
#[utoipa::path(
    get,
    path = "/api/v1/live",
    tag = "Health",
    responses((status = 200, body = ProbeResponse))
)]
pub async fn live(State(state): State<AppState>) -> Json<ProbeResponse> {
    Json(ProbeResponse {
        status: "alive".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        uptime: state.started_at.elapsed().as_secs() as i64,
    })
}

/// Traffic readiness. Requires durable storage and at least one connected
/// adapter when adapters have been registered.
#[utoipa::path(
    get,
    path = "/api/v1/ready",
    tag = "Health",
    responses(
        (status = 200, body = ReadinessResponse),
        (status = 503, body = ReadinessResponse)
    )
)]
pub async fn ready(State(state): State<AppState>) -> (StatusCode, Json<ReadinessResponse>) {
    let statuses = state.adapter_manager.list_statuses().await;
    let connected = statuses.iter().filter(|status| status.connected).count();
    let storage_ready = state.auth_manager.storage_ready().await;
    let auth_schema_version = state.auth_manager.schema_version().await;
    let schema_ready = auth_schema_version.is_ok();
    let auth_schema_version = auth_schema_version.unwrap_or(-1);
    let message_storage_ready = state.message_store.storage_ready().await;
    let stale_before = chrono::Utc::now().timestamp_millis() - 5 * 60 * 1000;
    let delivery_stats = state
        .message_store
        .outbound_delivery_stats(stale_before)
        .await;
    let delivery_journal_ready = delivery_stats.is_ok();
    let stats = delivery_stats.unwrap_or_default();
    let metering_ready = state.auth_manager.probe_metering_ready().await;
    let quota_ready = state.auth_manager.quota_ready();
    let rotation_transitions = state.auth_manager.rotation_transition_count().await;
    let rotations_ready = rotation_transitions.is_ok();
    let pending_key_rotations = rotation_transitions.unwrap_or_default().max(0) as u64;
    let adapters_ready = statuses.is_empty() || connected > 0;
    let ready = storage_ready
        && schema_ready
        && message_storage_ready
        && delivery_journal_ready
        && metering_ready
        && quota_ready
        && rotations_ready
        && adapters_ready;
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(ReadinessResponse {
            status: if ready { "ready" } else { "not_ready" }.into(),
            storage: if storage_ready {
                "ready"
            } else {
                "unavailable"
            }
            .into(),
            auth_schema_version,
            message_storage: if message_storage_ready {
                "ready"
            } else {
                "unavailable"
            }
            .into(),
            metering: if metering_ready {
                "ready"
            } else {
                "unavailable"
            }
            .into(),
            quota: if quota_ready { "ready" } else { "unavailable" }.into(),
            key_rotations: if !rotations_ready {
                "unavailable"
            } else if pending_key_rotations > 0 {
                "attention_required"
            } else {
                "ready"
            }
            .into(),
            pending_key_rotations,
            delivery_journal: if !delivery_journal_ready {
                "unavailable"
            } else if stats.stale_pending > 0 || stats.unpublished_events > 0 {
                "attention_required"
            } else {
                "ready"
            }
            .into(),
            pending_deliveries: stats.pending,
            stale_pending_deliveries: stats.stale_pending,
            unpublished_delivery_events: stats.unpublished_events,
            adapters_total: statuses.len(),
            adapters_connected: connected,
        }),
    )
}

/// 健康检查
///
/// 返回网关服务的当前状态，包括适配器连接情况和活跃会话数。
#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "Health",
    responses(
        (status = 200, description = "Service health status", body = HealthResponse)
    )
)]
pub async fn health_check(State(state): State<AppState>) -> Json<HealthResponse> {
    let statuses = state.adapter_manager.list_statuses().await;
    let connected = statuses.iter().filter(|s| s.connected).count();

    Json(HealthResponse {
        status: if connected > 0 {
            "healthy".to_string()
        } else {
            "degraded".to_string()
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
        schema_version: easybot_core::storage::migration::SCHEMA_VERSION,
        uptime: state.started_at.elapsed().as_secs() as i64,
        adapters: AdapterSummary {
            total: statuses.len(),
            connected,
        },
        sessions: SessionSummary {
            active: state.session_manager.count(),
        },
    })
}
