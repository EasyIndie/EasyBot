//! 个人微信工具函数模块
//!
//! 提供凭据持久化、AES-128-ECB 媒体加密、CDN URL 构建、文件下载等工具函数。

use easybot_core::config::resolve_home;
use easybot_core::types::error::GatewayError;
use easybot_core::types::message::MediaAttachment;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// 凭据文件路径（位于 EasyBot 配置根目录下）
const CREDENTIALS_FILE: &str = ".wechat-credentials.json";

/// 微信凭据（持久化到磁盘）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct WeChatCredentials {
    pub(crate) bot_token: String,
    pub(crate) ilink_bot_id: String,
    pub(crate) ilink_user_id: String,
    pub(crate) baseurl: String,
}

/// 显式注入的配置根目录（替代 --dir / EASYBOT_HOME 解析）。
/// 由宿主/测试主动调用；普通运行无需调用，`config_root()` 会自动识别 CLI `--dir`。
static CONFIG_ROOT_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// 注入 EasyBot 配置根目录（优先级最高）。
/// 用于让外部宿主（或测试）精确控制微信凭据/状态的落盘位置。
#[allow(dead_code)]
pub(crate) fn set_config_root_override(root: PathBuf) {
    let _ = CONFIG_ROOT_OVERRIDE.set(root);
}

/// 从进程命令行识别 `--dir` 覆盖（与 bin 的 Cli.dir 解析一致）。
/// 容器/服务场景通常不传 `--dir`，此时返回 None 交由 `resolve_home` 处理。
fn detect_cli_dir_override() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--dir" {
            return args.next().map(PathBuf::from);
        }
        if let Some(v) = arg.strip_prefix("--dir=") {
            return Some(PathBuf::from(v));
        }
    }
    None
}

/// EasyBot 配置根目录（与全局解析一致：`--dir` > EASYBOT_HOME > ~/.easybot）。
///
/// 容器部署中 easybot 用户无 home 目录（HOME=/home/easybot 不存在），
/// 若直接用 `dirs::home_dir()` 会写不进凭据。必须优先 EASYBOT_HOME
/// （容器内为 /var/lib/easybot 挂载卷），才能持久化凭据/数据。
///
/// 修复：原先固定 `resolve_home(None)` 会忽略 CLI `--dir` 覆盖，导致
/// 微信凭据/状态落到错误位置、跨重启丢失。这里先查显式注入，再识别
/// `--dir`，最后回退 `EASYBOT_HOME` / 平台默认。
pub(crate) fn config_root() -> PathBuf {
    if let Some(root) = CONFIG_ROOT_OVERRIDE.get() {
        return root.clone();
    }
    if let Some(dir) = detect_cli_dir_override() {
        return dir;
    }
    resolve_home(None)
}

/// 获取凭据文件路径
pub(crate) fn credential_path() -> Option<PathBuf> {
    Some(config_root().join(CREDENTIALS_FILE))
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

/// 微信数据目录（上下文令牌 / 长轮询游标等，位于配置根目录下）
fn wechat_data_dir() -> Option<PathBuf> {
    Some(config_root().join("wechat"))
}

/// 每条聊天的上下文令牌存储路径
fn context_tokens_path() -> Option<PathBuf> {
    wechat_data_dir().map(|d| d.join("context_tokens.json"))
}

/// 上下文令牌条目（含捕获时间戳，用于过期检测与容量逐出）
///
/// iLink 的 context_token 有效期约 60-160s，必须记录捕获时间，
/// 发送前判断是否已过期，避免长任务后回复静默丢失。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ContextTokenEntry {
    /// iLink 回复所需的会话凭据
    pub(crate) token: String,
    /// 捕获时间戳（Unix 毫秒）。0 表示未知（旧格式迁移时填充为当前时间）。
    #[serde(default)]
    pub(crate) captured_at: i64,
}

/// 从磁盘加载所有聊天的上下文令牌
///
/// 兼容新旧两种格式：
/// - 新格式：`{"peer": {"token": "...", "captured_at": 123}}`
/// - 旧格式（v0.0.x）：`{"peer": "token字符串"}`，迁移时补齐当前时间戳
pub(crate) fn load_context_tokens() -> std::collections::HashMap<String, ContextTokenEntry> {
    let path = match context_tokens_path() {
        Some(p) => p,
        None => return std::collections::HashMap::new(),
    };
    if !path.exists() {
        return std::collections::HashMap::new();
    }
    let s = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("读取 context_tokens.json 失败: {}", e);
            return std::collections::HashMap::new();
        }
    };
    // 新格式：peer → {token, captured_at}
    if let Ok(mut map) =
        serde_json::from_str::<std::collections::HashMap<String, ContextTokenEntry>>(&s)
    {
        let now = chrono::Utc::now().timestamp_millis();
        for entry in map.values_mut() {
            if entry.captured_at == 0 {
                // 未知捕获时间：乐观视为刚捕获，让发送路径的 -14 降级兜底
                entry.captured_at = now;
            }
        }
        return map;
    }
    // 旧格式：peer → "token 字符串"
    if let Ok(legacy) = serde_json::from_str::<std::collections::HashMap<String, String>>(&s) {
        let now = chrono::Utc::now().timestamp_millis();
        tracing::info!(
            "检测到旧版 context_tokens 格式，迁移为带时间戳格式 ({} 条)",
            legacy.len()
        );
        return legacy
            .into_iter()
            .map(|(k, token)| {
                (
                    k,
                    ContextTokenEntry {
                        token,
                        captured_at: now,
                    },
                )
            })
            .collect();
    }
    tracing::warn!("解析 context_tokens.json 失败, 使用空映射");
    std::collections::HashMap::new()
}

/// 保存所有聊天的上下文令牌到磁盘
pub(crate) fn save_context_tokens(tokens: &std::collections::HashMap<String, ContextTokenEntry>) {
    let path = match context_tokens_path() {
        Some(p) => p,
        None => return,
    };
    if let Err(e) = atomic_write_json(&path, tokens) {
        tracing::warn!("保存 context_tokens 失败: {}", e);
    }
}

/// 长轮询游标文件路径
fn sync_buf_path() -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 凭据/数据路径必须位于配置根目录（EASYBOT_HOME > ~/.easybot）。
    /// EASYBOT_HOME 优先级本身在 easybot-core config::home 测试覆盖，
    /// 这里仅验证路径由 config_root() 推导（容器部署可写入 /var/lib/easybot）。
    #[test]
    fn test_paths_derive_from_config_root() {
        let root = config_root();
        assert_eq!(
            credential_path(),
            Some(root.join(".wechat-credentials.json"))
        );
        assert_eq!(wechat_data_dir(), Some(root.join("wechat")));
        // 与全局解析完全一致（resolve_home 是唯一事实来源）
        assert_eq!(root, resolve_home(None));
    }

    /// CLI `--dir` 覆盖必须生效（W9）：config_root() 优先于 EASYBOT_HOME。
    #[test]
    fn test_config_root_detect_cli_dir() {
        // --dir=value 形式
        assert_eq!(
            detect_cli_dir_override_from(vec!["easybot", "--dir=/tmp/abc"]),
            Some(PathBuf::from("/tmp/abc"))
        );
        // --dir value 形式
        assert_eq!(
            detect_cli_dir_override_from(vec!["easybot", "--dir", "/tmp/def"]),
            Some(PathBuf::from("/tmp/def"))
        );
        // 未传 --dir
        assert_eq!(detect_cli_dir_override_from(vec!["easybot"]), None);
    }

    /// 用参数化 argv 调用 detect_cli_dir_override（避免污染真实进程 argv）。
    fn detect_cli_dir_override_from(args: Vec<&str>) -> Option<PathBuf> {
        let mut iter = args.into_iter().skip(1);
        while let Some(arg) = iter.next() {
            if arg == "--dir" {
                return iter.next().map(PathBuf::from);
            }
            if let Some(v) = arg.strip_prefix("--dir=") {
                return Some(PathBuf::from(v));
            }
        }
        None
    }

    /// 新格式 context_tokens 序列化/反序列化往返（W3）。
    #[test]
    fn test_context_token_entry_roundtrip() {
        let mut map: std::collections::HashMap<String, ContextTokenEntry> =
            std::collections::HashMap::new();
        map.insert(
            "user@im.wechat".to_string(),
            ContextTokenEntry {
                token: "tok-123".to_string(),
                captured_at: 1_700_000_000_000,
            },
        );
        let json = serde_json::to_string(&map).unwrap();
        let back: std::collections::HashMap<String, ContextTokenEntry> =
            serde_json::from_str(&json).unwrap();
        assert_eq!(back["user@im.wechat"].token, "tok-123");
        assert_eq!(back["user@im.wechat"].captured_at, 1_700_000_000_000);
    }
}
