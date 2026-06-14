//! 消息收发路由

use std::sync::Arc;
use axum::{
    Json,
    extract::{State, Path, Query},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use crate::AppState;
use crate::response::{ApiError, api_error};
use easybot_core::types::message::*;
use easybot_core::types::event::GatewayEvent;

/// 发送消息请求
#[derive(Deserialize, ToSchema)]
pub struct SendMessageRequest {
    /// 目标格式 "platform:chatId"，例如 "telegram:123456"
    #[schema(example = "telegram:123456")]
    pub target: String,
    /// 消息文本内容
    #[schema(example = "Hello, World!")]
    pub text: String,
    /// 文本解析模式（markdown / html / none）
    pub parse_mode: Option<ParseMode>,
    /// 被回复消息 ID（可选）
    pub reply_to: Option<String>,
    /// 平台特有元数据
    pub metadata: Option<serde_json::Value>,
}

/// 批量发送请求
#[derive(Deserialize, ToSchema)]
pub struct BatchSendRequest {
    /// 目标列表，每个元素格式 "platform:chatId"
    #[schema(example = json!(["telegram:123456", "telegram:789012"]))]
    pub targets: Vec<String>,
    /// 消息文本
    #[schema(example = "Broadcast message")]
    pub text: String,
    /// 文本解析模式
    pub parse_mode: Option<ParseMode>,
}

/// 消息历史查询参数
#[derive(Deserialize, ToSchema, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MessageHistoryParams {
    pub session_key: Option<String>,
    pub platform: Option<String>,
    pub chat_id: Option<String>,
    pub limit: Option<usize>,
    pub before: Option<i64>,
}

/// 发送消息
///
/// 向指定 IM 平台的目标聊天发送一条文本消息。
/// 目标格式为 "platform:chatId"，例如 "telegram:123456789"。
#[utoipa::path(
    post,
    path = "/api/v1/messages/send",
    tag = "Messages",
    request_body = SendMessageRequest,
    responses(
        (status = 200, description = "Message sent", body = serde_json::Value),
        (status = 400, description = "Invalid request or target format"),
        (status = 404, description = "Platform or chat not found"),
    )
)]
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

    let mut resp = serde_json::json!({
        "id": result.message_id,
        "status": if result.success { "sent" } else { "failed" },
        "messageId": result.message_id,
        "timestamp": result.timestamp,
    });
    if let Some(ref err) = result.error {
        resp["error"] = serde_json::json!(err);
    }
    if let Some(ref err_code) = result.error_code {
        resp["errorCode"] = serde_json::json!(err_code);
    }
    Ok(Json(resp))
}

/// 批量发送消息
///
/// 向多个目标发送相同的文本消息。每个目标格式为 "platform:chatId"。
/// 使用并发限制（最大 5 个并发）和整体 30 秒超时。
#[utoipa::path(
    post,
    path = "/api/v1/messages/batch-send",
    tag = "Messages",
    request_body = BatchSendRequest,
    responses(
        (status = 200, description = "Batch send results", body = serde_json::Value),
        (status = 400, description = "Invalid request"),
    )
)]
pub async fn batch_send(
    State(state): State<AppState>,
    Json(req): Json<BatchSendRequest>,
) -> Json<serde_json::Value> {
    let parse_mode = req.parse_mode.unwrap_or_default();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(5)); // 最大并发 5
    let results = Arc::new(tokio::sync::Mutex::new(serde_json::Map::new()));
    let mut handles = Vec::with_capacity(req.targets.len());

    // 并发发送所有目标
    for target in &req.targets {
        let target = target.clone();
        let text = req.text.clone();
        let parse_mode = parse_mode.clone();
        let semaphore = semaphore.clone();
        let results = results.clone();
        let state = state.clone();

        let handle = tokio::spawn(async move {
            let _permit = semaphore.acquire().await;

            let _result = match parse_target(&target) {
                Some((platform, chat_id)) => {
                    let send_result = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        state.adapter_manager.send_message(&platform, SendTextParams {
                            chat_id,
                            message: OutboundMessage {
                                text,
                                parse_mode,
                            },
                            reply_to: None,
                            metadata: None,
                        }),
                    ).await;

                    match send_result {
                        Ok(Ok(r)) => {
                            results.lock().await.insert(target.clone(), serde_json::json!({
                                "status": "sent",
                                "messageId": r.message_id,
                            }));
                        }
                        Ok(Err(e)) => {
                            results.lock().await.insert(target.clone(), serde_json::json!({
                                "status": "failed",
                                "error": e.to_string(),
                            }));
                        }
                        Err(_) => {
                            results.lock().await.insert(target.clone(), serde_json::json!({
                                "status": "failed",
                                "error": "Request timed out (15s)",
                            }));
                        }
                    }
                }
                None => {
                    results.lock().await.insert(target.clone(), serde_json::json!({
                        "status": "failed",
                        "error": format!("Invalid target: {}", target),
                    }));
                }
            };
        });

        handles.push(handle);
    }

    // 等待所有发送完成（整体超时 30 秒）
    let all_futures = futures::future::join_all(handles);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), all_futures).await;

    let final_results = Arc::try_unwrap(results).unwrap().into_inner();

    Json(serde_json::json!({
        "total": req.targets.len(),
        "results": final_results,
    }))
}

/// 编辑消息（Phase 2 实现）
#[utoipa::path(
    put,
    path = "/api/v1/messages/{message_id}",
    tag = "Messages",
    params(
        ("message_id" = String, Path, description = "Platform message ID")
    ),
    responses(
        (status = 200, description = "Not yet implemented", body = serde_json::Value),
    )
)]
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

/// 删除消息（Phase 2 实现）
#[utoipa::path(
    delete,
    path = "/api/v1/messages/{message_id}",
    tag = "Messages",
    params(
        ("message_id" = String, Path, description = "Platform message ID")
    ),
    responses(
        (status = 200, description = "Not yet implemented", body = serde_json::Value),
    )
)]
pub async fn delete_message(
    State(_state): State<AppState>,
    Path(_message_id): Path<String>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "error": "NOT_IMPLEMENTED",
        "message": "Message deletion not yet implemented in Phase 1"
    }))
}

/// 消息历史查询响应
#[derive(Serialize, ToSchema)]
pub struct MessageHistoryResponse {
    pub messages: Vec<serde_json::Value>,
    pub has_more: bool,
}

/// 查询消息历史
///
/// Phase 1 简化：仅返回内存中的会话信息，消息内容存储将在 Phase 2 实现。
#[utoipa::path(
    get,
    path = "/api/v1/messages",
    tag = "Messages",
    params(
        MessageHistoryParams
    ),
    responses(
        (status = 200, description = "Message history", body = MessageHistoryResponse),
    )
)]
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
