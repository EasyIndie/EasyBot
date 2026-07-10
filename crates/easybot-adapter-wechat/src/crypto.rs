//! 个人微信工具函数模块
//!
//! 提供凭据持久化、AES-128-ECB 媒体加密、CDN URL 构建、文件下载等工具函数。

use easybot_core::types::error::GatewayError;
use easybot_core::types::message::MediaAttachment;
use std::time::Duration;

/// 凭据文件路径（相对于 home 目录的 .easybot/）
const CREDENTIALS_FILE: &str = ".easybot/.wechat-credentials.json";

/// 微信凭据（持久化到磁盘）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct WeChatCredentials {
    pub(crate) bot_token: String,
    pub(crate) ilink_bot_id: String,
    pub(crate) ilink_user_id: String,
    pub(crate) baseurl: String,
}

/// 获取凭据文件路径
pub(crate) fn credential_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(CREDENTIALS_FILE))
}

/// 从磁盘加载凭据
pub(crate) fn load_credentials_from_disk() -> Option<WeChatCredentials> {
    let path = credential_path()?;
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).ok(),
        Err(_) => None,
    }
}

/// 保存凭据到磁盘
pub(crate) fn save_credentials_to_disk(creds: &WeChatCredentials) {
    let path = match credential_path() {
        Some(p) => p,
        None => {
            tracing::warn!("无法确定凭据文件路径");
            return;
        }
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(creds) {
        Ok(json) => match std::fs::write(&path, &json) {
            Ok(_) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
                tracing::info!("个人微信凭据已保存到 {:?}", path);
            }
            Err(e) => tracing::warn!("保存凭据失败: {}", e),
        },
        Err(e) => tracing::warn!("序列化凭据失败: {}", e),
    }
}

/// 清除磁盘上的凭据文件（当 bot_token 过期/失效时调用）
/// 使下次 init() 无法从磁盘恢复凭据，从而触发重新扫码登录
pub(crate) fn clear_credentials_from_disk() {
    let path = match credential_path() {
        Some(p) => p,
        None => {
            tracing::warn!("无法确定凭据文件路径");
            return;
        }
    };
    if path.exists() {
        match std::fs::remove_file(&path) {
            Ok(_) => tracing::info!("个人微信过期凭据已清除: {:?}", path),
            Err(e) => tracing::warn!("清除凭据文件失败: {} ({:?})", e, path),
        }
    }
}

/// 原子写入 JSON 到磁盘（临时文件 + rename，防止写一半崩溃导致文件损坏）
pub(crate) fn atomic_write_json<T: serde::Serialize>(
    path: &std::path::Path,
    value: &T,
) -> std::io::Result<()> {
    let json =
        serde_json::to_string_pretty(value).map_err(|e| std::io::Error::other(e.to_string()))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, &json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// 微信数据目录
fn wechat_data_dir() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".easybot").join("wechat"))
}

/// 每条聊天的上下文令牌存储路径
fn context_tokens_path() -> Option<std::path::PathBuf> {
    wechat_data_dir().map(|d| d.join("context_tokens.json"))
}

/// 从磁盘加载所有聊天的上下文令牌
pub(crate) fn load_context_tokens() -> std::collections::HashMap<String, String> {
    let path = match context_tokens_path() {
        Some(p) => p,
        None => return std::collections::HashMap::new(),
    };
    if !path.exists() {
        return std::collections::HashMap::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            tracing::warn!("解析 context_tokens.json 失败 ({}), 使用空映射", e);
            std::collections::HashMap::new()
        }),
        Err(e) => {
            tracing::warn!("读取 context_tokens.json 失败: {}", e);
            std::collections::HashMap::new()
        }
    }
}

/// 保存所有聊天的上下文令牌到磁盘
pub(crate) fn save_context_tokens(tokens: &std::collections::HashMap<String, String>) {
    let path = match context_tokens_path() {
        Some(p) => p,
        None => return,
    };
    if let Err(e) = atomic_write_json(&path, tokens) {
        tracing::warn!("保存 context_tokens 失败: {}", e);
    }
}

/// 长轮询游标文件路径
fn sync_buf_path() -> Option<std::path::PathBuf> {
    wechat_data_dir().map(|d| d.join("sync_buf.txt"))
}

/// 从磁盘加载长轮询游标
pub(crate) fn load_sync_buf() -> String {
    let path = match sync_buf_path() {
        Some(p) => p,
        None => return String::new(),
    };
    if !path.exists() {
        return String::new();
    }
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// 保存长轮询游标到磁盘
pub(crate) fn save_sync_buf(buf: &str) {
    let path = match sync_buf_path() {
        Some(p) => p,
        None => return,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, buf) {
        tracing::warn!("保存 sync_buf 失败: {}", e);
    }
}

pub(crate) fn base64_encode_uin(uin: u32) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(uin.to_le_bytes())
}

// ── AES-128-ECB 媒体加密工具函数 ──

/// PKCS7 填充
pub(crate) fn pkcs7_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let pad_len = block_size - (data.len() % block_size);
    let mut padded = Vec::with_capacity(data.len() + pad_len);
    padded.extend_from_slice(data);
    padded.resize(data.len() + pad_len, pad_len as u8);
    padded
}

/// AES-128-ECB 加密
pub(crate) fn aes_128_ecb_encrypt(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    use aes::cipher::{BlockCipherEncrypt, KeyInit};

    let cipher = aes::Aes128::new_from_slice(key).expect("AES-128 key must be 16 bytes");
    let padded = pkcs7_pad(plaintext, 16);
    let mut result = Vec::with_capacity(padded.len());

    for chunk in padded.chunks(16) {
        let mut block = aes::cipher::Block::<aes::Aes128>::default();
        block.copy_from_slice(chunk);
        cipher.encrypt_block(&mut block);
        result.extend_from_slice(&block);
    }

    result
}

/// 计算 AES 加密后的文件大小（含 PKCS7 填充）
pub(crate) fn aes_padded_size(raw_size: usize) -> usize {
    (raw_size + 1).div_ceil(16) * 16
}

/// 编码 AES key 为 iLink API 期望的格式
pub(crate) fn encode_aes_key_for_api(key: &[u8; 16]) -> String {
    use base64::Engine;
    let hex_str: String = key.iter().map(|b| format!("{:02x}", b)).collect();
    base64::engine::general_purpose::STANDARD.encode(hex_str.as_bytes())
}

/// 生成新的 32 位十六进制 filekey
pub(crate) fn generate_filekey() -> String {
    let uuid = uuid::Uuid::new_v4();
    let hex_str: String = uuid
        .as_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    hex_str[..32].to_string()
}

/// 构建 CDN 上传 URL
pub(crate) fn build_cdn_upload_url(cdn_base: &str, upload_param: &str, filekey: &str) -> String {
    let encoded_param = url_encode_for_cdn(upload_param);
    format!(
        "{}/upload?encrypted_query_param={}&filekey={}",
        cdn_base.trim_end_matches('/'),
        encoded_param,
        filekey
    )
}

/// CDN URL 百分号编码
pub(crate) fn url_encode_for_cdn(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                c.to_string()
            } else {
                format!("%{:02X}", c as u8)
            }
        })
        .collect()
}

/// 计算数据的 MD5 并返回十六进制字符串
pub(crate) fn md5_hex(data: &[u8]) -> String {
    format!("{:x}", md5::compute(data))
}

/// 从 URL 下载文件内容
pub(crate) async fn download_media(
    url: &str,
    client: &reqwest::Client,
) -> Result<Vec<u8>, GatewayError> {
    // SECURITY: Validate URL to prevent SSRF attacks
    if easybot_core::config::validate_url_for_ssrf(url).is_err() {
        return Err(GatewayError::Internal(
            "Media URL targets an internal/blocked host".into(),
        ));
    }

    let resp =
        client.get(url).send().await.map_err(|e| {
            GatewayError::Internal(format!("Failed to download media from URL: {}", e))
        })?;

    if !resp.status().is_success() {
        return Err(GatewayError::Internal(format!(
            "Failed to download media: HTTP {}",
            resp.status().as_u16()
        )));
    }

    tokio::time::timeout(Duration::from_secs(60), resp.bytes())
        .await
        .map_err(|_| GatewayError::Internal("Media download timeout (60s)".to_string()))?
        .map(|b| b.to_vec())
        .map_err(|e| GatewayError::Internal(format!("Failed to read media bytes: {}", e)))
}

/// 从 MediaAttachment 获取文件数据（优先 URL，其次 base64 data）
pub(crate) async fn resolve_media_data(
    media: &MediaAttachment,
    client: &reqwest::Client,
) -> Result<Vec<u8>, GatewayError> {
    if let Some(ref url) = media.url {
        download_media(url, client).await
    } else if let Some(ref b64_data) = media.data {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(b64_data)
            .map_err(|e| {
                GatewayError::Internal(format!("Failed to decode base64 media data: {}", e))
            })
    } else {
        Err(GatewayError::Internal(
            "Media attachment has neither url nor data".to_string(),
        ))
    }
}
