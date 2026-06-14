//! 消息持久化器
//!
//! 订阅事件总线上的入站消息事件，将消息持久化到存储层。
//! 与会话桥接器（SessionBridge）协同工作，各自专注：
//! - SessionBridge：创建/更新会话
//! - MessagePersister：持久化消息内容

use std::sync::Arc;
use tracing::{info, warn};

use crate::bus::EventBus;
use crate::storage::{MessageStore, StoredMessage};
use crate::types::message::InboundMessage;

/// 消息持久化器
///
/// 订阅 `message.inbound` 事件，将每条入站消息写入 MessageStore。
pub struct MessagePersister;

impl MessagePersister {
    /// 启动消息持久化后台任务
    pub fn start(
        event_bus: Arc<EventBus>,
        message_store: Arc<dyn MessageStore>,
    ) {
        let mut event_rx = event_bus.subscribe_many(&[
            crate::types::event::event_types::MESSAGE_INBOUND,
        ]);

        tokio::spawn(async move {
            info!("Message persister started");

            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        if let Ok(inbound) = serde_json::from_value::<InboundMessage>(event.data) {
                            let stored = StoredMessage::from_inbound(&inbound);
                            if let Err(e) = message_store.store_message(&stored).await {
                                warn!("Failed to persist inbound message: {}", e);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Message persister lagged by {} events", n);
                    }
                    Err(_) => {
                        info!("Message persister stopped (event bus closed)");
                        break;
                    }
                }
            }
        });
    }
}
