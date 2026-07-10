//! API 路由层集成测试
//!
//! 启动 axum 测试服务器，通过 HTTP 请求验证所有路由端点的请求/响应流程。
//! 使用真实 AppState（内存 SQLite、空适配器管理器），覆盖：
//! - 公共路由（无需认证：/health）
//! - 受保护路由（需 Bearer Token：适配器、消息、会话等）
//! - 认证/鉴权流程
//! - 输入验证（非法参数、边界值）
//! - 错误路径（平台不存在、目标格式错误等）

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::Value;

mod common;

/// URL 辅助函数：拼接 base URL 和路径
fn url(base: &SocketAddr, path: &str) -> String {
    format!("http://{}{}", base, path)
}

/// 启动测试服务器，返回 (state, api_key, addr)
async fn test_server() -> (easybot_api::AppState, String, SocketAddr) {
    let (state, key) = common::test_app_state().await;
    let router = easybot_api::server::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    // 等待服务器就绪
    tokio::time::sleep(Duration::from_millis(50)).await;

    (state, key, addr)
}

/// 带 Bearer Token 认证的 HTTP 客户端
fn authed_client(key: &str) -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", key).parse().unwrap(),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap()
}

/// 无认证的 HTTP 客户端
fn client() -> reqwest::Client {
    reqwest::Client::new()
}

// ── 公共路由 ──

#[tokio::test]
async fn test_health_endpoint_returns_200() {
    let (_state, _key, addr) = test_server().await;

    let resp = client()
        .get(url(&addr, "/api/v1/health"))
        .send()
        .await
        .expect("Health check request failed");

    assert_eq!(resp.status(), 200, "Health endpoint should return 200");
    let body: Value = resp.json().await.expect("Response should be valid JSON");
    assert_eq!(body["status"], "degraded", "No adapters → degraded");
    assert!(body["version"].is_string(), "Version should be present");
    assert!(body["uptime"].is_number(), "Uptime should be a number");
    assert_eq!(body["adapters"]["total"], 0, "No adapters registered");
    assert_eq!(body["adapters"]["connected"], 0);
}

// ── 认证/鉴权 ──

#[tokio::test]
async fn test_protected_route_requires_auth() {
    let (_state, _key, addr) = test_server().await;

    // 不带 Authorization 头
    let resp = client()
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 401, "Should require authentication");
}

#[tokio::test]
async fn test_invalid_auth_returns_401() {
    let (_state, _key, addr) = test_server().await;

    let resp = client()
        .get(url(&addr, "/api/v1/adapters"))
        .header(reqwest::header::AUTHORIZATION, "Bearer invalid-key")
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 401, "Invalid key should be rejected");
}

// ── 适配器端点 ──

#[tokio::test]
async fn test_adapters_list_empty() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["adapters"].is_array(), "adapters should be an array");
    assert_eq!(body["adapters"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_adapter_status_not_found() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/adapters/nonexistent/status"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 404);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "PLATFORM_NOT_FOUND");
}

#[tokio::test]
async fn test_start_nonexistent_adapter() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .post(url(&addr, "/api/v1/adapters/nonexistent/start"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(!body["ok"].as_bool().unwrap_or(true));
}

#[tokio::test]
async fn test_stop_nonexistent_adapter() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .post(url(&addr, "/api/v1/adapters/nonexistent/stop"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("Request failed");

    // AdapterManager's stop() returns Ok(()) even for platforms
    // that were never started, so we get a 200 with ok: true.
    // This is acceptable — stopping a non-existent adapter is a no-op.
    let status = resp.status();
    assert!(status.is_success(), "Stop should return success");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
}

// ── 发送消息（错误路径）──

#[tokio::test]
async fn test_send_message_invalid_target_format() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .post(url(&addr, "/api/v1/messages/send"))
        .json(&serde_json::json!({
            "target": "invalid-format-without-colon",
            "text": "Hello"
        }))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 400, "Invalid target should be 400");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "INVALID_REQUEST");
}

#[tokio::test]
async fn test_send_message_adapter_not_connected() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .post(url(&addr, "/api/v1/messages/send"))
        .json(&serde_json::json!({
            "target": "telegram:12345",
            "text": "Hello"
        }))
        .send()
        .await
        .expect("Request failed");

    // Adapter not connected → 503 (Service Unavailable)
    assert_eq!(resp.status(), 503, "No adapter → 503");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "ADAPTER_NOT_CONNECTED");
}

#[tokio::test]
async fn test_send_message_empty_target() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .post(url(&addr, "/api/v1/messages/send"))
        .json(&serde_json::json!({
            "target": "",
            "text": "Hello"
        }))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_send_message_text_too_long() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let long_text = "A".repeat(20000);

    let resp = client
        .post(url(&addr, "/api/v1/messages/send"))
        .json(&serde_json::json!({
            "target": "telegram:12345",
            "text": long_text
        }))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "MESSAGE_TOO_LONG");
}

// ── 批量发送 ──

#[tokio::test]
async fn test_batch_send_too_many_targets() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let targets: Vec<String> = (0..101).map(|i| format!("telegram:{}", i)).collect();

    let resp = client
        .post(url(&addr, "/api/v1/messages/batch-send"))
        .json(&serde_json::json!({
            "targets": targets,
            "text": "broadcast"
        }))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_batch_send_with_invalid_target() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .post(url(&addr, "/api/v1/messages/batch-send"))
        .json(&serde_json::json!({
            "targets": ["invalid-target"],
            "text": "broadcast"
        }))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["total"], 1);
    let results = body["results"].as_object().unwrap();
    assert_eq!(
        results["invalid-target"]["status"],
        "failed",
        "Invalid target should report failure"
    );
}

// ── 编辑/删除消息（错误路径）──

#[tokio::test]
async fn test_edit_message_invalid_target() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .put(url(&addr, "/api/v1/messages/12345"))
        .json(&serde_json::json!({
            "target": "invalid",
            "text": "updated"
        }))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "INVALID_REQUEST");
}

#[tokio::test]
async fn test_edit_message_nonexistent_platform() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .put(url(&addr, "/api/v1/messages/12345"))
        .json(&serde_json::json!({
            "target": "nonexistent:123",
            "text": "updated"
        }))
        .send()
        .await
        .expect("Request failed");

    // Nonexistent platform → 503 (AdapterNotConnected maps to 503)
    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "ADAPTER_NOT_CONNECTED");
}

#[tokio::test]
async fn test_delete_message_invalid_target() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .delete(url(&addr, "/api/v1/messages/12345"))
        .json(&serde_json::json!({
            "target": ""
        }))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 400);
}

// ── 消息历史 ──

#[tokio::test]
async fn test_message_history_empty() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/messages"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["messages"].is_array(), "messages should be an array");
    assert_eq!(body["messages"].as_array().unwrap().len(), 0);
    assert!(!body["has_more"].as_bool().unwrap_or(true), "has_more should be false");
}

#[tokio::test]
async fn test_message_history_with_filter_params() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/messages?platform=telegram&limit=10"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["messages"].is_array());
}

// ── 会话端点 ──

#[tokio::test]
async fn test_sessions_list_empty() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/sessions"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["sessions"].is_array(), "sessions should be an array");
}

// ── 配置端点 ──

#[tokio::test]
async fn test_get_config_returns_config() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/config"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body.get("server").is_some() || body.get("api").is_some(),
        "Config should have top-level fields"
    );
}

// ── 系统信息 ──

#[tokio::test]
async fn test_system_info_returns_system_data() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/system"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.get("cpu").is_some() || body.get("memory").is_some());
}

// ── 日志端点 ──

#[tokio::test]
async fn test_logs_endpoint_returns_logs() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/logs"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
}

// ── API Key 管理 ──

#[tokio::test]
async fn test_list_api_key_types() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/api-keys/types"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_list_api_keys() {
    let (_state, key, addr) = test_server().await;
    let client = authed_client(&key);

    let resp = client
        .get(url(&addr, "/api/v1/api-keys"))
        .send()
        .await
        .expect("Request failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.is_array());
    assert!(
        body.as_array().unwrap().len() >= 1,
        "Should have at least the test key"
    );
}
