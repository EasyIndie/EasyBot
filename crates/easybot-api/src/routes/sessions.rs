//! 会话管理路由

use axum::{
    Json,
    extract::{State, Path},
};
use crate::AppState;
use easybot_core::types::session::SessionFilter;

/// GET /api/v1/sessions
pub async fn list_sessions(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let sessions = state.session_manager.list(Some(SessionFilter {
        platform: None,
        active_within_minutes: None,
        limit: Some(100),
        offset: None,
    }));

    Json(serde_json::json!({
        "sessions": sessions,
        "total": sessions.len(),
    }))
}

/// GET /api/v1/sessions/{key}
pub async fn get_session(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Json<serde_json::Value> {
    match state.session_manager.get(&key) {
        Some(session) => Json(serde_json::to_value(session).unwrap_or_default()),
        None => Json(serde_json::json!({
            "error": "NOT_FOUND",
            "message": format!("Session '{}' not found", key),
        })),
    }
}

/// DELETE /api/v1/sessions/{key}
pub async fn delete_session(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Json<serde_json::Value> {
    if state.session_manager.delete(&key) {
        Json(serde_json::json!({ "ok": true }))
    } else {
        Json(serde_json::json!({
            "ok": false,
            "error": format!("Session '{}' not found", key),
        }))
    }
}
