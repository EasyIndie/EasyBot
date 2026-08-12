//! 出站平台 HTTP 通用机制：access token 缓存 + 刷新重试。
//!
//! QQ / 飞书等平台适配器持有短时 access_token（约 7200s），需要：
//! - 缓存复用（避免每次请求都去鉴权端点取 token）
//! - **单飞行刷新**（并发请求共享一次刷新，避免惊群）
//! - **token 被拒 → 强制刷新 → 重试一次**（服务端可能提前失效 token）
//!
//! 平台特有部分由适配器提供：
//! - `TokenFetcher`：调用平台鉴权端点，返回 `(token, ttl_secs)` 或错误
//! - `is_token_invalid` 谓词：判定某个 API 错误是否为"token 无效/过期"
//!
//! 本模块只提供机制，不感知任何平台业务码（11244 / 99991663 等）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;

use crate::types::error::GatewayError;

/// 平台 token 获取闭包：返回 `(token, ttl_secs)` 或错误。
pub type TokenFetcher =
    Box<dyn Fn() -> BoxFuture<'static, Result<(String, u64), GatewayError>> + Send + Sync>;

/// access token 缓存 + 单飞行刷新。
///
/// `margin`：距过期不足该时长即视为需要刷新（各平台对齐官方刷新窗口，
/// 如 QQ 60s、飞书 300s）。
#[derive(Clone)]
pub struct CachedTokenStore {
    state: Arc<Mutex<Inner>>,
    /// 单飞行锁：同一时刻至多一个刷新在途，其余调用者等待后复用结果
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    fetch: Arc<TokenFetcher>,
    margin: Duration,
}

struct Inner {
    token: Option<String>,
    expires_at: Option<Instant>,
}

impl CachedTokenStore {
    /// `fetch` 返回 `(token, ttl_secs)`，`ttl_secs` 由平台鉴权端点给出。
    pub fn new<F>(fetch: F, margin: Duration) -> Self
    where
        F: Fn() -> BoxFuture<'static, Result<(String, u64), GatewayError>> + Send + Sync + 'static,
    {
        Self {
            state: Arc::new(Mutex::new(Inner {
                token: None,
                expires_at: None,
            })),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            fetch: Arc::new(Box::new(fetch)),
            margin,
        }
    }

    /// 返回有效 token；缺失或距过期不足 `margin` 时自动刷新（单飞行）。
    pub async fn get(&self) -> Result<String, GatewayError> {
        if let Some(token) = self.cached_token() {
            return Ok(token);
        }
        self.refresh_inner(false).await
    }

    /// 无条件刷新（token 被平台拒绝后调用）。单飞行——并发调用者串行化，
    /// 各自发起一次刷新（不合并），但不会同时打鉴权端点。
    pub async fn force_refresh(&self) -> Result<String, GatewayError> {
        self.refresh_inner(true).await
    }

    /// 同步检查是否需要刷新（空 / 已过期 / 距过期不足 margin）。
    pub fn needs_refresh(&self) -> bool {
        self.cached_token().is_none()
    }

    /// 同步读取缓存中的有效 token；无效 / 缺失返回 None。
    fn cached_token(&self) -> Option<String> {
        let guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let inner = &*guard;
        let token = inner.token.as_ref()?;
        let expires_at = inner.expires_at?;
        let now = Instant::now();
        if now >= expires_at {
            return None;
        }
        if expires_at - now <= self.margin {
            return None;
        }
        Some(token.clone())
    }

    async fn refresh_inner(&self, force: bool) -> Result<String, GatewayError> {
        // 单飞行：同时只允许一个刷新在途
        let _guard = self.refresh_lock.lock().await;

        // 非强制刷新时双重检查：等待锁期间可能已有其他调用者刷新完成
        if !force && let Some(token) = self.cached_token() {
            return Ok(token);
        }

        let (token, ttl_secs) = (self.fetch)().await?;
        let expires_at = Instant::now() + Duration::from_secs(ttl_secs);
        {
            let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            guard.token = Some(token.clone());
            guard.expires_at = Some(expires_at);
        }
        Ok(token)
    }
}

/// 获取 token → 发起请求 → 若返回"token 无效/过期"错误则**强制刷新一次** → 再请求一次。
///
/// - 恰好一次刷新、至多两次请求，**无无限重试**。
/// - `is_token_invalid`：平台谓词，判定错误是否为"token 无效/过期"。
/// - `request`：以 token 为参数发起实际请求。闭包可借用适配器与请求体
///   （不克隆大 body，如 QQ 媒体上传），`'a` 生命周期覆盖整个调用。
pub async fn send_with_token_retry<'a, T, P, F>(
    store: &'a CachedTokenStore,
    is_token_invalid: P,
    request: F,
) -> Result<T, GatewayError>
where
    P: Fn(&GatewayError) -> bool,
    F: Fn(String) -> BoxFuture<'a, Result<T, GatewayError>>,
    T: Send + 'a,
{
    let token = store.get().await?;
    match request(token).await {
        Ok(value) => Ok(value),
        Err(e) if is_token_invalid(&e) => {
            tracing::warn!("平台 API 返回 token 无效/过期错误，强制刷新 token 后重试一次");
            let new_token = match store.force_refresh().await {
                Ok(t) => t,
                Err(refresh_err) => {
                    // 刷新失败：用被拒绝的旧 token 重试无意义，返回原始请求错误
                    tracing::warn!("token 刷新失败：{}，返回原始请求错误", refresh_err);
                    return Err(e);
                }
            };
            request(new_token).await
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 构造一个返回固定 token 的 fetch 闭包（每次调用计数）。
    fn counting_fetch(calls: Arc<AtomicU64>, token: &'static str, ttl: u64) -> TokenFetcher {
        Box::new(move || {
            let c = calls.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok((token.to_string(), ttl))
            })
        })
    }

    #[tokio::test]
    async fn get_returns_cached_without_refetch_when_fresh() {
        let calls = Arc::new(AtomicU64::new(0));
        let store = CachedTokenStore::new(
            counting_fetch(calls.clone(), "tok-1", 3600),
            Duration::from_secs(60),
        );

        let t1 = store.get().await.unwrap();
        assert_eq!(t1, "tok-1");
        let t2 = store.get().await.unwrap();
        assert_eq!(t2, "tok-1");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fresh token 不应触发二次 fetch"
        );
    }

    #[tokio::test]
    async fn get_fetches_when_empty() {
        let calls = Arc::new(AtomicU64::new(0));
        let store = CachedTokenStore::new(
            counting_fetch(calls.clone(), "tok-1", 3600),
            Duration::from_secs(60),
        );

        assert_eq!(store.get().await.unwrap(), "tok-1");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn get_refetches_when_within_margin() {
        // ttl == margin：刷新后立即再次视为临界 → 每次 get 都刷新
        let calls = Arc::new(AtomicU64::new(0));
        let store = CachedTokenStore::new(
            counting_fetch(calls.clone(), "tok-1", 60),
            Duration::from_secs(60),
        );

        store.get().await.unwrap();
        store.get().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "margin 内应触发刷新");
    }

    #[tokio::test]
    async fn concurrent_get_single_flight_fetches_once() {
        let calls = Arc::new(AtomicU64::new(0));
        let calls_for_fetch = calls.clone();
        let store = Arc::new(CachedTokenStore::new(
            move || -> BoxFuture<'static, Result<(String, u64), GatewayError>> {
                let c = calls_for_fetch.clone();
                Box::pin(async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    // 模拟较慢的鉴权请求，放大并发窗口
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(("tok-sf".to_string(), 3600))
                })
            },
            Duration::from_secs(60),
        ));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let s = store.clone();
            handles.push(tokio::spawn(async move { s.get().await.unwrap() }));
        }
        let mut tokens = Vec::new();
        for h in handles {
            tokens.push(h.await.unwrap());
        }
        assert!(tokens.iter().all(|t| t == "tok-sf"));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "并发 get 应共享单次刷新");
    }

    #[tokio::test]
    async fn force_refresh_always_fetches() {
        let calls = Arc::new(AtomicU64::new(0));
        let store = CachedTokenStore::new(
            counting_fetch(calls.clone(), "tok-1", 3600),
            Duration::from_secs(60),
        );

        store.force_refresh().await.unwrap();
        store.force_refresh().await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "force_refresh 每次都应刷新"
        );
    }

    #[tokio::test]
    async fn get_propagates_fetch_error_when_no_cache() {
        let store = CachedTokenStore::new(
            || -> BoxFuture<'static, Result<(String, u64), GatewayError>> {
                Box::pin(async {
                    Err::<(String, u64), _>(GatewayError::Transient("network down".into()))
                })
            },
            Duration::from_secs(60),
        );
        let err = store.get().await.unwrap_err();
        assert!(matches!(err, GatewayError::Transient(_)));
    }

    #[test]
    fn needs_refresh_true_when_empty_and_false_when_fresh() {
        let empty = CachedTokenStore::new(
            || -> BoxFuture<'static, Result<(String, u64), GatewayError>> {
                Box::pin(async { Ok(("tok-1".to_string(), 3600)) })
            },
            Duration::from_secs(60),
        );
        assert!(empty.needs_refresh(), "空 store 应需要刷新");
    }

    #[tokio::test]
    async fn needs_refresh_false_after_get() {
        let store = CachedTokenStore::new(
            || -> BoxFuture<'static, Result<(String, u64), GatewayError>> {
                Box::pin(async { Ok(("tok-1".to_string(), 3600)) })
            },
            Duration::from_secs(60),
        );
        store.get().await.unwrap();
        assert!(!store.needs_refresh(), "拿到新鲜 token 后不应需要刷新");
    }

    // ── send_with_token_retry ──

    #[tokio::test]
    async fn retry_success_on_first_try() {
        let calls = Arc::new(AtomicU64::new(0));
        let store = CachedTokenStore::new(
            counting_fetch(calls.clone(), "t1", 3600),
            Duration::from_secs(60),
        );
        let req_calls = Arc::new(AtomicU64::new(0));

        let result = send_with_token_retry(&store, |_| false, {
            let rc = req_calls.clone();
            move |token: String| {
                let rc = rc.clone();
                Box::pin(async move {
                    rc.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(token, "t1");
                    Ok::<String, GatewayError>("ok".to_string())
                })
            }
        })
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(req_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "首次成功不应触发 force_refresh"
        );
    }

    #[tokio::test]
    async fn retry_refreshes_and_succeeds_on_second_try() {
        // fetch #1 → token-1；fetch #2（force）→ token-2
        let fetch_n = Arc::new(AtomicU64::new(0));
        let fn_c = fetch_n.clone();
        let store = CachedTokenStore::new(
            move || -> BoxFuture<'static, Result<(String, u64), GatewayError>> {
                let n = fn_c.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    let tok = if n == 0 { "token-1" } else { "token-2" };
                    Ok((tok.to_string(), 3600))
                })
            },
            Duration::from_secs(60),
        );

        let result =
            send_with_token_retry(&store, |e| matches!(e, GatewayError::Unauthorized(_)), {
                move |token: String| {
                    Box::pin(async move {
                        if token == "token-1" {
                            Err::<String, _>(GatewayError::Unauthorized("token invalid".into()))
                        } else {
                            Ok::<String, _>("sent".to_string())
                        }
                    })
                }
            })
            .await;

        assert_eq!(result.unwrap(), "sent");
        assert_eq!(
            fetch_n.load(Ordering::SeqCst),
            2,
            "get 刷新 1 次 + force_refresh 1 次"
        );
    }

    #[tokio::test]
    async fn retry_gives_up_after_second_token_invalid() {
        let calls = Arc::new(AtomicU64::new(0));
        let store = CachedTokenStore::new(
            counting_fetch(calls.clone(), "token-x", 3600),
            Duration::from_secs(60),
        );
        let req_calls = Arc::new(AtomicU64::new(0));

        let result =
            send_with_token_retry(&store, |e| matches!(e, GatewayError::Unauthorized(_)), {
                let rc = req_calls.clone();
                move |_token: String| {
                    let rc = rc.clone();
                    Box::pin(async move {
                        rc.fetch_add(1, Ordering::SeqCst);
                        Err::<String, _>(GatewayError::Unauthorized("token invalid".into()))
                    })
                }
            })
            .await;

        assert!(matches!(result, Err(GatewayError::Unauthorized(_))));
        assert_eq!(
            req_calls.load(Ordering::SeqCst),
            2,
            "至多两次请求，无死循环"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2, "get 刷新 + force_refresh");
    }

    #[tokio::test]
    async fn retry_no_refresh_on_non_token_error() {
        let calls = Arc::new(AtomicU64::new(0));
        let store = CachedTokenStore::new(
            counting_fetch(calls.clone(), "t1", 3600),
            Duration::from_secs(60),
        );

        let result = send_with_token_retry(&store, |_| false, {
            move |_token: String| {
                Box::pin(async { Err::<String, _>(GatewayError::Internal("other".into())) })
            }
        })
        .await;

        assert!(matches!(result, Err(GatewayError::Internal(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "非 token 错误不应触发刷新");
    }

    #[tokio::test]
    async fn retry_propagates_refresh_error_without_second_request() {
        // fetch #1 → 成功（token-1）；fetch #2（force）→ 失败
        let fetch_n = Arc::new(AtomicU64::new(0));
        let fn_c = fetch_n.clone();
        let store = CachedTokenStore::new(
            move || -> BoxFuture<'static, Result<(String, u64), GatewayError>> {
                let n = fn_c.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if n == 0 {
                        Ok(("token-1".to_string(), 3600))
                    } else {
                        Err::<(String, u64), _>(GatewayError::Transient("refresh failed".into()))
                    }
                })
            },
            Duration::from_secs(60),
        );
        let req_calls = Arc::new(AtomicU64::new(0));

        let result =
            send_with_token_retry(&store, |e| matches!(e, GatewayError::Unauthorized(_)), {
                let rc = req_calls.clone();
                move |_token: String| {
                    let rc = rc.clone();
                    Box::pin(async move {
                        rc.fetch_add(1, Ordering::SeqCst);
                        Err::<String, _>(GatewayError::Unauthorized("token invalid".into()))
                    })
                }
            })
            .await;

        // 刷新失败 → 返回原始请求错误，不发起第二次请求
        assert!(matches!(result, Err(GatewayError::Unauthorized(_))));
        assert_eq!(req_calls.load(Ordering::SeqCst), 1);
    }
}
