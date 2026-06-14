//! 会话管理路由

use axum::{
    Json,
    extract::{State, Path},
    http::StatusCode,
};
use crate::AppState;
use easybot_core::types::session::SessionFilter;

/// 获取会话列表
#[utoipa::path(
    get,
    path = "/api/v1/sessions",
    tag = "Sessions",
    responses(
        (status = 200, description = "List of active sessions", body = serde_json::Value),
    )
)]
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

/// 获取会话详情
#[utoipa::path(
    get,
    path = "/api/v1/sessions/{key}",
    tag = "Sessions",
    params(
        ("key" = String, Path, description = "Session key in 'platform:chatId' format")
    ),
    responses(
        (status = 200, description = "Session details", body = serde_json::Value),
        (status = 404, description = "Session not found"),
    )
)]
pub async fn get_session(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.session_manager.get(&key) {
        Some(session) => (StatusCode::OK, Json(serde_json::to_value(session).unwrap_or_default())),
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": "NOT_FOUND",
            "message": format!("Session '{}' not found", key),
        }))),
    }
}

/// 删除会话
#[utoipa::path(
    delete,
    path = "/api/v1/sessions/{key}",
    tag = "Sessions",
    params(
        ("key" = String, Path, description = "Session key to delete")
    ),
    responses(
        (status = 200, description = "Delete result", body = serde_json::Value),
        (status = 404, description = "Session not found"),
    )
)]
pub async fn delete_session(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if state.session_manager.delete(&key).await {
        (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("Session '{}' not found", key),
        })))
    }
}
