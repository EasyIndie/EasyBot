//! 单元测试：平台身份、能力声明、状态（测试金字塔第 1 层，离线同步执行）。
//!
//! 运行：cargo test

use easybot_plugin_sdk::{AdapterState, CapabilityName, PlatformAdapter};
use hello_adapter::HelloAdapter;

#[test]
fn platform_identity() {
    let a = HelloAdapter::new();
    assert_eq!(a.platform_name(), "hello-adapter");
    assert_eq!(a.display_name(), "Hello Adapter");
}

#[test]
fn declares_text_capability() {
    let a = HelloAdapter::new();
    let text = a
        .capabilities()
        .iter()
        .find(|c| c.name == CapabilityName::Text)
        .expect("must declare Text capability");
    assert!(text.supported, "Text capability should be supported");
}

#[test]
fn starts_in_created_state() {
    let a = HelloAdapter::new();
    assert_eq!(a.state(), AdapterState::Created);
}
