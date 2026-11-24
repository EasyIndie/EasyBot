//! 适配器契约测试（conformance）。
//!
//! `CLAUDE.md` 里有 27 条「适配器约定」（心跳语义、`retry_transport` 覆盖规则、
//! 网络错误必须 `Transient`、缓存必须有上限、`disconnect` 必须幂等……）。
//! 这些约定目前只靠文档 + code review 保证，一旦漏掉就要等线上出问题才暴露
//! （例：进程外插件路径至今没有心跳通道，健康状态永远 `Healthy`）。
//!
//! 本模块把这些**可机器验证**的约定变成断言。用法（在每个适配器的测试里）：
//!
//! ```ignore
//! #[tokio::test]
//! async fn telegram_passes_conformance() {
//!     easybot_adapter_kit::conformance::run_all(
//!         || Box::new(TelegramAdapter::new()),
//!         easybot_adapter_kit::conformance::sample_config(),
//!     )
//!     .await;
//! }
//! ```
//!
//! 设计取舍：
//!
//! - **只断言与平台无关的语义**。平台特有的行为（QQ 的 `msg_type` 分派、飞书的
//!   `image_key` 上传）留在各自的测试里。
//! - **失败信息聚合**：一次跑完所有检查再 panic，避免「修一个跑一次」。
//! - **不触碰网络**：只做生命周期与本机状态断言，`connect`/`send` 需要真实凭据，
//!   不在本套件内。

use easybot_core::{
    AdapterConfig, AdapterState, CapabilityName, GatewayError, HealthStatus, PlatformAdapter,
};

/// 一份「看起来合法」的适配器配置：凭据非空、启用、无 base_url 覆盖。
///
/// 仅用于生命周期断言（不会真的发起请求）。平台需要额外字段时，可在
/// `extra` 里补充（例如 QQ 的 `app_id`）。
pub fn sample_config() -> AdapterConfig {
    AdapterConfig {
        enabled: Some(true),
        token: Some("conformance-token".to_string()),
        api_key: Some("conformance-app-id".to_string()),
        base_url: None,
        extra: serde_json::json!({}),
    }
}

/// 在 [`sample_config`] 基础上写入平台特有的 `extra` 字段。
///
/// 飞书 / QQ 需要 `extra.app_id`，微信需要 `extra.bot_token`；其余适配器用它
/// 覆盖凭据来源即可。
pub fn sample_config_with_extra(extra: serde_json::Value) -> AdapterConfig {
    let mut cfg = sample_config();
    cfg.extra = extra;
    cfg
}

/// 跑完整套契约检查。失败时 panic，并一次性列出**所有**不满足的条目。
///
/// `factory` 每次调用都必须返回一个**全新**适配器实例（检查之间不共享状态）；
/// `sample` 用于 `init()`。
pub async fn run_all<F>(factory: F, sample: AdapterConfig)
where
    F: Fn() -> Box<dyn PlatformAdapter>,
{
    let mut failures: Vec<String> = Vec::new();

    check(&mut failures, "identity", identity(&*factory()).await);
    check(
        &mut failures,
        "capabilities",
        capabilities(&*factory()).await,
    );
    check(
        &mut failures,
        "initial_state",
        initial_state(&*factory()).await,
    );
    check(
        &mut failures,
        "health_before_init",
        health_before_init(&*factory()).await,
    );
    check(
        &mut failures,
        "runtime_config_before_init",
        runtime_config_before_init(&*factory()).await,
    );
    check(
        &mut failures,
        "init_is_idempotent",
        init_is_idempotent(factory(), sample.clone()).await,
    );
    check(
        &mut failures,
        "init_marks_enabled",
        init_marks_enabled(factory(), sample.clone()).await,
    );
    check(
        &mut failures,
        "disconnect_is_idempotent",
        disconnect_is_idempotent(factory()).await,
    );
    check(
        &mut failures,
        "retry_transport_is_safe_before_connect",
        retry_transport_is_safe_before_connect(factory()).await,
    );

    assert!(
        failures.is_empty(),
        "适配器未通过 conformance（{} 项失败）：\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn check(failures: &mut Vec<String>, name: &str, result: Result<(), String>) {
    match result {
        Ok(()) => tracing::debug!(check = name, "conformance ok"),
        Err(msg) => failures.push(format!("  ✗ {name}: {msg}")),
    }
}

/// `platform_name()` 参与会话 key（`platform:chatId[:threadId]`），因此不得为空、
/// 不得含分隔符 `:`、不得含空白；`display_name()` 用于 UI，不得为空。
async fn identity(adapter: &dyn PlatformAdapter) -> Result<(), String> {
    let name = adapter.platform_name();
    if name.is_empty() {
        return Err("platform_name() 为空".into());
    }
    if name.contains(':') {
        return Err(format!(
            "platform_name() 含 ':'，会破坏会话 key 约定: {name:?}"
        ));
    }
    if name.chars().any(char::is_whitespace) {
        return Err(format!("platform_name() 含空白: {name:?}"));
    }
    if adapter.display_name().trim().is_empty() {
        return Err("display_name() 为空".into());
    }
    Ok(())
}

/// 能力声明：非空、无重复项，且必须声明 `Text`（当前所有适配器都支持文本）。
async fn capabilities(adapter: &dyn PlatformAdapter) -> Result<(), String> {
    let caps = adapter.capabilities();
    if caps.is_empty() {
        return Err("capabilities() 为空".into());
    }
    let mut seen: Vec<&CapabilityName> = Vec::with_capacity(caps.len());
    for cap in caps {
        if seen.contains(&&cap.name) {
            return Err(format!("capabilities() 存在重复项: {:?}", cap.name));
        }
        seen.push(&cap.name);
    }
    if !caps
        .iter()
        .any(|c| c.name == CapabilityName::Text && c.supported)
    {
        return Err("capabilities() 未声明支持的 Text 能力".into());
    }
    Ok(())
}

/// 初始状态必须是 `Created`，且 `status_summary()` 与身份信息一致。
async fn initial_state(adapter: &dyn PlatformAdapter) -> Result<(), String> {
    if adapter.state() != AdapterState::Created {
        return Err(format!(
            "初始 state 应为 Created，实际 {:?}",
            adapter.state()
        ));
    }
    if adapter.is_connected() {
        return Err("未连接却报告 is_connected() = true".into());
    }
    let summary = adapter.status_summary();
    if summary.platform != adapter.platform_name() {
        return Err(format!(
            "status_summary.platform ({}) 与 platform_name() ({}) 不一致",
            summary.platform,
            adapter.platform_name()
        ));
    }
    if summary.display_name != adapter.display_name() {
        return Err("status_summary.display_name 与 display_name() 不一致".into());
    }
    if summary.connected {
        return Err("status_summary.connected 在未连接时为 true".into());
    }
    match summary.health {
        Some(HealthStatus::Down) => Ok(()),
        // 前端依赖 health 非 None；未连接时语义必须是 Down。
        other => Err(format!(
            "status_summary.health 应为 Some(Down)，实际 {other:?}"
        )),
    }
}

/// `health()` 在 init/connect 之前必须报告 `Down` + `!connected`。
async fn health_before_init(adapter: &dyn PlatformAdapter) -> Result<(), String> {
    let report = adapter.health().await;
    if report.status != HealthStatus::Down {
        return Err(format!(
            "init 前 health.status 应为 Down，实际 {:?}",
            report.status
        ));
    }
    if report.connected {
        return Err("init 前 health.connected 应为 false".into());
    }
    Ok(())
}

/// init 之前 `runtime_config().enabled` 必须为 false（避免「没初始化就显示启用」）。
async fn runtime_config_before_init(adapter: &dyn PlatformAdapter) -> Result<(), String> {
    let cfg = adapter.runtime_config();
    if cfg.enabled {
        return Err("init 前 runtime_config().enabled 应为 false".into());
    }
    Ok(())
}

/// `init()` 必须幂等：重复调用不报错、不产生第二份连接/子进程。
///
/// 该约定来自 v0.0.42 的真实故障：`IpcPluginAdapter::init` 非幂等，
/// 被 `AdapterManager::start()` 调两次会替换子进程，导致插件永远无法启动。
async fn init_is_idempotent(
    mut adapter: Box<dyn PlatformAdapter>,
    sample: AdapterConfig,
) -> Result<(), String> {
    adapter
        .init(sample.clone())
        .await
        .map_err(|e| format!("首次 init 失败: {e}"))?;
    adapter
        .init(sample)
        .await
        .map_err(|e| format!("重复 init 失败（必须幂等）: {e}"))?;
    Ok(())
}

/// init 之后 `runtime_config().enabled` 必须为 true。
async fn init_marks_enabled(
    mut adapter: Box<dyn PlatformAdapter>,
    sample: AdapterConfig,
) -> Result<(), String> {
    adapter
        .init(sample)
        .await
        .map_err(|e| format!("init 失败: {e}"))?;
    if !adapter.runtime_config().enabled {
        return Err("init 后 runtime_config().enabled 应为 true".into());
    }
    Ok(())
}

/// `disconnect()` 必须幂等：未连接时调用不 panic，连续两次均成功，最终为 `Stopped`。
async fn disconnect_is_idempotent(mut adapter: Box<dyn PlatformAdapter>) -> Result<(), String> {
    adapter
        .disconnect()
        .await
        .map_err(|e| format!("未连接时 disconnect 失败: {e}"))?;
    if adapter.state() != AdapterState::Stopped {
        return Err(format!(
            "disconnect 后 state 应为 Stopped，实际 {:?}",
            adapter.state()
        ));
    }
    adapter
        .disconnect()
        .await
        .map_err(|e| format!("重复 disconnect 失败（必须幂等）: {e}"))?;
    if adapter.state() != AdapterState::Stopped {
        return Err(format!(
            "重复 disconnect 后 state 应为 Stopped，实际 {:?}",
            adapter.state()
        ));
    }
    if adapter.is_connected() {
        return Err("disconnect 后仍报告 is_connected() = true".into());
    }
    Ok(())
}

/// `retry_transport()` 在未连接时也必须安全（不得 panic）。
///
/// 返回值允许 `Ok(false)`（回退到完整重连，默认实现）或 `Ok(true)`
/// （含鉴权的适配器跳过鉴权直接重启），不作断言；只要求不 panic、
/// 且错误类型是 `GatewayError`。
async fn retry_transport_is_safe_before_connect(
    mut adapter: Box<dyn PlatformAdapter>,
) -> Result<(), String> {
    match adapter.retry_transport().await {
        Ok(_) => Ok(()),
        Err(GatewayError::AdapterNotConnected(_))
        | Err(GatewayError::Transient(_))
        | Err(GatewayError::Internal(_)) => Ok(()),
        Err(other) => Err(format!(
            "未连接时 retry_transport 返回了意外的错误类型: {} ({other})",
            other.error_code()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use easybot_core::{Capability, HealthReport, InitResult};

    /// 一个刻意写错的最小适配器：用于验证套件**能**抓到问题。
    struct BadAdapter {
        state: AdapterState,
        configured: bool,
    }

    impl BadAdapter {
        fn new() -> Self {
            Self {
                state: AdapterState::Created,
                configured: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl PlatformAdapter for BadAdapter {
        fn platform_name(&self) -> &str {
            "bad:adapter" // 含 ':' → 应被 identity 抓住
        }
        fn display_name(&self) -> &str {
            ""
        }
        fn capabilities(&self) -> &[Capability] {
            const CAPS: &[Capability] = &[];
            CAPS
        }
        async fn init(&mut self, _config: AdapterConfig) -> Result<InitResult, GatewayError> {
            self.configured = true;
            Ok(InitResult {
                ok: true,
                error: None,
            })
        }
        async fn connect(&mut self) -> Result<easybot_core::ConnectResult, GatewayError> {
            Err(GatewayError::AdapterNotConnected("bad".into()))
        }
        async fn disconnect(&mut self) -> Result<(), GatewayError> {
            self.state = AdapterState::Stopped;
            Ok(())
        }
        fn state(&self) -> AdapterState {
            self.state.clone()
        }
        async fn health(&self) -> HealthReport {
            HealthReport {
                status: HealthStatus::Healthy, // init 前应为 Down → 应被抓
                connected: true,
                last_connected_at: None,
                last_error_at: None,
                last_error: None,
                messages_in: 0,
                messages_out: 0,
                errors: 0,
                uptime: None,
            }
        }
        async fn send(
            &self,
            _params: easybot_core::SendTextParams,
        ) -> Result<easybot_core::SendResult, GatewayError> {
            Err(GatewayError::CapabilityNotSupported("send".into()))
        }
        async fn get_chat_info(
            &self,
            _chat_id: &str,
        ) -> Result<easybot_core::ChatInfo, GatewayError> {
            Err(GatewayError::CapabilityNotSupported("get_chat_info".into()))
        }
        fn status_summary(&self) -> easybot_core::AdapterStatusSummary {
            easybot_core::AdapterStatusSummary {
                platform: self.platform_name().to_string(),
                display_name: self.display_name().to_string(),
                state: self.state.clone(),
                connected: false,
                health: Some(HealthStatus::Healthy),
                last_error: None,
                uptime: None,
                messages_in: 0,
                messages_out: 0,
            }
        }
        fn runtime_config(&self) -> easybot_core::AdapterRuntimeConfig {
            easybot_core::AdapterRuntimeConfig {
                enabled: self.configured,
                token_configured: false,
                extra: serde_json::json!({}),
            }
        }
    }

    #[tokio::test]
    #[should_panic(expected = "适配器未通过 conformance")]
    async fn suite_rejects_violations() {
        run_all(|| Box::new(BadAdapter::new()), sample_config()).await;
    }
}
