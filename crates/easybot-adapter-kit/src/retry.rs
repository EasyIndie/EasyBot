//! 重试计数与退避接线。
//!
//! 5 个适配器各自维护「重连尝试次数 / 连续失败次数」的计数器，并各自把
//! 计数喂给 [`easybot_core::util::backoff_with_jitter`]。计数本身不难，难的是
//! **一致的重置时机**：成功一次就该归零，否则一次抖动会永久抬高退避上限。
//!
//! ```
//! use easybot_adapter_kit::retry::RetryTracker;
//!
//! let mut tracker = RetryTracker::new();
//! assert_eq!(tracker.attempts(), 0);
//! let delay = tracker.on_failure();   // 第一次失败 → ~1s
//! assert!(delay.as_secs_f64() >= 0.75);
//! tracker.on_success();               // 成功 → 归零
//! assert_eq!(tracker.attempts(), 0);
//! ```

use std::time::Duration;

use easybot_core::util::backoff_with_jitter;

/// 连续失败计数 + 指数退避。
///
/// 语义与 `CLAUDE.md` 的「适配器重连：指数退避 5s→300s，最多 20 次封顶」兼容：
/// 退避曲线来自 [`backoff_with_jitter`]（1s…30s 封顶，带 ±25% jitter），
/// 上限次数由调用方通过 [`should_give_up`](RetryTracker::should_give_up) 判断。
#[derive(Debug, Default, Clone)]
pub struct RetryTracker {
    attempts: u32,
    total_failures: u64,
}

impl RetryTracker {
    /// 新建计数器（attempts = 0）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前**连续**失败次数。
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// 自创建以来的累计失败次数（不随成功归零，用于指标/日志）。
    pub fn total_failures(&self) -> u64 {
        self.total_failures
    }

    /// 记录一次失败并返回本次应等待的退避时长。
    ///
    /// 退避按**当前** attempts 计算，随后自增；因此第一次失败得到
    /// `attempt=0` 的 ~1s 延迟，而非一上来就等十几秒。
    pub fn on_failure(&mut self) -> Duration {
        let delay = backoff_with_jitter(self.attempts);
        self.attempts = self.attempts.saturating_add(1);
        self.total_failures = self.total_failures.saturating_add(1);
        delay
    }

    /// 记录一次成功：连续失败归零（累计计数保留）。
    pub fn on_success(&mut self) {
        self.attempts = 0;
    }

    /// 是否已达到重连次数上限。
    ///
    /// 与既有 `AdapterManager` 的「最多 20 次」语义一致；传入 0 表示不限。
    pub fn should_give_up(&self, max_attempts: u32) -> bool {
        max_attempts != 0 && self.attempts >= max_attempts
    }

    /// 手动重置（例如管理员手动重启适配器）。
    pub fn reset(&mut self) {
        self.attempts = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_zero() {
        let t = RetryTracker::new();
        assert_eq!(t.attempts(), 0);
        assert_eq!(t.total_failures(), 0);
        assert!(!t.should_give_up(20));
    }

    #[test]
    fn first_failure_waits_about_one_second() {
        let mut t = RetryTracker::new();
        let d = t.on_failure();
        assert!(d.as_secs_f64() >= 0.75 && d.as_secs_f64() <= 1.25, "{d:?}");
        assert_eq!(t.attempts(), 1);
    }

    #[test]
    fn backoff_grows_with_consecutive_failures() {
        let mut t = RetryTracker::new();
        let first = t.on_failure();
        for _ in 0..4 {
            t.on_failure();
        }
        let later = t.on_failure();
        assert!(later > first, "backoff should grow: {first:?} → {later:?}");
    }

    #[test]
    fn success_resets_consecutive_but_keeps_total() {
        let mut t = RetryTracker::new();
        t.on_failure();
        t.on_failure();
        assert_eq!(t.attempts(), 2);
        t.on_success();
        assert_eq!(t.attempts(), 0);
        assert_eq!(t.total_failures(), 2);
    }

    #[test]
    fn give_up_honours_max_attempts() {
        let mut t = RetryTracker::new();
        for _ in 0..20 {
            t.on_failure();
        }
        assert!(t.should_give_up(20));
        assert!(!t.should_give_up(0), "0 表示不限");
        t.reset();
        assert!(!t.should_give_up(20));
    }
}
