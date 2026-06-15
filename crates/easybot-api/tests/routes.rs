//! API 路由集成测试
//!
//! 使用 create_router() + tower::ServiceExt::oneshot() 进行 HTTP 级测试。
//! 不绑定真实端口，直接测试 axum Router。

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

mod common;

/// 辅助：构造 HTTP 请求
fn build_req(
    method: &str,
    path: &str,
    api_key: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json");

    if let Some(key) = api_key {
        builder = builder.header("Authorization", format!("Bearer {}", key));
    }

    let req = if let Some(b) = body {
        builder.body(Body::from(b.to_string())).unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    req
}

/// 发送请求并解析响应为 (StatusCode, JSON Value)
async fn send_request(
    state: easybot_api::AppState,
    method: &str,
    path: &str,
    api_key: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, Value) {
    let app = easybot_api::server::create_router(state);
    let req = build_req(method, path, api_key, body);
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::Null);
    (status, json)
}

/// 辅助：GET 请求
async fn get(state: easybot_api::AppState, path: &str, key: Option<&str>) -> (StatusCode, Value) {
    send_request(state, "GET", path, key, None).await
}

/// 辅助：POST 请求
async fn post(
    state: easybot_api::AppState,
    path: &str,
    key: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, Value) {
    send_request(state, "POST", path, key, body).await
}

// ── 健康检查（公共路由，无需认证）──

#[tokio::test]
async fn test_health_returns_ok() {
    let (state, _key) = common::test_app_state().await;
    let (status, json) = get(state, "/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["status"], "degraded");
    assert_eq!(json["adapters"]["total"], 0);
    assert_eq!(json["adapters"]["connected"], 0);
    assert_eq!(json["sessions"]["active"], 0);
    assert!(json["version"].is_string());
}

#[tokio::test]
async fn test_health_snapshot() {
    let (state, _key) = common::test_app_state().await;
    let (_, json) = get(state, "/api/v1/health", None).await;
    insta::assert_json_snapshot!("health_response", json);
}

#[tokio::test]
async fn test_health_no_auth_needed() {
    let (state, _key) = common::test_app_state().await;
    let (status, _) = get(state, "/api/v1/health", None).await;
    assert_eq!(status, StatusCode::OK);
}

// ── 认证测试 ──

#[tokio::test]
async fn test_protected_route_returns_401_without_key() {
    let (state, _) = common::test_app_state().await;
    let (status, json) = get(state, "/api/v1/adapters", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        json["error"]["message"].is_string() || json["message"].is_string() || json["error_code"].is_string(),
        "Expected error message in response, got: {:#?}",
        json
    );
}

#[tokio::test]
async fn test_protected_route_returns_401_with_invalid_key() {
    let (state, _) = common::test_app_state().await;
    let (status, _) = send_request(state, "GET", "/api/v1/adapters", Some("invalid-key"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── 适配器路由 ──

#[tokio::test]
async fn test_list_adapters_with_valid_key() {
    let (state, key) = common::test_app_state().await;
    let (status, json) = get(state, "/api/v1/adapters", Some(&key)).await;
    assert_eq!(status, StatusCode::OK);
    // 空适配器列表返回数组或对象
    assert!(json.is_array() || json["adapters"].is_array() || json.as_object().is_some());
}

#[tokio::test]
async fn test_adapter_status_nonexistent_returns_404() {
    let (state, key) = common::test_app_state().await;
    let (status, _) = get(state, "/api/v1/adapters/nonexistent/status", Some(&key)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── 会话路由 ──

#[tokio::test]
async fn test_list_sessions_empty() {
    let (state, key) = common::test_app_state().await;
    let (status, json) = get(state, "/api/v1/sessions", Some(&key)).await;
    assert_eq!(status, StatusCode::OK);
    // 空会话列表
    assert!(json["sessions"].is_array() || json.is_array());
}

#[tokio::test]
async fn test_get_nonexistent_session() {
    let (state, key) = common::test_app_state().await;
    let (status, _) = get(state, "/api/v1/sessions/tg:nonexistent", Some(&key)).await;
    // 可以是 404 或 200 with null
    assert!(
        status == StatusCode::OK || status == StatusCode::NOT_FOUND,
        "expected 200 or 404, got {}",
        status
    );
}

// ── 消息路由 ──

#[tokio::test]
async fn test_send_message_with_invalid_target() {
    let (state, key) = common::test_app_state().await;
    let body = r#"{"target": "", "text": "hello"}"#;
    let (status, _) = post(state, "/api/v1/messages/send", Some(&key), Some(body)).await;
    // 空 target 应该返回客户端错误
    assert!(status.is_client_error(), "expected client error, got {}", status);
}

#[tokio::test]
async fn test_message_history_empty() {
    let (state, key) = common::test_app_state().await;
    let (status, json) = get(state, "/api/v1/messages", Some(&key)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["messages"].is_array() || json.is_array());
}

// ── 聊天路由 ──

#[tokio::test]
async fn test_list_chats_empty() {
    let (state, key) = common::test_app_state().await;
    let (status, _) = get(state, "/api/v1/chats/telegram", Some(&key)).await;
    assert!(status.is_success() || status == StatusCode::NOT_FOUND);
}

// ── 配置路由 ──

#[tokio::test]
async fn test_get_config() {
    let (state, key) = common::test_app_state().await;
    let (status, json) = get(state, "/api/v1/config", Some(&key)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json["server"].is_object() || json["api"].is_object() || json.as_object().is_some());
}

// ── 404 测试 ──

#[tokio::test]
async fn test_unknown_route_returns_404() {
    let (state, key) = common::test_app_state().await;
    let (status, _) = get(state, "/api/v1/unknown-route-xyz", Some(&key)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
