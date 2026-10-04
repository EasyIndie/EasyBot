//! 进程外插件入口（Slack 示例）。
//!
//! 宿主把本可执行文件作为子进程启动，经 stdin/stdout 逐行 JSON 通信；
//! `run_plugin!` 负责建运行时、应答握手、驱动 `PlatformAdapter` 生命周期
//! 并把事件回传给宿主。适配器实现见 `src/lib.rs`。

use slack_plugin::SlackAdapter;

easybot_plugin_sdk::run_plugin!(SlackAdapter, SlackAdapter::new);
