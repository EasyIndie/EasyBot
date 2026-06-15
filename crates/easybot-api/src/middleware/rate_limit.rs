//! 速率限制中间件
//!
//! 基于 IP 的滑动窗口速率限制器。使用 DashMap + VecDeque 实现，
//! 无需额外依赖。支持可配置的每分钟请求数和突发大小。

use axum::{
    extract::{ConnectInfo, State},
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use crate::response::ApiError;
use easybot_core::types::error::GatewayError;

/// 速率限制配置
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// 是否启用
    pub enabled: bool,
    /// 每分钟允许的请求数
    pub requests_per_minute: u64,
    /// 突发大小（允许的即时请求峰值）
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_minute: 60,
            burst_size: 10,
        }
    }
}

/// 斜率限制器内部状态
struct SlidingWindow {
    /// 时间戳窗口（毫秒时间戳）
    timestamps: VecDeque<i64>,
}

impl SlidingWindow {
    fn new() -> Self {
        Self {
            timestamps: VecDeque::with_capacity(1024),
        }
    }

    /// 清除窗口外的过期时间戳
    fn prune(&mut self, window_start: i64) {
        while let Some(&ts) = self.timestamps.front() {
            if ts < window_start {
                self.timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    /// 检查是否允许请求。如果允许则记录，否则返回 false。
    fn check_and_record(&mut self, max_requests: u64, burst: u32, now: i64) -> bool {
        let window_ms = 60_000; // 1 分钟窗口
        let window_start = now - window_ms;

        // 修剪过期条目
        self.prune(window_start);

        // 检查是否超过限制
        let count = self.timestamps.len() as u64;
        if count >= max_requests {
            // 如果设置了突发大小，检查近期突发
            if burst > 0 {
                let burst_window = 1_000; // 1 秒突发窗口
                let burst_start = now - burst_window;
                let burst_count = self
                    .timestamps
                    .iter()
                    .filter(|&&ts| ts >= burst_start)
                    .count() as u32;
                if burst_count >= burst {
                    return false;
                }
                // 允许突发内的请求，但限制总数量
                self.timestamps.push_back(now);
                return true;
            }
            return false;
        }

        // 记录此请求
        self.timestamps.push_back(now);
        true
    }
}

/// 速率限制器
#[derive(Clone)]
pub struct RateLimiter {
    buckets: Arc<DashMap<String, Arc<RwLock<SlidingWindow>>>>,
    config: Arc<RwLock<RateLimitConfig>>,
}

impl RateLimiter {
    /// 创建新的速率限制器
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
            config: Arc::new(RwLock::new(config)),
        }
    }

    /// 检查单个请求是否允许
    pub async fn check(&self, client_ip: &str) -> bool {
        let config = self.config.read().await;
        if !config.enabled {
            return true;
        }

        let max_requests = config.requests_per_minute;
        let burst = config.burst_size;
        drop(config);

        let now = chrono::Utc::now().timestamp_millis();

        // 获取或创建客户端的滑动窗口
        let entry = self
            .buckets
            .entry(client_ip.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(SlidingWindow::new())));

        let mut window = entry.write().await;
        window.check_and_record(max_requests, burst, now)
    }

    /// 更新配置
    pub async fn update_config(&self, config: RateLimitConfig) {
        let mut c = self.config.write().await;
        *c = config;
    }
}

/// 速率限制中间件函数
pub async fn rate_limit_middleware(
    State(rate_limiter): State<RateLimiter>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // 获取客户端 IP
    let client_ip = if let Some(forwarded) = req
        .headers()
        .get("X-Forwarded-For")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next().map(|s| s.trim().to_string()))
    {
        forwarded
    } else if let Some(addr) = req.extensions().get::<ConnectInfo<std::net::SocketAddr>>() {
        addr.0.ip().to_string()
    } else {
        "unknown".to_string()
    };

    if rate_limiter.check(&client_ip).await {
        next.run(req).await
    } else {
        warn!("Rate limit exceeded for IP: {}", client_ip);
        ApiError(GatewayError::RateLimited {
            retry_after_ms: 60000,
        })
        .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sliding_window_under_limit() {
        let mut window = SlidingWindow::new();
        let now = chrono::Utc::now().timestamp_millis();

        // 10 个非同时请求都在限制内
        for i in 0..10 {
            assert!(window.check_and_record(60, 10, now + i * 1000));
        }
    }

    #[tokio::test]
    async fn test_sliding_window_over_limit() {
        let mut window = SlidingWindow::new();

        // 60 个请求同时发送（全部在同一个 60 秒窗口内）
        for _ in 0..60 {
            assert!(window.check_and_record(60, 10, 1_000_000));
        }

        // count=60, max=60, burst 检查：60 个请求都在最近 1 秒内，burst_count=60 >= 10
        // 第 61 个应该被拒绝
        assert!(!window.check_and_record(60, 10, 1_000_001));
    }

    #[tokio::test]
    async fn test_rate_limiter_under_limit_allows_burst() {
        // RateLimiter 使用真实时间，短时间内 burst_size 控制并发
        let limiter = RateLimiter::new(RateLimitConfig {
            enabled: true,
            requests_per_minute: 10,
            burst_size: 5,
        });

        // 5 个突发请求应该都能通过（<= burst_size）
        for _ in 0..5 {
            assert!(limiter.check("10.0.0.1").await);
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_disabled() {
        let limiter = RateLimiter::new(RateLimitConfig {
            enabled: false,
            requests_per_minute: 1,
            burst_size: 1,
        });

        // 即使限制为 1/分钟，不启用时也应该全部通过
        for _ in 0..10 {
            assert!(limiter.check("127.0.0.1").await);
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_enabled() {
        let limiter = RateLimiter::new(RateLimitConfig {
            enabled: true,
            requests_per_minute: 3,
            burst_size: 3,
        });

        assert!(limiter.check("192.168.1.1").await);
        assert!(limiter.check("192.168.1.1").await);
        assert!(limiter.check("192.168.1.1").await);
        // 第 4 个请求应该被拒绝（3/分钟）
        assert!(!limiter.check("192.168.1.1").await);

        // 不同 IP 不受影响
        assert!(limiter.check("10.0.0.1").await);
    }

    #[tokio::test]
    async fn test_rate_limiter_prune() {
        let mut window = SlidingWindow::new();
        let base = chrono::Utc::now().timestamp_millis();

        // 旧请求（1 小时前）
        window.check_and_record(60, 10, base - 3_600_000);

        // 修剪窗口（这应该只保留最近 1 分钟内的请求）
        window.prune(base);

        // 检查窗口是否为空（因为旧请求已被修剪）
        assert!(
            window.timestamps.is_empty(),
            "Old timestamps should be pruned"
        );
    }
}
