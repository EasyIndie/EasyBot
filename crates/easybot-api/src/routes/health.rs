//! 健康检查路由

use axum::{Json, extract::State};
use serde::Serialize;
use crate::AppState;

/// 健康检查响应
#[derive(Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime: i64,
    pub adapters: AdapterSummary,
    pub sessions: SessionSummary,
}

#[derive(Serialize)]
pub struct AdapterSummary {
    pub total: usize,
    pub connected: usize,
}

#[derive(Serialize)]
pub struct SessionSummary {
    pub active: usize,
}

/// GET /api/v1/health
pub async fn health_check(
    State(state): State<AppState>,
) -> Json<HealthResponse> {
    let statuses = state.adapter_manager.list_statuses().await;
    let connected = statuses.iter().filter(|s| s.connected).count();

    Json(HealthResponse {
        status: if connected > 0 { "healthy".to_string() } else { "degraded".to_string() },
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime: 0, // TODO: 记录启动时间
        adapters: AdapterSummary {
            total: statuses.len(),
            connected,
        },
        sessions: SessionSummary {
            active: state.session_manager.count(),
        },
    })
}
