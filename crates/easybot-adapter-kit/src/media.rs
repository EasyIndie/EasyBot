//! 媒体下载 / 解码：**唯一**的「取媒体字节」实现。
//!
//! 改造前 4 个适配器各写一份，防护能力还互不一致：
//!
//! | 适配器 | 大小上限 | SSRF 校验 | 超时 |
//! |---|---|---|---|
//! | 飞书 | ✅ 流式 25 MB | ❌ | 仅靠 client 超时 |
//! | QQ | ⚠️ 只看 `Content-Length` | ❌ | — |
//! | Discord | ⚠️ 只看 `Content-Length` | ❌ | — |
//! | 微信 | ❌ 无（`resp.bytes()` 无界） | ✅ | ✅ 60s |
//!
//! `Content-Length` 不是保证：chunked 响应或伪造头就能绕过，随后的
//! `resp.bytes()` 会把整个响应读进内存 —— 一条恶意媒体链接即可打爆进程。
//!
//! 本模块把四件事绑在一起，任何适配器都无法「只拿一半」：
//!
//! 1. **SSRF 校验**（拒绝非 http(s)、内网/回环主机）；
//! 2. **流式计数的大小上限**（默认 25 MB，超过立即断开）；
//! 3. **总超时**（默认 60s，含读取 body）；
//! 4. **错误分类**（网络 → `Transient`，超限/非法 URL → `InvalidRequest`）。
//!
//! ```
//! # async fn demo(client: reqwest::Client) {
//! use easybot_adapter_kit::media;
//! let bytes = media::download(&client, "https://example.com/a.png").await.unwrap();
//! # let _ = bytes;
//! # }
//! ```

use std::time::Duration;

use easybot_core::GatewayError;
use easybot_core::types::message::MediaAttachment;

/// 默认下载上限（25 MB），与飞书/QQ 原有上限一致。
pub const DEFAULT_MAX_BYTES: u64 = 25 * 1024 * 1024;

/// 默认总超时（含 body 读取）。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// 显式放行内网/回环媒体 URL 的环境变量。
///
/// 默认**严格**：`localhost` / `127.0.0.1` / `0.0.0.0` / 云元数据端点一律拒绝
/// （否则 API 调用方可以让网关去抓自己的管理接口再转发到 IM 平台）。
///
/// 自建媒体服务器或端到端测试确实需要本地地址时，显式设置
/// `EASYBOT_ALLOW_PRIVATE_MEDIA_URLS=1` 即可放行 —— 这是个需要用户主动做出的决定，
/// 不会被某个默认配置悄悄打开。
pub const ALLOW_PRIVATE_ENV: &str = "EASYBOT_ALLOW_PRIVATE_MEDIA_URLS";

fn ssrf_validation_enabled() -> bool {
    match std::env::var(ALLOW_PRIVATE_ENV) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v == "1" || v == "true" || v == "yes")
        }
        Err(_) => true,
    }
}

/// 下载结果：字节 + 服务端声明的 `Content-Type`。
///
/// QQ / Discord 上传时需要用 `Content-Type` 决定文件名与 mime，
/// 因此与字节一起返回，避免适配器再发一次请求拿头。
#[derive(Debug, Clone)]
pub struct DownloadedMedia {
    /// 响应体字节（已按上限校验）。
    pub bytes: Vec<u8>,
    /// `Content-Type`（缺失时为 `None`）。
    pub content_type: Option<String>,
}

/// 下载媒体字节（默认上限与超时）。
pub async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, GatewayError> {
    download_with_limits(client, url, DEFAULT_MAX_BYTES, DEFAULT_TIMEOUT)
        .await
        .map(|m| m.bytes)
}

/// 下载媒体字节 + `Content-Type`（默认上限与超时）。
pub async fn download_full(
    client: &reqwest::Client,
    url: &str,
) -> Result<DownloadedMedia, GatewayError> {
    download_with_limits(client, url, DEFAULT_MAX_BYTES, DEFAULT_TIMEOUT).await
}

/// 下载媒体字节，显式指定上限与超时。
///
/// - SSRF 校验在发起请求**之前**执行；
/// - 响应体按 chunk 累加计数，`total > max_bytes` 立即中止（不依赖 `Content-Length`）；
/// - 整个请求 + 读取包在 `timeout` 内。
pub async fn download_with_limits(
    client: &reqwest::Client,
    url: &str,
    max_bytes: u64,
    timeout: Duration,
) -> Result<DownloadedMedia, GatewayError> {
    if ssrf_validation_enabled() {
        easybot_core::config::validate_url_for_ssrf(url).map_err(|_| {
            GatewayError::InvalidRequest("Media URL targets an internal/blocked host".to_string())
        })?;
    }

    let fut = read_body(client, url, max_bytes);
    match tokio::time::timeout(timeout, fut).await {
        Ok(result) => result,
        Err(_) => Err(GatewayError::Transient(format!(
            "Media download timed out after {timeout:?}"
        ))),
    }
}

async fn read_body(
    client: &reqwest::Client,
    url: &str,
    max_bytes: u64,
) -> Result<DownloadedMedia, GatewayError> {
    let resp = client.get(url).send().await.map_err(|e| {
        // 网络层失败必须包 Transient（CLAUDE.md 约定）
        crate::http::transport_error("download media", e)
    })?;

    if !resp.status().is_success() {
        return Err(GatewayError::Transient(format!(
            "Media download failed: HTTP {}",
            resp.status().as_u16()
        )));
    }

    // 快速失败：声明的大小已超限就没必要开始读（但**不能**只依赖它）
    if let Some(len) = resp.content_length()
        && len > max_bytes
    {
        return Err(GatewayError::InvalidRequest(format!(
            "Rejected media download: {len} bytes exceeds {max_bytes} limit"
        )));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let mut resp = resp;
    let mut total: u64 = 0;
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| crate::http::transport_error("read media body", e))?
    {
        total = total.saturating_add(chunk.len() as u64);
        if total > max_bytes {
            return Err(GatewayError::InvalidRequest(format!(
                "Rejected media download: exceeds {max_bytes} bytes limit"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(DownloadedMedia {
        bytes: buf,
        content_type,
    })
}

/// 把 base64 内联数据解码为字节。
pub fn decode_base64(data: &str) -> Result<Vec<u8>, GatewayError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| GatewayError::InvalidRequest(format!("base64 decode failed: {e}")))
}

/// 从 [`MediaAttachment`] 取字节：优先 `url`，其次内联 `data`。
///
/// 适配器发送媒体时统一走这里，避免「有的适配器校验 URL、有的不校验」。
pub async fn resolve(
    client: &reqwest::Client,
    media: &MediaAttachment,
) -> Result<Vec<u8>, GatewayError> {
    if let Some(url) = &media.url {
        download(client, url).await
    } else if let Some(data) = &media.data {
        decode_base64(data)
    } else {
        Err(GatewayError::InvalidRequest(
            "Media attachment has neither url nor data".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_internal_hosts_before_requesting() {
        let client = crate::http::default_client();
        for url in [
            "http://127.0.0.1/x.png",
            "http://localhost/x.png",
            "http://169.254.169.254/latest/meta-data",
            "file:///etc/passwd",
        ] {
            let err = download(&client, url).await.unwrap_err();
            assert!(
                matches!(err, GatewayError::InvalidRequest(_)),
                "{url} 应被 SSRF 校验拒绝, 实际 {err:?}"
            );
        }
    }

    #[test]
    fn decodes_base64_and_rejects_garbage() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert!(decode_base64("!!!not-base64!!!").is_err());
    }

    #[tokio::test]
    async fn resolve_prefers_url_and_reports_missing_data() {
        let client = crate::http::default_client();
        let media = MediaAttachment {
            media_type: easybot_core::types::message::MediaType::Image,
            url: Some("http://127.0.0.1/x.png".into()),
            data: None,
            mime_type: "image/png".into(),
            filename: None,
            caption: None,
            thumbnail_url: None,
            file_size: None,
            duration: None,
        };
        // URL 分支被执行（并被 SSRF 拒绝），而不是走 data
        assert!(matches!(
            resolve(&client, &media).await,
            Err(GatewayError::InvalidRequest(_))
        ));

        let inline = MediaAttachment {
            url: None,
            data: Some("aGVsbG8=".into()),
            ..media.clone()
        };
        assert_eq!(resolve(&client, &inline).await.unwrap(), b"hello");

        let empty = MediaAttachment {
            url: None,
            data: None,
            ..media
        };
        assert!(resolve(&client, &empty).await.is_err());
    }
}
