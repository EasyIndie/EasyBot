//! 聊天信息路由

use axum::{
    Json,
    extract::{State, Path},
};
use crate::AppState;

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
    Path(platform): Path<String>,
) -> Json<serde_json::Value> {
    // Phase 1 简化：通过会话管理器获取活跃聊天
    let sessions = state.session_manager.list(None);
    let chats: Vec<serde_json::Value> = sessions
        .iter()
        .filter(|s| s.platform == platform)
        .map(|s| serde_json::json!({
            "chatId": s.chat_id,
            "name": s.source.chat_name,
            "type": format!("{:?}", s.source.chat_type),
        }))
        .collect();

    Json(serde_json::json!({ "chats": chats }))
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
    Path((platform, chat_id)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    let key = format!("{}:{}", platform, chat_id);
    match state.session_manager.get(&key) {
        Some(session) => Json(serde_json::json!({
            "chatId": session.chat_id,
            "name": session.source.chat_name,
            "type": format!("{:?}", session.source.chat_type),
            "available": true,
        })),
        None => Json(serde_json::json!({
            "chatId": chat_id,
            "available": false,
            "error": "No active session for this chat",
        })),
    }
}
