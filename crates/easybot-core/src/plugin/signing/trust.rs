//! 发布者信任存储
//!
//! `.trust` 文件（`{home}/plugins/.trust`）记录用户**显式**信任的发布者，
//! 对齐 VS Code 1.97 发布者信任对话框语义：
//!
//! - 首次安装非信任发布者的插件时需确认；CLI `--yes` 跳过确认但**不会**自动加入 `.trust`
//! - 显式 `easybot plugin trust <publisher>` 才加入
//! - 信任是**按发布者**而非按插件
//!
//! 信任决策 = 配置 `plugins.trusted_publishers`（内置 + 覆盖） ∪ 用户 `.trust` 文件。
//! 配置侧校验由 `PluginManager` 完成；本文件只负责用户级 `.trust` 状态的读写与匹配。

use super::{SigningError, public_key_fingerprint};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

/// 发布者信任判定抽象
///
/// 供加载器与生产门禁校验"这把公钥是否代表受信任的发布者"。
/// 由调用方组合配置 `trusted_publishers` 与用户 `.trust` 实现
/// （`PluginManager` / 生产门禁提供组合实现）。
pub trait PublisherTrust {
    /// 发布者的这把公钥是否受信任
    fn is_trusted(&self, publisher: &str, public_key_b64: &str) -> bool;
}

impl PublisherTrust for TrustStore {
    fn is_trusted(&self, publisher: &str, public_key_b64: &str) -> bool {
        TrustStore::is_trusted(self, publisher, public_key_b64)
    }
}

/// 单个信任条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    /// 发布者标识
    pub publisher: String,
    /// 发布者公钥指纹（base64 公钥前 16 字符）
    #[serde(rename = "publicKeyFingerprint")]
    pub public_key_fingerprint: String,
    /// 信任时间（RFC 3339）
    #[serde(rename = "trustedAt")]
    pub trusted_at: String,
}

/// 用户级信任存储（`{home}/plugins/.trust`）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustStore {
    pub publishers: Vec<TrustEntry>,
}

impl TrustStore {
    /// 从磁盘加载；文件不存在或损坏时返回空存储（幂等）
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// 原子写回磁盘（temp + rename，防半写损坏）
    pub fn save(&self, path: &Path) -> Result<(), SigningError> {
        let text = serde_json::to_string_pretty(self)?;
        let tmp = std::path::PathBuf::from(format!("{}.tmp", path.display()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 该发布者的**这把公钥**是否受信任
    pub fn is_trusted(&self, publisher: &str, public_key_b64: &str) -> bool {
        let fp = public_key_fingerprint(public_key_b64);
        self.publishers
            .iter()
            .any(|e| e.publisher == publisher && e.public_key_fingerprint == fp)
    }

    /// 是否已信任该发布者（不考虑公钥——用于 UI 展示与首次提示判断）
    pub fn is_publisher_known(&self, publisher: &str) -> bool {
        self.publishers.iter().any(|e| e.publisher == publisher)
    }

    /// 添加（或更新）一个发布者信任条目
    pub fn add(&mut self, publisher: &str, public_key_b64: &str) {
        self.publishers.retain(|e| e.publisher != publisher);
        self.publishers.push(TrustEntry {
            publisher: publisher.to_string(),
            public_key_fingerprint: public_key_fingerprint(public_key_b64),
            trusted_at: chrono::Utc::now().to_rfc3339(),
        });
    }

    /// 移除发布者信任
    pub fn remove(&mut self, publisher: &str) {
        self.publishers.retain(|e| e.publisher != publisher);
    }
}

/// 组合信任判定：配置 `plugins.trusted_publishers`（内置 + 覆盖）∪ 用户 `.trust`。
///
/// 加载器在同步 `verify_signature` 上下文中调用，故用 `std::sync::RwLock`
/// 读用户 `.trust`（tokio RwLock 的 blocking 读取在 async 上下文会 panic）。
#[derive(Clone, Default)]
pub struct CompositePublisherTrust {
    /// 用户级 `.trust` 状态（由 `PluginManager` 持有并维护）
    store: Arc<RwLock<TrustStore>>,
    /// 配置 `plugins.trusted_publishers`：发布者 → 公钥 base64
    configured: HashMap<String, String>,
}

impl CompositePublisherTrust {
    pub fn new(store: Arc<RwLock<TrustStore>>, configured: HashMap<String, String>) -> Self {
        Self { store, configured }
    }
}

impl PublisherTrust for CompositePublisherTrust {
    fn is_trusted(&self, publisher: &str, public_key_b64: &str) -> bool {
        // 配置侧：精确公钥匹配（内置默认列表 + 用户配置覆盖）
        if self
            .configured
            .get(publisher)
            .map(|k| k == public_key_b64)
            .unwrap_or(false)
        {
            return true;
        }
        // 用户侧 `.trust`
        self.store
            .read()
            .map(|s| s.is_trusted(publisher, public_key_b64))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trust_add_and_check() {
        let mut store = TrustStore::default();
        let (_, verifying) = super::super::generate_keypair();
        let pk = super::super::encode_public_key(&verifying);

        assert!(!store.is_trusted("pub-a", &pk));
        store.add("pub-a", &pk);
        assert!(store.is_trusted("pub-a", &pk));
        assert!(!store.is_trusted("pub-b", &pk), "publisher must match");
    }

    #[test]
    fn test_add_replaces_same_publisher() {
        let mut store = TrustStore::default();
        let (_, v1) = super::super::generate_keypair();
        let (_, v2) = super::super::generate_keypair();
        store.add("pub-a", &super::super::encode_public_key(&v1));
        store.add("pub-a", &super::super::encode_public_key(&v2));

        assert_eq!(store.publishers.len(), 1);
        assert!(store.is_trusted("pub-a", &super::super::encode_public_key(&v2)));
        assert!(!store.is_trusted("pub-a", &super::super::encode_public_key(&v1)));
    }

    #[test]
    fn test_remove() {
        let mut store = TrustStore::default();
        let (_, verifying) = super::super::generate_keypair();
        let pk = super::super::encode_public_key(&verifying);
        store.add("pub-a", &pk);
        store.remove("pub-a");
        assert!(!store.is_publisher_known("pub-a"));
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("trust-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".trust");

        let (_, verifying) = super::super::generate_keypair();
        let pk = super::super::encode_public_key(&verifying);
        let mut store = TrustStore::default();
        store.add("pub-a", &pk);
        store.save(&path).unwrap();

        let loaded = TrustStore::load(&path);
        assert!(loaded.is_trusted("pub-a", &pk));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_missing_returns_empty() {
        let store = TrustStore::load(Path::new("/nonexistent/.trust"));
        assert!(store.publishers.is_empty());
    }
}
