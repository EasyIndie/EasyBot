//! tracing 日志系统初始化辅助。
//!
//! 控制台/文件输出（`fmt` 层）与管理后台 `/api/v1/logs` 内存环形缓冲
//! （`LogCollector` 层）**必须共用同一过滤规则**。此前 `LogCollector` 层没有挂
//! 过滤器，因而接收并保存所有 target 的 `TRACE`/`DEBUG` 事件（`hyper` /
//! `sqlx` / `tower_http` / `axum` / `rustls` 等），造成两个后果：
//!
//! 1. 环形缓冲（上限 5000 条）被依赖库内部噪音占满，业务与插件日志被挤掉，
//!    管理后台「日志」页几乎不可用；
//! 2. `tracing` 的全局最大级别被抬到 `TRACE`——未过滤的层不会对最大级别提供
//!    任何约束，于是所有 `trace!`/`debug!` 宏（包括依赖里的）都会被真实构造与
//!    记录，带来不必要的 CPU 与内存开销。
//!
//! 过滤优先级：`RUST_LOG`（存在且非空）> `easybot=<配置级别>`。
//! `docs/16`、`docs/17` 承诺 `RUST_LOG=<target>=<level>` 可按 target 细分，
//! 因此这里读取 `RUST_LOG`，而不是只认配置里的级别。

use tracing_subscriber::EnvFilter;

/// 构建控制台/文件与内存收集器共用的过滤规则。
///
/// - `log_level`：来自配置 `logging.level` 或 CLI `--debug`；
/// - `rust_log`：调用方传入的 `RUST_LOG`（显式传参便于单测注入，避免直接读环境变量）。
pub fn build_filter(log_level: &str, rust_log: Option<&str>) -> EnvFilter {
    match rust_log.map(str::trim).filter(|value| !value.is_empty()) {
        Some(directives) => EnvFilter::new(directives),
        None => EnvFilter::new(format!("easybot={log_level}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use easybot_api::log_collector::LogCollector;
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;

    /// 用给定过滤器把事件送进内存收集器，返回实际被保留的 message。
    fn captured(filter: EnvFilter, emit: impl FnOnce()) -> Vec<String> {
        let collector = LogCollector::new(64);
        let subscriber = tracing_subscriber::registry().with(collector.clone().with_filter(filter));
        tracing::subscriber::with_default(subscriber, emit);
        collector
            .query(None, None, 64, None)
            .into_iter()
            .map(|entry| entry.message)
            .collect()
    }

    #[test]
    fn collector_only_keeps_matching_target_and_level() {
        // 配置 info：保留 easybot* 的 INFO，丢弃 easybot 的 DEBUG 与依赖库噪音。
        let kept = captured(build_filter("info", None), || {
            tracing::info!(target: "easybot_core::test", "kept-app-log");
            tracing::debug!(target: "easybot_core::test", "dropped-debug");
            tracing::info!(target: "sqlx::query", "dropped-dependency-noise");
        });
        assert_eq!(kept, vec!["kept-app-log".to_string()]);
    }

    #[test]
    fn explicit_rust_log_takes_over() {
        // RUST_LOG=my_adapter=trace：只采集该 target，配置里的 easybot 级别被覆盖。
        let kept = captured(build_filter("info", Some("my_adapter=trace")), || {
            tracing::trace!(target: "my_adapter::ws", "plugin-detail");
            tracing::info!(target: "easybot_core::test", "hidden-by-rust-log");
        });
        assert_eq!(kept, vec!["plugin-detail".to_string()]);
    }

    #[test]
    fn blank_rust_log_falls_back_to_config_level() {
        let kept = captured(build_filter("info", Some("   ")), || {
            tracing::info!(target: "easybot_core::test", "kept-app-log");
        });
        assert_eq!(kept, vec!["kept-app-log".to_string()]);
    }
}
