//! 有界缓存容器
//!
//! 提供带「容量上限 + TTL 淘汰」的并发安全缓存，供各平台适配器复用，
//! 落实「所有适配器缓存必须有大小上限或 TTL 淘汰」的资源管理铁律。
//!
//! 语义：
//! - **容量上限**：超出 `capacity` 时按 FIFO 逐出最旧条目。
//! - **TTL 淘汰**：过期条目在插入/读取时惰性移除，也可由调用方定期 `prune()`。
//!
//! 用途举例：角色缓存（`chat_id:sender_id → SenderRole`）、会话路由表
//! （`chat_id → ChatType`）、会话令牌映射（`chat_id → context_token`）等
//! 只增不减会泄漏内存的适配器缓存。

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Entry<V> {
    value: V,
    inserted_at: Instant,
}

#[derive(Debug)]
struct Inner<K, V> {
    map: HashMap<K, Entry<V>>,
    /// FIFO 插入顺序，用于容量溢出时逐出最旧条目。
    order: VecDeque<K>,
}

/// 有界缓存：容量上限 + TTL 淘汰。
///
/// 内部使用 `Mutex<HashMap>`，适用于条目数受上限约束、读多写少的适配器缓存。
/// `get`/`contains` 返回给调用方的值均为克隆，不持有内部锁。
///
/// 注意：容量逐出按 FIFO（最旧插入先被淘汰），`get` 不刷新顺序。
/// 需要 LRU 语义的场景请自行按需 `remove`+`insert`（或联系 core 扩展）。
#[derive(Debug)]
pub struct BoundedCache<K, V> {
    inner: Mutex<Inner<K, V>>,
    capacity: usize,
    ttl: Duration,
}

impl<K, V> BoundedCache<K, V>
where
    K: Eq + Hash + Clone + std::fmt::Debug,
    V: Clone + std::fmt::Debug,
{
    /// 创建有界缓存。`capacity` 为 0 时视为 1（至少容纳一条），`ttl` 为 0 表示永不过期。
    pub fn new(capacity: usize, ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
            capacity: capacity.max(1),
            ttl,
        }
    }

    /// 读取条目。命中且未过期时返回克隆；条目过期则惰性移除并返回 `None`。
    pub fn get(&self, key: &K) -> Option<V> {
        let mut inner = self.inner.lock().expect("BoundedCache mutex poisoned");
        if self.is_expired(key, &inner) {
            self.remove_inner(&mut inner, key);
            return None;
        }
        inner.map.get(key).map(|e| e.value.clone())
    }

    /// 写入条目。覆盖已存在键（保持原插入顺序）；新增键推入 FIFO 尾部。
    /// 写入后若超出容量，逐出最旧条目。同时惰性移除已过期条目。
    pub fn insert(&self, key: K, value: V) {
        let mut inner = self.inner.lock().expect("BoundedCache mutex poisoned");
        self.prune_inner(&mut inner);
        if inner.map.contains_key(&key) {
            inner.map.insert(
                key.clone(),
                Entry {
                    value,
                    inserted_at: Instant::now(),
                },
            );
        } else {
            inner.map.insert(
                key.clone(),
                Entry {
                    value,
                    inserted_at: Instant::now(),
                },
            );
            inner.order.push_back(key);
        }
        self.evict_inner(&mut inner);
    }

    /// 移除条目，返回被移除的值（若存在）。
    pub fn remove(&self, key: &K) -> Option<V> {
        let mut inner = self.inner.lock().expect("BoundedCache mutex poisoned");
        self.remove_inner(&mut inner, key)
    }

    /// 是否包含未过期的条目。
    pub fn contains(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// 当前条目数（未做过期清理，仅反映 map 大小）。
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("BoundedCache mutex poisoned")
            .map
            .len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 清空所有条目。
    pub fn clear(&self) {
        let mut inner = self.inner.lock().expect("BoundedCache mutex poisoned");
        inner.map.clear();
        inner.order.clear();
    }

    /// 主动移除所有已过期条目，返回移除数量。可在后台定时任务中调用，
    /// 避免过期条目占用内存直至被重访问。
    pub fn prune(&self) -> usize {
        let mut inner = self.inner.lock().expect("BoundedCache mutex poisoned");
        self.prune_inner(&mut inner)
    }

    fn is_expired(&self, key: &K, inner: &Inner<K, V>) -> bool {
        if self.ttl.is_zero() {
            return false;
        }
        inner
            .map
            .get(key)
            .map(|e| e.inserted_at.elapsed() > self.ttl)
            .unwrap_or(false)
    }

    fn remove_inner(&self, inner: &mut Inner<K, V>, key: &K) -> Option<V> {
        let removed = inner.map.remove(key).map(|e| e.value);
        if removed.is_some()
            && let Some(pos) = inner.order.iter().position(|k| k == key)
        {
            inner.order.remove(pos);
        }
        removed
    }

    fn prune_inner(&self, inner: &mut Inner<K, V>) -> usize {
        let mut removed = 0;
        let expired: Vec<K> = inner
            .map
            .iter()
            .filter(|(k, _)| self.is_expired(k, inner))
            .map(|(k, _)| k.clone())
            .collect();
        for k in expired {
            self.remove_inner(inner, &k);
            removed += 1;
        }
        removed
    }

    fn evict_inner(&self, inner: &mut Inner<K, V>) {
        while inner.map.len() > self.capacity {
            let Some(oldest) = inner.order.pop_front() else {
                break;
            };
            inner.map.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn respects_capacity_fifo() {
        let cache = BoundedCache::new(3, Duration::from_secs(60));
        cache.insert(1, "a");
        cache.insert(2, "b");
        cache.insert(3, "c");
        cache.insert(4, "d"); // over capacity → evict oldest (1)
        assert_eq!(cache.len(), 3);
        assert!(!cache.contains(&1));
        assert!(cache.contains(&2));
        assert!(cache.contains(&3));
        assert!(cache.contains(&4));
        // 覆盖已存在键不改变容量占用
        cache.insert(2, "bb");
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get(&2), Some("bb"));
    }

    #[test]
    fn evicts_expired_on_access() {
        let cache = BoundedCache::new(10, Duration::from_millis(30));
        cache.insert(1, "a");
        assert_eq!(cache.get(&1), Some("a"));
        thread::sleep(Duration::from_millis(50));
        assert_eq!(cache.get(&1), None);
        assert!(!cache.contains(&1));
    }

    #[test]
    fn zero_ttl_means_never_expires() {
        let cache = BoundedCache::new(10, Duration::from_millis(0));
        cache.insert(1, "a");
        assert_eq!(cache.get(&1), Some("a"));
    }

    #[test]
    fn prune_removes_all_expired() {
        let cache = BoundedCache::new(10, Duration::from_millis(20));
        cache.insert(1, "a");
        cache.insert(2, "b");
        // 过期条目只有在不被 get/insert 触碰时才会残留；此时全部过期 → 显式 prune 全清
        thread::sleep(Duration::from_millis(40));
        assert_eq!(cache.prune(), 2);
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
    }

    #[test]
    fn remove_returns_value() {
        let cache = BoundedCache::new(10, Duration::from_secs(60));
        cache.insert(1, "a");
        assert_eq!(cache.remove(&1), Some("a"));
        assert_eq!(cache.remove(&1), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn clear_empties() {
        let cache = BoundedCache::new(10, Duration::from_secs(60));
        cache.insert(1, "a");
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn single_entry_capacity_min_one() {
        let cache = BoundedCache::new(0, Duration::from_secs(60));
        cache.insert(1, "a");
        cache.insert(2, "b");
        assert_eq!(cache.len(), 1);
    }
}
