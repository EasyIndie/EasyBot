//! 插件 FFI 入口宏
//!
//! 提供 `declare_plugin!` 宏，为插件适配器生成 C ABI 导出函数。
//!
//! # 安全性
//!
//! 宏生成的函数使用 `Box<Box<dyn PlatformAdapter>>` 来将 Rust 胖指针
//! （128 bits）包装为瘦指针（64 bits），以便通过 `*mut c_void` 跨 FFI 传递。
//! 主机端通过相同方式解包。

/// 当前 SDK ABI 版本
///
/// 当 `PlatformAdapter` trait 或核心类型发生不兼容变更时递增。
pub const EASYBOT_PLUGIN_ABI_VERSION: u32 = 1;

/// 声明插件 C ABI 入口点
///
/// 为插件适配器类型生成三个 `#[no_mangle] extern "C"` 函数：
///
/// - `easybot_abi_version()` — 返回 ABI 版本常量
/// - `easybot_plugin_create()` — 创建适配器实例，返回 `*mut c_void`
/// - `easybot_plugin_destroy(ptr)` — 销毁适配器实例
///
/// # 用法
///
/// ```ignore
/// use easybot_plugin_sdk::prelude::*;
///
/// struct MyAdapter { ... }
/// impl PlatformAdapter for MyAdapter { ... }
///
/// declare_plugin!(MyAdapter, MyAdapter::new);
/// ```
#[macro_export]
macro_rules! declare_plugin {
    ($plugin_type:ty, $constructor:path) => {
        #[no_mangle]
        pub extern "C" fn easybot_abi_version() -> u32 {
            $crate::EASYBOT_PLUGIN_ABI_VERSION
        }

        #[no_mangle]
        pub extern "C" fn easybot_plugin_create() -> *mut std::ffi::c_void {
            let adapter: Box<dyn $crate::PlatformAdapter> =
                Box::new($constructor());
            // Box<dyn PlatformAdapter> 是胖指针，包装一层 Box 变成瘦指针
            let boxed: Box<Box<dyn $crate::PlatformAdapter>> = Box::new(adapter);
            Box::into_raw(boxed) as *mut std::ffi::c_void
        }

        #[no_mangle]
        pub unsafe extern "C" fn easybot_plugin_destroy(
            ptr: *mut std::ffi::c_void,
        ) {
            if !ptr.is_null() {
                let inner: Box<Box<dyn $crate::PlatformAdapter>> =
                    Box::from_raw(ptr as *mut Box<dyn $crate::PlatformAdapter>);
                drop(inner);
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;

    /// 最小化测试适配器，仅实现必要方法
    struct TestAdapter {
        name: String,
        state: AdapterState,
    }

    impl TestAdapter {
        fn new() -> Self {
            Self {
                name: "test-adapter".into(),
                state: AdapterState::Created,
            }
        }
    }

    #[async_trait]
    impl PlatformAdapter for TestAdapter {
        fn platform_name(&self) -> &str { &self.name }
        fn display_name(&self) -> &str { "Test Adapter" }
        fn capabilities(&self) -> &[Capability] { &[] }

        async fn init(&mut self, _config: AdapterConfig) -> Result<InitResult, GatewayError> {
            self.state = AdapterState::Starting;
            Ok(InitResult { ok: true, error: None })
        }

        async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
            self.state = AdapterState::Connected;
            Ok(ConnectResult { ok: true, error: None, bot_info: None })
        }

        async fn disconnect(&mut self) -> Result<(), GatewayError> {
            self.state = AdapterState::Stopped;
            Ok(())
        }

        fn state(&self) -> AdapterState { self.state.clone() }

        async fn health(&self) -> HealthReport {
            HealthReport {
                status: HealthStatus::Healthy,
                connected: self.state == AdapterState::Connected,
                last_connected_at: None,
                last_error_at: None,
                last_error: None,
                messages_in: 0,
                messages_out: 0,
                errors: 0,
                uptime: None,
            }
        }

        async fn send(&self, _params: SendTextParams) -> Result<SendResult, GatewayError> {
            Ok(SendResult {
                success: true,
                message_id: Some("test-msg".into()),
                timestamp: None,
                error: None,
                error_code: None,
                retryable: false,
            })
        }

        async fn get_chat_info(&self, _chat_id: &str) -> Result<ChatInfo, GatewayError> {
            Err(GatewayError::capability_not_supported("get_chat_info"))
        }

        fn runtime_config(&self) -> AdapterRuntimeConfig {
            AdapterRuntimeConfig { enabled: true, token_configured: false, extra: serde_json::Value::Null }
        }

        fn status_summary(&self) -> AdapterStatusSummary {
            AdapterStatusSummary {
                platform: self.name.clone(),
                display_name: "Test Adapter".into(),
                state: self.state.clone(),
                connected: self.state == AdapterState::Connected,
                health: None,
                last_error: None,
                uptime: None,
                messages_in: 0,
                messages_out: 0,
            }
        }
    }

    declare_plugin!(TestAdapter, TestAdapter::new);

    /// ABI 版本常量正确
    #[test]
    fn test_abi_version_constant() {
        assert_eq!(EASYBOT_PLUGIN_ABI_VERSION, 1);
    }

    /// declare_plugin! 生成 easybot_abi_version() 返回正确的版本号
    #[test]
    fn test_declare_plugin_abi_version() {
        assert_eq!(easybot_abi_version(), EASYBOT_PLUGIN_ABI_VERSION);
    }

    /// easybot_plugin_create 返回非空指针
    #[test]
    fn test_plugin_create_returns_non_null() {
        let ptr = easybot_plugin_create();
        assert!(!ptr.is_null(), "create 必须返回非空指针");
        unsafe { easybot_plugin_destroy(ptr); }
    }

    /// easybot_plugin_create → destroy 不崩溃
    #[test]
    fn test_plugin_create_destroy_roundtrip() {
        let ptr = easybot_plugin_create();
        unsafe { easybot_plugin_destroy(ptr); }
    }

    /// 空指针 destroy 不崩溃
    #[test]
    fn test_plugin_destroy_null_pointer() {
        unsafe { easybot_plugin_destroy(std::ptr::null_mut()); }
    }

    /// 通过胖指针检查创建的对象可调用 trait 方法
    #[test]
    fn test_plugin_created_adapter_methods() {
        let ptr = easybot_plugin_create();
        assert!(!ptr.is_null());

        unsafe {
            let inner: &Box<dyn PlatformAdapter> =
                &*(ptr as *const Box<dyn PlatformAdapter>);
            assert_eq!(inner.platform_name(), "test-adapter");
            assert_eq!(inner.state(), AdapterState::Created);
        }

        unsafe { easybot_plugin_destroy(ptr); }
    }
}
