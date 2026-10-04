//! PoC/测试用**进程外插件**：通过 stdin/stdout 逐行 JSON 与宿主通信，**不依赖 dlopen**。
//!
//! 协议与类型见 `easybot-plugin-protocol`。此二进制供宿主侧集成测试拉起。

#![allow(missing_docs)]

use std::io::{BufRead, Write};

use easybot_plugin_protocol::{
    ConnectOutcome, ErrorKind, HandshakeResult, Notification, PROTOCOL_VERSION, Request, Response,
    SendOutcome, SendParams, methods,
};
use serde::Serialize;
use serde_json::json;

const PLATFORM: &str = "ipc-mock";
const DISPLAY_NAME: &str = "IPC Mock Adapter";

/// 写一行 JSON 并 flush。
fn write_msg(out: &mut impl Write, value: &impl Serialize) {
    if let Ok(line) = serde_json::to_string(value) {
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();

    for raw in stdin.lock().lines() {
        let Ok(raw) = raw else { break };
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        // 忽略无法解析的行，保持协议健壮
        let Ok(req) = serde_json::from_str::<Request>(raw) else {
            continue;
        };
        let id = req.id;

        match req.method.as_str() {
            methods::HANDSHAKE => {
                let hs = HandshakeResult {
                    protocol: PROTOCOL_VERSION,
                    platform: PLATFORM.to_string(),
                    display_name: DISPLAY_NAME.to_string(),
                    sdk_version: 1,
                    capabilities: Vec::new(),
                };
                write_msg(&mut out, &Response::ok(id, json!(hs)));
            }
            methods::INIT => {
                write_msg(&mut out, &Response::ok(id, json!({ "ok": true })));
                // 上报宿主注入的环境（P1-4：EASYBOT_HOME / EASYBOT_PLUGIN_DIR）
                write_msg(
                    &mut out,
                    &Notification::event(
                        "plugin.ready",
                        json!({
                            "home": std::env::var("EASYBOT_HOME").ok(),
                            "plugin_dir": std::env::var("EASYBOT_PLUGIN_DIR").ok(),
                        }),
                    ),
                );
            }
            methods::CONNECT => {
                write_msg(&mut out, &Response::ok(id, json!(ConnectOutcome::ok())));
                // 主动推送一条入站消息事件，验证"插件 → 宿主"的反向事件流
                write_msg(
                    &mut out,
                    &Notification::event(
                        "message.inbound",
                        json!({
                            "id": "plugin-msg-1",
                            "platform": PLATFORM,
                            "chat_id": "chat-1",
                            "text": "hello from out-of-process plugin",
                            "timestamp": 1
                        }),
                    ),
                );
                // 测试用：可令插件在 connect 后主动退出（模拟崩溃）
                if std::env::var("IPC_MOCK_EXIT_AFTER_CONNECT").is_ok() {
                    break;
                }
            }
            methods::SEND => match serde_json::from_value::<SendParams>(req.params.clone()) {
                Ok(p) => {
                    let message_id = format!("ipc-{}-{}", p.chat_id, p.text.len());
                    write_msg(
                        &mut out,
                        &Response::ok(id, json!(SendOutcome::ok(Some(message_id)))),
                    );
                }
                Err(e) => write_msg(
                    &mut out,
                    &Response::err(
                        id,
                        format!("bad send params: {e}"),
                        Some(ErrorKind::Permanent),
                    ),
                ),
            },
            methods::DISCONNECT => write_msg(&mut out, &Response::ok(id, json!({}))),
            // 回复后退出：宿主 `disconnect()` 会等待该响应
            methods::SHUTDOWN => {
                write_msg(&mut out, &Response::ok(id, json!({})));
                break;
            }
            other => write_msg(
                &mut out,
                &Response::err(
                    id,
                    format!("unknown method '{other}'"),
                    Some(ErrorKind::Unsupported),
                ),
            ),
        }
    }
}
