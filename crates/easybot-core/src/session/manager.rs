//! 会话管理器实现
//!
//! 提供内存中的会话管理功能，暂不涉及持久化存储。
//! Phase 1 使用 DashMap 内存存储，后续接入 SQLite。

use dashmap::DashMap;
use crate::types::session::{Session, SessionFilter, SessionSource, ResetPolicy};

/// 默认构造会话使用的重置策略
const DEFAULT_RESET_POLICY: ResetPolicy = ResetPolicy::Never;

/// 会话管理器
///
/// 管理所有活跃会话的生命周期。
/// Phase 1 使用内存存储，Phase 4 接入 SQLite 持久化。
pub struct SessionManager {
    sessions: DashMap<String, Session>,
}

impl SessionManager {
    /// 创建新的会话管理器
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// 获取或创建会话
    ///
    /// 根据 session_key 查找已有会话，不存在则创建新的。
    pub fn get_or_create(
        &self,
        key: &str,
        source: SessionSource,
    ) -> Session {
        if let Some(entry) = self.sessions.get(key) {
            let session = entry.value().clone();
            drop(entry);
            // 更新活跃时间（"写"操作在 DashMap 中需要可变引用）
            if let Some(mut entry) = self.sessions.get_mut(key) {
                entry.updated_at = chrono::Utc::now().timestamp_millis();
            }
            session
        } else {
            let now = chrono::Utc::now().timestamp_millis();
            let session = Session {
                key: key.to_string(),
                platform: source.platform.clone(),
                chat_id: source.chat_id.clone(),
                thread_id: None,
                created_at: now,
                updated_at: now,
                source,
                reset_policy: DEFAULT_RESET_POLICY,
                metadata: serde_json::json!({}),
            };
            self.sessions.insert(key.to_string(), session.clone());
            session
        }
    }

    /// 获取会话
    pub fn get(&self, key: &str) -> Option<Session> {
        self.sessions.get(key).map(|e| e.value().clone())
    }

    /// 删除会话
    pub fn delete(&self, key: &str) -> bool {
        self.sessions.remove(key).is_some()
    }

    /// 列出会话
    pub fn list(&self, filter: Option<SessionFilter>) -> Vec<Session> {
        let mut results: Vec<Session> = self
            .sessions
            .iter()
            .map(|e| e.value().clone())
            .collect();

        // 按更新时间降序排列（最近活跃的在前）
        results.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        // 应用过滤条件
        if let Some(f) = filter {
            if let Some(platform) = f.platform {
                results.retain(|s| s.platform == platform);
            }
            if let Some(active_mins) = f.active_within_minutes {
                let cutoff = chrono::Utc::now().timestamp_millis()
                    - (active_mins as i64 * 60 * 1000);
                results.retain(|s| s.updated_at >= cutoff);
            }
            if let Some(limit) = f.limit {
                results.truncate(limit);
            }
            if let Some(offset) = f.offset {
                if offset < results.len() {
                    results.drain(0..offset);
                } else {
                    results.clear();
                }
            }
        }

        results
    }

    /// 获取会话数量
    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    /// 更新会话
    pub fn update(&self, key: &str, mutation: SessionMutation) -> Option<Session> {
        if let Some(mut entry) = self.sessions.get_mut(key) {
            let session = entry.value_mut();
            session.updated_at = chrono::Utc::now().timestamp_millis();
            if let Some(policy) = mutation.reset_policy {
                session.reset_policy = policy;
            }
            if let Some(meta) = mutation.metadata {
                session.metadata = meta;
            }
            Some(session.clone())
        } else {
            None
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 会话变更参数
#[derive(Default)]
pub struct SessionMutation {
    pub reset_policy: Option<ResetPolicy>,
    pub metadata: Option<serde_json::Value>,
}

impl SessionMutation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_reset_policy(mut self, policy: ResetPolicy) -> Self {
        self.reset_policy = Some(policy);
        self
    }

    pub fn with_metadata(mut self, meta: serde_json::Value) -> Self {
        self.metadata = Some(meta);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::message::ChatType;

    fn make_source(platform: &str, chat_id: &str) -> SessionSource {
        SessionSource {
            platform: platform.to_string(),
            chat_id: chat_id.to_string(),
            chat_name: None,
            chat_type: ChatType::Dm,
            user_id: None,
            user_name: None,
            is_bot: false,
        }
    }

    #[test]
    fn test_get_or_create() {
        let mgr = SessionManager::new();
        let key = "telegram:12345";
        let source = make_source("telegram", "12345");

        let session = mgr.get_or_create(key, source);
        assert_eq!(session.key, key);
        assert_eq!(session.platform, "telegram");

        // 再次获取应返回同一会话
        let session2 = mgr.get_or_create(key, make_source("telegram", "12345"));
        assert_eq!(session2.created_at, session.created_at);
    }

    #[test]
    fn test_delete() {
        let mgr = SessionManager::new();
        mgr.get_or_create("test:1", make_source("test", "1"));
        assert!(mgr.delete("test:1"));
        assert!(!mgr.delete("nonexistent"));
    }

    #[test]
    fn test_list_filter_by_platform() {
        let mgr = SessionManager::new();
        mgr.get_or_create("telegram:1", make_source("telegram", "1"));
        mgr.get_or_create("discord:2", make_source("discord", "2"));

        let filter = SessionFilter {
            platform: Some("telegram".to_string()),
            ..Default::default()
        };
        let results = mgr.list(Some(filter));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].platform, "telegram");
    }
}
