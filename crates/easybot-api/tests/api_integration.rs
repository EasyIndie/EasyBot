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
use std::sync::Arc;
use std::time::Duration;

use easybot_core::storage::OutboundDelivery;
use serde_json::Value;
use sha2::{Digest, Sha256};

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

async fn test_server_with_metrics() -> (easybot_api::AppState, String, SocketAddr) {
    let (mut state, key) = common::test_app_state().await;
    state.metrics = Some(Arc::new(easybot_api::metrics::MetricsRegistry::new()));
    let router = easybot_api::server::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
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

#[tokio::test]
async fn test_delivery_journal_query_and_manual_reconciliation_are_audited() {
    let (state, key, addr) = test_server().await;
    let actor = state.auth_manager.authenticate(&key).await.unwrap();
    state
        .message_store
        .prepare_outbound_delivery(&OutboundDelivery {
            id: "pending-delivery-api-test".into(),
            actor_id: actor.id.clone(),
            idempotency_key: Some("customer-operation-123".into()),
            platform: "telegram".into(),
            chat_id: "customer-chat".into(),
            request_json: serde_json::json!({"text": "uncertain delivery"}),
            created_at: chrono::Utc::now().timestamp_millis(),
        })
        .await
        .unwrap();

    let http = authed_client(&key);
    let before: Value = http
        .get(url(&addr, "/api/v1/messages/deliveries"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["deliveries"][0]["state"], "pending");

    let response = http
        .post(url(
            &addr,
            "/api/v1/messages/deliveries/pending-delivery-api-test/reconcile",
        ))
        .json(&serde_json::json!({
            "resolution": "succeeded",
            "evidence": "Telegram history search found platform message tg-12345"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let duplicate = http
        .post(url(
            &addr,
            "/api/v1/messages/deliveries/pending-delivery-api-test/reconcile",
        ))
        .json(&serde_json::json!({
            "resolution": "failed",
            "evidence": "A second conclusion must never overwrite the first conclusion"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 409);

    let after: Value = http
        .get(url(&addr, "/api/v1/messages/deliveries"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["deliveries"][0]["state"], "succeeded");
    assert_eq!(
        after["deliveries"][0]["reconciliation_evidence"],
        "Telegram history search found platform message tg-12345"
    );
    let audit = state.auth_manager.list_audit_events(10).await;
    assert!(
        audit
            .iter()
            .any(|event| event.action == "message.delivery.reconcile.completed")
    );
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
    assert_eq!(resp.headers()["cache-control"], "no-store, max-age=0");
    assert_eq!(resp.headers()["x-frame-options"], "DENY");
    assert_eq!(resp.headers()["referrer-policy"], "no-referrer");
    assert!(resp.headers().contains_key("content-security-policy"));
    let request_id = resp.headers()["x-request-id"].to_str().unwrap();
    assert!(uuid::Uuid::parse_str(request_id).is_ok());
    let body: Value = resp.json().await.expect("Response should be valid JSON");
    assert_eq!(body["status"], "degraded", "No adapters → degraded");
    assert!(body["version"].is_string(), "Version should be present");
    assert!(body["uptime"].is_number(), "Uptime should be a number");
    assert_eq!(body["adapters"]["total"], 0, "No adapters registered");
    assert_eq!(body["adapters"]["connected"], 0);
}

#[tokio::test]
async fn test_cors_allows_idempotency_and_exposes_commercial_response_headers() {
    let (_state, _key, addr) = test_server().await;
    let preflight = client()
        .request(
            reqwest::Method::OPTIONS,
            url(&addr, "/api/v1/messages/send"),
        )
        .header("Origin", "http://localhost:3000")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "authorization,content-type,idempotency-key",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), 200);
    let allowed = preflight.headers()["access-control-allow-headers"]
        .to_str()
        .unwrap()
        .to_ascii_lowercase();
    assert!(allowed.contains("idempotency-key"));

    let response = client()
        .get(url(&addr, "/api/v1/health"))
        .header("Origin", "http://localhost:3000")
        .send()
        .await
        .unwrap();
    let exposed = response.headers()["access-control-expose-headers"]
        .to_str()
        .unwrap()
        .to_ascii_lowercase();
    assert!(exposed.contains("idempotency-replayed"));
    assert!(exposed.contains("x-request-id"));
    assert!(exposed.contains("x-ratelimit-remaining"));
}

#[tokio::test]
async fn test_liveness_and_readiness_are_public_and_distinct() {
    let (_state, _key, addr) = test_server().await;
    let live = client()
        .get(url(&addr, "/api/v1/live"))
        .send()
        .await
        .unwrap();
    assert_eq!(live.status(), 200);
    assert_eq!(live.json::<Value>().await.unwrap()["status"], "alive");

    let ready = client()
        .get(url(&addr, "/api/v1/ready"))
        .send()
        .await
        .unwrap();
    assert_eq!(ready.status(), 200);
    let body = ready.json::<Value>().await.unwrap();
    assert_eq!(body["status"], "ready");
    assert_eq!(body["storage"], "ready");
    assert_eq!(body["message_storage"], "ready");
    assert_eq!(body["metering"], "ready");
    assert_eq!(body["quota"], "ready");
    assert_eq!(body["delivery_journal"], "ready");
    assert_eq!(body["pending_deliveries"], 0);
    assert_eq!(body["stale_pending_deliveries"], 0);
    assert_eq!(body["unpublished_delivery_events"], 0);
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
async fn test_nested_api_routes_enforce_fine_grained_permissions() {
    let (state, _admin_key, addr) = test_server().await;
    let (_, restricted_key) = state
        .auth_manager
        .create_key("adapter-viewer", vec!["adaptersread".into()], None, vec![])
        .await
        .unwrap();
    let client = authed_client(&restricted_key);

    assert_eq!(
        client
            .get(url(&addr, "/api/v1/adapters"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for path in [
        "/api/v1/config",
        "/api/v1/sessions",
        "/api/v1/api-keys",
        "/api/v1/logs",
        "/api/v1/system",
    ] {
        assert_eq!(
            client.get(url(&addr, path)).send().await.unwrap().status(),
            403,
            "restricted key unexpectedly accessed {path}"
        );
    }
    assert_eq!(
        client
            .post(url(&addr, "/api/v1/messages/send"))
            .json(&serde_json::json!({"target":"telegram:1","text":"no"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
}

#[tokio::test]
async fn test_api_key_rotation_revokes_old_key_and_preserves_policy() {
    let (state, admin_key, addr) = test_server().await;
    let expires_at = chrono::Utc::now().timestamp_millis() + 24 * 60 * 60 * 1_000;
    let created = authed_client(&admin_key)
        .post(url(&addr, "/api/v1/api-keys"))
        .json(&serde_json::json!({
            "name": "rotating-customer",
            "permissions": ["adaptersread", "messagesread", "messagessend"],
            "event_filters": [],
            "requests_per_minute": 4,
            "expires_at": expires_at
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);
    let original: Value = created.json().await.unwrap();
    let original_id = original["id"].as_str().unwrap();
    let original_key = original["key"].as_str().unwrap();
    let original_auth = state.auth_manager.authenticate(original_key).await.unwrap();
    for expected_remaining in [3, 2] {
        let response = authed_client(original_key)
            .get(url(&addr, "/api/v1/adapters"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()["x-ratelimit-remaining"],
            expected_remaining.to_string()
        );
    }
    state
        .message_store
        .prepare_outbound_delivery(&OutboundDelivery {
            id: "delivery-created-before-rotation".into(),
            actor_id: original_auth.subject_id.clone(),
            idempotency_key: Some("rotation-operation-123".into()),
            platform: "telegram".into(),
            chat_id: "rotating-chat".into(),
            request_json: serde_json::json!({"text": "uncertain before rotation"}),
            created_at: 1,
        })
        .await
        .unwrap();
    state
        .auth_manager
        .reserve_idempotency(
            &original_auth.subject_id,
            "rotation-operation-123",
            "request-hash",
        )
        .await
        .unwrap();

    let replacement_expiry = expires_at + 60_000;
    let rotated = authed_client(&admin_key)
        .post(url(
            &addr,
            &format!("/api/v1/api-keys/{original_id}/rotate"),
        ))
        .json(&serde_json::json!({"expires_at": replacement_expiry}))
        .send()
        .await
        .unwrap();
    assert_eq!(rotated.status(), 200);
    let replacement: Value = rotated.json().await.unwrap();
    assert!(
        replacement["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|permission| permission == "adaptersread")
    );
    assert_eq!(replacement["requests_per_minute"], 4);
    assert_eq!(replacement["expires_at"], replacement_expiry);

    assert_eq!(
        authed_client(original_key)
            .get(url(&addr, "/api/v1/adapters"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let replacement_key = replacement["key"].as_str().unwrap();
    let replacement_auth = state
        .auth_manager
        .authenticate(replacement_key)
        .await
        .unwrap();
    assert_ne!(replacement_auth.id, original_auth.id);
    assert_eq!(replacement_auth.subject_id, original_auth.subject_id);
    let deliveries: Value = authed_client(replacement_key)
        .get(url(&addr, "/api/v1/messages/deliveries"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        deliveries["deliveries"][0]["id"],
        "delivery-created-before-rotation"
    );
    let reconciled = authed_client(replacement_key)
        .post(url(
            &addr,
            "/api/v1/messages/deliveries/delivery-created-before-rotation/reconcile",
        ))
        .json(&serde_json::json!({
            "resolution": "failed",
            "evidence": "Platform history confirmed that no message was created"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reconciled.status(), 200);
    let replacement_quota = authed_client(replacement_key)
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .unwrap();
    assert_eq!(replacement_quota.status(), 429);
    assert_eq!(replacement_quota.headers()["x-ratelimit-remaining"], "0");
    assert!(matches!(
        state
            .auth_manager
            .reserve_idempotency(
                &replacement_auth.subject_id,
                "rotation-operation-123",
                "request-hash",
            )
            .await
            .unwrap(),
        easybot_core::auth::IdempotencyReservation::InProgress
    ));
    let audit = state.auth_manager.list_audit_events(20).await;
    assert!(
        audit
            .iter()
            .any(|event| event.action == "api_key.rotation.prepared")
    );
    assert!(audit.iter().any(|event| event.action == "api_key.rotated"));
}

#[tokio::test]
async fn test_production_admin_login_issues_expiring_memory_session() {
    let (mut state, _dev_key) = common::test_app_state().await;
    state.dev_api_key = None;
    let router = easybot_api::server::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let response = client()
        .post(url(&addr, "/admin/login"))
        .json(&serde_json::json!({"password":"easybot"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let auth = state
        .auth_manager
        .authenticate(body["key"].as_str().unwrap())
        .await
        .unwrap();
    let session = state
        .auth_manager
        .list_keys()
        .await
        .into_iter()
        .find(|key| key.id == auth.id)
        .unwrap();
    let remaining = session.expires_at.unwrap() - chrono::Utc::now().timestamp_millis();
    assert!(remaining > 50 * 60 * 1_000 && remaining <= 60 * 60 * 1_000);
    assert!(
        state
            .auth_manager
            .list_audit_events(10)
            .await
            .iter()
            .any(|event| event.action == "admin.session.issued")
    );
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

#[tokio::test]
async fn test_authenticated_requests_are_metered_by_key_id() {
    let (state, key, addr) = test_server_with_metrics().await;
    let key_id = state.auth_manager.list_keys().await[0].id.clone();

    let ok = authed_client(&key)
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let unauthorized = client()
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 401);

    let rendered = state.metrics.as_ref().unwrap().render();
    assert!(rendered.contains(&format!(
        "api_key_requests_total{{key_id=\"{key_id}\",status_class=\"2xx\"}} 1"
    )));
    assert!(rendered.contains("api_auth_failures_total{reason=\"missing_header\"} 1"));
    assert!(!rendered.contains(&key));

    let (_, metrics_key) = state
        .auth_manager
        .create_key("prometheus", vec!["metricsread".into()], None, vec![])
        .await
        .unwrap();
    let metrics_response = authed_client(&metrics_key)
        .get(url(&addr, "/api/v1/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(metrics_response.status(), 200);

    let (_, unprivileged_key) = state
        .auth_manager
        .create_key("unprivileged", vec!["adaptersread".into()], None, vec![])
        .await
        .unwrap();
    let forbidden = authed_client(&unprivileged_key)
        .get(url(&addr, "/api/v1/metrics"))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);
}

#[tokio::test]
async fn test_durable_usage_export_is_reconciled_and_permission_scoped() {
    let (state, admin_key, addr) = test_server().await;
    let (customer_id, customer_key) = state
        .auth_manager
        .create_key("billed-customer", vec!["adaptersread".into()], None, vec![])
        .await
        .unwrap();
    let (_, billing_key) = state
        .auth_manager
        .create_key("billing-export", vec!["billingread".into()], None, vec![])
        .await
        .unwrap();

    for _ in 0..2 {
        let response = authed_client(&customer_key)
            .get(url(&addr, "/api/v1/adapters"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
    let subject_id = state
        .auth_manager
        .authenticate(&customer_key)
        .await
        .unwrap()
        .subject_id;
    let replacement_expiry = chrono::Utc::now().timestamp_millis() + 24 * 60 * 60 * 1_000;
    let rotated = authed_client(&admin_key)
        .post(url(
            &addr,
            &format!("/api/v1/api-keys/{customer_id}/rotate"),
        ))
        .json(&serde_json::json!({"expires_at": replacement_expiry}))
        .send()
        .await
        .unwrap();
    assert_eq!(rotated.status(), 200);
    let replacement: Value = rotated.json().await.unwrap();
    assert_eq!(replacement["subject_id"], subject_id);
    let replacement_id = replacement["id"].as_str().unwrap().to_string();
    let replacement_key = replacement["key"].as_str().unwrap().to_string();
    let response = authed_client(&replacement_key)
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let now = chrono::Utc::now().timestamp_millis();
    let export = authed_client(&billing_key)
        .get(url(
            &addr,
            &format!(
                "/api/v1/usage?from={}&to={}&key_id={customer_id}",
                now - 3_600_000,
                now + 3_600_000
            ),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(export.status(), 200);
    let body: Value = export.json().await.unwrap();
    assert_eq!(body["total_requests"], 2);
    assert_eq!(body["records"][0]["key_id"], customer_id);
    assert_eq!(body["records"][0]["subject_id"], subject_id);
    assert_eq!(body["records"][0]["status_class"], 2);

    let subject_export = authed_client(&billing_key)
        .get(url(
            &addr,
            &format!(
                "/api/v1/usage?from={}&to={}&subject_id={subject_id}&limit=1",
                now - 3_600_000,
                now + 3_600_000
            ),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(subject_export.status(), 200);
    let subject_body: Value = subject_export.json().await.unwrap();
    assert_eq!(subject_body["total_requests"], 3);
    assert_eq!(subject_body["records"].as_array().unwrap().len(), 1);
    assert_eq!(
        subject_body["page_requests"],
        subject_body["records"][0]["request_count"]
    );
    assert_eq!(subject_body["truncated"], true);
    assert_eq!(subject_body["next_offset"], 1);
    let second_page: Value = authed_client(&billing_key)
        .get(url(
            &addr,
            &format!(
                "/api/v1/usage?from={}&to={}&subject_id={subject_id}&limit=1&offset=1",
                now - 3_600_000,
                now + 3_600_000
            ),
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(second_page["total_requests"], 3);
    assert_eq!(second_page["truncated"], false);
    let key_ids = subject_body["records"]
        .as_array()
        .unwrap()
        .iter()
        .chain(second_page["records"].as_array().unwrap())
        .map(|record| record["key_id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert!(key_ids.contains(customer_id.as_str()));
    assert!(key_ids.contains(replacement_id.as_str()));

    let forbidden = authed_client(&replacement_key)
        .get(url(
            &addr,
            &format!(
                "/api/v1/usage?from={}&to={}",
                now - 3_600_000,
                now + 3_600_000
            ),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);
}

#[tokio::test]
async fn test_financial_event_ledger_is_idempotent_conflict_safe_and_scoped() {
    let (state, _admin_key, addr) = test_server().await;
    let (_, writer) = state
        .auth_manager
        .create_key("payment-bridge", vec!["billingwrite".into()], None, vec![])
        .await
        .unwrap();
    let (_, reader) = state
        .auth_manager
        .create_key("finance-reader", vec!["billingread".into()], None, vec![])
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let event = serde_json::json!({
        "provider":"stripe", "event_id":"evt_test_1", "event_type":"payment_succeeded",
        "object_id":"pi_test_1", "customer_ref":"customer-42", "amount_minor":9900,
        "currency":"USD", "occurred_at":now
    });
    let created = authed_client(&writer)
        .post(url(&addr, "/api/v1/billing/events"))
        .json(&event)
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);
    assert_eq!(
        created.json::<Value>().await.unwrap()["disposition"],
        "created"
    );
    let duplicate = authed_client(&writer)
        .post(url(&addr, "/api/v1/billing/events"))
        .json(&event)
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 200);
    assert_eq!(
        duplicate.json::<Value>().await.unwrap()["disposition"],
        "duplicate"
    );
    let mut conflict = event.clone();
    conflict["amount_minor"] = 10000.into();
    let conflict_response = authed_client(&writer)
        .post(url(&addr, "/api/v1/billing/events"))
        .json(&conflict)
        .send()
        .await
        .unwrap();
    assert_eq!(conflict_response.status(), 409);

    for (event_id, event_type, object_id) in [
        ("evt_refund_1", "refund_succeeded", "re_test_1"),
        ("evt_dispute_1", "chargeback_opened", "dp_test_1"),
    ] {
        let mut followup = event.clone();
        followup["event_id"] = event_id.into();
        followup["event_type"] = event_type.into();
        followup["object_id"] = object_id.into();
        let response = authed_client(&writer)
            .post(url(&addr, "/api/v1/billing/events"))
            .json(&followup)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    let listed = authed_client(&reader)
        .get(url(
            &addr,
            &format!(
                "/api/v1/billing/events?from={}&to={}&customer_ref=customer-42",
                now - 1000,
                now + 1000
            ),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);
    let body: Value = listed.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert!(
        events
            .iter()
            .any(|item| item["event_type"] == "refund_succeeded")
    );
    assert!(
        events
            .iter()
            .any(|item| item["event_type"] == "chargeback_opened")
    );
    let forbidden = authed_client(&reader)
        .post(url(&addr, "/api/v1/billing/events"))
        .json(&event)
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);
}

#[tokio::test]
async fn test_per_key_quota_returns_headers_and_429() {
    let (state, _key, addr) = test_server().await;
    let (_, quota_key) = state
        .auth_manager
        .create_key_with_quota(
            "starter-plan",
            vec!["adaptersread".into()],
            None,
            vec![],
            Some(2),
        )
        .await
        .unwrap();
    let client = authed_client(&quota_key);

    let first = client
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    assert_eq!(first.headers()["x-ratelimit-limit"], "2");
    assert_eq!(first.headers()["x-ratelimit-remaining"], "1");

    let second = client
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200);
    assert_eq!(second.headers()["x-ratelimit-remaining"], "0");

    let limited = client
        .get(url(&addr, "/api/v1/adapters"))
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), 429);
    assert!(limited.headers().contains_key("retry-after"));
    assert_eq!(limited.headers()["x-ratelimit-remaining"], "0");
}

#[tokio::test]
async fn test_audit_ledger_records_management_actions_and_enforces_permission() {
    let (state, admin_key, addr) = test_server().await;
    let created = authed_client(&admin_key)
        .post(url(&addr, "/api/v1/api-keys"))
        .json(&serde_json::json!({
            "name": "audited-customer",
            "permissions": ["messagesread"],
            "event_filters": [],
            "requests_per_minute": 100
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 200);

    let response = authed_client(&admin_key)
        .get(url(&addr, "/api/v1/audit-events?limit=10"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["integrity_valid"], true);
    assert_eq!(body["events"][0]["action"], "api_key.created");
    assert!(body["events"][0]["metadata"].get("key").is_none());

    let config_update = authed_client(&admin_key)
        .put(url(&addr, "/api/v1/config"))
        .json(&serde_json::json!({"logging":{"level":"warn"}}))
        .send()
        .await
        .unwrap();
    let config_status = config_update.status();
    let config_body = config_update.text().await.unwrap();
    assert_eq!(config_status, 409, "{config_body}");
    let adapter_start = authed_client(&admin_key)
        .post(url(&addr, "/api/v1/adapters/nonexistent/start"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(adapter_start.status(), 200);
    let actions = state
        .auth_manager
        .list_audit_events(20)
        .await
        .into_iter()
        .map(|event| event.action)
        .collect::<Vec<_>>();
    for expected in [
        "config.update.rejected",
        "adapter.start.requested",
        "adapter.start.completed",
    ] {
        assert!(
            actions.iter().any(|action| action == expected),
            "missing {expected}"
        );
    }

    let (_, restricted) = state
        .auth_manager
        .create_key("no-audit", vec!["adaptersread".into()], None, vec![])
        .await
        .unwrap();
    let forbidden = authed_client(&restricted)
        .get(url(&addr, "/api/v1/audit-events"))
        .send()
        .await
        .unwrap();
    assert_eq!(forbidden.status(), 403);
}

#[tokio::test]
async fn test_privacy_export_and_cascade_delete_remove_message_payloads() {
    use easybot_core::storage::{MessageFilter, MessageRole, OutboundDelivery, StoredMessage};
    use easybot_core::types::message::ChatType;
    use easybot_core::types::session::SessionSource;

    let (state, key, addr) = test_server().await;
    let session_key = "telegram:privacy-user";
    state
        .session_manager
        .get_or_create(
            session_key,
            SessionSource {
                platform: "telegram".into(),
                chat_id: "privacy-user".into(),
                chat_name: Some("Private User".into()),
                chat_type: ChatType::Dm,
                user_id: Some("subject-1".into()),
                user_name: Some("Data Subject".into()),
                is_bot: false,
                user_username: None,
                user_role: None,
            },
        )
        .await;
    state
        .message_store
        .store_message(&StoredMessage {
            id: "privacy-message-1".into(),
            session_key: session_key.into(),
            platform: "telegram".into(),
            chat_id: "privacy-user".into(),
            role: MessageRole::User,
            text: Some("personal data".into()),
            raw_data: serde_json::json!({"sensitive":"payload"}),
            timestamp: 1,
            created_at: 1,
        })
        .await
        .unwrap();
    let actor = state.auth_manager.authenticate(&key).await.unwrap();
    for (id, chat_id, text) in [
        (
            "privacy-delivery-1",
            "privacy-user",
            "personal outbound data",
        ),
        (
            "unrelated-delivery",
            "another-user",
            "must survive deletion",
        ),
    ] {
        state
            .message_store
            .prepare_outbound_delivery(&OutboundDelivery {
                id: id.into(),
                actor_id: actor.id.clone(),
                idempotency_key: None,
                platform: "telegram".into(),
                chat_id: chat_id.into(),
                request_json: serde_json::json!({"text": text}),
                created_at: 1,
            })
            .await
            .unwrap();
    }

    let export = authed_client(&key)
        .get(url(&addr, "/api/v1/sessions/telegram:privacy-user/export"))
        .send()
        .await
        .unwrap();
    assert_eq!(export.status(), 200);
    let exported: Value = export.json().await.unwrap();
    assert_eq!(exported["messages"][0]["raw_data"]["sensitive"], "payload");
    assert_eq!(
        exported["outbound_deliveries"][0]["request_json"]["text"],
        "personal outbound data"
    );

    let deleted = authed_client(&key)
        .delete(url(&addr, "/api/v1/sessions/telegram:privacy-user"))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 200);
    let body: Value = deleted.json().await.unwrap();
    assert_eq!(body["deleted_messages"], 1);
    assert_eq!(body["deleted_deliveries"], 1);
    assert!(state.session_manager.get(session_key).is_none());
    let remaining = state
        .message_store
        .list_messages(&MessageFilter {
            session_key: Some(session_key.into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(remaining.is_empty());
    assert!(
        state
            .message_store
            .list_outbound_deliveries_by_session("telegram", "privacy-user", 10, 0)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        state
            .message_store
            .list_outbound_deliveries_by_session("telegram", "another-user", 10, 0)
            .await
            .unwrap()
            .len(),
        1,
        "privacy deletion must not affect another chat"
    );
    assert!(
        state
            .auth_manager
            .list_audit_events(10)
            .await
            .iter()
            .any(|event| event.action == "privacy.session.deleted")
    );
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
async fn test_send_message_replays_persisted_idempotent_response_and_rejects_reuse() {
    let (state, key, addr) = test_server().await;
    let key_id = state.auth_manager.list_keys().await[0].id.clone();
    let request = easybot_api::routes::messages::SendMessageRequest {
        target: "telegram:12345".into(),
        text: "commercial-once".into(),
        parse_mode: None,
        media: None,
        keyboard: None,
        reply_to: None,
        metadata: None,
    };
    let request_hash = Sha256::digest(serde_json::to_vec(&request).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let idem_key = "order-request-123";
    assert_eq!(
        state
            .auth_manager
            .reserve_idempotency(&key_id, idem_key, &request_hash)
            .await
            .unwrap(),
        easybot_core::auth::IdempotencyReservation::Acquired
    );
    state
        .auth_manager
        .complete_idempotency(
            &key_id,
            idem_key,
            &request_hash,
            200,
            r#"{"id":"platform-message-1","status":"sent"}"#,
        )
        .await
        .unwrap();

    let replay = authed_client(&key)
        .post(url(&addr, "/api/v1/messages/send"))
        .header("Idempotency-Key", idem_key)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.headers()["idempotency-replayed"], "true");
    assert_eq!(
        replay.json::<Value>().await.unwrap()["id"],
        "platform-message-1"
    );

    let conflict = authed_client(&key)
        .post(url(&addr, "/api/v1/messages/send"))
        .header("Idempotency-Key", idem_key)
        .json(&serde_json::json!({"target":"telegram:12345","text":"changed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
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

    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "INVALID_REQUEST");
}

#[tokio::test]
async fn test_batch_send_rejects_duplicate_targets_and_replays_idempotent_result() {
    let (state, key, addr) = test_server().await;
    let duplicate = authed_client(&key)
        .post(url(&addr, "/api/v1/messages/batch-send"))
        .json(&serde_json::json!({
            "targets":["telegram:1","telegram:1"], "text":"duplicate"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(duplicate.status(), 400);

    let request = easybot_api::routes::messages::BatchSendRequest {
        targets: vec!["telegram:1".into(), "telegram:2".into()],
        text: "commercial-batch-once".into(),
        parse_mode: None,
    };
    let hash = Sha256::digest(serde_json::to_vec(&request).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let key_id = state.auth_manager.list_keys().await[0].id.clone();
    let idem = "batch-request-123";
    assert_eq!(
        state
            .auth_manager
            .reserve_idempotency(&key_id, idem, &hash)
            .await
            .unwrap(),
        easybot_core::auth::IdempotencyReservation::Acquired
    );
    state.auth_manager.complete_idempotency(&key_id, idem, &hash, 200,
        r#"{"total":2,"results":{"telegram:1":{"status":"sent"},"telegram:2":{"status":"sent"}}}"#)
        .await.unwrap();
    let replay = authed_client(&key)
        .post(url(&addr, "/api/v1/messages/batch-send"))
        .header("Idempotency-Key", idem)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 200);
    assert_eq!(replay.headers()["idempotency-replayed"], "true");
    assert_eq!(replay.json::<Value>().await.unwrap()["total"], 2);
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
    assert!(
        !body["has_more"].as_bool().unwrap_or(true),
        "has_more should be false"
    );
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

#[tokio::test]
async fn test_message_history_rejects_limit_above_contract() {
    let (_state, key, addr) = test_server().await;
    let response = authed_client(&key)
        .get(url(&addr, "/api/v1/messages?limit=201"))
        .send()
        .await
        .expect("Request failed");
    assert_eq!(response.status(), 400);
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
        !body.as_array().unwrap().is_empty(),
        "Should have at least the test key"
    );
}
