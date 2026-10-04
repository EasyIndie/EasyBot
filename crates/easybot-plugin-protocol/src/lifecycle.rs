//! 生命周期与消息收发的强类型 params/result（宿主与 SDK 共用）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::message::ErrorKind;

/// `handshake` 的结果：插件自述。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeResult {
    /// 插件实现的协议版本，宿主须与 [`crate::PROTOCOL_VERSION`] 校验。
    pub protocol: u32,
    /// 平台唯一标识（参与会话 key）。
    pub platform: String,
    /// 人类可读显示名。
    pub display_name: String,
    /// SDK 版本。
    pub sdk_version: u32,
    /// 能力声明（`CapabilityName` 的字符串形式）。
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// `init` 的参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitParams {
    /// 适配器配置（宿主 `AdapterConfig` 的序列化结果）。
    #[serde(default)]
    pub config: Value,
    /// 插件数据目录（宿主根据 `--dir` / `EASYBOT_HOME` 解析；供插件持久化凭据）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

/// `connect` 的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectOutcome {
    /// 是否成功。
    pub ok: bool,
    /// 失败原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 失败分类（宿主健康监测据此决定重试或停用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
}

impl ConnectOutcome {
    /// 成功。
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            error_kind: None,
        }
    }

    /// 失败。
    pub fn failed(error: impl Into<String>, error_kind: Option<ErrorKind>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            error_kind,
        }
    }
}

/// `send` 的参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendParams {
    /// 目标会话。
    pub chat_id: String,
    /// 文本内容。
    pub text: String,
    /// 解析模式（`markdown` / `html`，可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<String>,
    /// 被回复消息 id（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// `send` / 媒体发送的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendOutcome {
    /// 是否成功。
    pub success: bool,
    /// 平台消息 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// 失败原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SendOutcome {
    /// 成功。
    pub fn ok(message_id: Option<String>) -> Self {
        Self {
            success: true,
            message_id,
            error: None,
        }
    }

    /// 失败。
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            success: false,
            message_id: None,
            error: Some(error.into()),
        }
    }
}

/// `send_media` 的参数。
///
/// 决策 D1=A：媒体**不内联**，宿主先把字节写入专用临时目录，这里只传路径；
/// 插件读取该路径后负责在成功后清理（或由宿主统一回收）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendMediaParams {
    /// 目标会话。
    pub chat_id: String,
    /// 本地文件路径（宿主创建的临时文件）。
    pub path: String,
    /// 媒体类型（`image` / `video` / `audio` / `document` / `sticker` / `animation`）。
    pub media_type: String,
    /// 文件名（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// 附文（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    /// 被回复消息 id（可选）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
}

/// `retry_transport` 的结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryTransportOutcome {
    /// 插件是否自行完成了轻量重启（`false` = 宿主应回退到完整 stop+start）。
    pub handled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let line = serde_json::to_string(value).unwrap();
        serde_json::from_str::<T>(&line).unwrap()
    }

    #[test]
    fn handshake_roundtrip() {
        let hs = HandshakeResult {
            protocol: 1,
            platform: "ipc-mock".into(),
            display_name: "IPC Mock".into(),
            sdk_version: 1,
            capabilities: vec!["messages.send".into()],
        };
        assert_eq!(roundtrip(&hs), hs);
    }

    #[test]
    fn handshake_defaults_capabilities_when_missing() {
        let hs: HandshakeResult = serde_json::from_str(
            r#"{"protocol":1,"platform":"p","display_name":"d","sdk_version":1}"#,
        )
        .unwrap();
        assert!(hs.capabilities.is_empty());
    }

    #[test]
    fn init_params_omits_home_when_none() {
        let p = InitParams {
            config: serde_json::json!({"enabled": true}),
            home: None,
        };
        let line = serde_json::to_string(&p).unwrap();
        assert!(!line.contains("home"), "None 字段不应序列化: {line}");
        assert_eq!(roundtrip(&p), p);
    }

    #[test]
    fn connect_outcome_variants() {
        assert!(roundtrip(&ConnectOutcome::ok()).ok);
        let f = ConnectOutcome::failed("nope", Some(ErrorKind::Permanent));
        let line = serde_json::to_string(&f).unwrap();
        assert!(line.contains("\"permanent\""));
        assert_eq!(roundtrip(&f), f);
    }

    #[test]
    fn send_params_and_outcome_roundtrip() {
        let p = SendParams {
            chat_id: "c1".into(),
            text: "hi".into(),
            parse_mode: Some("markdown".into()),
            reply_to: None,
        };
        assert_eq!(roundtrip(&p), p);

        let ok = SendOutcome::ok(Some("m1".into()));
        assert_eq!(roundtrip(&ok), ok);
        let failed = SendOutcome::failed("boom");
        assert_eq!(roundtrip(&failed), failed);
    }

    #[test]
    fn send_media_params_carries_path_not_bytes() {
        let p = SendMediaParams {
            chat_id: "c1".into(),
            path: "/tmp/easybot-media/abc.png".into(),
            media_type: "image".into(),
            filename: Some("abc.png".into()),
            caption: None,
            reply_to: None,
        };
        let line = serde_json::to_string(&p).unwrap();
        assert!(line.contains("/tmp/easybot-media/abc.png"));
        assert!(!line.contains("base64"), "D1=A：媒体不得内联 base64");
        assert_eq!(roundtrip(&p), p);
    }

    #[test]
    fn retry_transport_roundtrip() {
        assert_eq!(
            roundtrip(&RetryTransportOutcome { handled: true }),
            RetryTransportOutcome { handled: true }
        );
    }
}
