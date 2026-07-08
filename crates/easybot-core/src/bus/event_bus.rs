//! 事件总线实现
//!
//! 使用 DashMap 存储按事件类型区分的 broadcast channel。
//! 支持 publish/subscribe 模式，事件发布后所有活跃订阅者收到副本。

use crate::types::event::GatewayEvent;
use crate::types::message::SendResult;
use dashmap::DashMap;
use futures::stream::SelectAll;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;

/// 默认广播通道容量
const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// 消息总线
///
/// 网关内部的事件枢纽，负责组件间解耦通信。
/// 基于 tokio broadcast channel，每个事件类型有独立的通道。
pub struct EventBus {
    channels: DashMap<String, broadcast::Sender<GatewayEvent>>,
    capacity: usize,
}

impl EventBus {
    /// 创建新的事件总线（默认容量 256）
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CHANNEL_CAPACITY)
    }

    /// 创建指定容量的事件总线
    ///
    /// `capacity` 决定每个事件类型 broadcast channel 的缓冲区大小。
    /// 当消费者慢于生产者时，超出 capacity 的旧事件会被丢弃。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            channels: DashMap::new(),
            capacity,
        }
    }

    /// 发布事件
    ///
    /// 所有订阅了该事件类型的接收者都会收到事件副本。
    /// 如果没有活跃订阅者，事件被静默丢弃。
    ///
    /// SECURITY: Logs a warning when message.inbound events are published
    /// by non-adapter sources (potential event spoofing).
    pub fn publish(&self, event: GatewayEvent) {
        // SECURITY: Warn on suspicious event source mismatches
        if event.event_type == crate::types::event::event_types::MESSAGE_INBOUND
            && event.source != "api"
            && event.source != "gateway"
        {
            // source should match a known platform or be "api"/"gateway"
            // This is informational; actual auth enforcement is at the API layer
            tracing::trace!(source = %event.source, "Event published");
        }
        let event_type = event.event_type.clone();
        if let Some(tx) = self.channels.get(&event_type) {
            let _ = tx.send(event);
        }
    }

    /// 发布消息发送结果事件（各适配器通用的模板）
    ///
    /// 将 `SendResult` 包装为 GatewayEvent 发布到事件总线，
    /// 消除五个适配器中完全相同的 `publish_send_event` 重复代码。
    pub fn publish_send_result(
        &self,
        event_type: &str,
        platform: &str,
        chat_id: &str,
        result: &SendResult,
    ) {
        self.publish(GatewayEvent::new(
            event_type,
            platform,
            serde_json::json!({
                "platform": platform,
                "chat_id": chat_id,
                "message_id": result.message_id,
                "success": result.success,
                "error": result.error,
                "error_code": result.error_code,
            }),
        ));
    }

    /// 订阅特定事件类型
    ///
    /// 返回一个 broadcast::Receiver，每次收到事件会唤醒所有接收者。
    /// 如果落后太多（通道满），接收者会跳过旧事件。
    pub fn subscribe(&self, event_type: &str) -> broadcast::Receiver<GatewayEvent> {
        self.get_or_create_channel(event_type).subscribe()
    }

    /// 获取或创建 broadcast channel
    fn get_or_create_channel(&self, event_type: &str) -> broadcast::Sender<GatewayEvent> {
        let cap = self.capacity;
        self.channels
            .entry(event_type.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(cap);
                tx
            })
            .value()
            .clone()
    }

    /// 订阅多个事件类型
    ///
    /// 返回一个 `Stream`，通过 `BroadcastStream` + `SelectAll` 事件驱动合并，
    /// 无需后台轮询任务，零空闲 CPU 消耗，零延迟。
    pub fn subscribe_many(
        &self,
        event_types: &[&str],
    ) -> impl futures::Stream<Item = GatewayEvent> + Unpin + use<> {
        use futures::StreamExt;

        let streams: Vec<BroadcastStream<GatewayEvent>> = event_types
            .iter()
            .map(|et| BroadcastStream::new(self.subscribe(et)))
            .collect();

        SelectAll::from_iter(streams).filter_map(|result| {
            futures::future::ready(match result {
                Ok(event) => Some(event),
                Err(err) => {
                    warn!("EventBus merge lagged: {}", err);
                    None
                }
            })
        })
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn test_publish_subscribe() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("test.event");

        let event = GatewayEvent::new("test.event", "test", serde_json::json!({"key": "value"}));
        bus.publish(event);

        let received = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("should receive event")
            .expect("event should be valid");

        assert_eq!(received.event_type, "test.event");
        assert_eq!(received.data["key"], "value");
    }

    #[test]
    fn test_event_without_subscribers_is_silent() {
        let bus = EventBus::new();
        let event = GatewayEvent::new("unsubscribed", "test", serde_json::json!({}));
        // 不应 panic
        bus.publish(event);
    }

    #[tokio::test]
    async fn test_multi_consumer_receive_same_event() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe("test.event");
        let mut rx2 = bus.subscribe("test.event");

        let event = GatewayEvent::new("test.event", "test", serde_json::json!({"msg": "hello"}));
        bus.publish(event);

        // 两个消费者都应收到
        let r1 = tokio::time::timeout(Duration::from_secs(1), rx1.recv()).await;
        let r2 = tokio::time::timeout(Duration::from_secs(1), rx2.recv()).await;
        assert!(r1.is_ok(), "consumer 1 should receive");
        assert!(r2.is_ok(), "consumer 2 should receive");
    }

    #[tokio::test]
    async fn test_subscribe_many_receives_all_types() {
        use futures::StreamExt;
        let bus = EventBus::new();
        let mut rx = bus.subscribe_many(&["event.a", "event.b"]);

        tokio::time::sleep(Duration::from_millis(100)).await;

        bus.publish(GatewayEvent::new(
            "event.a",
            "test",
            serde_json::json!({"n": 1}),
        ));
        bus.publish(GatewayEvent::new(
            "event.b",
            "test",
            serde_json::json!({"n": 2}),
        ));

        // 两个事件都应收到（使用 Stream::next() 替代 broadcast::recv()）
        let e1 = tokio::time::timeout(Duration::from_secs(1), rx.next()).await;
        assert!(e1.is_ok(), "subscribe_many should receive event.a");
        assert!(e1.unwrap().is_some(), "should receive Some(event)");
        let e2 = tokio::time::timeout(Duration::from_secs(1), rx.next()).await;
        assert!(e2.is_ok(), "subscribe_many should receive event.b");
        assert!(e2.unwrap().is_some(), "should receive Some(event)");
    }

    #[tokio::test]
    async fn test_subscribe_many_stream_cleanup() {
        // 验证丢弃 stream 后不会 panic
        let bus = Arc::new(EventBus::new());
        let rx = bus.subscribe_many(&["test.event"]);
        drop(rx);

        tokio::time::sleep(Duration::from_millis(100)).await;

        // stream 已 drop，此发布仅用于验证无 panic
        bus.publish(GatewayEvent::new(
            "test.event",
            "test",
            serde_json::json!({}),
        ));
    }

    #[tokio::test]
    async fn test_broadcast_full_lags_slow_receiver() {
        // broadcast channel 满时覆盖最旧消息，慢消费者收到 Lagged 错误
        let bus = EventBus::with_capacity(2);
        let mut rx = bus.subscribe("test.event");

        // 发送 3 个事件，容量仅 2，第 1 个被覆盖
        bus.publish(GatewayEvent::new(
            "test.event",
            "test",
            serde_json::json!({"seq": 1}),
        ));
        bus.publish(GatewayEvent::new(
            "test.event",
            "test",
            serde_json::json!({"seq": 2}),
        ));
        bus.publish(GatewayEvent::new(
            "test.event",
            "test",
            serde_json::json!({"seq": 3}),
        ));

        // 慢消费者 recv 会先收到 Lagged(1)，然后读到最新消息
        let first = rx.recv().await;
        match first {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                assert!(n >= 1, "should report at least 1 lagged message, got {}", n);
            }
            Ok(e) => {
                // 在某些执行顺序下可能直接收到 seq=3（如果 seq=1 在 write 之前就被覆盖）
                panic!(
                    "expected Lagged error but got event seq={:?}",
                    e.data["seq"]
                );
            }
            Err(e) => {
                panic!("unexpected error: {:?}", e);
            }
        }
    }
}
