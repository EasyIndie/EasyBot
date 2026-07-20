//! 会话管理路由

use crate::AppState;
use crate::response::ApiError;
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
};
use easybot_core::storage::MessageFilter;
use easybot_core::types::error::GatewayError;
use easybot_core::types::session::SessionFilter;
use serde::Deserialize;

/// 获取会话列表
#[utoipa::path(
    get,
    path = "/api/v1/sessions",
    tag = "Sessions",
    responses(
        (status = 200, description = "List of active sessions", body = serde_json::Value),
    )
)]
pub async fn list_sessions(State(state): State<AppState>) -> Json<serde_json::Value> {
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
        Some(session) => (
            StatusCode::OK,
            Json(serde_json::to_value(session).unwrap_or_default()),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "NOT_FOUND",
                "message": format!("Session '{}' not found", key),
            })),
        ),
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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(key): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let existing_session = state.session_manager.get(&key);
    let session_existed = existing_session.is_some();
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "privacy.session.delete.requested",
            &format!("session:{key}"),
            serde_json::json!({"existed":session_existed}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    let deleted_messages = state
        .message_store
        .delete_messages_by_session(&key)
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error.to_string())))?;
    let (platform, chat_id) = existing_session
        .as_ref()
        .map(|session| (session.platform.as_str(), session.chat_id.as_str()))
        .or_else(|| key.split_once(':'))
        .unwrap_or(("", ""));
    let deleted_deliveries = if platform.is_empty() || chat_id.is_empty() {
        0
    } else {
        state
            .message_store
            .delete_outbound_deliveries_by_session(platform, chat_id)
            .await
            .map_err(|error| ApiError(GatewayError::StorageError(error.to_string())))?
    };
    let deleted_session = state.session_manager.delete(&key).await;
    if session_existed || deleted_session || deleted_messages > 0 || deleted_deliveries > 0 {
        state
            .auth_manager
            .record_audit(
                &actor.id,
                "privacy.session.deleted",
                &format!("session:{key}"),
                serde_json::json!({
                    "deleted_messages":deleted_messages,
                    "deleted_deliveries":deleted_deliveries
                }),
            )
            .await
            .map_err(|error| ApiError(GatewayError::Internal(error)))?;
        Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "ok": true,
                "deleted_messages": deleted_messages,
                "deleted_deliveries": deleted_deliveries
            })),
        ))
    } else {
        Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!("Session '{}' not found", key),
            })),
        ))
    }
}

#[derive(Deserialize)]
pub struct ExportQuery {
    pub limit: Option<usize>,
    pub before: Option<i64>,
    pub offset: Option<usize>,
    pub delivery_offset: Option<usize>,
}

/// Export one page of all personal data associated with a session.
#[utoipa::path(
    get,
    path = "/api/v1/sessions/{key}/export",
    tag = "Privacy",
    params(
        ("key" = String, Path),
        ("limit" = Option<usize>, Query),
        ("before" = Option<i64>, Query),
        ("offset" = Option<usize>, Query)
        ,("delivery_offset" = Option<usize>, Query)
    ),
    responses((status = 200, body = serde_json::Value), (status = 404))
)]
pub async fn export_session_data(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(key): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "privacy.session.export.requested",
            &format!("session:{key}"),
            serde_json::json!({
                "limit":query.limit,
                "before":query.before,
                "offset":query.offset,
                "delivery_offset":query.delivery_offset
            }),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    let session = state
        .session_manager
        .get(&key)
        .ok_or_else(|| ApiError(GatewayError::ChatNotFound(key.clone())))?;
    let limit = query.limit.unwrap_or(500).clamp(1, 1_000);
    let messages = state
        .message_store
        .list_messages(&MessageFilter {
            session_key: Some(key.clone()),
            limit: Some(limit + 1),
            before: query.before,
            offset: query.offset,
            ..Default::default()
        })
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error.to_string())))?;
    let truncated = messages.len() > limit;
    let page: Vec<_> = messages.into_iter().take(limit).collect();
    let next_before = truncated
        .then(|| page.last().map(|message| message.timestamp))
        .flatten();
    let next_offset = truncated.then_some(query.offset.unwrap_or(0) + limit);
    let delivery_offset = query.delivery_offset.unwrap_or(0);
    let deliveries = state
        .message_store
        .list_outbound_deliveries_by_session(
            &session.platform,
            &session.chat_id,
            limit + 1,
            delivery_offset,
        )
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error.to_string())))?;
    let deliveries_truncated = deliveries.len() > limit;
    let delivery_page: Vec<_> = deliveries.into_iter().take(limit).collect();
    let next_delivery_offset =
        deliveries_truncated.then_some(delivery_offset.saturating_add(limit));
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "privacy.session.exported",
            &format!("session:{key}"),
            serde_json::json!({
                "message_records":page.len(),
                "delivery_records":delivery_page.len()
            }),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    Ok(Json(serde_json::json!({
        "exported_at": chrono::Utc::now().timestamp_millis(),
        "session": session,
        "messages": page,
        "outbound_deliveries": delivery_page,
        "deliveries_truncated": deliveries_truncated,
        "next_delivery_offset": next_delivery_offset,
        "truncated": truncated,
        "next_before": next_before,
        "next_offset": next_offset
    })))
}
