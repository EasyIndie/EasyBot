//! 健康检查路由

use crate::AppState;
use axum::{extract::State, Json};
use serde::Serialize;
use utoipa::ToSchema;

/// 健康检查响应
#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    #[schema(example = "healthy")]
    pub status: String,
    #[schema(example = "0.1.0")]
    pub version: String,
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
