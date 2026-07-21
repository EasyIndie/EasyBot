//! GitHub API 客户端
//!
//! 从 GitHub Releases API 获取版本信息、发布清单和校验和。
//! 支持 `GITHUB_TOKEN` 环境变量将速率限制从 60 提升到 5000 次/小时。

use super::types::{ReleaseInfo, UpdateError, VersionManifest};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// GitHub Releases API 基础 URL
const GITHUB_API_BASE: &str = "https://api.github.com";
/// 缓存有效期（秒）
const CACHE_TTL_SECS: u64 = 300; // 5 分钟
/// 未认证速率限制：60 req/h。认证速率限制：5000 req/h
///
/// GitHub API 客户端
pub struct GitHubClient {
    client: reqwest::Client,
    owner: String,
    repo: String,
    base_url: String,
    #[allow(dead_code)]
    token: Option<String>,

    // 简单内存缓存
    release_cache: Option<(ReleaseInfo, Instant)>,
    manifest_cache: Option<(VersionManifest, Instant)>,
    checksums_cache: Option<(HashMap<String, String>, Instant)>,
}

impl GitHubClient {
    /// 创建新的 GitHub API 客户端
    ///
    /// 自动检查 `GITHUB_TOKEN` 环境变量以提升速率限制。
    pub fn new(owner: &str, repo: &str) -> Self {
        Self::with_base_url(owner, repo, GITHUB_API_BASE)
    }

    /// 创建带自定义 base URL 的客户端（用于测试）
    fn with_base_url(owner: &str, repo: &str, base_url: &str) -> Self {
        let token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("easybot-updater/1.0"),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/vnd.github.v3+json"),
        );
        if let Some(ref t) = token {
            let auth = format!("Bearer {}", t);
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&auth) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        GitHubClient {
            client,
            owner: owner.to_string(),
            repo: repo.to_string(),
            base_url: base_url.to_string(),
            token,
            release_cache: None,
            manifest_cache: None,
            checksums_cache: None,
        }
    }

    /// 获取最新 Release 信息
    pub async fn latest_release(&mut self) -> Result<ReleaseInfo, UpdateError> {
        // 检查缓存
        if let Some((ref cached, timestamp)) = self.release_cache
            && timestamp.elapsed() < Duration::from_secs(CACHE_TTL_SECS)
        {
            return Ok(cached.clone());
        }

        let url = format!(
            "{}/repos/{}/{}/releases/latest",
            self.base_url, self.owner, self.repo
        );

        let resp = self.client.get(&url).send().await?;

        // 检查速率限制
        if resp.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(UpdateError::RateLimited);
        }
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(UpdateError::Other(
                "No releases found for this repository".into(),
            ));
        }

        let status = resp.status();
        if !status.is_success() {
            return Err(UpdateError::NetworkError(format!(
                "GitHub API returned {}",
                status
            )));
        }

        let release: ReleaseInfo = resp.json().await?;

        // 更新缓存
        self.release_cache = Some((release.clone(), Instant::now()));

        Ok(release)
    }

    /// 获取版本清单文件内容
    ///
    /// 从指定 tag 的 release asset 中下载 `easybot-version.json`。
    pub async fn version_manifest(&mut self, tag: &str) -> Result<VersionManifest, UpdateError> {
        // 检查缓存
        if let Some((ref cached, timestamp)) = self.manifest_cache
            && timestamp.elapsed() < Duration::from_secs(CACHE_TTL_SECS)
        {
            return Ok(cached.clone());
        }

        // 先从 release 中找到 version.json 资产
        let release_url = format!(
            "{}/repos/{}/{}/releases/tags/{}",
            self.base_url, self.owner, self.repo, tag
        );

        let resp = self.client.get(&release_url).send().await?;
        if !resp.status().is_success() {
            return Err(UpdateError::NetworkError(format!(
                "Failed to fetch release {}: {}",
                tag,
                resp.status()
            )));
        }

        let release: ReleaseInfo = resp.json().await?;

        let asset_url = release
            .assets
            .iter()
            .find(|a| a.name == "easybot-version.json")
            .map(|a| a.download_url.clone());

        let url = match asset_url {
            Some(url) => url,
            None => {
                // 没有版本清单文件可能是旧版 release
                // 返回一个合理的默认值
                return Ok(VersionManifest {
                    version: tag.trim_start_matches('v').to_string(),
                    tag: tag.to_string(),
                    release_date: None,
                    schema_version: 1,
                    requires_db_migration: false,
                    migrations: Vec::new(),
                    requires_config_migration: false,
                    config_changes: Vec::new(),
                    breaking_changes: Vec::new(),
                    plugin_abi_version: 1,
                    min_upgradable_from: "0.0.10".to_string(),
                });
            }
        };

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            // 回退到默认值
            return Ok(VersionManifest {
                version: tag.trim_start_matches('v').to_string(),
                tag: tag.to_string(),
                release_date: None,
                schema_version: 1,
                requires_db_migration: false,
                migrations: Vec::new(),
                requires_config_migration: false,
                config_changes: Vec::new(),
                breaking_changes: Vec::new(),
                plugin_abi_version: 1,
                min_upgradable_from: "0.0.10".to_string(),
            });
        }

        let manifest: VersionManifest = resp.json().await?;
        self.manifest_cache = Some((manifest.clone(), Instant::now()));
        Ok(manifest)
    }

    /// 获取 checksums.txt 内容
    ///
    /// 返回 `{文件名: 十六进制哈希}` 映射。
    /// 需要先获取 release 信息以获取 asset URL。
    pub async fn checksums(&mut self, tag: &str) -> Result<HashMap<String, String>, UpdateError> {
        // 检查缓存
        if let Some((ref cached, timestamp)) = self.checksums_cache
            && timestamp.elapsed() < Duration::from_secs(CACHE_TTL_SECS)
        {
            return Ok(cached.clone());
        }

        // 获取 checksums.txt 的下载 URL
        let release_url = format!(
            "{}/repos/{}/{}/releases/tags/{}",
            self.base_url, self.owner, self.repo, tag
        );

        let resp = self.client.get(&release_url).send().await?;
        if !resp.status().is_success() {
            return Err(UpdateError::NetworkError(format!(
                "Failed to fetch release {}: {}",
                tag,
                resp.status()
            )));
        }

        let release: ReleaseInfo = resp.json().await?;

        let checksum_url = release
            .assets
            .iter()
            .find(|a| a.name == "checksums.txt")
            .map(|a| a.download_url.clone())
            .ok_or_else(|| {
                UpdateError::NetworkError("checksums.txt not found in release assets".into())
            })?;

        let resp = self.client.get(&checksum_url).send().await?;
        if !resp.status().is_success() {
            return Err(UpdateError::NetworkError(format!(
                "Failed to download checksums.txt: {}",
                resp.status()
            )));
        }

        let text = resp.text().await?;
        let checksums = parse_checksums(&text);

        self.checksums_cache = Some((checksums.clone(), Instant::now()));
        Ok(checksums)
    }

    /// 下载二进制文件到指定路径（流式）
    ///
    /// 使用 `reqwest` 分块流式写入，避免大文件加载到内存。
    pub async fn download_binary(
        &self,
        url: &str,
        dest: &std::path::Path,
    ) -> Result<(), UpdateError> {
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(UpdateError::NetworkError(format!(
                "Failed to download binary: {}",
                resp.status()
            )));
        }

        let total_size = resp.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;
        let mut last_log: u64 = 0;

        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(UpdateError::IoError)?;

        use tokio::io::AsyncWriteExt;

        // 使用 chunk() 逐块读取
        let mut stream = resp;
        while let Some(chunk) = stream.chunk().await.map_err(UpdateError::HttpError)? {
            file.write_all(&chunk).await.map_err(UpdateError::IoError)?;

            downloaded += chunk.len() as u64;

            // 每 10MB 记录一次进度
            if total_size > 0 && downloaded - last_log > 10_000_000 {
                let pct = (downloaded as f64 / total_size as f64) * 100.0;
                tracing::debug!(
                    "Downloaded {:.1}% ({}/{} MB)",
                    pct,
                    downloaded / 1_000_000,
                    total_size / 1_000_000
                );
                last_log = downloaded;
            }
        }

        // 校验下载完整性
        if total_size > 0 && downloaded != total_size {
            return Err(UpdateError::NetworkError(format!(
                "Download incomplete: {}/{} bytes",
                downloaded, total_size
            )));
        }

        tracing::info!(
            "Binary download complete: {} bytes from {}",
            downloaded,
            url
        );
        Ok(())
    }
}

/// 解析 checksums.txt 内容
///
/// 格式: `<hex_hash>  <filename>`（两空格分隔，与 sha256sum 输出一致）
pub fn parse_checksums(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 格式: "abc123  filename"
        if let Some((hash, name)) = line.split_once("  ") {
            let hash = hash.trim().to_string();
            let name = name.trim().to_string();
            if !hash.is_empty() && !name.is_empty() {
                map.insert(name, hash);
            }
        }
    }
    map
}

/// 计算文件的 SHA256 哈希（十六进制小写）
pub fn sha256_hex(path: &std::path::Path) -> Result<String, UpdateError> {
    use sha2::Digest;
    let data = std::fs::read(path)?;
    let hash = sha2::Sha256::digest(&data);
    Ok(hash.iter().map(|b| format!("{:02x}", b)).collect())
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_checksums_normal() {
        let content = "abc123def456  easybot-x86_64-unknown-linux-musl\n\
                       789abc012def  easybot-aarch64-unknown-linux-musl\n";
        let map = parse_checksums(content);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("easybot-x86_64-unknown-linux-musl").unwrap(),
            "abc123def456"
        );
        assert_eq!(
            map.get("easybot-aarch64-unknown-linux-musl").unwrap(),
            "789abc012def"
        );
    }

    #[test]
    fn test_parse_checksums_empty() {
        let map = parse_checksums("");
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_checksums_skips_bad_lines() {
        let content = "abc123  good\nbadline\n  \n  trailing_data_no_hash\n";
        let map = parse_checksums(content);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("good").unwrap(), "abc123");
    }

    #[test]
    fn test_sha256_hex_non_existent_file() {
        let result = sha256_hex(std::path::Path::new("/nonexistent/file"));
        assert!(result.is_err());
    }

    #[test]
    fn test_sha256_hex_empty_file() {
        let dir = std::env::temp_dir().join(format!("easybot_test_sha256_{}", std::process::id()));
        let file_path = dir.join("empty.bin");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(&file_path, b"").unwrap();

        let hash = sha256_hex(&file_path).unwrap();
        // SHA256 of empty string
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_github_client_new() {
        let client = GitHubClient::new("EasyIndie", "EasyBot");
        // 验证客户端创建成功
        assert_eq!(client.owner, "EasyIndie");
        assert_eq!(client.repo, "EasyBot");
    }

    #[tokio::test]
    async fn test_github_api_latest_release_mock() {
        // 启动 wiremock 服务器
        let mock_server = wiremock::MockServer::start().await;

        // Mock 最新 release 响应
        let release_body = serde_json::json!({
            "tag_name": "v0.1.0",
            "html_url": "https://github.com/EasyIndie/EasyBot/releases/tag/v0.1.0",
            "body": "Release notes",
            "published_at": "2026-07-22T00:00:00Z",
            "assets": [
                {
                    "name": "easybot-x86_64-unknown-linux-musl",
                    "size": 12345678,
                    "browser_download_url": "https://github.com/EasyIndie/EasyBot/releases/download/v0.1.0/easybot-x86_64-unknown-linux-musl"
                },
                {
                    "name": "checksums.txt",
                    "size": 512,
                    "browser_download_url": "https://github.com/EasyIndie/EasyBot/releases/download/v0.1.0/checksums.txt"
                }
            ]
        });

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/repos/EasyIndie/EasyBot/releases/latest",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(&release_body)
                    .insert_header("Content-Type", "application/json"),
            )
            .mount(&mock_server)
            .await;

        // 创建指向 mock 服务器的客户端
        let mut client = GitHubClient::with_base_url("EasyIndie", "EasyBot", &mock_server.uri());

        // 测试获取最新 release
        let release = client.latest_release().await.expect("Should fetch release");
        assert_eq!(release.tag_name, "v0.1.0");
        assert_eq!(release.assets.len(), 2);
        assert_eq!(release.assets[0].name, "easybot-x86_64-unknown-linux-musl");
    }

    #[tokio::test]
    async fn test_github_api_rate_limit() {
        let mock_server = wiremock::MockServer::start().await;

        // Mock 403 响应（速率限制）
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/repos/EasyIndie/EasyBot/releases/latest",
            ))
            .respond_with(wiremock::ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let mut client = GitHubClient::with_base_url("EasyIndie", "EasyBot", &mock_server.uri());

        let result = client.latest_release().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            UpdateError::RateLimited => {} // 预期的错误
            e => panic!("Expected RateLimited, got: {}", e),
        }
    }

    #[tokio::test]
    async fn test_github_api_not_found() {
        let mock_server = wiremock::MockServer::start().await;

        // Mock 404 响应
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/repos/EasyIndie/EasyBot/releases/latest",
            ))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let mut client = GitHubClient::with_base_url("EasyIndie", "EasyBot", &mock_server.uri());

        let result = client.latest_release().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_github_api_checksums_mock() {
        let mock_server = wiremock::MockServer::start().await;

        let checksums_content =
            "abc123def456  easybot-x86_64-unknown-linux-musl\n789abc012def  checksums.txt\n";

        // Mock release 列表返回
        let release_body = serde_json::json!({
            "tag_name": "v0.1.0",
            "html_url": "https://github.com/EasyIndie/EasyBot/releases/tag/v0.1.0",
            "body": "",
            "assets": [
                {
                    "name": "checksums.txt",
                    "size": 512,
                    "browser_download_url": format!("{}/assets/checksums.txt", mock_server.uri())
                }
            ]
        });

        // Mock release by tag
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/repos/EasyIndie/EasyBot/releases/tags/v0.1.0",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(&release_body)
                    .insert_header("Content-Type", "application/json"),
            )
            .mount(&mock_server)
            .await;

        // Mock checksums.txt asset
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/assets/checksums.txt"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(checksums_content))
            .mount(&mock_server)
            .await;

        let mut client = GitHubClient::with_base_url("EasyIndie", "EasyBot", &mock_server.uri());

        let checksums = client
            .checksums("v0.1.0")
            .await
            .expect("Should fetch checksums");
        assert_eq!(checksums.len(), 2);
        assert_eq!(
            checksums.get("easybot-x86_64-unknown-linux-musl").unwrap(),
            "abc123def456"
        );
    }
}
