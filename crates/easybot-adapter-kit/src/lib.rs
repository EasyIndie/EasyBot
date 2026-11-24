//! # 适配器共享骨架（adapter-kit）
//!
//! 收拢 5 个适配器里重复实现的平台无关逻辑，并把 `CLAUDE.md` 的适配器约定
//! 变成可执行的契约测试。
//!
//! ## 为什么需要它
//!
//! 改造前 `crates/easybot-core/src/util.rs` 只有 1 个函数（`backoff_with_jitter`），
//! 于是每个适配器各自重写：重试计数、有界缓存、HTTP 客户端、媒体转存、鉴权重试、
//! chat_type 路由 —— 连注释都在互相引用（`adapter-feishu` 写着「参考 QQ
//! CHAT_TYPE_CACHE_LIMIT 模式」）。新增平台的成本因此是「抄 3000 行 + 记住 27 条约定」。
//!
//! ## 模块
//!
//! | 模块 | 收走的重复 |
//! |---|---|
//! | [`cache`] | 5 份 `if cache.len() > LIMIT { clear() }` |
//! | [`http`] | 5 处 `Client::builder()` + 网络错误必须 `Transient` 的约定 |
//! | [`retry`] | 5 份重连计数与重置时机 |
//! | [`conformance`] | 5 份重复的生命周期单测 + 27 条文字约定 |
//!
//! ## 用法
//!
//! ```ignore
//! use easybot_adapter_kit::{cache::BoundedCache, http, retry::RetryTracker};
//!
//! let mut admins: BoundedCache<String, Vec<i64>> = BoundedCache::new(5_000);
//! let client = http::client(std::time::Duration::from_secs(15));
//! let mut backoff = RetryTracker::new();
//! ```
//!
//! 契约测试：
//!
//! ```ignore
//! #[tokio::test]
//! async fn conformance() {
//!     easybot_adapter_kit::conformance::run_all(
//!         || Box::new(MyAdapter::new()),
//!         easybot_adapter_kit::conformance::sample_config(),
//!     )
//!     .await;
//! }
//! ```
//!
//! ## 明确不做
//!
//! 不提供 `HttpClient` / `WsClient` 这类 **I/O trait 抽象** —— 那是为可替换
//! I/O backend（wasm 宿主代管 I/O）准备的，当前只有一个 backend，多一层间接
//! 没有收益。见 Epic #140 的评估。

#![warn(missing_docs)]

pub mod cache;
pub mod conformance;
pub mod http;
pub mod retry;

pub use cache::BoundedCache;
pub use retry::RetryTracker;
