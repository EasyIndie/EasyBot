#![allow(unused_qualifications)]
//! 消息收发路由

use crate::AppState;
use crate::response::{ApiError, api_error};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use easybot_core::storage::{MessageFilter, StoredMessage};
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::message::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

/// 批量发送最大目标数（防止单次请求耗尽 tokio 任务预算）
const MAX_BATCH_TARGETS: usize = 100;
/// Maximum message text length (characters)
const MAX_MESSAGE_LENGTH: usize = 16384;
/// Maximum metadata JSON size (bytes)
const MAX_METADATA_SIZE: usize = 65536;

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
    /// 媒体附件（可选，如果提供则发送媒体消息）
    pub media: Option<MediaAttachment>,
    /// 行内键盘（可选，如果提供则发送交互式消息，优先级低于 media）
    pub keyboard: Option<InlineKeyboard>,
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
#[allow(unused_qualifications)]
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
    let (platform, chat_id) = parse_target(&req.target).ok_or_else(|| {
        api_error(GatewayError::InvalidRequest(
            "Invalid target format. Expected 'platform:chatId'".to_string(),
        ))
    })?;

    // SECURITY: Validate message length
    if req.text.len() > MAX_MESSAGE_LENGTH {
        return Err(api_error(GatewayError::MessageTooLong {
            current: req.text.len(),
            max: MAX_MESSAGE_LENGTH,
        }));
    }

    // SECURITY: Validate metadata size
    if let Some(ref meta) = req.metadata {
        let meta_json = serde_json::to_string(meta).unwrap_or_default();
        if meta_json.len() > MAX_METADATA_SIZE {
            return Err(api_error(GatewayError::InvalidRequest(format!(
                "metadata 超过最大大小 {} 字节",
                MAX_METADATA_SIZE
            ))));
        }
    }

    // 分发：media > keyboard > 纯文本
    let result = if let Some(media) = req.media {
        state
            .adapter_manager
            .send_media(
                &platform,
                SendMediaParams {
                    chat_id: chat_id.clone(),
                    media,
                    text: Some(req.text.clone()),
                    reply_to: req.reply_to,
                },
            )
            .await
            .map_err(api_error)?
    } else if let Some(keyboard) = req.keyboard {
        state
            .adapter_manager
            .send_interactive(
                &platform,
                SendInteractiveParams {
                    chat_id: chat_id.clone(),
                    text: req.text.clone(),
                    keyboard,
                    reply_to: req.reply_to,
                },
            )
            .await
            .map_err(api_error)?
    } else {
        state
            .adapter_manager
            .send_message(
                &platform,
                SendTextParams {
                    chat_id: chat_id.clone(),
                    message: OutboundMessage {
                        text: req.text.clone(),
                        parse_mode: req.parse_mode.unwrap_or_default(),
                    },
                    reply_to: req.reply_to,
                    metadata: req.metadata,
                },
            )
            .await
            .map_err(api_error)?
    };

    // 记录出站消息指标
    if let Some(ref metrics) = state.metrics {
        metrics.record_outbound_message(&platform);
    }

    // 持久化出站消息
    let stored = StoredMessage::from_outbound(&platform, &chat_id, None, &req.text, &result);
    if let Err(e) = state.message_store.store_message(&stored).await {
        tracing::warn!("Failed to persist outbound message: {}", e);
    }

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
        // SECURITY: Sanitize adapter errors — truncate and strip potential
        // internal details (URLs, tokens, stack traces) before returning to client
        let sanitized = sanitize_error_message(err);
        resp["error"] = serde_json::json!(sanitized);
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
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.targets.len() > MAX_BATCH_TARGETS {
        return Err(api_error(GatewayError::InvalidRequest(format!(
            "最多支持 {} 个目标，当前 {} 个",
            MAX_BATCH_TARGETS,
            req.targets.len()
        ))));
    }

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

            match parse_target(&target) {
                Some((platform, chat_id)) => {
                    let send_result = tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        state.adapter_manager.send_message(
                            &platform,
                            SendTextParams {
                                chat_id,
                                message: OutboundMessage { text, parse_mode },
                                reply_to: None,
                                metadata: None,
                            },
                        ),
                    )
                    .await;

                    match send_result {
                        Ok(Ok(r)) => {
                            // 记录出站消息指标
                            if let Some(ref metrics) = state.metrics {
                                metrics.record_outbound_message(&platform);
                            }
                            results.lock().await.insert(
                                target.clone(),
                                serde_json::json!({
                                    "status": "sent",
                                    "messageId": r.message_id,
                                }),
                            );
                        }
                        Ok(Err(e)) => {
                            results.lock().await.insert(
                                target.clone(),
                                serde_json::json!({
                                    "status": "failed",
                                    "error": e.to_string(),
                                }),
                            );
                        }
                        Err(_) => {
                            results.lock().await.insert(
                                target.clone(),
                                serde_json::json!({
                                    "status": "failed",
                                    "error": "Request timed out (15s)",
                                }),
                            );
                        }
                    }
                }
                None => {
                    results.lock().await.insert(
                        target.clone(),
                        serde_json::json!({
                            "status": "failed",
                            "error": format!("Invalid target: {}", target),
                        }),
                    );
                }
            };
        });

        handles.push(handle);
    }

    // 等待所有发送完成（整体超时 30 秒）
    let all_futures = futures::future::join_all(handles);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), all_futures).await;

    // 安全解包 Arc：若 task 超时后仍持有 Arc 引用，回退到克隆数据
    let final_results = match Arc::try_unwrap(results) {
        Ok(mutex) => mutex.into_inner(),
        Err(arc) => {
            tracing::warn!("batch_send: Arc 仍有多个引用，使用 blocking_lock 回退");
            arc.blocking_lock().clone()
        }
    };

    Ok(Json(serde_json::json!({
        "total": req.targets.len(),
        "results": final_results,
    })))
}

/// 编辑消息请求
#[derive(Deserialize, ToSchema)]
pub struct EditMessageRequest {
    /// 目标格式 "platform:chatId"，例如 "telegram:123456"
    #[schema(example = "telegram:123456")]
    pub target: String,
    /// 新的消息文本
    #[schema(example = "Updated text")]
    pub text: String,
    /// 文本解析模式
    pub parse_mode: Option<ParseMode>,
    /// 更新后的行内键盘（可选）
    pub keyboard: Option<InlineKeyboard>,
}

/// 删除消息请求
#[derive(Deserialize, ToSchema)]
pub struct DeleteMessageRequest {
    /// 目标格式 "platform:chatId"，例如 "telegram:123456"
    #[schema(example = "telegram:123456")]
    pub target: String,
}

/// 编辑消息
///
/// 编辑已发送的消息内容。目标格式为 "platform:chatId"。
/// 仅当适配器支持 MessageEdit 能力时有效。
#[utoipa::path(
    put,
    path = "/api/v1/messages/{message_id}",
    tag = "Messages",
    params(
        ("message_id" = String, Path, description = "Platform message ID")
    ),
    request_body = EditMessageRequest,
    responses(
        (status = 200, description = "Message edited", body = serde_json::Value),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Message or platform not found"),
    )
)]
pub async fn edit_message(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
    Json(req): Json<EditMessageRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (platform, chat_id) = parse_target(&req.target).ok_or_else(|| {
        api_error(GatewayError::InvalidRequest(
            "Invalid target format. Expected 'platform:chatId'".to_string(),
        ))
    })?;

    let params = EditMessageParams {
        chat_id,
        message_id: message_id.clone(),
        message: OutboundMessage {
            text: req.text,
            parse_mode: req.parse_mode.unwrap_or_default(),
        },
        keyboard: req.keyboard,
    };

    let result = state
        .adapter_manager
        .edit_message(&platform, params)
        .await
        .map_err(api_error)?;

    // 发布事件
    state.event_bus.publish(GatewayEvent::new(
        easybot_core::types::event::event_types::MESSAGE_SENT,
        "api",
        serde_json::json!({
            "action": "edit",
            "message_id": message_id,
            "result": result,
        }),
    ));

    Ok(Json(serde_json::json!({
        "ok": result.success,
        "updated_at": result.updated_at,
        "error": result.error,
    })))
}

/// 删除消息
///
/// 删除已发送的消息。目标格式为 "platform:chatId"。
/// 仅当适配器支持 MessageDelete 能力时有效。
#[utoipa::path(
    delete,
    path = "/api/v1/messages/{message_id}",
    tag = "Messages",
    params(
        ("message_id" = String, Path, description = "Platform message ID")
    ),
    request_body = DeleteMessageRequest,
    responses(
        (status = 200, description = "Message deleted", body = serde_json::Value),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Message or platform not found"),
    )
)]
pub async fn delete_message(
    State(state): State<AppState>,
    Path(message_id): Path<String>,
    Json(req): Json<DeleteMessageRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (platform, chat_id) = parse_target(&req.target).ok_or_else(|| {
        api_error(GatewayError::InvalidRequest(
            "Invalid target format. Expected 'platform:chatId'".to_string(),
        ))
    })?;

    let result = state
        .adapter_manager
        .delete_message(&platform, &chat_id, &message_id)
        .await
        .map_err(api_error)?;

    // 发布事件
    state.event_bus.publish(GatewayEvent::new(
        easybot_core::types::event::event_types::MESSAGE_SENT,
        "api",
        serde_json::json!({
            "action": "delete",
            "message_id": message_id,
            "result": result,
        }),
    ));

    Ok(Json(serde_json::json!({
        "ok": result.success,
        "error": result.error,
    })))
}

/// 消息历史查询响应
#[derive(Serialize, ToSchema)]
pub struct MessageHistoryResponse {
    pub messages: Vec<serde_json::Value>,
    pub has_more: bool,
}

/// 查询消息历史
///
/// 从持久化存储中查询消息历史，支持按会话键、平台、聊天 ID 过滤。
/// 分页使用 before 游标（基于时间戳）和 limit。
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
    let limit = params.limit.unwrap_or(50);

    let filter = MessageFilter {
        session_key: params.session_key,
        platform: params.platform,
        chat_id: params.chat_id,
        limit: Some(limit + 1), // 多取一条判断 has_more
        offset: None,
        before: params.before,
    };

    let messages = state
        .message_store
        .list_messages(&filter)
        .await
        .unwrap_or_default();

    let has_more = messages.len() > limit;
    let raw_payload_enabled = state.config.api.raw_payload_enabled;
    let messages: Vec<serde_json::Value> = messages
        .into_iter()
        .take(limit)
        .map(|m| {
            let mut value = serde_json::to_value(&m).unwrap_or_default();
            // SECURITY: Strip raw platform payload metadata unless explicitly enabled
            if !raw_payload_enabled && let Some(obj) = value.as_object_mut() {
                obj.remove("metadata");
            }
            value
        })
        .collect();

    Json(MessageHistoryResponse { messages, has_more })
}

/// SECURITY: Sanitize adapter error messages to prevent information leakage.
/// Truncates long errors and strips common internal patterns.
fn sanitize_error_message(err: &str) -> String {
    // Truncate at 256 chars to prevent verbose API error bodies from leaking
    let truncated: String = err.chars().take(256).collect();
    if truncated.len() < err.len() {
        format!("{}...", truncated)
    } else {
        truncated
    }
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
