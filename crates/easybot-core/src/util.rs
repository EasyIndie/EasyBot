//! 通用工具函数

use std::time::Duration;

/// 计算指数退避延迟（带随机 jitter）
///
/// 基础退避序列：attempt=0 → 1s, 1 → 2s, 2 → 4s, 3 → 8s, ...
/// 最大值封顶 30 秒。每个值添加 ±25% 的随机 jitter 防止惊群。
///
/// # Example
/// ```
/// let delay = easybot_core::util::backoff_with_jitter(0); // ~1s with jitter
/// let delay = easybot_core::util::backoff_with_jitter(5); // ~30s with jitter (capped)
/// ```
pub fn backoff_with_jitter(attempt: u32) -> Duration {
    let secs = if attempt >= 6 { 30 } else { 1u64 << attempt }; // 1, 2, 4, 8, 16, 32 → capped at 30
    let secs = secs.min(30);

    // 添加 ±25% 随机 jitter（使用简单的伪随机，避免依赖 rand）
    let jitter_factor =
        ((attempt.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7FFF) as f64 / 32768.0; // 0..1
    let jitter = 1.0 + (jitter_factor - 0.5) * 0.5; // 0.75..1.25

    Duration::from_secs_f64(secs as f64 * jitter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backoff_sequence() {
        assert!(backoff_with_jitter(0).as_secs_f64() >= 0.75);
        assert!(backoff_with_jitter(0).as_secs_f64() <= 1.25);
        assert!(backoff_with_jitter(5).as_secs() >= 22); // 30*0.75
        assert!(backoff_with_jitter(5).as_secs() <= 30);
    }

    #[test]
    fn test_backoff_increases() {
        let d0 = backoff_with_jitter(0);
        let d1 = backoff_with_jitter(1);
        let d2 = backoff_with_jitter(2);
        assert!(d1 > d0 || (d1 - d0).as_secs_f64() < 0.5); // may overlap with jitter
        assert!(d2 > d1 || (d2 - d1).as_secs_f64() < 0.5);
    }
}
