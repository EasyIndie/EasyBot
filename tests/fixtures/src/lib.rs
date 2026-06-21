//! 测试 Fixture 集
//!
//! 为所有测试用例提供确定性的、版本可控的最小合法媒体文件。
//! 每个文件包含正确的 magic bytes 和最小可用结构，总大小 < 300 字节。
//!
//! # 使用方式
//!
//! ```rust
//! use fixtures;
//!
//! // 获取 base64 编码的数据（用于 send_media data 模式）
//! let b64 = fixtures::image_base64();
//!
//! // 构建 MediaAttachment
//! let attachment = fixtures::image_attachment();
//!
//! // 构建完整 SendMediaParams（指定 chat_id）
//! let params = fixtures::send_image_params("chat-123");
//! ```

use base64::Engine;
use easybot_core::types::message::{MediaAttachment, MediaType, SendMediaParams};

// ── 原始字节（编译期嵌入） ──

/// PNG 图片（1x1 像素 RGBA，70 字节）
pub const IMAGE_BYTES: &[u8] = include_bytes!("../files/image.png");

/// MP3 音频帧（MPEG Audio Layer III 静音帧，102 字节）
pub const AUDIO_BYTES: &[u8] = include_bytes!("../files/audio.mp3");

/// MP4 视频（仅 ftyp + moov 初始化段，133 字节）
pub const VIDEO_BYTES: &[u8] = include_bytes!("../files/video.mp4");

/// PDF 文档（最小合法结构含 xref 和 trailer，298 字节）
pub const DOCUMENT_BYTES: &[u8] = include_bytes!("../files/document.pdf");

/// WebP 贴纸（RIFF + WEBP + VP8L 最小头，30 字节）
pub const STICKER_BYTES: &[u8] = include_bytes!("../files/sticker.webp");

/// GIF 动画（GIF89a 1x1 像素，36 字节）
pub const ANIMATION_BYTES: &[u8] = include_bytes!("../files/animation.gif");

// ── Base64 编码 ──

#[inline]
pub fn image_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(IMAGE_BYTES)
}

#[inline]
pub fn audio_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(AUDIO_BYTES)
}

#[inline]
pub fn video_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(VIDEO_BYTES)
}

#[inline]
pub fn document_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(DOCUMENT_BYTES)
}

#[inline]
pub fn sticker_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(STICKER_BYTES)
}

#[inline]
pub fn animation_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(ANIMATION_BYTES)
}

// ── MediaAttachment 构建器 ──

pub fn image_attachment() -> MediaAttachment {
    MediaAttachment {
        media_type: MediaType::Image,
        url: None,
        data: Some(image_base64()),
        mime_type: "image/png".to_string(),
        filename: Some("test_image.png".to_string()),
        caption: None,
        thumbnail_url: None,
        file_size: Some(IMAGE_BYTES.len() as u64),
        duration: None,
    }
}

pub fn audio_attachment() -> MediaAttachment {
    MediaAttachment {
        media_type: MediaType::Audio,
        url: None,
        data: Some(audio_base64()),
        mime_type: "audio/mpeg".to_string(),
        filename: Some("test_audio.mp3".to_string()),
        caption: None,
        thumbnail_url: None,
        file_size: Some(AUDIO_BYTES.len() as u64),
        duration: Some(0.5),
    }
}

pub fn video_attachment() -> MediaAttachment {
    MediaAttachment {
        media_type: MediaType::Video,
        url: None,
        data: Some(video_base64()),
        mime_type: "video/mp4".to_string(),
        filename: Some("test_video.mp4".to_string()),
        caption: None,
        thumbnail_url: None,
        file_size: Some(VIDEO_BYTES.len() as u64),
        duration: Some(0.5),
    }
}

pub fn document_attachment() -> MediaAttachment {
    MediaAttachment {
        media_type: MediaType::Document,
        url: None,
        data: Some(document_base64()),
        mime_type: "application/pdf".to_string(),
        filename: Some("test_document.pdf".to_string()),
        caption: None,
        thumbnail_url: None,
        file_size: Some(DOCUMENT_BYTES.len() as u64),
        duration: None,
    }
}

pub fn sticker_attachment() -> MediaAttachment {
    MediaAttachment {
        media_type: MediaType::Sticker,
        url: None,
        data: Some(sticker_base64()),
        mime_type: "image/webp".to_string(),
        filename: Some("test_sticker.webp".to_string()),
        caption: None,
        thumbnail_url: None,
        file_size: Some(STICKER_BYTES.len() as u64),
        duration: None,
    }
}

pub fn animation_attachment() -> MediaAttachment {
    MediaAttachment {
        media_type: MediaType::Animation,
        url: None,
        data: Some(animation_base64()),
        mime_type: "image/gif".to_string(),
        filename: Some("test_animation.gif".to_string()),
        caption: None,
        thumbnail_url: None,
        file_size: Some(ANIMATION_BYTES.len() as u64),
        duration: Some(1.0),
    }
}

// ── 按 MediaType 分发 ──

/// 根据 MediaType 返回对应的默认测试用 MediaAttachment。
pub fn attachment_for_type(media_type: MediaType) -> MediaAttachment {
    match media_type {
        MediaType::Image => image_attachment(),
        MediaType::Audio => audio_attachment(),
        MediaType::Video => video_attachment(),
        MediaType::Document => document_attachment(),
        MediaType::Sticker => sticker_attachment(),
        MediaType::Animation => animation_attachment(),
    }
}

// ── SendMediaParams 快捷构建器 ──

pub fn send_image_params(chat_id: impl Into<String>) -> SendMediaParams {
    SendMediaParams {
        chat_id: chat_id.into(),
        media: image_attachment(),
        text: None,
        reply_to: None,
    }
}

pub fn send_audio_params(chat_id: impl Into<String>) -> SendMediaParams {
    SendMediaParams {
        chat_id: chat_id.into(),
        media: audio_attachment(),
        text: None,
        reply_to: None,
    }
}

pub fn send_video_params(chat_id: impl Into<String>) -> SendMediaParams {
    SendMediaParams {
        chat_id: chat_id.into(),
        media: video_attachment(),
        text: None,
        reply_to: None,
    }
}

pub fn send_document_params(chat_id: impl Into<String>) -> SendMediaParams {
    SendMediaParams {
        chat_id: chat_id.into(),
        media: document_attachment(),
        text: None,
        reply_to: None,
    }
}

// ── 自测 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_bytes_non_empty() {
        assert!(!IMAGE_BYTES.is_empty());
        assert!(!AUDIO_BYTES.is_empty());
        assert!(!VIDEO_BYTES.is_empty());
        assert!(!DOCUMENT_BYTES.is_empty());
        assert!(!STICKER_BYTES.is_empty());
        assert!(!ANIMATION_BYTES.is_empty());
    }

    #[test]
    fn test_all_bytes_under_500() {
        // All fixture files are intentionally tiny
        assert!(IMAGE_BYTES.len() <= 500);
        assert!(AUDIO_BYTES.len() <= 500);
        assert!(VIDEO_BYTES.len() <= 500);
        assert!(DOCUMENT_BYTES.len() <= 500);
        assert!(STICKER_BYTES.len() <= 500);
        assert!(ANIMATION_BYTES.len() <= 500);
    }

    #[test]
    fn test_magic_bytes() {
        // PNG signature: 89 50 4E 47 0D 0A 1A 0A
        assert_eq!(&IMAGE_BYTES[0..8], b"\x89PNG\r\n\x1a\n");
        // MP3 frame sync: 0xFF 0xFB (or 0xFF 0xFA, 0xFF 0xF3, etc.)
        assert_eq!(AUDIO_BYTES[0], 0xFF);
        assert!(AUDIO_BYTES[1] & 0xE0 == 0xE0, "MP3 sync bits should be 111");
        // MP4 ftyp box at offset 4
        assert_eq!(&VIDEO_BYTES[4..8], b"ftyp");
        // PDF header
        assert!(DOCUMENT_BYTES.starts_with(b"%PDF-"));
        // WebP RIFF container: "RIFF" + size + "WEBP"
        assert_eq!(&STICKER_BYTES[0..4], b"RIFF");
        assert_eq!(&STICKER_BYTES[8..12], b"WEBP");
        // GIF89a
        assert!(ANIMATION_BYTES.starts_with(b"GIF89a"));
    }

    #[test]
    fn test_base64_roundtrip() {
        for b64 in [
            image_base64(),
            audio_base64(),
            video_base64(),
            document_base64(),
            sticker_base64(),
            animation_base64(),
        ] {
            base64::engine::general_purpose::STANDARD
                .decode(&b64)
                .expect("base64 should decode");
        }
    }

    #[test]
    fn test_attachment_for_type_covers_all_variants() {
        use MediaType::*;
        for mt in [Image, Audio, Video, Document, Sticker, Animation] {
            let a = attachment_for_type(mt);
            assert_eq!(a.media_type, mt, "incorrect type for {mt:?}");
            assert!(a.data.is_some(), "{mt:?} should have base64 data");
            assert!(!a.mime_type.is_empty(), "{mt:?} should have mime_type");
            assert!(a.filename.is_some(), "{mt:?} should have filename");
        }
    }

    #[test]
    fn test_attachments_have_sensible_defaults() {
        let img = image_attachment();
        assert_eq!(img.media_type, MediaType::Image);
        assert_eq!(img.mime_type, "image/png");
        assert!(img.filename.as_ref().unwrap().ends_with(".png"));

        let audio = audio_attachment();
        assert!(audio.duration.is_some());

        let anim = animation_attachment();
        assert_eq!(anim.mime_type, "image/gif");
    }
}
