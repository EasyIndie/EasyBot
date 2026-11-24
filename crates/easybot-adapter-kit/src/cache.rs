//! 有界缓存：容量上限 + 可选 TTL。
//!
//! 背景：5 个适配器各自手写了 `if cache.len() > LIMIT { cache.clear() }`
//! （`adapter-feishu/src/lib.rs` 的注释甚至写着「参考 QQ CHAT_TYPE_CACHE_LIMIT 模式」）。
//! 手写版本有两个问题：
//!
//! 1. **清空而非淘汰** —— 一旦超限就整体 `clear()`，命中率瞬间归零，随后
//!    又立刻从平台 API 全量回填，形成周期性抖动；
//! 2. **容易漏** —— 新增一个 `HashMap` 缓存时没人提醒你加上限。
//!
//! 统一到本类型后，容量上限由类型本身保证，超限按**插入顺序淘汰最旧项**
//! （FIFO，与 logrotate 式的直觉一致），可选的 TTL 用于「数据会过期」的场景
//! （如飞书 30s 的角色缓存）。
//!
//! ```
//! use easybot_adapter_kit::cache::BoundedCache;
//! use std::time::Duration;
//!
//! let mut cache: BoundedCache<String, u32> = BoundedCache::new(2);
//! cache.insert("a".into(), 1);
//! cache.insert("b".into(), 2);
//! cache.insert("c".into(), 3); // 淘汰最旧的 "a"
//! assert_eq!(cache.get(&"a".into()), None);
//! assert_eq!(cache.get(&"c".into()), Some(&3));
//!
//! let mut ttl_cache: BoundedCache<String, u32> =
//!     BoundedCache::with_ttl(8, Duration::from_secs(30));
//! ```

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::time::{Duration, Instant};

/// 容量有上限、可选 TTL 的键值缓存。
///
/// - 插入时若已存在同 key，**只更新值**，不改变其在淘汰队列中的位置
///   （重新插入不会"续命"，避免热 key 长期占位把冷 key 挤不出去）。
/// - 超出容量时淘汰**最早插入**的键。
/// - 设置 TTL 时，`get`/`contains_key` 遇到过期项视为未命中并顺手删除；
///   后台任务可周期性调用 [`sweep`](BoundedCache::sweep) 主动回收。
#[derive(Debug)]
pub struct BoundedCache<K, V> {
    capacity: usize,
    ttl: Option<Duration>,
    map: HashMap<K, Entry<V>>,
    order: VecDeque<K>,
}

#[derive(Debug, Clone)]
struct Entry<V> {
    value: V,
    inserted_at: Instant,
}

impl<K, V> BoundedCache<K, V>
where
    K: Eq + Hash + Clone,
{
    /// 创建只有容量上限的缓存。`capacity` 为 0 时缓存永不保存任何值
    /// （用于"允许显式关闭缓存"的配置项）。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            ttl: None,
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// 创建带 TTL 的缓存。`ttl` 为 0 表示立即过期。
    pub fn with_ttl(capacity: usize, ttl: Duration) -> Self {
        Self {
            capacity,
            ttl: Some(ttl),
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// 容量上限。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// TTL 配置。
    pub fn ttl(&self) -> Option<Duration> {
        self.ttl
    }

    /// 当前条目数（不含已过期但尚未清理的条目）。
    pub fn len(&mut self) -> usize {
        self.sweep();
        self.map.len()
    }

    /// 是否为空（不含已过期但尚未清理的条目）。
    pub fn is_empty(&mut self) -> bool {
        self.len() == 0
    }

    /// 读取。过期项视为未命中。
    pub fn get(&mut self, key: &K) -> Option<&V> {
        if self.is_expired(key) {
            self.remove(key);
            return None;
        }
        self.map.get(key).map(|e| &e.value)
    }

    /// 可变借用（用于「命中后原地更新一条记录」的场景，如管理员列表）。
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        if self.is_expired(key) {
            self.remove(key);
            return None;
        }
        self.map.get_mut(key).map(|e| &mut e.value)
    }

    /// 是否存在且未过期。
    pub fn contains_key(&mut self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// 写入。返回被淘汰的键（可能因为没有淘汰而为 `None`）。
    ///
    /// 容量为 0 时不会写入任何值，返回 `Some(key)` 表示立即被丢弃。
    pub fn insert(&mut self, key: K, value: V) -> Option<K> {
        if self.capacity == 0 {
            return Some(key);
        }
        if let Some(entry) = self.map.get_mut(&key) {
            entry.value = value;
            entry.inserted_at = Instant::now();
            return None;
        }
        self.map.insert(
            key.clone(),
            Entry {
                value,
                inserted_at: Instant::now(),
            },
        );
        self.order.push_back(key);
        self.evict_overflow()
    }

    /// 删除并返回值。
    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.order.retain(|k| k != key);
        self.map.remove(key).map(|e| e.value)
    }

    /// 保留满足条件的条目（用于「按前缀批量失效」，如聊天元数据变更）。
    ///
    /// 返回被移除的条目数。
    pub fn retain<F>(&mut self, mut keep: F) -> usize
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        let before = self.map.len();
        let order = &mut self.order;
        self.map.retain(|key, entry| {
            if keep(key, &mut entry.value) {
                true
            } else {
                order.retain(|k| k != key);
                false
            }
        });
        before - self.map.len()
    }

    /// 清空全部条目。
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    /// 主动清理所有已过期条目，返回清理数量。
    pub fn sweep(&mut self) -> usize {
        let Some(ttl) = self.ttl else {
            return 0;
        };
        let now = Instant::now();
        let expired: Vec<K> = self
            .map
            .iter()
            .filter(|(_, e)| now.duration_since(e.inserted_at) >= ttl)
            .map(|(k, _)| k.clone())
            .collect();
        for key in &expired {
            self.remove(key);
        }
        expired.len()
    }

    fn is_expired(&self, key: &K) -> bool {
        let Some(ttl) = self.ttl else {
            return false;
        };
        self.map
            .get(key)
            .is_some_and(|e| e.inserted_at.elapsed() >= ttl)
    }

    fn evict_overflow(&mut self) -> Option<K> {
        let mut evicted = None;
        while self.map.len() > self.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if self.map.remove(&oldest).is_some() {
                evicted = Some(oldest);
            }
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(cap: usize) -> BoundedCache<String, u32> {
        BoundedCache::new(cap)
    }

    #[test]
    fn evicts_oldest_on_overflow() {
        let mut c = cache(3);
        c.insert("a".into(), 1);
        c.insert("b".into(), 2);
        c.insert("c".into(), 3);
        assert_eq!(c.insert("d".into(), 4), Some("a".to_string()));
        assert_eq!(c.get(&"a".to_string()), None);
        assert_eq!(c.get(&"d".to_string()), Some(&4));
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn reinserting_existing_key_does_not_grow() {
        let mut c = cache(2);
        c.insert("a".into(), 1);
        c.insert("a".into(), 2);
        assert_eq!(c.len(), 1);
        assert_eq!(c.get(&"a".to_string()), Some(&2));
    }

    #[test]
    fn zero_capacity_discards_everything() {
        let mut c = cache(0);
        assert_eq!(c.insert("a".into(), 1), Some("a".to_string()));
        assert_eq!(c.get(&"a".to_string()), None);
    }

    #[test]
    fn ttl_expires_entries() {
        let mut c: BoundedCache<String, u32> = BoundedCache::with_ttl(8, Duration::from_millis(30));
        c.insert("a".into(), 1);
        assert_eq!(c.get(&"a".to_string()), Some(&1));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(c.get(&"a".to_string()), None);
        assert!(c.is_empty());
    }

    #[test]
    fn sweep_reports_removed_entries() {
        let mut c: BoundedCache<String, u32> = BoundedCache::with_ttl(8, Duration::from_millis(20));
        c.insert("a".into(), 1);
        c.insert("b".into(), 2);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(c.sweep(), 2);
        assert_eq!(c.sweep(), 0);
    }

    #[test]
    fn retain_drops_matching_entries_and_keeps_order_consistent() {
        let mut c = cache(4);
        c.insert("chat1:a".into(), 1);
        c.insert("chat1:b".into(), 2);
        c.insert("chat2:a".into(), 3);
        let removed = c.retain(|k, _| !k.starts_with("chat1:"));
        assert_eq!(removed, 2);
        assert_eq!(c.len(), 1);
        // 淘汰队列同步清理：再插入两项不应误淘汰残留 key
        c.insert("chat3:a".into(), 4);
        c.insert("chat4:a".into(), 5);
        assert_eq!(c.len(), 3);
        assert_eq!(c.get(&"chat4:a".to_string()), Some(&5));
    }

    #[test]
    fn remove_clears_eviction_order() {
        let mut c = cache(2);
        c.insert("a".into(), 1);
        c.remove(&"a".to_string());
        c.insert("b".into(), 2);
        c.insert("c".into(), 3);
        // 不应因为残留的 "a" 占位而误淘汰
        assert_eq!(c.len(), 2);
        assert_eq!(c.get(&"c".to_string()), Some(&3));
    }
}
