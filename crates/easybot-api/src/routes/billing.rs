//! Provider-neutral financial event ledger.

use crate::{AppState, response::ApiError};
use axum::{
    Json,
    extract::{Extension, Query, State},
};
use easybot_core::{
    auth::{BillingEvent, BillingEventFilter, BillingEventWrite},
    types::error::GatewayError,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct CreateBillingEventRequest {
    pub provider: String,
    pub event_id: String,
    pub event_type: String,
    pub object_id: String,
    pub customer_ref: String,
    pub amount_minor: i64,
    pub currency: String,
    pub occurred_at: i64,
}

#[derive(Serialize, ToSchema)]
pub struct BillingEventWriteResponse {
    pub disposition: String,
}

#[derive(Deserialize, ToSchema)]
pub struct BillingEventQuery {
    pub from: i64,
    pub to: i64,
    pub customer_ref: Option<String>,
    pub provider: Option<String>,
    pub event_type: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub struct BillingEventsResponse {
    pub events: Vec<BillingEvent>,
    pub integrity_valid: bool,
    pub total_events: i64,
    pub truncated: bool,
    pub next_offset: Option<usize>,
}

const EVENT_TYPES: &[&str] = &[
    "invoice_paid",
    "invoice_voided",
    "payment_succeeded",
    "payment_failed",
    "refund_succeeded",
    "chargeback_opened",
    "chargeback_closed",
];

fn valid_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

#[utoipa::path(
    post, path = "/api/v1/billing/events", tag = "Billing",
    request_body = CreateBillingEventRequest,
    responses((status = 200, body = BillingEventWriteResponse), (status = 409))
)]
pub async fn create_billing_event(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Json(body): Json<CreateBillingEventRequest>,
) -> Result<Json<BillingEventWriteResponse>, ApiError> {
    for (name, value) in [
        ("provider", &body.provider),
        ("event_id", &body.event_id),
        ("object_id", &body.object_id),
        ("customer_ref", &body.customer_ref),
    ] {
        if !valid_identifier(value, 128) {
            return Err(ApiError(GatewayError::InvalidRequest(format!(
                "invalid {name}"
            ))));
        }
    }
    if !EVENT_TYPES.contains(&body.event_type.as_str()) {
        return Err(ApiError(GatewayError::InvalidRequest(
            "unsupported event_type".into(),
        )));
    }
    if body.amount_minor < 0 || body.amount_minor > 100_000_000_000_000 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "amount_minor out of range".into(),
        )));
    }
    if body.currency.len() != 3 || !body.currency.chars().all(|c| c.is_ascii_uppercase()) {
        return Err(ApiError(GatewayError::InvalidRequest(
            "currency must be three uppercase ASCII letters".into(),
        )));
    }
    let now = chrono::Utc::now().timestamp_millis();
    if body.occurred_at < now - 7 * 366 * 86_400_000 || body.occurred_at > now + 5 * 60_000 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "occurred_at outside accepted window".into(),
        )));
    }
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "billing.event.write.requested",
            &format!("billing_event:{}:{}", body.provider, body.event_id),
            serde_json::json!({"event_type":body.event_type,"customer_ref":body.customer_ref}),
        )
        .await
        .map_err(|e| ApiError(GatewayError::Internal(e)))?;
    let event = BillingEvent {
        provider: body.provider.clone(),
        event_id: body.event_id.clone(),
        event_type: body.event_type,
        object_id: body.object_id,
        customer_ref: body.customer_ref,
        amount_minor: body.amount_minor,
        currency: body.currency,
        occurred_at: body.occurred_at,
        received_at: 0,
        event_hash: String::new(),
    };
    let result = state
        .auth_manager
        .record_billing_event(event)
        .await
        .map_err(|e| ApiError(GatewayError::StorageError(e)))?;
    let disposition = match result {
        BillingEventWrite::Created => "created",
        BillingEventWrite::Duplicate => "duplicate",
        BillingEventWrite::Conflict => "conflict",
    };
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "billing.event.write.completed",
            &format!("billing_event:{}:{}", body.provider, body.event_id),
            serde_json::json!({"disposition":disposition}),
        )
        .await
        .map_err(|e| ApiError(GatewayError::Internal(e)))?;
    if result == BillingEventWrite::Conflict {
        return Err(ApiError(GatewayError::Conflict(
            "provider/event_id already exists with different immutable content".into(),
        )));
    }
    Ok(Json(BillingEventWriteResponse {
        disposition: disposition.into(),
    }))
}

#[utoipa::path(
    get, path = "/api/v1/billing/events", tag = "Billing",
    params(
        ("from"=i64, Query), ("to"=i64, Query),
        ("customer_ref"=Option<String>, Query),
        ("provider"=Option<String>, Query),
        ("event_type"=Option<String>, Query),
        ("limit"=Option<usize>, Query), ("offset"=Option<usize>, Query)
    ),
    responses((status = 200, body = BillingEventsResponse))
)]
pub async fn list_billing_events(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Query(query): Query<BillingEventQuery>,
) -> Result<Json<BillingEventsResponse>, ApiError> {
    if query.from < 0 || query.to <= query.from || query.to - query.from > 366 * 86_400_000 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "billing range must be <= 366 days".into(),
        )));
    }
    if query
        .customer_ref
        .as_ref()
        .is_some_and(|v| !valid_identifier(v, 128))
    {
        return Err(ApiError(GatewayError::InvalidRequest(
            "invalid customer_ref".into(),
        )));
    }
    if query
        .provider
        .as_ref()
        .is_some_and(|value| !valid_identifier(value, 128))
    {
        return Err(ApiError(GatewayError::InvalidRequest(
            "invalid provider".into(),
        )));
    }
    if query
        .event_type
        .as_ref()
        .is_some_and(|value| !EVENT_TYPES.contains(&value.as_str()))
    {
        return Err(ApiError(GatewayError::InvalidRequest(
            "unsupported event_type".into(),
        )));
    }
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    if !(1..=1_000).contains(&limit) || offset > 10_000_000 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "billing pagination requires limit 1-1000 and offset <= 10000000".into(),
        )));
    }
    let ledger_valid = state
        .auth_manager
        .verify_billing_ledger_integrity()
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?;
    if !ledger_valid {
        state
            .auth_manager
            .record_audit(
                &actor.id,
                "billing.events.integrity_failed",
                "billing-events",
                serde_json::json!({"scope":"full_ledger"}),
            )
            .await
            .map_err(|error| ApiError(GatewayError::Internal(error)))?;
        return Err(ApiError(GatewayError::StorageError(
            "financial event ledger integrity validation failed".into(),
        )));
    }
    let events = state
        .auth_manager
        .billing_events(
            query.from,
            query.to,
            BillingEventFilter {
                customer_ref: query.customer_ref.as_deref(),
                provider: query.provider.as_deref(),
                event_type: query.event_type.as_deref(),
            },
            limit + 1,
            offset,
        )
        .await
        .map_err(|e| ApiError(GatewayError::StorageError(e)))?;
    let truncated = events.len() > limit;
    let events: Vec<_> = events.into_iter().take(limit).collect();
    if !easybot_core::auth::ApiKeyManager::verify_billing_event_integrity(&events) {
        state
            .auth_manager
            .record_audit(
                &actor.id,
                "billing.events.integrity_failed",
                "billing-events",
                serde_json::json!({
                    "from":query.from,"to":query.to,
                    "customer_ref":query.customer_ref,
                    "provider":query.provider,
                    "event_type":query.event_type,
                    "offset":offset,"limit":limit
                }),
            )
            .await
            .map_err(|error| ApiError(GatewayError::Internal(error)))?;
        return Err(ApiError(GatewayError::StorageError(
            "financial event integrity validation failed".into(),
        )));
    }
    let total_events = state
        .auth_manager
        .billing_event_count(
            query.from,
            query.to,
            BillingEventFilter {
                customer_ref: query.customer_ref.as_deref(),
                provider: query.provider.as_deref(),
                event_type: query.event_type.as_deref(),
            },
        )
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?;
    let next_offset = truncated.then_some(offset.saturating_add(limit));
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "billing.events.read",
            "billing-events",
            serde_json::json!({
                "from":query.from,"to":query.to,
                "customer_ref":query.customer_ref,
                "provider":query.provider,
                "event_type":query.event_type,
                "records":events.len(),
                "total_events":total_events,
                "offset":offset,"limit":limit,"truncated":truncated
            }),
        )
        .await
        .map_err(|e| ApiError(GatewayError::Internal(e)))?;
    Ok(Json(BillingEventsResponse {
        events,
        integrity_valid: true,
        total_events,
        truncated,
        next_offset,
    }))
}
