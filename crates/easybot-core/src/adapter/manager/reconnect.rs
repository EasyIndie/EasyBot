//! 重连分类与退避：错误结构化分类、启发式兜底、指数退避。

use super::*;

/// Per-platform reconnect state tracked by the health monitor.
///
/// Maximum total reconnection attempts before switching to slow retry mode.
/// After this many failures, the adapter transitions to 30-minute retry intervals.
pub(super) const MAX_TOTAL_RECONNECT_ATTEMPTS: u32 = 20;

/// Maximum number of transport-only retries before escalating to full restart.
/// Transport retries skip re-authentication — only the background task is restarted.
pub(super) const MAX_TRANSPORT_RETRIES: u32 = 5;

/// Backoff interval between transport-only retry attempts.
pub(super) const TRANSPORT_RETRY_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default)]
pub(super) struct ReconnectState {
    pub(super) consecutive_failures: u32,
    pub(super) total_failures: u32,
    pub(super) backoff_until: Option<Instant>,
    /// Number of transport-only retry attempts (resets on full reconnect success).
    pub(super) transport_retries: u32,
    /// Whether the last health check was Healthy (used for hysteresis).
    pub(super) was_healthy: bool,
    /// Set when a permanent failure (invalid credentials) is detected.
    /// Once set, the health monitor stops retrying for this platform.
    pub(super) permanent_failure: bool,
}

/// Classifies a reconnect failure to decide the retry strategy.
#[derive(Debug)]
pub(super) enum ReconnectFailure {
    /// Transient: network unavailable, DNS failure, timeout — should retry.
    Transient(String),
    /// Permanent: invalid credentials, token revoked, API returns 401/403 — no retry.
    Permanent(String),
}

/// Classify a [`GatewayError`] into [`ReconnectFailure`] to decide the retry strategy.
///
/// Structured-first: the `GatewayError` variant itself carries the classification
/// (auth rejections are permanent; transient/network/timeout/rate-limit are retryable).
/// Only errors without a structured signal fall back to message heuristics.
pub(super) fn classify_error(error: &GatewayError) -> ReconnectFailure {
    match error {
        // Structured permanent signals: credentials are bad, retrying won't help.
        GatewayError::AuthFailed(msg)
        | GatewayError::Unauthorized(msg)
        | GatewayError::Forbidden(msg) => {
            return ReconnectFailure::Permanent(msg.clone());
        }
        // Structured transient signals: network/temporary — safe to retry.
        GatewayError::Transient(msg) | GatewayError::RequestTimeout(msg) => {
            return ReconnectFailure::Transient(msg.clone());
        }
        GatewayError::RateLimited { .. } => {
            return ReconnectFailure::Transient(error.to_string());
        }
        _ => {}
    }
    classify_error_heuristic(error)
}

/// Fallback classification from the error message text.
///
/// Network/transient signals are checked FIRST so a transient failure wins over
/// permanent-looking keywords — e.g. the QQ connect error
/// `"QQ auth failed (getAppAccessToken): ... connection timed out"` contains both
/// `"auth failed"` and `"timeout"`; the underlying cause is a network outage, so it
/// must be classified Transient.
pub(super) fn classify_error_heuristic(error: &GatewayError) -> ReconnectFailure {
    let msg = error.to_string().to_lowercase();

    // Network-related errors → transient (will recover when network is back).
    // Covers reqwest/hyper error vocabulary and rate-limit indicators.
    if msg.contains("dns")
        || msg.contains("resolve")
        || msg.contains("timeout")
        || msg.contains("timed out")
        || msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("reset by peer")
        || msg.contains("error sending request")
        || msg.contains("io error")
        || msg.contains("send failed")
        || msg.contains("request failed")
        || msg.contains("read failed")
        || msg.contains("write failed")
        || msg.contains("tcp")
        || msg.contains("connect")
        || msg.contains("unreachable")
        || msg.contains("route")
        || msg.contains("peer")
        || msg.contains("closed")
        || msg.contains("network")
        || msg.contains("refused")
        || msg.contains("rate limit")
        || msg.contains("rate limited")
        || msg.contains("429")
        || msg.contains("too many requests")
    {
        return ReconnectFailure::Transient(error.to_string());
    }

    // Auth-related errors → permanent (credentials are bad, retrying won't help).
    if msg.contains("unauthorized")
        || msg.contains(" 401 ")
        || msg.contains(" 403 ")
        || msg.contains("forbidden")
        || msg.contains("auth failed")
        || msg.contains("invalid token")
        || msg.contains("access token expired")
        || msg.contains("access token invalid")
        || msg.contains("invalid access token")
    {
        return ReconnectFailure::Permanent(error.to_string());
    }

    // Default: treat unknown errors as transient to avoid permanently
    // disabling an adapter from a one-off unexpected error.
    ReconnectFailure::Transient(error.to_string())
}

/// Whether a [`GatewayError`] carries a structured transient signal
/// (network/temporary/timeout/rate-limit). Used to preserve the classification
/// across the connect() failure path without relying on message wording.
pub(crate) fn is_transient_error(error: &GatewayError) -> bool {
    matches!(
        error,
        GatewayError::Transient(_)
            | GatewayError::RequestTimeout(_)
            | GatewayError::RateLimited { .. }
    )
}

/// Exponential backoff: 5s → 10s → 30s → 60s → 120s, capped at 300s.
pub(super) fn compute_backoff(consecutive_failures: u32) -> Duration {
    let secs = match consecutive_failures {
        0 => 5,
        1 => 10,
        2 => 30,
        3 => 60,
        4 => 120,
        _ => 300, // capped at 5 minutes
    };
    Duration::from_secs(secs)
}
