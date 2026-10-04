//! 报文类型与方法名常量。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 协议方法名（宿主 → 插件的请求 + 插件 → 宿主的通知）。
pub mod methods {
    // ── 生命周期 ──
    /// 握手：插件回传协议版本与能力。
    pub const HANDSHAKE: &str = "handshake";
    /// 初始化（不建立网络连接）。
    pub const INIT: &str = "init";
    /// 连接。
    pub const CONNECT: &str = "connect";
    /// 断开。
    pub const DISCONNECT: &str = "disconnect";
    /// 仅重启后台传输任务（不重新鉴权）。
    pub const RETRY_TRANSPORT: &str = "retry_transport";
    /// 宿主请求插件退出。
    pub const SHUTDOWN: &str = "shutdown";

    // ── 出站 ──
    /// 发送文本消息。
    pub const SEND: &str = "send";
    /// 发送媒体消息。
    pub const SEND_MEDIA: &str = "send_media";
    /// 发送媒体组 / 相册。
    pub const SEND_MEDIA_GROUP: &str = "send_media_group";
    /// 发送交互式（卡片）消息。
    pub const SEND_INTERACTIVE: &str = "send_interactive";
    /// 发送输入指示器。
    pub const SEND_TYPING: &str = "send_typing";
    /// 发送流式草稿。
    pub const SEND_DRAFT: &str = "send_draft";
    /// 应答按钮回调。
    pub const ANSWER_CALLBACK: &str = "answer_callback";
    /// 编辑消息。
    pub const EDIT_MESSAGE: &str = "edit_message";
    /// 删除消息。
    pub const DELETE_MESSAGE: &str = "delete_message";

    // ── 查询 / 富化 ──
    /// 获取聊天信息。
    pub const GET_CHAT_INFO: &str = "get_chat_info";
    /// 列出聊天。
    pub const LIST_CHATS: &str = "list_chats";
    /// 富化会话来源。
    pub const ENRICH_SOURCE: &str = "enrich_source";
    /// 健康报告。
    pub const HEALTH: &str = "health";

    // ── 游标持久化 ──
    /// 读取重连游标状态。
    pub const CURSOR_STATE: &str = "cursor_state";
    /// 恢复重连游标状态。
    pub const RESTORE_CURSOR_STATE: &str = "restore_cursor_state";

    // ── 通知（插件 → 宿主，无 id） ──
    /// 推送网关事件（宿主发布到事件总线）。
    pub const NOTIFY_EVENT: &str = "event";
    /// 推送日志（可选）。
    pub const NOTIFY_LOG: &str = "log";
}

/// 宿主 → 插件的请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 请求 id（插件须在响应中原样回传）。
    pub id: u64,
    /// 方法名，见 [`methods`]。
    pub method: String,
    /// 参数（方法相关）。
    #[serde(default)]
    pub params: Value,
}

impl Request {
    /// 构造请求。
    pub fn new(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            id,
            method: method.into(),
            params,
        }
    }
}

/// 插件 → 宿主的响应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// 对应请求 id。
    pub id: u64,
    /// 是否成功。
    pub ok: bool,
    /// 成功时的结果。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// 失败时的错误。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorPayload>,
}

impl Response {
    /// 构造成功响应。
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// 构造失败响应。
    pub fn err(id: u64, message: impl Into<String>, kind: Option<ErrorKind>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(ErrorPayload {
                message: message.into(),
                kind,
            }),
        }
    }
}

/// 错误载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorPayload {
    /// 人类可读错误信息。
    pub message: String,
    /// 错误分类（宿主据此决定重试或停用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ErrorKind>,
}

/// 错误分类，与宿主 `GatewayError` 的健康监测分类对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// 瞬态（网络/限流/超时）→ 宿主退避重试。
    Transient,
    /// 永久（凭据被拒/参数非法）→ 宿主停用适配器。
    Permanent,
    /// 能力不支持。
    Unsupported,
}

/// 插件 → 宿主的通知（无 id，不期望响应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    /// 通知方法名，见 [`methods`] 的 `NOTIFY_*`。
    pub method: String,
    /// 参数。
    #[serde(default)]
    pub params: Value,
}

impl Notification {
    /// 构造事件通知。
    pub fn event(event_type: &str, data: Value) -> Self {
        Self {
            method: methods::NOTIFY_EVENT.to_string(),
            params: serde_json::json!({ "event": event_type, "data": data }),
        }
    }

    /// 构造日志通知。
    pub fn log(level: &str, message: &str) -> Self {
        Self {
            method: methods::NOTIFY_LOG.to_string(),
            params: serde_json::json!({ "level": level, "message": message }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let req = Request::new(7, methods::SEND, serde_json::json!({"chat_id": "c1"}));
        let line = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), req);
    }

    #[test]
    fn request_defaults_missing_params() {
        let req: Request = serde_json::from_str(r#"{"id":1,"method":"connect"}"#).unwrap();
        assert_eq!(req.params, Value::Null);
    }

    #[test]
    fn response_ok_omits_error_and_roundtrips() {
        let res = Response::ok(1, serde_json::json!({"ok": true}));
        let line = serde_json::to_string(&res).unwrap();
        assert!(!line.contains("error"), "None 字段不应序列化: {line}");
        assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), res);
    }

    #[test]
    fn response_err_omits_result_and_carries_kind() {
        let res = Response::err(2, "boom", Some(ErrorKind::Transient));
        let line = serde_json::to_string(&res).unwrap();
        assert!(!line.contains("result"), "None 字段不应序列化: {line}");
        assert!(
            line.contains("\"kind\":\"transient\""),
            "kind 应序列化为 snake_case: {line}"
        );
        assert_eq!(serde_json::from_str::<Response>(&line).unwrap(), res);
    }

    #[test]
    fn notification_roundtrip() {
        let note = Notification::event("message.inbound", serde_json::json!({"chat_id": "c1"}));
        let line = serde_json::to_string(&note).unwrap();
        let parsed: Notification = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed.method, methods::NOTIFY_EVENT);
        assert_eq!(parsed.params["event"], "message.inbound");
    }

    #[test]
    fn error_kind_serialization() {
        assert_eq!(
            serde_json::to_string(&ErrorKind::Permanent).unwrap(),
            "\"permanent\""
        );
        assert_eq!(
            serde_json::to_string(&ErrorKind::Unsupported).unwrap(),
            "\"unsupported\""
        );
    }
}
