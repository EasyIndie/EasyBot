//! 会话管理器实现
//!
//! 提供内存中的会话管理功能，可选支持 SQLite 持久化。
//! DashMap 提供快速读取，持久化写入委托给 SessionStore。

use std::sync::Arc;
use dashmap::DashMap;
use crate::storage::SessionStore;
use crate::types::session::{Session, SessionFilter, SessionSource, ResetPolicy};

/// 默认构造会话使用的重置策略
const DEFAULT_RESET_POLICY: ResetPolicy = ResetPolicy::Never;

/// 会话管理器
///
/// 管理所有活跃会话的生命周期。
/// - 快速读取：DashMap（O(1) 查找）
/// - 写入持久化：可选的 SessionStore
pub struct SessionManager {
    sessions: DashMap<String, Session>,
    store: Option<Arc<dyn SessionStore>>,
}

impl SessionManager {
    /// 创建纯内存会话管理器（无持久化）
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            store: None,
        }
    }

    /// 创建带持久化存储的会话管理器
    pub fn with_store(store: Arc<dyn SessionStore>) -> Self {
        Self {
            sessions: DashMap::new(),
            store: Some(store),
        }
    }

    /// 从存储层加载所有会话到 DashMap
    ///
    /// 应在启动时调用，将持久化的会话恢复到内存中。
    pub async fn load_from_store(&self) -> Result<usize, crate::storage::StoreError> {
        if let Some(ref store) = self.store {
            let sessions = store.load_all_sessions().await?;
            let count = sessions.len();
            for session in sessions {
                self.sessions.insert(session.key.clone(), session);
            }
            Ok(count)
        } else {
            Ok(0)
        }
    }

    /// 获取或创建会话
    ///
    /// 根据 session_key 查找已有会话，不存在则创建新的。
    /// 如果配置了持久化存储，创建/更新操作会同步写入。
    pub async fn get_or_create(
        &self,
        key: &str,
        source: SessionSource,
    ) -> Session {
        if let Some(entry) = self.sessions.get(key) {
            let session = entry.value().clone();
            drop(entry);
            // 更新活跃时间
            if let Some(mut entry) = self.sessions.get_mut(key) {
                entry.updated_at = chrono::Utc::now().timestamp_millis();
                // 持久化更新
                if let Some(ref store) = self.store {
                    let _ = store.upsert_session(&entry.clone()).await;
                }
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
            // 持久化新会话
            if let Some(ref store) = self.store {
                let _ = store.upsert_session(&session).await;
            }
            session
        }
    }

    /// 获取会话（同步，仅读 DashMap）
    pub fn get(&self, key: &str) -> Option<Session> {
        self.sessions.get(key).map(|e| e.value().clone())
    }

    /// 删除会话
    ///
    /// 从 DashMap 和持久化存储中同时删除。
    pub async fn delete(&self, key: &str) -> bool {
        let removed = self.sessions.remove(key).is_some();
        if removed {
            if let Some(ref store) = self.store {
                let _ = store.delete_session(key).await;
            }
        }
        removed
    }

    /// 列出会话（同步，仅读 DashMap）
    pub fn list(&self, filter: Option<SessionFilter>) -> Vec<Session> {
        let mut results: Vec<Session> = self
            .sessions
            .iter()
            .map(|e| e.value().clone())
            .collect();

        // 按更新时间降序排列（最近活跃的在前）
        results.sort_by_key(|b| std::cmp::Reverse(b.updated_at));

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

    /// 获取会话数量（同步）
    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    /// 获取底层 SessionStore 的引用（用于 TTL 清理等后台任务）
    pub fn store_ref(&self) -> Option<Arc<dyn SessionStore>> {
        self.store.clone()
    }

    /// 更新会话
    ///
    /// 更新 DashMap 中的会话，并持久化到存储。
    pub async fn update(&self, key: &str, mutation: SessionMutation) -> Option<Session> {
        if let Some(mut entry) = self.sessions.get_mut(key) {
            let session = entry.value_mut();
            session.updated_at = chrono::Utc::now().timestamp_millis();
            if let Some(policy) = mutation.reset_policy {
                session.reset_policy = policy;
            }
            if let Some(meta) = mutation.metadata {
                session.metadata = meta;
            }
            let cloned = session.clone();
            // 持久化更新
            if let Some(ref store) = self.store {
                let _ = store.upsert_session(&cloned).await;
            }
            Some(cloned)
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

    #[tokio::test]
    async fn test_get_or_create() {
        let mgr = SessionManager::new();
        let key = "telegram:12345";
        let source = make_source("telegram", "12345");

        let session = mgr.get_or_create(key, source).await;
        assert_eq!(session.key, key);
        assert_eq!(session.platform, "telegram");

        // 再次获取应返回同一会话
        let session2 = mgr.get_or_create(key, make_source("telegram", "12345")).await;
        assert_eq!(session2.created_at, session.created_at);
    }

    #[tokio::test]
    async fn test_delete() {
        let mgr = SessionManager::new();
        mgr.get_or_create("test:1", make_source("test", "1")).await;
        assert!(mgr.delete("test:1").await);
        assert!(!mgr.delete("nonexistent").await);
    }

    #[tokio::test]
    async fn test_list_filter_by_platform() {
        let mgr = SessionManager::new();
        mgr.get_or_create("telegram:1", make_source("telegram", "1")).await;
        mgr.get_or_create("discord:2", make_source("discord", "2")).await;

        let filter = SessionFilter {
            platform: Some("telegram".to_string()),
            ..Default::default()
        };
        let results = mgr.list(Some(filter));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].platform, "telegram");
    }

    #[tokio::test]
    async fn test_concurrent_get_or_create() {
        let mgr = Arc::new(SessionManager::new());
        let mut handles = Vec::new();

        for _ in 0..50 {
            let mgr = mgr.clone();
            handles.push(tokio::spawn(async move {
                let s = mgr.get_or_create("concurrent:1", make_source("test", "1")).await;
                assert_eq!(s.key, "concurrent:1");
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(mgr.count(), 1, "single session should exist");
        assert!(mgr.get("concurrent:1").is_some(), "session should be findable");
    }

    #[tokio::test]
    async fn test_concurrent_read_during_write() {
        // 验证并发读写不 panic（DashMap 保证内部安全）
        let mgr = Arc::new(SessionManager::new());
        mgr.get_or_create("rw:1", make_source("test", "1")).await;

        let mgr_w = mgr.clone();
        let mgr_r = mgr.clone();
        let write_handle = tokio::spawn(async move {
            for i in 0..100 {
                let source = make_source("test", &i.to_string());
                mgr_w.get_or_create(&format!("rw:{}", i), source).await;
            }
        });

        let read_handle = tokio::spawn(async move {
            for _ in 0..100 {
                let _ = mgr_r.list(None);
                let _ = mgr_r.count();
                tokio::task::yield_now().await;
            }
        });

        let (w, r) = tokio::join!(write_handle, read_handle);
        w.unwrap();
        r.unwrap();
        // 验证最终的 state 一致
        assert!(mgr.count() >= 1, "should have at least 1 session");
    }
}
