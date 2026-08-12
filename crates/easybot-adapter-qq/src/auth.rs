//! QQ Access Token 管理
//!
//! QQ 统一机器人平台使用 AppId + ClientSecret 换取 Access Token，
//! 有效期 7200 秒。缓存 + 单飞行刷新由 core 的 `CachedTokenStore` 提供，
//! 本模块只负责平台特有部分：调用鉴权端点、生成鉴权头、判定
//! "token 无效/过期"错误（`is_qq_token_invalid_response`）。

use std::time::Duration;

use easybot_core::http::{CachedTokenStore, TokenFetcher};
use easybot_core::types::error::GatewayError;

/// QQ 统一机器人平台 Access Token 存储（`CachedTokenStore` 薄包装）
///
/// Token 有效期 7200 秒，margin 60 秒（对齐官方"最后 60s 窗口重取发新 token"）。
/// `get()` 缺失/临界时自动单飞行刷新；`force_refresh()` 供 token 被拒后强制刷新。
#[derive(Clone)]
pub(crate) struct QqTokenStore {
    pub(crate) inner: CachedTokenStore,
}

impl QqTokenStore {
    pub(crate) fn new(app_id: String, client_secret: String, auth_base_url: String) -> Self {
        // fetch 闭包在构造时捕获凭据（与旧实现每次读取 self 字段等价，但改为捕获 owned 值）
        let fetch: TokenFetcher = Box::new(move || {
            // 每次调用 clone 凭据供 async move 使用（刷新低频，clone 开销可忽略）
            let app_id = app_id.clone();
            let client_secret = client_secret.clone();
            let auth_base_url = auth_base_url.clone();
            Box::pin(
                async move { qq_fetch_access_token(&app_id, &client_secret, &auth_base_url).await },
            )
        });
        Self {
            inner: CachedTokenStore::new(fetch, Duration::from_secs(60)),
        }
    }

    /// 无条件刷新（connect 引导 / token 被拒后 / 定时刷新）
    pub(crate) async fn refresh(&self) -> Result<(), GatewayError> {
        self.inner.force_refresh().await.map(|_| ())
    }

    /// 返回 `QQBot {token}` 鉴权字符串；缺失/临界时自动刷新（单飞行）
    pub(crate) async fn get(&self) -> Result<String, GatewayError> {
        self.inner.get().await.map(|t| qq_auth_header(&t))
    }

    /// 同步检查是否需要刷新（空 / 已过期 / 距过期不足 margin）
    pub(crate) fn needs_refresh(&self) -> bool {
        self.inner.needs_refresh()
    }
}

/// 从 QQ 鉴权端点获取 access token，返回 `(token, ttl_secs)`。
///
/// ★ 错误分类（2026 协议调研）：按响应 `code` 结构化判定——
/// - `100001`（限流）→ 瞬态 `Transient`，不得永久停用
/// - `100016`/`100007`/`10004`（凭据/机器人状态问题）→ 永久 `AuthFailed`
/// - 成功响应**无 `code` 字段**；缺 `access_token` → 凭据问题 `AuthFailed`
/// - 网络/解析层失败 → 瞬态 `Transient`（重试安全）
async fn qq_fetch_access_token(
    app_id: &str,
    client_secret: &str,
    auth_base_url: &str,
) -> Result<(String, u64), GatewayError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| GatewayError::Internal(format!("Failed to create HTTP client: {}", e)))?;

    let body = serde_json::json!({
        "appId": app_id,
        "clientSecret": client_secret,
    });

    let resp = client
        .post(format!("{}/app/getAppAccessToken", auth_base_url))
        .json(&body)
        .send()
        .await
        // 网络层失败（DNS/超时/拒连）是瞬态错误——健康监控应退避重试，不得永久停用。
        .map_err(|e| {
            GatewayError::Transient(format!("QQ getAppAccessToken request failed: {}", e))
        })?;

    let data: serde_json::Value = resp.json().await.map_err(|e| {
        // 解析失败通常是代理/网络产物，同样按瞬态处理（重试安全）。
        GatewayError::Transient(format!("QQ getAppAccessToken parse failed: {}", e))
    })?;

    // 错误响应带 code 字段（成功响应无 code）
    if let Some(code) = data["code"].as_i64() {
        let msg = data["message"].as_str().unwrap_or("");
        return Err(match code {
            // 100001 限流 → 瞬态，健康监控退避重试
            100001 => GatewayError::Transient(format!(
                "QQ getAppAccessToken rate limited (code {code}): {msg}"
            )),
            // 凭据/机器人状态问题 → 永久
            100016 | 100007 | 10004 => GatewayError::AuthFailed(format!(
                "QQ getAppAccessToken rejected (code {code}): {msg}"
            )),
            // 其他 code：未知错误，保守按永久处理（多为 appid/secret 相关）
            other => GatewayError::AuthFailed(format!(
                "QQ getAppAccessToken failed (code {other}): {msg}"
            )),
        });
    }

    let access_token = data["access_token"].as_str().ok_or_else(|| {
        // 鉴权端点返回了响应但缺少 access_token —— 凭据问题，永久。
        GatewayError::AuthFailed("QQ getAppAccessToken: missing access_token".to_string())
    })?;
    let expires_in = data["expires_in"].as_u64().unwrap_or(7200);

    Ok((access_token.to_string(), expires_in))
}

/// 生成 `QQBot {token}` 鉴权字符串
pub(crate) fn qq_auth_header(token: &str) -> String {
    format!("QQBot {token}")
}

/// ★ 唯一 token 失效检测源。
///
/// QQ 对 token 失效的返回形态（2026 协议调研）：
/// - HTTP 401 Unauthorized
/// - body 业务码 11244（`"token not exist or expire"`，HTTP 500 或偶发 200）
/// - 业务码 11242（官方：校验 token 系统错误，"重试一次会好，最多重试一次"）
pub(crate) fn is_qq_token_invalid_response(status: u16, body: &str) -> bool {
    status == 401
        || body.contains("11244")
        || body.contains("token not exist or expire")
        || body.contains("11242")
}

/// 生成与 `is_token_invalid_error` 解析锁步的错误消息。
/// 格式：`QQ API error (METHOD path): STATUS - body`
pub(crate) fn qq_api_error_msg(method: &str, path: &str, status: u16, body: &str) -> String {
    format!("QQ API error ({method} {path}): {status} - {body}")
}

/// 适配 `send_with_token_retry` 的谓词：解析 `qq_api_error_msg` 产物 → 委托
/// `is_qq_token_invalid_response`。生产者/解析器同在本文件，锁步维护。
pub(crate) fn is_token_invalid_error(e: &GatewayError) -> bool {
    let GatewayError::Internal(msg) = e else {
        return false;
    };
    let Some(rest) = msg.strip_prefix("QQ API error (") else {
        return false;
    };
    let Some(after) = rest.split("): ").nth(1) else {
        return false;
    };
    let mut parts = after.splitn(2, " - ");
    let Some(status) = parts.next().and_then(|s| s.parse::<u16>().ok()) else {
        return false;
    };
    let body = parts.next().unwrap_or("");
    is_qq_token_invalid_response(status, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_header_format() {
        assert_eq!(qq_auth_header("abc123"), "QQBot abc123");
    }

    #[test]
    fn invalid_response_detects_http_401() {
        assert!(is_qq_token_invalid_response(401, ""));
        assert!(is_qq_token_invalid_response(401, "{}"));
    }

    #[test]
    fn invalid_response_detects_code_11244() {
        assert!(is_qq_token_invalid_response(
            500,
            r#"{"code":11244,"err_code":11244,"message":"token not exist or expire"}"#
        ));
        assert!(is_qq_token_invalid_response(
            200,
            r#"{"code":11244,"message":"token not exist or expire"}"#
        ));
    }

    #[test]
    fn invalid_response_detects_code_11242() {
        assert!(is_qq_token_invalid_response(
            500,
            r#"{"code":11242,"message":"校验token系统错误"}"#
        ));
    }

    #[test]
    fn invalid_response_ignores_unrelated() {
        assert!(!is_qq_token_invalid_response(
            500,
            r#"{"code":11263,"message":"resource not found"}"#
        ));
        assert!(!is_qq_token_invalid_response(
            200,
            r#"{"code":0,"data":{}}"#
        ));
    }

    #[test]
    fn error_predicate_parses_formatted_message() {
        // 生产/解析锁步：错误消息 → 谓词
        let err = GatewayError::Internal(qq_api_error_msg(
            "POST",
            "/v2/groups/123/messages",
            500,
            r#"{"code":11244,"message":"token not exist or expire"}"#,
        ));
        assert!(is_token_invalid_error(&err));

        let err = GatewayError::Internal(qq_api_error_msg("GET", "/gateway/bot", 401, ""));
        assert!(is_token_invalid_error(&err));

        let err = GatewayError::Internal(qq_api_error_msg(
            "GET",
            "/users/@me",
            404,
            r#"{"code":11263}"#,
        ));
        assert!(!is_token_invalid_error(&err));
    }

    #[test]
    fn error_predicate_rejects_non_internal() {
        assert!(!is_token_invalid_error(&GatewayError::Transient(
            "network".into()
        )));
        assert!(!is_token_invalid_error(&GatewayError::AuthFailed(
            "bad creds".into()
        )));
    }
}
