//! 事件总线实现
//!
//! 使用 DashMap 存储按事件类型区分的 broadcast channel。
//! 支持 publish/subscribe 模式，事件发布后所有活跃订阅者收到副本。

use std::time::Duration;
use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::warn;
use crate::types::event::GatewayEvent;

/// 默认广播通道容量
const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// 合并循环空闲时的休眠时间（避免 busy loop）
const MERGE_POLL_INTERVAL_MS: u64 = 10;

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
    pub fn publish(&self, event: GatewayEvent) {
        let event_type = event.event_type.clone();
        if let Some(tx) = self.channels.get(&event_type) {
            let _ = tx.send(event);
        }
    }

    /// 订阅特定事件类型
    ///
    /// 返回一个 broadcast::Receiver，每次收到事件会唤醒所有接收者。
    /// 如果落后太多（通道满），接收者会跳过旧事件。
    pub fn subscribe(&self, event_type: &str) -> broadcast::Receiver<GatewayEvent> {
        self.get_or_create_channel(event_type).subscribe()
    }

    /// 获取或创建 broadcast channel
    fn get_or_create_channel(
        &self,
        event_type: &str,
    ) -> broadcast::Sender<GatewayEvent> {
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
    /// 创建一个合并的接收器，订阅列表中所有事件类型。
    /// 使用单个后台任务轮询所有 channel，避免为每个事件类型 spawn 独立 task。
    pub fn subscribe_many(&self, event_types: &[&str]) -> broadcast::Receiver<GatewayEvent> {
        let (global_tx, global_rx) = broadcast::channel(self.capacity);

        let mut receivers: Vec<broadcast::Receiver<GatewayEvent>> = event_types
            .iter()
            .map(|et| self.subscribe(et))
            .collect();

        tokio::spawn(async move {
            loop {
                // 尝试从所有 receiver 读取可用事件
                let mut had_data = false;
                for i in (0..receivers.len()).rev() {
                    match receivers[i].try_recv() {
                        Ok(event) => {
                            had_data = true;
                            if global_tx.send(event).is_err() {
                                return; // 下游 receiver 已 drop
                            }
                        }
                        Err(broadcast::error::TryRecvError::Closed) => {
                            receivers.swap_remove(i);
                        }
                        Err(broadcast::error::TryRecvError::Lagged(n)) => {
                            warn!("EventBus merge lagged by {} events", n);
                            had_data = true;
                        }
                        Err(broadcast::error::TryRecvError::Empty) => {}
                    }
                }

                if receivers.is_empty() {
                    break;
                }

                if had_data {
                    tokio::task::yield_now().await;
                } else {
                    tokio::time::sleep(Duration::from_millis(MERGE_POLL_INTERVAL_MS)).await;
                }
            }
        });

        global_rx
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

    #[tokio::test]
    async fn test_publish_subscribe() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe("test.event");

        let event = GatewayEvent::new("test.event", "test", serde_json::json!({"key": "value"}));
        bus.publish(event);

        let received = tokio::time::timeout(
            tokio::time::Duration::from_secs(1),
            rx.recv(),
        )
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
        let bus = EventBus::new();
        let mut rx = bus.subscribe_many(&["event.a", "event.b"]);

        tokio::time::sleep(Duration::from_millis(100)).await;

        bus.publish(GatewayEvent::new("event.a", "test", serde_json::json!({"n": 1})));
        bus.publish(GatewayEvent::new("event.b", "test", serde_json::json!({"n": 2})));

        // 两个事件都应收到
        let e1 = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
        assert!(e1.is_ok(), "subscribe_many should receive event.a");
        let e2 = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await;
        assert!(e2.is_ok(), "subscribe_many should receive event.b");
    }

    #[tokio::test]
    async fn test_subscribe_many_dropped_receiver_stops_task() {
        let bus = Arc::new(EventBus::new());
        let rx = bus.subscribe_many(&["test.event"]);
        drop(rx); // 丢弃接收器，后台任务应退出

        // 给后台任务一点时间清理
        tokio::time::sleep(Duration::from_millis(200)).await;

        // 后台任务已退出，此发布仅用于验证无 panic
        bus.publish(GatewayEvent::new("test.event", "test", serde_json::json!({})));
    }

    #[tokio::test]
    async fn test_broadcast_full_lags_slow_receiver() {
        // broadcast channel 满时覆盖最旧消息，慢消费者收到 Lagged 错误
        let bus = EventBus::with_capacity(2);
        let mut rx = bus.subscribe("test.event");

        // 发送 3 个事件，容量仅 2，第 1 个被覆盖
        bus.publish(GatewayEvent::new("test.event", "test", serde_json::json!({"seq": 1})));
        bus.publish(GatewayEvent::new("test.event", "test", serde_json::json!({"seq": 2})));
        bus.publish(GatewayEvent::new("test.event", "test", serde_json::json!({"seq": 3})));

        // 慢消费者 recv 会先收到 Lagged(1)，然后读到最新消息
        let first = rx.recv().await;
        match first {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                assert!(n >= 1, "should report at least 1 lagged message, got {}", n);
            }
            Ok(e) => {
                // 在某些执行顺序下可能直接收到 seq=3（如果 seq=1 在 write 之前就被覆盖）
                panic!("expected Lagged error but got event seq={:?}", e.data["seq"]);
            }
            Err(e) => {
                panic!("unexpected error: {:?}", e);
            }
        }
    }
}
