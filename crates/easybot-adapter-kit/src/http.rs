//! HTTP 客户端工厂：统一超时、User-Agent 与错误归类。
//!
//! 现状是 5 个适配器各自 `reqwest::Client::builder().timeout(...)`，参数与
//! 默认值互不一致，还有漏网的 `reqwest::Client::new()`（**无超时**，请求可
//! 无限挂起 —— `adapter-discord/src/lib.rs` 的 guild owner 查询就是一例）。
//!
//! 统一到 [`client`] 后：
//!
//! - 所有请求都有超时（连接 + 总时长）；
//! - 带 `easybot/<version>` User-Agent，便于平台侧排障；
//! - 调用方用 [`transport_error`] 把网络层失败包成
//!   [`GatewayError::Transient`]，避免健康监测器把网络抖动误判为永久故障。

use std::time::Duration;

use easybot_core::GatewayError;
use reqwest::StatusCode;

/// 默认请求总超时。与改造前各适配器最常见的取值（15s）一致。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

/// 默认连接超时。避免 DNS/握手阶段长时间阻塞。
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 构造统一的 HTTP 客户端。
///
/// `timeout` 为单请求总超时（含读响应体）；连接阶段另有
/// [`DEFAULT_CONNECT_TIMEOUT`] 兜底。构建失败说明 TLS 后端异常，
/// 属于不可恢复的启动期错误，直接 panic 与既有适配器行为一致。
pub fn client(timeout: Duration) -> reqwest::Client {
    match builder(timeout).build() {
        Ok(client) => client,
        Err(e) => {
            tracing::error!(error = %e, "构建 HTTP 客户端失败（TLS 后端异常）");
            panic!("构建 reqwest Client 失败: {e}");
        }
    }
}

/// 使用 [`DEFAULT_TIMEOUT`] 的构造快捷方式。
pub fn default_client() -> reqwest::Client {
    client(DEFAULT_TIMEOUT)
}

/// 可继续定制的 builder（需要额外 header / 重定向策略时使用）。
pub fn builder(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(DEFAULT_CONNECT_TIMEOUT)
        .timeout(timeout)
        .user_agent(user_agent())
}

/// `easybot/<crate version>`。
pub fn user_agent() -> String {
    format!("easybot/{}", env!("CARGO_PKG_VERSION"))
}

/// 把 `reqwest` 的网络层错误包成 [`GatewayError::Transient`]。
///
/// `CLAUDE.md` 的约定：**reqwest `send()` 的网络层失败必须用 `Transient` 包裹**，
/// 否则健康监测器的启发式会把 `"X auth failed: <网络错误>"` 误判为永久凭据错误。
/// 本函数就是那条约定的唯一实现点。
pub fn transport_error(context: &str, err: reqwest::Error) -> GatewayError {
    if err.is_timeout() {
        GatewayError::Transient(format!("{context}: 请求超时 ({err})"))
    } else if err.is_connect() {
        GatewayError::Transient(format!("{context}: 连接失败 ({err})"))
    } else if err.is_request() || err.is_body() || err.is_decode() {
        // 请求构造/解码失败属于本地或协议问题，同样按瞬态处理：
        // 让健康监测器重试而不是直接停用适配器。
        GatewayError::Transient(format!("{context}: 请求失败 ({err})"))
    } else {
        GatewayError::Transient(format!("{context}: {err}"))
    }
}

/// 把 HTTP 状态码归类为网关错误。
///
/// - 401/403 → [`GatewayError::AuthFailed`]（永久，健康监测器不再重试）
/// - 429 → [`GatewayError::RateLimited`]（瞬态，退避重试）
/// - 5xx → [`GatewayError::Transient`]
/// - 其它 4xx → [`GatewayError::InvalidRequest`]（本地参数问题，重试无意义）
pub fn status_error(context: &str, status: StatusCode, body: &str) -> GatewayError {
    status_error_with_retry(context, status, body, None)
}

/// 同 [`status_error`]，但允许携带平台返回的 `retry_after`（毫秒）。
///
/// 平台通常在 429 响应里给出 `Retry-After` 头或业务字段，直接透传即可让
/// 上层退避等待正确时长；未提供时回退 1s。
pub fn status_error_with_retry(
    context: &str,
    status: StatusCode,
    body: &str,
    retry_after_ms: Option<u64>,
) -> GatewayError {
    let detail = format!("{context}: HTTP {} — {}", status.as_u16(), truncate(body));
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => GatewayError::AuthFailed(detail),
        StatusCode::TOO_MANY_REQUESTS => GatewayError::RateLimited {
            retry_after_ms: retry_after_ms.unwrap_or(1_000),
        },
        s if s.is_server_error() => GatewayError::Transient(detail),
        _ => GatewayError::InvalidRequest(detail),
    }
}

fn truncate(body: &str) -> String {
    const MAX: usize = 300;
    if body.len() <= MAX {
        body.to_string()
    } else {
        let mut end = MAX;
        while !body.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}…", &body[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use easybot_core::GatewayError;

    #[test]
    fn user_agent_carries_version() {
        let ua = user_agent();
        assert!(ua.starts_with("easybot/"), "unexpected UA: {ua}");
        assert!(ua.len() > "easybot/".len());
    }

    #[test]
    fn builder_applies_timeout() {
        // 能构建即成功；超时值无法直接读取，这里保证不 panic。
        let _ = client(Duration::from_secs(3));
        let _ = default_client();
    }

    #[test]
    fn unauthorized_is_permanent() {
        let err = status_error("getMe", StatusCode::UNAUTHORIZED, "unauthorized");
        assert!(matches!(err, GatewayError::AuthFailed(_)));
        assert_eq!(err.error_code(), "AUTH_FAILED");
    }

    #[test]
    fn forbidden_is_permanent() {
        let err = status_error("send", StatusCode::FORBIDDEN, "forbidden");
        assert!(matches!(err, GatewayError::AuthFailed(_)));
    }

    #[test]
    fn rate_limited_carries_retry_after() {
        let err =
            status_error_with_retry("send", StatusCode::TOO_MANY_REQUESTS, "slow", Some(2500));
        match err {
            GatewayError::RateLimited { retry_after_ms } => assert_eq!(retry_after_ms, 2500),
            other => panic!("expected RateLimited, got {other:?}"),
        }
        // 未提供时回退 1s
        let err = status_error("send", StatusCode::TOO_MANY_REQUESTS, "slow");
        match err {
            GatewayError::RateLimited { retry_after_ms } => assert_eq!(retry_after_ms, 1_000),
            other => panic!("expected RateLimited, got {other:?}"),
        }
    }

    #[test]
    fn server_error_is_transient() {
        let err = status_error("send", StatusCode::BAD_GATEWAY, "bad gateway");
        assert!(matches!(err, GatewayError::Transient(_)));
    }

    #[test]
    fn bad_request_is_invalid() {
        let err = status_error("send", StatusCode::BAD_REQUEST, "bad");
        assert!(matches!(err, GatewayError::InvalidRequest(_)));
    }

    #[test]
    fn body_is_truncated_on_char_boundary() {
        let body = "错".repeat(500);
        let err = status_error("send", StatusCode::BAD_REQUEST, &body);
        let msg = err.to_string();
        assert!(msg.len() < 400, "message too long: {}", msg.len());
    }
}
