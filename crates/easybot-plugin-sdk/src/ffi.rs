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
