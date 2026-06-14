//! 消息收发路由

use axum::{
    Json,
    extract::{State, Path, Query},
};
use serde::{Deserialize, Serialize};
use crate::AppState;
use crate::response::{ApiError, api_error};
use easybot_core::types::message::*;
use easybot_core::types::event::GatewayEvent;

/// 发送消息请求
#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub target: String,
    pub text: String,
    pub parse_mode: Option<ParseMode>,
    pub reply_to: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// 批量发送请求
#[derive(Deserialize)]
pub struct BatchSendRequest {
    pub targets: Vec<String>,
    pub text: String,
    pub parse_mode: Option<ParseMode>,
}

/// 消息历史查询参数
#[derive(Deserialize)]
pub struct MessageHistoryParams {
    pub session_key: Option<String>,
    pub platform: Option<String>,
    pub chat_id: Option<String>,
    pub limit: Option<usize>,
    pub before: Option<i64>,
}

/// POST /api/v1/messages/send
pub async fn send_message(
    State(state): State<AppState>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (platform, chat_id) = parse_target(&req.target)
        .ok_or_else(|| api_error(easybot_core::types::error::GatewayError::InvalidRequest(
            "Invalid target format. Expected 'platform:chatId'".to_string()
        )))?;

    let result = state.adapter_manager.send_message(&platform, SendTextParams {
        chat_id,
        message: OutboundMessage {
            text: req.text,
            parse_mode: req.parse_mode.unwrap_or_default(),
        },
        reply_to: req.reply_to,
        metadata: req.metadata,
    }).await.map_err(api_error)?;

    // 发布事件
    state.event_bus.publish(GatewayEvent::new(
        easybot_core::types::event::event_types::MESSAGE_SENT,
        "api",
        serde_json::to_value(&result).unwrap_or_default(),
    ));

    Ok(Json(serde_json::json!({
        "id": result.message_id,
        "status": if result.success { "sent" } else { "failed" },
        "messageId": result.message_id,
        "timestamp": result.timestamp,
    })))
}

/// POST /api/v1/messages/batch-send
pub async fn batch_send(
    State(state): State<AppState>,
    Json(req): Json<BatchSendRequest>,
) -> Json<serde_json::Value> {
    let mut results = serde_json::Map::new();
    let parse_mode = req.parse_mode.unwrap_or_default();

    for target in &req.targets {
        let result = match parse_target(target) {
            Some((platform, chat_id)) => {
                state.adapter_manager.send_message(&platform, SendTextParams {
                    chat_id,
                    message: OutboundMessage {
                        text: req.text.clone(),
                        parse_mode: parse_mode.clone(),
                    },
                    reply_to: None,
                    metadata: None,
                }).await
            }
            None => Err(easybot_core::types::error::GatewayError::InvalidRequest(
                format!("Invalid target: {}", target)
            )),
        };

        match result {
            Ok(r) => {
                results.insert(target.clone(), serde_json::json!({
                    "status": "sent",
                    "messageId": r.message_id,
                }));
            }
            Err(e) => {
                results.insert(target.clone(), serde_json::json!({
                    "status": "failed",
                    "error": e.to_string(),
                }));
            }
        }
    }

    Json(serde_json::json!({
        "total": req.targets.len(),
        "results": results,
    }))
}

/// PUT /api/v1/messages/{message_id}
pub async fn edit_message(
    State(_state): State<AppState>,
    Path(_message_id): Path<String>,
    Json(_req): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    // Phase 1 简化：编辑消息需要 platform + chat_id 上下文
    Json(serde_json::json!({
        "error": "NOT_IMPLEMENTED",
        "message": "Message editing not yet implemented in Phase 1"
    }))
}

/// DELETE /api/v1/messages/{message_id}
pub async fn delete_message(
    State(_state): State<AppState>,
    Path(_message_id): Path<String>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "error": "NOT_IMPLEMENTED",
        "message": "Message deletion not yet implemented in Phase 1"
    }))
}

/// 消息历史查询（Phase 1 简化：仅返回内存中的信息）
#[derive(Serialize)]
pub struct MessageHistoryResponse {
    pub messages: Vec<serde_json::Value>,
    pub has_more: bool,
}

/// GET /api/v1/messages
pub async fn message_history(
    State(state): State<AppState>,
    Query(params): Query<MessageHistoryParams>,
) -> Json<MessageHistoryResponse> {
    // Phase 1: 仅查询活跃会话
    if let Some(key) = &params.session_key {
        if let Some(session) = state.session_manager.get(key) {
            return Json(MessageHistoryResponse {
                messages: vec![serde_json::json!({
                    "sessionKey": session.key,
                    "platform": session.platform,
                    "chatId": session.chat_id,
                    "createdAt": session.created_at,
                    "updatedAt": session.updated_at,
                })],
                has_more: false,
            });
        }
    }

    Json(MessageHistoryResponse {
        messages: vec![],
        has_more: false,
    })
}

/// 解析 "platform:chatId" 格式
fn parse_target(target: &str) -> Option<(String, String)> {
    let colon = target.find(':')?;
    let platform = target[..colon].to_string();
    let chat_id = target[colon + 1..].to_string();
    if platform.is_empty() || chat_id.is_empty() {
        return None;
    }
    Some((platform, chat_id))
}
