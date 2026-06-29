//! Webhook 事件转发
//!
//! 订阅 EventBus 事件，根据配置将事件通过 HTTP POST 转发到外部 URL。
//! 支持 HMAC-SHA256 签名验证，按事件类型和平台过滤。

use hmac::{Hmac, KeyInit, Mac};
use reqwest::Client;
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, trace, warn};

use crate::bus::EventBus;
use crate::types::config::WebhookConfig;
use crate::types::event::GatewayEvent;

/// HMAC-SHA256 类型别名
type HmacSha256 = Hmac<Sha256>;

/// 从字节切片生成小写十六进制字符串
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Webhook 事件转发器
pub struct WebhookDispatcher;

impl WebhookDispatcher {
    /// 启动 Webhook 事件转发
    ///
    /// 创建一个后台任务，订阅 EventBus 事件并根据 WebhookConfig 转发到外部 URL。
    /// 如果 webhooks 为空则无操作。
    pub fn start(event_bus: Arc<EventBus>, webhooks: Vec<WebhookConfig>) {
        if webhooks.is_empty() {
            return;
        }

        let count = webhooks.len();
        info!("Webhook dispatcher starting with {} webhook(s)", count);

        tokio::spawn(async move {
            Self::run(event_bus, webhooks).await;
        });
    }

    async fn run(event_bus: Arc<EventBus>, webhooks: Vec<WebhookConfig>) {
        let client = match Client::builder().timeout(Duration::from_secs(10)).build() {
            Ok(c) => Arc::new(c),
            Err(e) => {
                error!("Failed to create HTTP client for webhook dispatcher: {}", e);
                return;
            }
        };

        // 收集所有需要订阅的事件类型
        let mut all_types: Vec<&str> = Vec::new();
        let mut catch_all = false;
        for wh in &webhooks {
            for event_type in &wh.events {
                if event_type == "*" {
                    catch_all = true;
                } else if !all_types.contains(&event_type.as_str()) {
                    all_types.push(event_type.as_str());
                }
            }
        }

        if catch_all {
            let known_types = crate::types::event::event_types::all();
            for t in known_types {
                if !all_types.contains(t) {
                    all_types.push(t);
                }
            }
        }

        if all_types.is_empty() {
            info!("No event types configured for webhooks, skipping");
            return;
        }

        let mut rx = event_bus.subscribe_many(&all_types);

        loop {
            match rx.recv().await {
                Ok(event) => {
                    let event_type = event.event_type.clone();
                    let platform = event
                        .data
                        .get("platform")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    tokio::spawn(Self::dispatch_event(
                        client.clone(),
                        webhooks.clone(),
                        event,
                        event_type,
                        platform,
                    ));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Webhook dispatcher lagged by {} events", n);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("Webhook dispatcher stopped (EventBus closed)");
                    break;
                }
            }
        }
    }

    /// 向所有匹配的 webhook 分发事件
    #[allow(clippy::needless_pass_by_value)]
    async fn dispatch_event(
        client: Arc<Client>,
        webhooks: Vec<WebhookConfig>,
        event: GatewayEvent,
        event_type: String,
        platform: String,
    ) {
        Self::dispatch_with_client(&client, &webhooks, &event, &event_type, &platform).await;
    }

    /// dispatch_event 内部实现，便于直接测试
    async fn dispatch_with_client(
        client: &Client,
        webhooks: &[WebhookConfig],
        event: &GatewayEvent,
        event_type: &str,
        platform: &str,
    ) {
        let payload_bytes = match serde_json::to_vec(&event) {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to serialize webhook event bytes: {}", e);
                return;
            }
        };

        for wh in webhooks {
            // 检查事件类型是否匹配
            if !wh.events.is_empty()
                && !wh.events.contains(&"*".to_string())
                && !wh.events.contains(&event_type.to_string())
            {
                continue;
            }

            // 检查平台是否匹配
            if let Some(ref platforms) = wh.platforms
                && !platforms.is_empty()
                && !platforms.contains(&platform.to_string())
            {
                continue;
            }

            // 构造签名头
            let signature = wh.secret.as_ref().map(|secret| {
                let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                    .expect("HMAC key should be valid");
                mac.update(&payload_bytes);
                let result = mac.finalize();
                let code = result.into_bytes();
                format!("sha256={}", hex_encode(&code))
            });

            // 发送 POST 请求
            let payload_json: serde_json::Value = match serde_json::from_slice(&payload_bytes) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        webhook = %wh.name,
                        event_type = %event_type,
                        "Webhook payload 反序列化失败，使用 null"
                    );
                    serde_json::Value::Null
                }
            };
            let mut req = client
                .post(&wh.url)
                .header("Content-Type", "application/json")
                .json(&payload_json);

            if let Some(ref sig) = signature {
                req = req.header("X-Signature-256", sig);
            }

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        trace!(
                            "Webhook '{}' delivered event '{}' to {} (status {})",
                            wh.name, event_type, wh.url, status,
                        );
                    } else {
                        warn!(
                            "Webhook '{}' returned {} for event '{}' to {}",
                            wh.name, status, event_type, wh.url,
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Webhook '{}' failed to deliver event '{}' to {}: {}",
                        wh.name, event_type, wh.url, e,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use std::sync::Arc;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{WebhookDispatcher, hex_encode};
    use crate::types::config::WebhookConfig;
    use crate::types::event::GatewayEvent;
    use crate::types::event::event_types::MESSAGE_INBOUND;

    /// 辅助：创建测试事件
    fn test_event() -> GatewayEvent {
        GatewayEvent {
            event_type: MESSAGE_INBOUND.to_string(),
            source: "test".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            data: json!({
                "platform": "telegram",
                "chat_id": "123",
                "text": "hello"
            }),
            metadata: None,
        }
    }

    /// 辅助：快速创建 WebhookConfig
    fn webhook_config(
        url: &str,
        events: Vec<&str>,
        platform: Option<&str>,
        secret: Option<&str>,
    ) -> WebhookConfig {
        WebhookConfig {
            name: "test-webhook".to_string(),
            url: url.to_string(),
            events: events.into_iter().map(String::from).collect(),
            platforms: platform.map(|p| vec![p.to_string()]),
            secret: secret.map(String::from),
        }
    }

    /// 辅助：通过 dispatch_with_client 直接测试，避免 EventBus 时序问题
    async fn dispatch_direct(webhooks: Vec<WebhookConfig>, event: GatewayEvent) {
        let client = reqwest::Client::new();
        let event_type = event.event_type.clone();
        let platform = event
            .data
            .get("platform")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        WebhookDispatcher::dispatch_with_client(&client, &webhooks, &event, &event_type, &platform)
            .await;
    }

    // ── hex_encode 单元测试 ──

    #[test]
    fn test_hex_encode_values() {
        assert_eq!(hex_encode(b"hello"), "68656c6c6f");
        assert_eq!(hex_encode(&[0u8, 255u8]), "00ff");
        assert_eq!(hex_encode(b""), "");
    }

    // ── dispatch 直接测试（dispatch_with_client） ──

    #[tokio::test]
    async fn test_dispatch_delivers_to_matching_webhook() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(header("Content-Type", "application/json"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1..)
            .mount(&mock_server)
            .await;

        let webhooks = vec![webhook_config(
            &mock_server.uri(),
            vec![MESSAGE_INBOUND],
            None,
            None,
        )];
        dispatch_direct(webhooks, test_event()).await;

        mock_server.verify().await;
    }

    #[tokio::test]
    async fn test_dispatch_hmac_signature() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1..)
            .mount(&mock_server)
            .await;

        let webhooks = vec![webhook_config(
            &mock_server.uri(),
            vec![MESSAGE_INBOUND],
            None,
            Some("test-secret"),
        )];
        dispatch_direct(webhooks, test_event()).await;

        mock_server.verify().await;

        // 检查签名头格式
        let reqs = mock_server.received_requests().await.unwrap_or_default();
        assert!(!reqs.is_empty(), "Expected at least one request");
        let req = &reqs[0];
        assert!(
            req.headers.contains_key("X-Signature-256"),
            "Expected X-Signature-256 header"
        );
        let sig = req.headers.get("X-Signature-256").unwrap();
        let sig_str = std::str::from_utf8(sig.as_bytes()).unwrap();
        assert!(
            sig_str.starts_with("sha256="),
            "Signature should start with sha256=, got: {}",
            sig_str
        );
    }

    #[tokio::test]
    async fn test_dispatch_filtered_by_event_type() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;

        let webhooks = vec![webhook_config(
            &mock_server.uri(),
            vec!["adapter.connected"],
            None,
            None,
        )];
        dispatch_direct(webhooks, test_event()).await;

        mock_server.verify().await;
    }

    #[tokio::test]
    async fn test_dispatch_filtered_by_platform() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;

        let webhooks = vec![webhook_config(
            &mock_server.uri(),
            vec![MESSAGE_INBOUND],
            Some("discord"),
            None,
        )];
        dispatch_direct(webhooks, test_event()).await;

        mock_server.verify().await;
    }

    #[tokio::test]
    async fn test_dispatch_catch_all() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1..)
            .mount(&mock_server)
            .await;

        let webhooks = vec![webhook_config(&mock_server.uri(), vec!["*"], None, None)];
        dispatch_direct(webhooks, test_event()).await;

        mock_server.verify().await;
    }

    #[tokio::test]
    async fn test_dispatch_no_secret_means_no_signature() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1..)
            .mount(&mock_server)
            .await;

        let webhooks = vec![webhook_config(
            &mock_server.uri(),
            vec![MESSAGE_INBOUND],
            None,
            None,
        )];
        dispatch_direct(webhooks, test_event()).await;

        let reqs = mock_server.received_requests().await.unwrap_or_default();
        assert!(!reqs.is_empty());
        let req = &reqs[0];
        assert!(
            !req.headers.contains_key("X-Signature-256"),
            "No signature header expected without secret"
        );
    }

    #[tokio::test]
    async fn test_dispatch_multiple_webhooks() {
        let s1 = MockServer::start().await;
        let s2 = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1..)
            .mount(&s1)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1..)
            .mount(&s2)
            .await;

        let webhooks = vec![
            webhook_config(&s1.uri(), vec![MESSAGE_INBOUND], None, None),
            webhook_config(&s2.uri(), vec![MESSAGE_INBOUND], None, None),
        ];
        dispatch_direct(webhooks, test_event()).await;

        s1.verify().await;
        s2.verify().await;
    }

    // ── EventBus 集成测试 ──

    #[tokio::test]
    async fn test_eventbus_integration() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock_server)
            .await;

        let event_bus = Arc::new(crate::bus::EventBus::new());
        let webhooks = vec![webhook_config(
            &mock_server.uri(),
            vec![MESSAGE_INBOUND],
            None,
            None,
        )];

        WebhookDispatcher::start(event_bus.clone(), webhooks);

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        event_bus.publish(test_event());
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        let reqs = mock_server.received_requests().await.unwrap_or_default();
        assert!(
            !reqs.is_empty(),
            "Integration: webhook should have received event via EventBus"
        );
    }

    #[tokio::test]
    async fn test_empty_webhooks_is_noop() {
        let event_bus = Arc::new(crate::bus::EventBus::new());
        WebhookDispatcher::start(event_bus, vec![]);
    }
}
