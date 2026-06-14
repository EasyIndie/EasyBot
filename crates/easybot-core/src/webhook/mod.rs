//! Webhook 事件转发
//!
//! 订阅 EventBus 事件，根据配置将事件通过 HTTP POST 转发到外部 URL。
//! 支持 HMAC-SHA256 签名验证，按事件类型和平台过滤。

use std::sync::Arc;
use std::time::Duration;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tracing::{info, warn, error, trace};
use reqwest::Client;

use crate::bus::EventBus;
use crate::types::event::GatewayEvent;
use crate::types::config::WebhookConfig;

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
        let client = match Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => Arc::new(c),
            Err(e) => {
                error!("Failed to create HTTP client for webhook dispatcher: {}", e);
                return;
            }
        };

        // 收集所有需要订阅的事件类型
        let mut all_types: Vec<&str> = Vec::new();
        let mut catch_all = false; // 有 webhook 订阅了 "*"
        for wh in &webhooks {
            for event_type in &wh.events {
                if event_type == "*" {
                    catch_all = true;
                } else if !all_types.contains(&event_type.as_str()) {
                    all_types.push(event_type.as_str());
                }
            }
        }

        // 如果有 catch-all，订阅所有已知事件类型
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
                    let platform = event.data.get("platform")
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
        let payload = match serde_json::to_value(&event) {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to serialize webhook event payload: {}", e);
                return;
            }
        };
        let payload_bytes = match serde_json::to_vec(&event) {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to serialize webhook event bytes: {}", e);
                return;
            }
        };

        for wh in &webhooks {
            // 检查事件类型是否匹配
            if !wh.events.is_empty()
                && !wh.events.contains(&"*".to_string())
                && !wh.events.contains(&event_type)
            {
                continue;
            }

            // 检查平台是否匹配（如果配置了平台过滤）
            if let Some(ref platforms) = wh.platforms {
                if !platforms.is_empty() && !platforms.contains(&platform) {
                    continue;
                }
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
            let mut req = client.post(&wh.url)
                .header("Content-Type", "application/json")
                .json(&payload);

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
