//! Per-caller request quotas for commercial plans.

use crate::AppState;
use crate::response::ApiError;
use axum::{
    extract::State,
    http::{HeaderValue, Request, header::HeaderName},
    middleware::Next,
    response::{IntoResponse, Response},
};
use easybot_core::types::error::GatewayError;

const LIMIT_HEADER: HeaderName = HeaderName::from_static("x-ratelimit-limit");
const REMAINING_HEADER: HeaderName = HeaderName::from_static("x-ratelimit-remaining");
const RESET_HEADER: HeaderName = HeaderName::from_static("x-ratelimit-reset");

pub async fn key_quota_middleware(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let Some(auth) = req.extensions().get::<easybot_core::auth::AuthInfo>() else {
        return next.run(req).await;
    };
    let Some(limit) = auth.requests_per_minute else {
        return next.run(req).await;
    };

    let decision = match state
        .auth_manager
        .consume_subject_quota(&auth.subject_id, limit)
        .await
    {
        Ok(decision) => {
            state.auth_manager.set_quota_ready(true);
            decision
        }
        Err(error) => {
            state.auth_manager.set_quota_ready(false);
            tracing::error!(subject_id = %auth.subject_id, %error, "durable subject quota unavailable");
            return ApiError(GatewayError::StorageError(
                "durable request quota is unavailable".into(),
            ))
            .into_response();
        }
    };
    if !decision.allowed {
        let mut response = ApiError(GatewayError::RateLimited {
            retry_after_ms: decision.retry_after_secs * 1_000,
        })
        .into_response();
        response.headers_mut().insert(
            axum::http::header::RETRY_AFTER,
            HeaderValue::from_str(&decision.retry_after_secs.to_string())
                .unwrap_or_else(|_| HeaderValue::from_static("60")),
        );
        add_quota_headers(&mut response, limit, 0, decision.retry_after_secs);
        return response;
    }

    let mut response = next.run(req).await;
    add_quota_headers(
        &mut response,
        limit,
        decision.remaining,
        decision.retry_after_secs,
    );
    response
}

fn add_quota_headers(response: &mut Response, limit: u32, remaining: u32, reset: u64) {
    for (name, value) in [
        (LIMIT_HEADER, limit.to_string()),
        (REMAINING_HEADER, remaining.to_string()),
        (RESET_HEADER, reset.to_string()),
    ] {
        if let Ok(value) = HeaderValue::from_str(&value) {
            response.headers_mut().insert(name, value);
        }
    }
}
