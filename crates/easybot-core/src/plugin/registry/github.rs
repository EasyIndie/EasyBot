//! GitHub Releases 注册表实现
//!
//! - 目录索引：从 `EasyIndie/EasyBot-Registry`（可覆盖）仓库的 `catalog.json` 读取
//! - 版本查询：枚举插件仓库的 Releases，解析 `easybot-plugin.json` asset
//! - 下载：流式下载并校验 `sha256`
//!
//! 复用 [`crate::updater::github::GitHubClient`]，其 5 分钟 TTL 缓存/限流/流式下载
//! 均直接适用。每个 `(owner, repo)` 一个客户端实例（各自带缓存）。

use super::PluginRegistry;
use super::types::{
    PluginArtifact, PluginCatalog, PluginRegistryError, PluginSource, PluginVersionMeta,
};
use crate::updater::github::GitHubClient;
use crate::updater::types::UpdateError;
use std::collections::HashMap;
use std::path::Path;
use tokio::sync::Mutex;

/// 官方市场目录仓库
pub const DEFAULT_CATALOG_OWNER: &str = "EasyIndie";
pub const DEFAULT_CATALOG_REPO: &str = "EasyBot-Registry";

/// 市场目录文件名（仓库默认分支根目录）
const CATALOG_FILE: &str = "catalog.json";
/// 插件版本元数据文件名（Release asset）
const PLUGIN_META_FILE: &str = "easybot-plugin.json";

/// GitHub Releases 注册表
pub struct GitHubRegistry {
    catalog_owner: String,
    catalog_repo: String,
    /// `(owner, repo)` → 带缓存的客户端
    clients: Mutex<HashMap<(String, String), GitHubClient>>,
}

impl GitHubRegistry {
    /// 使用官方市场目录仓库创建注册表
    pub fn new() -> Self {
        Self::with_catalog(DEFAULT_CATALOG_OWNER, DEFAULT_CATALOG_REPO)
    }

    /// 使用自定义市场目录仓库创建注册表
    pub fn with_catalog(owner: &str, repo: &str) -> Self {
        Self {
            catalog_owner: owner.to_string(),
            catalog_repo: repo.to_string(),
            clients: Mutex::new(HashMap::new()),
        }
    }

    /// 创建指向 mock base URL 的注册表（仅测试用）
    #[cfg(test)]
    fn with_base_url(owner: &str, repo: &str, base_url: &str) -> Self {
        let mut clients = HashMap::new();
        clients.insert(
            (owner.to_string(), repo.to_string()),
            GitHubClient::with_base_url(owner, repo, base_url),
        );
        Self {
            catalog_owner: owner.to_string(),
            catalog_repo: repo.to_string(),
            clients: Mutex::new(clients),
        }
    }
}

impl Default for GitHubRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 将 `UpdateError` 映射为 `PluginRegistryError`
fn map_err(e: UpdateError) -> PluginRegistryError {
    match e {
        UpdateError::RateLimited => PluginRegistryError::RateLimited,
        UpdateError::NetworkError(msg) => PluginRegistryError::NetworkError(msg),
        UpdateError::HttpError(e) => PluginRegistryError::HttpError(e),
        UpdateError::IoError(e) => PluginRegistryError::IoError(e),
        UpdateError::JsonError(e) => PluginRegistryError::JsonError(e),
        other => PluginRegistryError::Other(other.to_string()),
    }
}

#[async_trait::async_trait]
impl PluginRegistry for GitHubRegistry {
    async fn catalog(&self) -> Result<PluginCatalog, PluginRegistryError> {
        let mut clients = self.clients.lock().await;
        let client = clients
            .entry((self.catalog_owner.clone(), self.catalog_repo.clone()))
            .or_insert_with(|| GitHubClient::new(&self.catalog_owner, &self.catalog_repo));

        let text = client.raw_file(CATALOG_FILE).await.map_err(map_err)?;
        let catalog: PluginCatalog = serde_json::from_str(&text)?;
        Ok(catalog)
    }

    async fn versions_for(
        &self,
        source: &PluginSource,
        limit: usize,
    ) -> Result<Vec<PluginVersionMeta>, PluginRegistryError> {
        let mut clients = self.clients.lock().await;
        let client = clients
            .entry((source.owner.clone(), source.repo.clone()))
            .or_insert_with(|| GitHubClient::new(&source.owner, &source.repo));

        let releases = client.releases(limit).await.map_err(map_err)?;

        let mut versions = Vec::new();
        for release in releases {
            let Some(asset) = release.assets.iter().find(|a| a.name == PLUGIN_META_FILE) else {
                continue;
            };

            let text = client
                .get_text(&asset.download_url)
                .await
                .map_err(map_err)?;

            match serde_json::from_str::<PluginVersionMeta>(&text) {
                Ok(meta) => versions.push(meta),
                Err(e) => {
                    tracing::warn!(
                        "Skipping release {} with invalid {PLUGIN_META_FILE}: {e}",
                        release.tag_name
                    );
                }
            }
        }

        Ok(versions)
    }

    async fn download(
        &self,
        artifact: &PluginArtifact,
        dest: &Path,
    ) -> Result<(), PluginRegistryError> {
        // 下载 URL 是绝对地址，任一客户端均可拉取（GitHubClient 的 owner/repo 仅用于路径构造）
        let client = GitHubClient::new("", "");
        client
            .download_binary(&artifact.url, dest)
            .await
            .map_err(map_err)?;

        // 下载完成后立即校验 sha256（防御性完整性检查，签名校验在安装流水线执行）
        let actual = crate::updater::github::sha256_hex(dest).map_err(map_err)?;
        if !actual.eq_ignore_ascii_case(&artifact.sha256) {
            let _ = std::fs::remove_file(dest);
            return Err(PluginRegistryError::ChecksumMismatch {
                expected: artifact.sha256.clone(),
                actual,
            });
        }

        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

    const CATALOG_BODY: &str = r#"{"schemaVersion":1,"plugins":[
        {"name":"slack","publisher":"easybot","owner":"EasyIndie","repo":"easybot-plugin-slack","verified":true}
    ]}"#;

    fn plugin_meta_body(version: &str) -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1,
            "name": "slack",
            "version": version,
            "sdkVersion": 1,
            "publisher": "easybot",
            "tag": format!("v{version}"),
            "channel": "stable",
            "artifacts": {
                "x86_64-unknown-linux-musl": {
                    "url": "https://example.com/libslack.so",
                    "size": 42,
                    "sha256": "abc",
                    "signature": "c2ln"
                }
            }
        })
    }

    #[tokio::test]
    async fn test_catalog_parses_and_finds() {
        let mock = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::path(
                "/repos/EasyIndie/marketplace/contents/catalog.json",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(CATALOG_BODY))
            .mount(&mock)
            .await;

        let registry = GitHubRegistry::with_base_url("EasyIndie", "marketplace", &mock.uri());
        let catalog = registry.catalog().await.expect("catalog should parse");
        assert_eq!(catalog.plugins.len(), 1);
        let source = catalog
            .find(Some("easybot"), "slack")
            .expect("qualified lookup should find slack");
        assert_eq!(source.repo, "easybot-plugin-slack");
        assert!(source.verified);

        assert!(catalog.find(Some("evil"), "slack").is_none());
    }

    #[tokio::test]
    async fn test_versions_for_parses_plugin_meta() {
        let mock = MockServer::start().await;

        let release_body = vec![
            serde_json::json!({
                "tag_name": "v1.0.0",
                "html_url": "",
                "body": "",
                "published_at": null,
                "assets": [{
                    "name": "easybot-plugin.json",
                    "size": 200,
                    "browser_download_url": format!("{}/assets/plugin.json", mock.uri())
                }]
            }),
            serde_json::json!({
                "tag_name": "v0.9.0",
                "html_url": "",
                "body": "",
                "published_at": null,
                "assets": []
            }),
        ];

        Mock::given(matchers::method("GET"))
            .and(matchers::path(
                "/repos/EasyIndie/easybot-plugin-slack/releases",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(&release_body))
            .mount(&mock)
            .await;

        Mock::given(matchers::method("GET"))
            .and(matchers::path("/assets/plugin.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(plugin_meta_body("1.0.0")))
            .mount(&mock)
            .await;

        let registry = GitHubRegistry::with_base_url("EasyIndie", "marketplace", &mock.uri());
        // 注入插件仓库的 mock 客户端（base_url 同样指向 mock）
        {
            let mut clients = registry.clients.lock().await;
            clients.insert(
                ("EasyIndie".to_string(), "easybot-plugin-slack".to_string()),
                GitHubClient::with_base_url("EasyIndie", "easybot-plugin-slack", &mock.uri()),
            );
        }
        let source = PluginSource {
            name: "slack".into(),
            publisher: "easybot".into(),
            owner: "EasyIndie".into(),
            repo: "easybot-plugin-slack".into(),
            display_name: None,
            description: None,
            tags: vec![],
            verified: true,
        };

        let versions = registry
            .versions_for(&source, 10)
            .await
            .expect("versions_for should parse");
        assert_eq!(versions.len(), 1, "release without asset should be skipped");
        assert_eq!(versions[0].version, "1.0.0");
        assert_eq!(versions[0].sdk_version, 1);
    }

    #[tokio::test]
    async fn test_download_verifies_sha256() {
        let mock = MockServer::start().await;
        let payload = b"fake-dylib-bytes";
        let sha = crate::updater::github::sha256_hex_bytes(payload);

        Mock::given(matchers::method("GET"))
            .and(matchers::path("/download/libdemo.so"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.to_vec()))
            .mount(&mock)
            .await;

        let registry = GitHubRegistry::with_base_url("EasyIndie", "marketplace", &mock.uri());
        let artifact = PluginArtifact {
            url: format!("{}/download/libdemo.so", mock.uri()),
            size: payload.len() as u64,
            sha256: sha.clone(),
            signature: Some("c2ln".into()),
            public_key: Some("cHVibGlj".into()),
            library: Some("libdemo.so".into()),
        };

        // 历史：本测试偶发 "sha256 of empty"（空串哈希 e3b0c442…）失败，曾被当作
        // wiremock/hyper 空应答竞争并以 8 次重试容忍。真因不在 HTTP 层：tokio::fs::File
        // 的 write_all 在字节拷入内部缓冲、派发后台阻塞写任务后即返回，`download_binary`
        // 未 flush 就返回，随后的 sha256 校验在负载下读到尚未落盘的空文件。修复 =
        // download_binary 返回前 `flush()`（见 updater/github.rs），并把 0 字节响应判为
        // 失败。因此本测试断言单次下载即成功——若再失败，是新缺陷，应修根因而非加重试。
        let dest = std::env::temp_dir().join(format!("registry-dl-{}", std::process::id()));
        registry
            .download(&artifact, &dest)
            .await
            .expect("download+verify should pass on first attempt");
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        let _ = std::fs::remove_file(&dest);
    }

    #[tokio::test]
    async fn test_download_sha256_mismatch_fails() {
        let mock = MockServer::start().await;

        Mock::given(matchers::method("GET"))
            .and(matchers::path("/download/libdemo.so"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"tampered"))
            .mount(&mock)
            .await;

        let registry = GitHubRegistry::with_base_url("EasyIndie", "marketplace", &mock.uri());
        let artifact = PluginArtifact {
            url: format!("{}/download/libdemo.so", mock.uri()),
            size: 8,
            sha256: "deadbeef".into(),
            signature: None,
            public_key: None,
            library: Some("libdemo.so".into()),
        };

        let dest = std::env::temp_dir().join(format!("registry-bad-{}", std::process::id()));
        let _ = std::fs::remove_file(&dest);
        let err = registry.download(&artifact, &dest).await.unwrap_err();
        assert!(matches!(err, PluginRegistryError::ChecksumMismatch { .. }));
        assert!(!dest.exists(), "failed download must clean up temp file");
    }
}
