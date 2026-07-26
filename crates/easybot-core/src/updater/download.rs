//! 二进制下载与校验模块
//!
//! 协调 GitHub API 客户端完成下载 + SHA256 校验的完整流程。

use super::github::{GitHubClient, sha256_hex};
use super::types::{UpdateError, current_asset_name};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 下载并校验二进制文件
///
/// 完成以下步骤：
/// 1. 获取当前平台的 asset 名称和 checksums
/// 2. 从最新 release 中查找匹配的 asset
/// 3. 下载到临时路径
/// 4. SHA256 校验
///
/// 返回下载文件的路径（在 `{home}/.update/{uuid}`）。
pub async fn download_and_verify(
    github: &mut GitHubClient,
    home: &Path,
    tag: &str,
    release_assets: &[super::types::ReleaseAsset],
) -> Result<(PathBuf, String, u64), UpdateError> {
    // 1. 获取当前平台的 asset 名称
    let asset_name = current_asset_name()?;

    // 2. 获取 checksums
    let checksums = fetch_checksums_map(github, tag).await?;

    // 3. 查找匹配的 asset
    let asset = release_assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| {
            UpdateError::NetworkError(format!(
                "No matching asset for {} in release {}",
                asset_name, tag
            ))
        })?;

    // 4. 获取预期的 SHA256
    let expected_sha256 = checksums.get(&asset_name).cloned().unwrap_or_default();

    // 5. 创建下载目录
    let update_dir = home.join(".update");
    tokio::fs::create_dir_all(&update_dir)
        .await
        .map_err(UpdateError::IoError)?;

    // 6. 生成临时文件名（带随机后缀防冲突）
    let temp_name = format!("{}.{}", asset_name, uuid::Uuid::new_v4());
    let temp_path = update_dir.join(&temp_name);

    // 7. 下载
    tracing::info!("Downloading {} ({} bytes)...", asset.name, asset.size);
    github
        .download_binary(&asset.download_url, &temp_path)
        .await?;

    // 8. SHA256 校验
    let actual_sha256 = sha256_hex(&temp_path)?;

    if !expected_sha256.is_empty() && !actual_sha256.eq_ignore_ascii_case(&expected_sha256) {
        // 校验失败：删除临时文件
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(UpdateError::ChecksumMismatch {
            expected: expected_sha256,
            actual: actual_sha256,
        });
    }

    if !expected_sha256.is_empty() {
        tracing::info!("SHA256 checksum verified: {}", &actual_sha256[..16]);
    } else {
        tracing::warn!("No checksums.txt found, skipping SHA256 verification");
    }

    Ok((temp_path, expected_sha256, asset.size))
}

/// 获取 checksums 映射表
///
/// 先尝试从 GitHub Releases 获取，失败时返回空映射（跳过校验）。
async fn fetch_checksums_map(
    github: &mut GitHubClient,
    tag: &str,
) -> Result<HashMap<String, String>, UpdateError> {
    match github.checksums(tag).await {
        Ok(checksums) => Ok(checksums),
        Err(_) => {
            tracing::warn!("Failed to fetch checksums.txt, will skip SHA256 verification");
            Ok(HashMap::new())
        }
    }
}
