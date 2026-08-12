#![allow(unused_qualifications)]
//! 消息收发路由

use crate::AppState;
use crate::response::{ApiError, api_error};
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use easybot_core::auth::IdempotencyReservation;
use easybot_core::auth::target_actions;
use easybot_core::storage::{
    MessageFilter, OutboundDelivery, OutboundDeliveryState, StoredMessage,
};
use easybot_core::types::error::GatewayError;
use easybot_core::types::event::GatewayEvent;
use easybot_core::types::message::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use utoipa::{IntoParams, ToSchema};

/// 批量发送最大目标数（防止单次请求耗尽 tokio 任务预算）
const MAX_BATCH_TARGETS: usize = 100;
/// Maximum message text length (characters)
const MAX_MESSAGE_LENGTH: usize = 16384;
/// Maximum metadata JSON size (bytes)
const MAX_METADATA_SIZE: usize = 65536;
const MAX_TARGET_LENGTH: usize = 320;
const MAX_PLATFORM_LENGTH: usize = 32;
const MAX_CHAT_ID_LENGTH: usize = 256;
const MAX_REPLY_ID_LENGTH: usize = 256;

async fn require_target_action(
    state: &AppState,
    actor: &easybot_core::auth::AuthInfo,
    platform: &str,
    chat_id: &str,
    action: &str,
) -> Result<(), ApiError> {
    if easybot_core::auth::permissions::has_global_target_access(actor) {
        return Ok(());
    }
    match state
        .auth_manager
        .target_authorized(&actor.subject_id, platform, chat_id, action)
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(api_error(GatewayError::Forbidden(format!(
            "subject is not authorized for {platform}:{chat_id}"
        )))),
        Err(error) => Err(api_error(GatewayError::StorageError(error))),
    }
}

/// 发送消息请求
#[derive(Deserialize, Serialize, ToSchema)]
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
#[derive(Deserialize, Serialize, ToSchema)]
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
    /// Page size, 1-200; defaults to 50.
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    pub before: Option<i64>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeliveryListParams {
    /// 1-200, defaults to 50.
    pub limit: Option<usize>,
}

#[derive(Deserialize, ToSchema)]
pub struct ReconcileDeliveryRequest {
    /// Confirmed external outcome: "succeeded" or "failed".
    pub resolution: String,
    /// 20-2000 characters describing the external evidence used.
    pub evidence: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/messages/deliveries",
    tag = "Messages",
    params(DeliveryListParams),
    responses((status = 200, description = "Actor-scoped outbound delivery journal"))
)]
pub async fn list_deliveries(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Query(query): Query<DeliveryListParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(api_error(GatewayError::InvalidRequest(
            "limit must be between 1 and 200".into(),
        )));
    }
    let deliveries = state
        .message_store
        .list_outbound_deliveries(&actor.subject_id, limit)
        .await
        .map_err(|error| api_error(GatewayError::StorageError(error.to_string())))?;
    Ok(Json(serde_json::json!({ "deliveries": deliveries })))
}

#[utoipa::path(
    post,
    path = "/api/v1/messages/deliveries/{delivery_id}/reconcile",
    tag = "Messages",
    params(("delivery_id" = String, Path, description = "Stable delivery ID")),
    request_body = ReconcileDeliveryRequest,
    responses(
        (status = 200, description = "Pending delivery reconciled"),
        (status = 400, description = "Invalid resolution or evidence"),
        (status = 409, description = "Delivery is absent, belongs to another actor, or is final")
    )
)]
pub async fn reconcile_delivery(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(delivery_id): Path<String>,
    Json(req): Json<ReconcileDeliveryRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if delivery_id.len() > 255 {
        return Err(api_error(GatewayError::InvalidRequest(
            "delivery_id is too long".into(),
        )));
    }
    let resolution = match req.resolution.as_str() {
        "succeeded" => OutboundDeliveryState::Succeeded,
        "failed" => OutboundDeliveryState::Failed,
        _ => {
            return Err(api_error(GatewayError::InvalidRequest(
                "resolution must be 'succeeded' or 'failed'".into(),
            )));
        }
    };
    let evidence = req.evidence.trim();
    if !(20..=2000).contains(&evidence.chars().count()) {
        return Err(api_error(GatewayError::InvalidRequest(
            "evidence must be 20-2000 characters".into(),
        )));
    }

    state
        .auth_manager
        .record_audit(
            &actor.id,
            "message.delivery.reconcile.requested",
            &delivery_id,
            serde_json::json!({ "resolution": req.resolution }),
        )
        .await
        .map_err(|error| api_error(GatewayError::StorageError(error)))?;
    let reconciled = state
        .message_store
        .reconcile_outbound_delivery(
            &delivery_id,
            &actor.subject_id,
            resolution,
            evidence,
            &actor.id,
        )
        .await
        .map_err(|error| api_error(GatewayError::StorageError(error.to_string())))?;
    if !reconciled {
        return Err(api_error(GatewayError::Conflict(
            "delivery is absent, belongs to another actor, or is already final".into(),
        )));
    }
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "message.delivery.reconcile.completed",
            &delivery_id,
            serde_json::json!({ "resolution": req.resolution }),
        )
        .await
        .map_err(|error| api_error(GatewayError::StorageError(error)))?;

    Ok(Json(serde_json::json!({
        "delivery_id": delivery_id,
        "state": req.resolution,
        "manually_reconciled": true,
    })))
}

/// 发送消息
///
/// 向指定 IM 平台的目标聊天发送一条文本消息。
/// 目标格式为 "platform:chatId"，例如 "telegram:123456789"。
#[utoipa::path(
    post,
    path = "/api/v1/messages/send",
    tag = "Messages",
    params(("Idempotency-Key" = Option<String>, Header, description = "8-128 safe ASCII characters; retained 24 hours")),
    request_body = SendMessageRequest,
    responses(
        (status = 200, description = "Message sent", body = serde_json::Value),
        (status = 400, description = "Invalid request or target format"),
        (status = 404, description = "Platform or chat not found"),
    )
)]
pub async fn send_message(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    headers: HeaderMap,
    Json(req): Json<SendMessageRequest>,
) -> Result<Response, ApiError> {
    let (platform, chat_id) = parse_target(&req.target).ok_or_else(|| {
        api_error(GatewayError::InvalidRequest(
            "Invalid target format. Expected 'platform:chatId'".to_string(),
        ))
    })?;
    require_target_action(
        &state,
        &actor,
        &platform,
        &chat_id,
        target_actions::MESSAGES_SEND,
    )
    .await?;

    // SECURITY: Validate message length
    if req.text.len() > MAX_MESSAGE_LENGTH {
        return Err(api_error(GatewayError::MessageTooLong {
            current: req.text.len(),
            max: MAX_MESSAGE_LENGTH,
        }));
    }
    if req
        .reply_to
        .as_ref()
        .is_some_and(|reply| reply.is_empty() || reply.len() > MAX_REPLY_ID_LENGTH)
    {
        return Err(api_error(GatewayError::InvalidRequest(
            "reply_to must be 1-256 bytes".into(),
        )));
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

    let idempotency_key = headers
        .get("idempotency-key")
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| {
            api_error(GatewayError::InvalidRequest(
                "Idempotency-Key must be ASCII".into(),
            ))
        })?;
    let request_hash = if let Some(key) = &idempotency_key {
        if key.len() < 8
            || key.len() > 128
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(api_error(GatewayError::InvalidRequest(
                "Idempotency-Key must be 8-128 safe ASCII characters".into(),
            )));
        }
        let encoded = serde_json::to_vec(&req)
            .map_err(|e| api_error(GatewayError::Internal(e.to_string())))?;
        let hash = Sha256::digest(encoded)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        match state
            .auth_manager
            .reserve_idempotency(&actor.subject_id, key, &hash)
            .await
            .map_err(|e| api_error(GatewayError::StorageError(e)))?
        {
            IdempotencyReservation::Acquired => {}
            IdempotencyReservation::Replay {
                status,
                response_json,
            } => {
                let status =
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let mut response = (
                    status,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    response_json,
                )
                    .into_response();
                response
                    .headers_mut()
                    .insert("idempotency-replayed", HeaderValue::from_static("true"));
                return Ok(response);
            }
            IdempotencyReservation::InProgress => {
                return Err(api_error(GatewayError::Conflict(
                    "request with this Idempotency-Key is still in progress or indeterminate"
                        .into(),
                )));
            }
            IdempotencyReservation::Conflict => {
                return Err(api_error(GatewayError::Conflict(
                    "Idempotency-Key was already used for a different request".into(),
                )));
            }
        }
        Some(hash)
    } else {
        None
    };

    let delivery_id = uuid::Uuid::now_v7().to_string();
    state
        .message_store
        .prepare_outbound_delivery(&OutboundDelivery {
            id: delivery_id.clone(),
            actor_id: actor.subject_id.clone(),
            idempotency_key: idempotency_key.clone(),
            platform: platform.clone(),
            chat_id: chat_id.clone(),
            request_json: serde_json::to_value(&req)
                .map_err(|e| api_error(GatewayError::Internal(e.to_string())))?,
            created_at: chrono::Utc::now().timestamp_millis(),
        })
        .await
        .map_err(|e| api_error(GatewayError::StorageError(e.to_string())))?;

    // 分发：media > keyboard > 纯文本
    const SEND_TIMEOUT_SECS: u64 = 15;
    let send_fut = async {
        if let Some(media) = req.media {
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
        }
    };

    let result =
        match tokio::time::timeout(std::time::Duration::from_secs(SEND_TIMEOUT_SECS), send_fut)
            .await
        {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                let error = serde_json::json!({
                    "error": sanitize_error_message(&e.to_string()),
                });
                state
                    .message_store
                    .finalize_outbound_delivery(
                        &delivery_id,
                        OutboundDeliveryState::Failed,
                        &error,
                        None,
                    )
                    .await
                    .map_err(|storage| {
                        api_error(GatewayError::StorageError(storage.to_string()))
                    })?;
                return Err(api_error(e));
            }
            Err(_) => {
                return Err(api_error(GatewayError::RequestTimeout(format!(
                    "send_message 超时 ({}s)",
                    SEND_TIMEOUT_SECS
                ))));
            }
        };

    // 记录出站消息指标
    if let Some(ref metrics) = state.metrics {
        metrics.record_outbound_message(&platform);
    }

    // 持久化出站消息
    let stored = StoredMessage::from_outbound(&platform, &chat_id, None, &req.text, &result);
    state
        .message_store
        .finalize_outbound_delivery(
            &delivery_id,
            if result.success {
                OutboundDeliveryState::Succeeded
            } else {
                OutboundDeliveryState::Failed
            },
            &serde_json::to_value(&result).unwrap_or_default(),
            Some(&stored),
        )
        .await
        .map_err(|e| api_error(GatewayError::StorageError(e.to_string())))?;

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
    if let (Some(key), Some(hash)) = (&idempotency_key, &request_hash) {
        let response_json = serde_json::to_string(&resp)
            .map_err(|e| api_error(GatewayError::Internal(e.to_string())))?;
        state
            .auth_manager
            .complete_idempotency(&actor.subject_id, key, hash, 200, &response_json)
            .await
            .map_err(|e| api_error(GatewayError::StorageError(e)))?;
    }
    let mut response = Json(resp).into_response();
    if idempotency_key.is_some() {
        response
            .headers_mut()
            .insert("idempotency-replayed", HeaderValue::from_static("false"));
    }
    Ok(response)
}

/// 应答按钮回调请求
#[derive(Deserialize, Serialize, ToSchema)]
pub struct AnswerCallbackRequest {
    /// 平台标识，如 "telegram"
    #[schema(example = "telegram")]
    pub platform: String,
    /// 回调所在聊天 ID（用于权限校验，来自 callback.received 事件的 chat_id）
    #[schema(example = "123456789")]
    pub chat_id: String,
    /// 回调 ID（来自 callback.received 事件的 id）
    #[schema(example = "callback_id_123")]
    pub callback_id: String,
    /// 通知文本（显示在按钮上方，可选）
    pub text: Option<String>,
    /// 可选跳转 URL（Telegram 特有）
    pub url: Option<String>,
    /// 以 alert 方式展示（默认 toast）
    pub show_alert: Option<bool>,
    /// 缓存应答时长（秒，Telegram 特有）
    pub cache_time: Option<u32>,
}

/// 应答按钮回调
///
/// 收到 `callback.received` 事件后调用，关闭按钮加载态。
/// Telegram 要求在收到 callback 后约 10s 内应答，否则按钮保持旋转。
#[utoipa::path(
    post,
    path = "/api/v1/callbacks/answer",
    tag = "Messages",
    request_body = AnswerCallbackRequest,
    responses(
        (status = 200, description = "Callback answered"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Adapter not connected / platform not found"),
    )
)]
pub async fn answer_callback(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Json(req): Json<AnswerCallbackRequest>,
) -> Result<Response, ApiError> {
    if req.platform.is_empty() || req.platform.len() > MAX_PLATFORM_LENGTH {
        return Err(api_error(GatewayError::InvalidRequest(
            "platform must be 1-32 bytes".into(),
        )));
    }
    if req.chat_id.is_empty() || req.chat_id.len() > MAX_CHAT_ID_LENGTH {
        return Err(api_error(GatewayError::InvalidRequest(
            "chat_id must be 1-256 bytes".into(),
        )));
    }
    if req.callback_id.is_empty() || req.callback_id.len() > MAX_REPLY_ID_LENGTH {
        return Err(api_error(GatewayError::InvalidRequest(
            "callback_id must be 1-256 bytes".into(),
        )));
    }
    // 校验动作权限（answer 视为对目标聊天的一次 send 级操作）
    require_target_action(
        &state,
        &actor,
        &req.platform,
        &req.chat_id,
        target_actions::MESSAGES_SEND,
    )
    .await?;

    state
        .adapter_manager
        .answer_callback_query(
            &req.platform,
            AnswerCallbackParams {
                callback_id: req.callback_id.clone(),
                text: req.text.clone(),
                url: req.url.clone(),
                show_alert: req.show_alert,
                cache_time: req.cache_time,
            },
        )
        .await
        .map_err(api_error)?;

    Ok(Json(serde_json::json!({ "success": true })).into_response())
}

/// 批量发送消息
///
/// 向多个目标发送相同的文本消息。每个目标格式为 "platform:chatId"。
/// 权限按 target 独立校验：无权限的目标以 `results` 中 `failed` 记录，不阻断有权限目标投递；
/// 仅当全部目标都无权限时整体返回 403。使用并发限制（最大 5 个并发）和整体 30 秒超时。
#[utoipa::path(
    post,
    path = "/api/v1/messages/batch-send",
    tag = "Messages",
    params(("Idempotency-Key" = Option<String>, Header, description = "8-128 safe ASCII characters; retained 24 hours")),
    request_body = BatchSendRequest,
    responses(
        (status = 200, description = "Batch send results", body = serde_json::Value),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "No target in the batch is authorized for the caller"),
    )
)]
pub async fn batch_send(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    headers: HeaderMap,
    Json(req): Json<BatchSendRequest>,
) -> Result<Response, ApiError> {
    if req.targets.is_empty() {
        return Err(api_error(GatewayError::InvalidRequest(
            "targets must not be empty".into(),
        )));
    }
    if req.targets.len() > MAX_BATCH_TARGETS {
        return Err(api_error(GatewayError::InvalidRequest(format!(
            "最多支持 {} 个目标，当前 {} 个",
            MAX_BATCH_TARGETS,
            req.targets.len()
        ))));
    }
    if req.text.len() > MAX_MESSAGE_LENGTH {
        return Err(api_error(GatewayError::MessageTooLong {
            current: req.text.len(),
            max: MAX_MESSAGE_LENGTH,
        }));
    }
    let mut unique_targets = std::collections::HashSet::with_capacity(req.targets.len());
    // 按 target 做权限校验：无权限/无法校验的目标只记录失败，不阻断有权限目标投递。
    let mut sendable: Vec<(String, String, String)> = Vec::new(); // (target, platform, chat_id)
    let mut pre_blocked: Vec<(String, &'static str, String)> = Vec::new(); // (target, status, error)
    let global_access = easybot_core::auth::permissions::has_global_target_access(&actor);
    for target in &req.targets {
        let Some((platform, chat_id)) = parse_target(target) else {
            return Err(api_error(GatewayError::InvalidRequest(
                "Invalid target; expected bounded platform:chatId".into(),
            )));
        };
        if !unique_targets.insert(target) {
            return Err(api_error(GatewayError::InvalidRequest(format!(
                "duplicate target: {target}"
            ))));
        }
        if global_access {
            sendable.push((target.clone(), platform, chat_id));
            continue;
        }
        match state
            .auth_manager
            .target_authorized(
                &actor.subject_id,
                &platform,
                &chat_id,
                target_actions::MESSAGES_SEND,
            )
            .await
        {
            Ok(true) => sendable.push((target.clone(), platform, chat_id)),
            Ok(false) => pre_blocked.push((
                target.clone(),
                "failed",
                format!("subject is not authorized for {platform}:{chat_id}"),
            )),
            Err(error) => pre_blocked.push((
                target.clone(),
                "indeterminate",
                format!("authorization check failed: {error}"),
            )),
        }
    }
    // 全部目标都不可投递时整体拒绝，避免调用方误以为已部分投递。
    if sendable.is_empty() {
        return Err(api_error(
            if pre_blocked.iter().all(|(_, status, _)| *status == "failed") {
                GatewayError::Forbidden(
                    "subject is not authorized for any target in this batch".into(),
                )
            } else {
                GatewayError::StorageError(
                    pre_blocked
                        .iter()
                        .find(|(_, status, _)| *status == "indeterminate")
                        .map(|(_, _, error)| error.clone())
                        .unwrap_or_else(|| "authorization storage unavailable".into()),
                )
            },
        ));
    }

    let idempotency_key = headers
        .get("idempotency-key")
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| {
            api_error(GatewayError::InvalidRequest(
                "Idempotency-Key must be ASCII".into(),
            ))
        })?;
    let request_hash = if let Some(key) = &idempotency_key {
        if key.len() < 8
            || key.len() > 128
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(api_error(GatewayError::InvalidRequest(
                "Idempotency-Key must be 8-128 safe ASCII characters".into(),
            )));
        }
        let encoded = serde_json::to_vec(&req)
            .map_err(|e| api_error(GatewayError::Internal(e.to_string())))?;
        let hash = Sha256::digest(encoded)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        match state
            .auth_manager
            .reserve_idempotency(&actor.subject_id, key, &hash)
            .await
            .map_err(|e| api_error(GatewayError::StorageError(e)))?
        {
            IdempotencyReservation::Acquired => {}
            IdempotencyReservation::Replay {
                status,
                response_json,
            } => {
                let status =
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let mut response = (
                    status,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    response_json,
                )
                    .into_response();
                response
                    .headers_mut()
                    .insert("idempotency-replayed", HeaderValue::from_static("true"));
                return Ok(response);
            }
            IdempotencyReservation::InProgress => {
                return Err(api_error(GatewayError::Conflict(
                    "batch with this Idempotency-Key is still in progress or indeterminate".into(),
                )));
            }
            IdempotencyReservation::Conflict => {
                return Err(api_error(GatewayError::Conflict(
                    "Idempotency-Key was already used for a different request".into(),
                )));
            }
        }
        Some(hash)
    } else {
        None
    };

    let parse_mode = req.parse_mode.unwrap_or_default();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(5)); // 最大并发 5
    let results = Arc::new(tokio::sync::Mutex::new(serde_json::Map::new()));
    // 无权限/无法校验的目标先占位，保证 results 覆盖所有 target，total 与条目数一致
    for (target, status, error) in &pre_blocked {
        results.lock().await.insert(
            target.clone(),
            serde_json::json!({ "status": status, "error": error }),
        );
    }
    let mut handles = Vec::with_capacity(sendable.len());

    // 并发发送所有已授权目标
    for (target, platform, chat_id) in sendable {
        let text = req.text.clone();
        let parse_mode = parse_mode.clone();
        let semaphore = semaphore.clone();
        let results = results.clone();
        let state = state.clone();
        let actor_id = actor.subject_id.clone();
        let idempotency_key = idempotency_key.clone();

        let handle = tokio::spawn(async move {
            let _permit = semaphore.acquire().await;

            let chat_id_for_store = chat_id.clone();
            let text_for_store = text.clone();
            let delivery_id = uuid::Uuid::now_v7().to_string();
            let delivery = OutboundDelivery {
                id: delivery_id.clone(),
                actor_id,
                idempotency_key,
                platform: platform.clone(),
                chat_id: chat_id.clone(),
                request_json: serde_json::json!({
                    "target": target.clone(),
                    "text": text.clone(),
                    "parse_mode": parse_mode.clone(),
                }),
                created_at: chrono::Utc::now().timestamp_millis(),
            };
            if let Err(error) = state
                .message_store
                .prepare_outbound_delivery(&delivery)
                .await
            {
                tracing::error!(%error, target, "refusing batch platform call because delivery intent could not be persisted");
                results.lock().await.insert(
                    target.clone(),
                    serde_json::json!({
                        "status": "failed",
                        "error": "Message persistence unavailable",
                    }),
                );
                return;
            }
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
                    let stored = StoredMessage::from_outbound(
                        &platform,
                        &chat_id_for_store,
                        None,
                        &text_for_store,
                        &r,
                    );
                    if let Err(error) = state
                        .message_store
                        .finalize_outbound_delivery(
                            &delivery_id,
                            if r.success {
                                OutboundDeliveryState::Succeeded
                            } else {
                                OutboundDeliveryState::Failed
                            },
                            &serde_json::to_value(&r).unwrap_or_default(),
                            Some(&stored),
                        )
                        .await
                    {
                        tracing::error!(%error, target, "batch platform result is indeterminate because delivery journal could not be finalized");
                        results.lock().await.insert(
                            target.clone(),
                            serde_json::json!({
                                "status": "indeterminate",
                                "error": "Platform result could not be committed",
                            }),
                        );
                        return;
                    }
                    let mut item = serde_json::json!({
                        "status": if r.success { "sent" } else { "failed" },
                        "messageId": r.message_id,
                    });
                    if let Some(error) = &r.error {
                        item["error"] = serde_json::json!(sanitize_error_message(error));
                    }
                    if let Some(error_code) = &r.error_code {
                        item["errorCode"] = serde_json::json!(error_code);
                    }
                    results.lock().await.insert(target.clone(), item);
                }
                Ok(Err(e)) => {
                    let sanitized = sanitize_error_message(&e.to_string());
                    if let Err(error) = state
                        .message_store
                        .finalize_outbound_delivery(
                            &delivery_id,
                            OutboundDeliveryState::Failed,
                            &serde_json::json!({ "error": sanitized }),
                            None,
                        )
                        .await
                    {
                        tracing::error!(%error, target, "failed to finalize rejected batch delivery");
                    }
                    results.lock().await.insert(
                        target.clone(),
                        serde_json::json!({
                            "status": "failed",
                            "error": sanitized,
                        }),
                    );
                }
                Err(_) => {
                    results.lock().await.insert(
                        target.clone(),
                        serde_json::json!({
                            "status": "indeterminate",
                            "error": "Platform result is unknown after timeout (15s)",
                        }),
                    );
                }
            }
        });

        handles.push(handle);
    }

    // 等待所有发送完成（整体超时 30 秒）
    let all_futures = futures::future::join_all(handles);
    if tokio::time::timeout(std::time::Duration::from_secs(30), all_futures)
        .await
        .is_err()
    {
        return Err(api_error(GatewayError::RequestTimeout(
            "batch send outcome is indeterminate; reuse the same Idempotency-Key".into(),
        )));
    }

    // 安全解包 Arc：若 task 超时后仍持有 Arc 引用，回退到克隆数据
    let final_results = match Arc::try_unwrap(results) {
        Ok(mutex) => mutex.into_inner(),
        Err(arc) => {
            tracing::warn!("batch_send: Arc 仍有多个引用，使用 blocking_lock 回退");
            arc.blocking_lock().clone()
        }
    };

    let response_body = serde_json::json!({
        "total": req.targets.len(),
        "results": final_results,
    });
    if let (Some(key), Some(hash)) = (&idempotency_key, &request_hash) {
        let response_json = serde_json::to_string(&response_body)
            .map_err(|e| api_error(GatewayError::Internal(e.to_string())))?;
        state
            .auth_manager
            .complete_idempotency(&actor.subject_id, key, hash, 200, &response_json)
            .await
            .map_err(|e| api_error(GatewayError::StorageError(e)))?;
    }
    let mut response = Json(response_body).into_response();
    if idempotency_key.is_some() {
        response
            .headers_mut()
            .insert("idempotency-replayed", HeaderValue::from_static("false"));
    }
    Ok(response)
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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(message_id): Path<String>,
    Json(req): Json<EditMessageRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (platform, chat_id) = parse_target(&req.target).ok_or_else(|| {
        api_error(GatewayError::InvalidRequest(
            "Invalid target format. Expected 'platform:chatId'".to_string(),
        ))
    })?;
    require_target_action(
        &state,
        &actor,
        &platform,
        &chat_id,
        target_actions::MESSAGES_SEND,
    )
    .await?;

    let params = EditMessageParams {
        chat_id: chat_id.clone(),
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
            "platform": platform,
            "chat_id": chat_id,
            "message_id": message_id,
            "result": {
                "success": result.success,
                "updated_at": result.updated_at,
                "error": result.error.as_deref().map(sanitize_error_message),
            },
        }),
    ));

    Ok(Json(serde_json::json!({
        "ok": result.success,
        "updated_at": result.updated_at,
        "error": result.error.as_deref().map(sanitize_error_message),
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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(message_id): Path<String>,
    Json(req): Json<DeleteMessageRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (platform, chat_id) = parse_target(&req.target).ok_or_else(|| {
        api_error(GatewayError::InvalidRequest(
            "Invalid target format. Expected 'platform:chatId'".to_string(),
        ))
    })?;
    require_target_action(
        &state,
        &actor,
        &platform,
        &chat_id,
        target_actions::MESSAGES_SEND,
    )
    .await?;

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
            "platform": platform,
            "chat_id": chat_id,
            "message_id": message_id,
            "result": {
                "success": result.success,
                "error": result.error.as_deref().map(sanitize_error_message),
            },
        }),
    ));

    Ok(Json(serde_json::json!({
        "ok": result.success,
        "error": result.error.as_deref().map(sanitize_error_message),
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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Query(params): Query<MessageHistoryParams>,
) -> Result<Json<MessageHistoryResponse>, ApiError> {
    let limit = params.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(api_error(GatewayError::InvalidRequest(
            "limit must be between 1 and 200".into(),
        )));
    }

    let target = match (&params.platform, &params.chat_id, &params.session_key) {
        (Some(platform), Some(chat_id), _) => Some((platform.clone(), chat_id.clone())),
        (_, _, Some(key)) => key.split_once(':').map(|(platform, rest)| {
            (
                platform.to_string(),
                rest.split(':').next().unwrap_or_default().to_string(),
            )
        }),
        _ => None,
    };
    if let Some((platform, chat_id)) = target {
        require_target_action(
            &state,
            &actor,
            &platform,
            &chat_id,
            target_actions::MESSAGES_READ,
        )
        .await?;
    } else if !easybot_core::auth::permissions::has_global_target_access(&actor) {
        return Err(api_error(GatewayError::InvalidRequest(
            "message history requires platform+chat_id or session_key".into(),
        )));
    }

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
        .map_err(|error| api_error(GatewayError::StorageError(error.to_string())))?;

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

    Ok(Json(MessageHistoryResponse { messages, has_more }))
}

/// SECURITY: Sanitize adapter error messages to prevent information leakage.
/// Truncates long errors and strips common internal patterns.
fn sanitize_error_message(err: &str) -> String {
    let redacted = crate::log_collector::redact_sensitive_text(err);
    // Truncate at 256 chars to prevent verbose API error bodies from leaking
    let truncated: String = redacted.chars().take(256).collect();
    if redacted.chars().count() > 256 {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

/// 解析 "platform:chatId" 格式
fn parse_target(target: &str) -> Option<(String, String)> {
    if target.len() > MAX_TARGET_LENGTH || target.chars().any(char::is_control) {
        return None;
    }
    let colon = target.find(':')?;
    let platform = &target[..colon];
    let chat_id = &target[colon + 1..];
    if platform.is_empty()
        || platform.len() > MAX_PLATFORM_LENGTH
        || !platform
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        || chat_id.is_empty()
        || chat_id.len() > MAX_CHAT_ID_LENGTH
    {
        return None;
    }
    Some((platform.to_string(), chat_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::sanitize_error_message;

    #[test]
    fn public_adapter_errors_are_redacted_and_bounded() {
        let error = format!(
            "request failed token=secret-value Bearer raw-token {}",
            "x".repeat(400)
        );
        let sanitized = sanitize_error_message(&error);
        assert!(!sanitized.contains("secret-value"));
        assert!(!sanitized.contains("raw-token"));
        assert!(sanitized.chars().count() <= 259);
    }

    #[test]
    fn target_parser_enforces_resource_bounds() {
        assert!(super::parse_target("telegram:123").is_some());
        assert!(super::parse_target(&format!("telegram:{}", "x".repeat(257))).is_none());
        assert!(super::parse_target("bad platform:123").is_none());
        assert!(super::parse_target("telegram:line\nbreak").is_none());
    }
}
