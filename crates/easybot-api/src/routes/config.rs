//! 配置管理路由

use axum::{Json, extract::State};
use crate::AppState;

/// GET /api/v1/config
pub async fn get_config(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    Json(serde_json::to_value(&*state.config).unwrap_or_default())
}

/// PUT /api/v1/config
pub async fn update_config(
    State(_state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    // Phase 1: 配置热重载暂不实现
    Json(serde_json::json!({
        "ok": false,
        "error": "NOT_IMPLEMENTED",
        "message": "Config hot-reload not yet implemented"
    }))
}
