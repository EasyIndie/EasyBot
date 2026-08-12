//! 聊天信息路由

use crate::AppState;
use crate::response::ApiError;
use axum::{
    Json,
    extract::{Extension, Path, State},
};
use easybot_core::auth::target_actions;
use easybot_core::types::error::GatewayError;

/// 获取指定平台的聊天列表
#[utoipa::path(
    get,
    path = "/api/v1/chats/{platform}",
    tag = "Chats",
    params(
        ("platform" = String, Path, description = "Platform identifier")
    ),
    responses(
        (status = 200, description = "List of chats", body = serde_json::Value),
    )
)]
pub async fn list_chats(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(platform): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Phase 1 简化：通过会话管理器获取活跃聊天
    let sessions = state.session_manager.list(None);
    let is_global = easybot_core::auth::permissions::has_global_target_access(&actor);
    let grants = if is_global {
        Vec::new()
    } else {
        state
            .auth_manager
            .list_target_grants(&actor.subject_id)
            .await
            .map_err(|error| ApiError(GatewayError::StorageError(error)))?
    };
    let chats: Vec<serde_json::Value> = sessions
        .iter()
        .filter(|session| {
            session.platform == platform
                && (is_global
                    || grants.iter().any(|grant| {
                        (grant.platform == "*" || grant.platform == session.platform)
                            && (grant.chat_id == "*" || grant.chat_id == session.chat_id)
                            && grant
                                .actions
                                .iter()
                                .any(|action| action == target_actions::SESSIONS_READ)
                    }))
        })
        .map(|s| {
            serde_json::json!({
                "chatId": s.chat_id,
                "name": s.custom_name.clone().or(s.source.chat_name.clone()),
                "type": format!("{:?}", s.source.chat_type),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "chats": chats })))
}

/// 获取指定聊天的详细信息
#[utoipa::path(
    get,
    path = "/api/v1/chats/{platform}/{chat_id}",
    tag = "Chats",
    params(
        ("platform" = String, Path, description = "Platform identifier"),
        ("chat_id" = String, Path, description = "Chat ID"),
    ),
    responses(
        (status = 200, description = "Chat info", body = serde_json::Value),
    )
)]
pub async fn get_chat(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path((platform, chat_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !easybot_core::auth::permissions::has_global_target_access(&actor) {
        match state
            .auth_manager
            .target_authorized(
                &actor.subject_id,
                &platform,
                &chat_id,
                target_actions::SESSIONS_READ,
            )
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return Err(ApiError(GatewayError::Forbidden(format!(
                    "subject is not authorized for {platform}:{chat_id}"
                ))));
            }
            Err(error) => return Err(ApiError(GatewayError::StorageError(error))),
        }
    }
    let key = format!("{}:{}", platform, chat_id);
    match state.session_manager.get(&key) {
        Some(session) => Ok(Json(serde_json::json!({
            "chatId": session.chat_id,
            "name": session.custom_name.clone().or(session.source.chat_name.clone()),
            "type": format!("{:?}", session.source.chat_type),
            "available": true,
        }))),
        None => Ok(Json(serde_json::json!({
            "chatId": chat_id,
            "available": false,
            "error": "No active session for this chat",
        }))),
    }
}
