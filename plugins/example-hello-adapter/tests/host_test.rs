//! PluginTestHost 集成测试：不启动真实网关，在内存宿主里跑通
//! `attach → init → connect → send → 事件流`（DX-4 / docs/plugin-methodology.md）。
//!
//! 传输可注入方法论：真实适配器把 HTTP client 通过构造器或 `init(config)` 注入，
//! 协议交互用 wiremock 替换（测试金字塔第 3 层），宿主只负责宿主侧语义。
//!
//! 运行：cargo test

use easybot_plugin_sdk::prelude::*;
use easybot_plugin_sdk::testing::{PluginTestHost, recv_event};

#[tokio::test]
async fn lifecycle_and_send_roundtrip() {
    let host = PluginTestHost::new();
    let mut adapter = hello_adapter::HelloAdapter::new();
    host.attach(&mut adapter);
    host.init(&mut adapter, PluginTestHost::config())
        .await
        .unwrap();

    let mut rx = host.subscribe(event_types::MESSAGE_INBOUND);
    let conn = host.connect(&mut adapter).await.unwrap();
    assert!(conn.ok, "connect should succeed");

    let result = host
        .send(
            &mut adapter,
            SendTextParams {
                chat_id: "c1".into(),
                message: OutboundMessage {
                    text: "hello".into(),
                    parse_mode: ParseMode::None,
                },
                reply_to: None,
                metadata: None,
            },
        )
        .await
        .unwrap();
    assert!(result.success);

    // 发送请求被宿主记录（断言插件发出的发送参数形态正确）
    assert_eq!(host.send_log().len(), 1);
    assert_eq!(host.send_log()[0].message.text, "hello");

    // 插件在 send 内发布了 message.inbound 事件（回显文本）
    let ev = recv_event(&mut rx, std::time::Duration::from_secs(1))
        .await
        .expect("plugin should publish an inbound event on send");
    assert_eq!(ev.event_type, event_types::MESSAGE_INBOUND);
    assert_eq!(ev.source, "hello-adapter");
    assert_eq!(ev.data["text"], "hello");

    host.disconnect(&mut adapter).await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Stopped);
}
