//! 内存环形日志缓冲
//!
//! 实现 `tracing_subscriber::Layer`，在运行时捕获日志事件，
//! 供管理后台 Logs 页面通过 API 查询。
//!
//! 线程安全：内部使用 `Arc<RwLock<VecDeque>>`。

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use tracing_subscriber::layer::Layer;
use tracing_subscriber::registry::LookupSpan;

/// 单条日志条目
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub timestamp: i64,
    pub level: String,
    pub target: String,
    pub message: String,
}

/// 内存环形日志缓冲
#[derive(Clone)]
pub struct LogCollector {
    buffer: Arc<RwLock<VecDeque<LogEntry>>>,
    max_entries: usize,
}

impl LogCollector {
    /// 创建新的日志收集器
    pub fn new(max_entries: usize) -> Self {
        Self {
            buffer: Arc::new(RwLock::new(VecDeque::with_capacity(max_entries))),
            max_entries,
        }
    }

    /// 查询日志条目（支持过滤和游标）
    pub fn query(
        &self,
        level: Option<&str>,
        search: Option<&str>,
        limit: usize,
        since: Option<i64>,
    ) -> Vec<LogEntry> {
        let buf = self.buffer.read().unwrap_or_else(|e| e.into_inner());
        let limit = limit.min(500);

        let level = level.map(|l| l.to_uppercase());
        let search = search.map(|s| s.to_lowercase());

        buf.iter()
            .filter(|e| {
                if let Some(ref level) = level
                    && e.level != *level
                {
                    return false;
                }
                if let Some(ref search) = search
                    && !e.message.to_lowercase().contains(search)
                    && !e.target.to_lowercase().contains(search)
                {
                    return false;
                }
                if let Some(since) = since
                    && e.timestamp <= since
                {
                    return false;
                }
                true
            })
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// 获取日志总数
    pub fn total(&self) -> usize {
        self.buffer.read().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// 访客模式：提取日志事件的 message 字段
struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.0 = format!("{:?}", value).trim_matches('"').to_string();
        }
    }
}

pub(crate) fn redact_sensitive_text(message: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static BEARER: OnceLock<Regex> = OnceLock::new();
    static EASYBOT_KEY: OnceLock<Regex> = OnceLock::new();
    static ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
    static URL_CREDENTIALS: OnceLock<Regex> = OnceLock::new();

    let message = BEARER
        .get_or_init(|| Regex::new(r"(?i)\bbearer\s+[^\s,;]+").unwrap())
        .replace_all(message, "Bearer ***REDACTED***");
    let message = EASYBOT_KEY
        .get_or_init(|| Regex::new(r"\beb_[A-Za-z0-9_-]+").unwrap())
        .replace_all(&message, "***REDACTED***");
    let message = ASSIGNMENT
        .get_or_init(|| {
            Regex::new(r"(?i)\b(token|secret|password|api[_-]?key|authorization)=([^\s&,;]+)")
                .unwrap()
        })
        .replace_all(&message, "$1=***REDACTED***");
    URL_CREDENTIALS
        .get_or_init(|| Regex::new(r"(https?://)[^/@\s]+@").unwrap())
        .replace_all(&message, "$1***REDACTED***@")
        .into_owned()
}

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for LogCollector {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let meta = event.metadata();
        let timestamp = chrono::Utc::now().timestamp_millis();
        let level = meta.level().to_string();
        let target = meta.target().to_string();

        let mut message = String::new();
        let mut visitor = MessageVisitor(&mut message);
        event.record(&mut visitor);

        let entry = LogEntry {
            timestamp,
            level,
            target,
            message: redact_sensitive_text(&message),
        };

        let mut buf = self.buffer.write().unwrap_or_else(|e| e.into_inner());
        if buf.len() >= self.max_entries {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_common_credentials_from_log_messages() {
        let redacted = redact_sensitive_text(
            "Bearer abc.def eb_deadbeef token=telegram-secret password=hunter2 https://user:pass@example.com/a?api_key=leak",
        );
        for secret in [
            "abc.def",
            "eb_deadbeef",
            "telegram-secret",
            "hunter2",
            "user:pass",
            "leak",
        ] {
            assert!(!redacted.contains(secret), "leaked {secret}: {redacted}");
        }
    }
}
