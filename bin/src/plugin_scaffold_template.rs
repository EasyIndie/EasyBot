//! `easybot plugin new` 脚手架的文件模板（T18 / DX-2）
//!
//! 所有模板以 const 字符串内嵌进二进制（离线可用，对齐
//! `generate_default_config` 写 gateway.yaml 的先例）。占位符统一使用
//! `__EASYBOT_*__` 形式，由 [`render`] 按 [`TemplateVars`] 替换。
//!
//! 模板产物为一个**独立可构建**的插件工程：SDK 走 git tag 依赖，
//! 作者无需 clone 主仓即可 `cargo build` / `cargo test`（DX-1 黄金路径）。

/// 替换占位符所需的变量。
pub struct TemplateVars {
    /// 插件名（kebab-case，同时是平台名与 Cargo 包名）。
    pub name: String,
    /// `name` 中 `-` 转 `_`（Rust lib 名 / 集成测试 import 路径）。
    pub crate_name: String,
    /// PascalCase 结构体名（如 `hello-adapter` → `HelloAdapter`）。
    pub struct_name: String,
    /// 人类可读显示名（如 `hello-adapter` → `Hello Adapter`）。
    pub display_name: String,
    /// 描述（写入 Cargo.toml / plugin.yaml / README）。
    pub description: String,
    /// 作者署名（写入 plugin.yaml / LICENSE）。
    pub author: String,
    /// SDK git 依赖 tag（`v{CARGO_PKG_VERSION}`，由编译时版本常量生成）。
    pub sdk_tag: String,
    /// SDK ABI 版本（`EASYBOT_PLUGIN_ABI_VERSION`）。
    pub sdk_version: u32,
}

/// 把模板中的 `__EASYBOT_*__` 占位符替换为实际值。
pub fn render(template: &str, vars: &TemplateVars) -> String {
    template
        .replace("__EASYBOT_NAME__", &vars.name)
        .replace("__EASYBOT_CRATE_NAME__", &vars.crate_name)
        .replace("__EASYBOT_STRUCT_NAME__", &vars.struct_name)
        .replace("__EASYBOT_DISPLAY_NAME__", &vars.display_name)
        .replace("__EASYBOT_DESCRIPTION__", &vars.description)
        .replace("__EASYBOT_AUTHOR__", &vars.author)
        .replace("__EASYBOT_SDK_TAG__", &vars.sdk_tag)
        .replace("__EASYBOT_SDK_VERSION__", &vars.sdk_version.to_string())
}

/// 生成后的工程目录布局（测试据此断言完整性）。
#[cfg(test)]
pub const FILES: &[&str] = &[
    "Cargo.toml",
    ".cargo/config.toml",
    "src/lib.rs",
    "plugin.yaml",
    "tests/unit.rs",
    "tests/host_test.rs",
    "README.md",
    ".gitignore",
    "LICENSE",
    ".github/workflows/plugin-publish.yml",
];

/// `Cargo.toml`：cdylib + SDK git tag 依赖 + release 对齐主仓。
pub const CARGO_TOML: &str = r#"# __EASYBOT_DESCRIPTION__
#
# EasyBot 插件工程（由 `easybot plugin new` 脚手架生成）。
# SDK 走 git tag 依赖——构建时自动拉取主仓，产物是自包含 cdylib，
# 作者无需 clone EasyBot 主仓。

[package]
name = "__EASYBOT_NAME__"
version = "0.1.0"
edition = "2024"
description = "__EASYBOT_DESCRIPTION__"
publish = false

[lib]
# cdylib：宿主用 libloading 进程内加载的共享库。
# rlib：让 `cargo test` 能链接本 crate（integration tests 需要）。
crate-type = ["cdylib", "rlib"]

[dependencies]
# SDK git 依赖。tag 由脚手架按当前 EasyBot 版本生成；升级 SDK 时改这里。
# `testing` feature 提供 PluginTestHost，供 `cargo test` 离线模拟宿主。
easybot-plugin-sdk = { git = "https://github.com/EasyIndie/EasyBot.git", tag = "__EASYBOT_SDK_TAG__", package = "easybot-plugin-sdk", features = ["testing"] }

serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
# 真实适配器的 connect 通常需要 tokio::spawn 建轮询/长连接任务（带并发上限）。
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }

[dev-dependencies]
# 无需额外依赖：PluginTestHost 已由 SDK testing feature 提供。

# 本地联调：把 SDK git 依赖替换为主仓本地 checkout（改 Cargo.toml，不动上面依赖行）。
# 取消注释并调整路径后 `cargo build` 即走本地 SDK：
# [patch."https://github.com/EasyIndie/EasyBot.git"]
# easybot-plugin-sdk = { path = "../EasyBot/crates/easybot-plugin-sdk" }

# release 对齐 EasyBot 主仓：fat LTO + strip + panic=abort。
# panic=abort 是硬要求——跨 FFI 边界不得 unwind，cdylib 内 panic 直接终止进程。
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = "symbols"
panic = "abort"
debug = 0
"#;

/// `.cargo/config.toml`：说明 `[patch]` 放 Cargo.toml（配置与清单分离）。
pub const CARGO_CONFIG: &str = r#"# Cargo 构建配置（对工程内所有 target 生效）。
#
# 注意：`[patch]` 属于 Cargo.toml 清单段，**不能**写在这里。
# 本地联调时请在 Cargo.toml 中取消注释 `[patch."https://github.com/EasyIndie/EasyBot.git"]`
# 并将 SDK 指向本地主仓 checkout（见 Cargo.toml 内注释）。
"#;

/// `src/lib.rs`：完整 `PlatformAdapter` 骨架（所有必需方法 + TODO 占位）。
pub const SRC_LIB_RS: &str = r#"//! __EASYBOT_DISPLAY_NAME__ — EasyBot 适配器插件（`easybot plugin new` 脚手架生成）
//!
//! 参考文档：
//!   - 快速上手：https://github.com/EasyIndie/EasyBot/blob/main/docs/plugin-quickstart.md
//!   - 完整指南：https://github.com/EasyIndie/EasyBot/blob/main/docs/plugin-guide.md
//!   - 方法论：  https://github.com/EasyIndie/EasyBot/blob/main/docs/plugin-methodology.md
//!
//! 构建 / 测试：
//!   cargo build --release     # 产出自包含 cdylib（git 依赖 SDK，无需 clone 主仓）
//!   cargo test                # 单元 + PluginTestHost 离线测试（无需启动真实网关）

use easybot_plugin_sdk::prelude::*;
use std::sync::Arc;

/// 适配器主体：持有状态与可选的事件总线。
///
/// TODO: 按平台需求添加字段——HTTP client、token、缓存（须带大小上限或 TTL，见
/// docs/plugin-methodology.md「缓存」一节）。
pub struct __EASYBOT_STRUCT_NAME__ {
    state: AdapterState,
    event_bus: Option<Arc<EventBus>>,
    // TODO: token: Option<String>,
    // TODO: client: Option<reqwest::Client>,
}

impl __EASYBOT_STRUCT_NAME__ {
    /// 构造器（`declare_plugin!` 的入口）。
    ///
    /// 若平台需要非默认初始化，可在此注入 HTTP client 等依赖——测试用 wiremock
    /// 替换（见 docs/plugin-methodology.md「传输可注入」）。
    pub fn new() -> Self {
        Self {
            state: AdapterState::Created,
            event_bus: None,
        }
    }
}

#[async_trait]
impl PlatformAdapter for __EASYBOT_STRUCT_NAME__ {
    fn platform_name(&self) -> &str {
        "__EASYBOT_NAME__"
    }

    fn display_name(&self) -> &str {
        "__EASYBOT_DISPLAY_NAME__"
    }

    fn capabilities(&self) -> &[Capability] {
        // 声明平台能力：宿主据此路由消息与展示。
        // TODO: 按平台调整 supported / limits（CapabilityName 见 SDK prelude）。
        &[
            Capability {
                name: CapabilityName::Text,
                supported: true,
                limits: None,
            },
            Capability {
                name: CapabilityName::Interactive,
                supported: false,
                limits: None,
            },
        ]
    }

    fn set_event_bus(&mut self, bus: Arc<EventBus>) {
        // 入站事件（消息/回调）经总线发布给宿主；测试宿主（PluginTestHost）也用它断言。
        self.event_bus = Some(bus);
    }

    async fn init(&mut self, _config: AdapterConfig) -> Result<InitResult, GatewayError> {
        // 解析配置。凭据优先来自适配器 env 或 gateway.yaml 的 `adapters.<platform>` 段。
        // TODO: self.token = _config.token.clone();
        // if self.token.is_none() {
        //     return Ok(InitResult {
        //         ok: false,
        //         error: Some("__EASYBOT_NAME__ token required".into()),
        //     });
        // }
        self.state = AdapterState::Starting;
        Ok(InitResult { ok: true, error: None })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        // TODO: 建立真实连接（WebSocket / 长轮询）。失败时分类标记：
        //   网络/临时失败 → ConnectResult::failed(msg, Some(ConnectErrorKind::Transient))
        //   凭据被拒     → ConnectResult::failed(msg, Some(ConnectErrorKind::Permanent))
        // 分类会穿透健康监测的重试/停用决策，见 docs/plugin-methodology.md。
        // 注意：后台轮询任务用 tokio::spawn 时须带 Semaphore/JoinSet 并发上限。
        self.state = AdapterState::Connected;
        Ok(ConnectResult::ok(Some(BotInfo {
            name: "__EASYBOT_DISPLAY_NAME__".into(),
            username: Some("__EASYBOT_NAME__".into()),
            id: "__EASYBOT_NAME__".into(),
        })))
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        // TODO: 停止后台任务、释放连接。
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state.clone()
    }

    async fn health(&self) -> HealthReport {
        HealthReport {
            status: if self.state == AdapterState::Connected {
                HealthStatus::Healthy
            } else {
                HealthStatus::Down
            },
            connected: self.state == AdapterState::Connected,
            last_connected_at: None,
            last_error_at: None,
            last_error: None,
            messages_in: 0,
            messages_out: 0,
            errors: 0,
            uptime: None,
        }
    }

    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
        // TODO: 调用平台发送 API（reqwest）。网络层失败须用 GatewayError::Transient
        // 包裹，让重连路径按瞬态处理；业务失败用 GatewayError::SendError。
        tracing::info!(chat_id = %params.chat_id, text = %params.message.text, "send");

        // 演示事件总线：把发送文本回显为 message.inbound 事件（tests/host_test.rs 断言用）。
        if let Some(bus) = &self.event_bus {
            bus.publish(GatewayEvent::new(
                event_types::MESSAGE_INBOUND,
                self.platform_name(),
                serde_json::json!({
                    "chat_id": params.chat_id,
                    "text": params.message.text,
                }),
            ));
        }
        Ok(SendResult::ok(format!("{}-{}", self.platform_name(), params.chat_id)))
    }

    async fn get_chat_info(&self, _chat_id: &str) -> Result<ChatInfo, GatewayError> {
        // TODO: 调用平台查询接口；暂未支持可返回 capability_not_supported。
        Err(GatewayError::capability_not_supported("get_chat_info"))
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: true,
            token_configured: false,
            extra: serde_json::Value::Null,
        }
    }

    fn status_summary(&self) -> AdapterStatusSummary {
        AdapterStatusSummary {
            platform: self.platform_name().to_string(),
            display_name: self.display_name().to_string(),
            state: self.state.clone(),
            connected: self.state == AdapterState::Connected,
            health: None,
            last_error: None,
            uptime: None,
            messages_in: 0,
            messages_out: 0,
        }
    }
}

// 声明插件入口点（FFI）。宿主经 `easybot_plugin_create` 创建适配器实例。
// 库文件名 = `lib{package-name}.{so|dylib|dll}`（Rust 用下划线连接 crate 名）。
declare_plugin!(__EASYBOT_STRUCT_NAME__, __EASYBOT_STRUCT_NAME__::new);
"#;

/// `plugin.yaml`：插件清单（安装端据此识别平台名 / ABI / 作者）。
pub const PLUGIN_YAML: &str = r#"# __EASYBOT_DISPLAY_NAME__ 插件清单
#
# 构建产物复制到宿主 `plugins/__EASYBOT_NAME__/` 后，宿主以此清单识别插件：
#   name        安装名（也是平台名，须与 src/lib.rs 的 `platform_name()` 一致）
#   sdk_version 编译所用 SDK 的 ABI 版本（须等于 SDK 的 EASYBOT_PLUGIN_ABI_VERSION）
#
# 本地手动安装（dev）：
#   mkdir -p ~/.easybot/plugins/__EASYBOT_NAME__
#   cp target/release/lib__EASYBOT_CRATE_NAME__.{so,dylib} ~/.easybot/plugins/__EASYBOT_NAME__/
#   cp plugin.yaml ~/.easybot/plugins/__EASYBOT_NAME__/

name: "__EASYBOT_NAME__"
display_name: "__EASYBOT_DISPLAY_NAME__"
description: "__EASYBOT_DESCRIPTION__"
version: "0.1.0"
sdk_version: __EASYBOT_SDK_VERSION__
author: "__EASYBOT_AUTHOR__"
"#;

/// `tests/unit.rs`：纯逻辑单元测试（无宿主、无需 tokio 运行时）。
pub const TESTS_UNIT: &str = r#"//! 单元测试：平台身份、能力声明、状态（测试金字塔第 1 层，离线同步执行）。
//!
//! 运行：cargo test

use easybot_plugin_sdk::{AdapterState, CapabilityName, PlatformAdapter};
use __EASYBOT_CRATE_NAME__::__EASYBOT_STRUCT_NAME__;

#[test]
fn platform_identity() {
    let a = __EASYBOT_STRUCT_NAME__::new();
    assert_eq!(a.platform_name(), "__EASYBOT_NAME__");
    assert_eq!(a.display_name(), "__EASYBOT_DISPLAY_NAME__");
}

#[test]
fn declares_text_capability() {
    let a = __EASYBOT_STRUCT_NAME__::new();
    let text = a
        .capabilities()
        .iter()
        .find(|c| c.name == CapabilityName::Text)
        .expect("must declare Text capability");
    assert!(text.supported, "Text capability should be supported");
}

#[test]
fn starts_in_created_state() {
    let a = __EASYBOT_STRUCT_NAME__::new();
    assert_eq!(a.state(), AdapterState::Created);
}
"#;

/// `tests/host_test.rs`：PluginTestHost 集成测试（测试金字塔第 2 层）。
pub const TESTS_HOST: &str = r#"//! PluginTestHost 集成测试：不启动真实网关，在内存宿主里跑通
//! `attach → init → connect → send → 事件流`（DX-4 / docs/plugin-methodology.md）。
//!
//! 传输可注入方法论：真实适配器把 HTTP client 通过构造器或 `init(config)` 注入，
//! 协议交互用 wiremock 替换（测试金字塔第 3 层），宿主只负责宿主侧语义。
//!
//! 运行：cargo test

use easybot_plugin_sdk::prelude::*;
use easybot_plugin_sdk::testing::{PluginTestHost, recv_event};

#[tokio::test]
async fn lifecycle_and_send_roundtrip() {
    let host = PluginTestHost::new();
    let mut adapter = __EASYBOT_CRATE_NAME__::__EASYBOT_STRUCT_NAME__::new();
    host.attach(&mut adapter);
    host.init(&mut adapter, PluginTestHost::config())
        .await
        .unwrap();

    let mut rx = host.subscribe(event_types::MESSAGE_INBOUND);
    let conn = host.connect(&mut adapter).await.unwrap();
    assert!(conn.ok, "connect should succeed");

    let result = host
        .send(
            &mut adapter,
            SendTextParams {
                chat_id: "c1".into(),
                message: OutboundMessage {
                    text: "hello".into(),
                    parse_mode: ParseMode::None,
                },
                reply_to: None,
                metadata: None,
            },
        )
        .await
        .unwrap();
    assert!(result.success);

    // 发送请求被宿主记录（断言插件发出的发送参数形态正确）
    assert_eq!(host.send_log().len(), 1);
    assert_eq!(host.send_log()[0].message.text, "hello");

    // 插件在 send 内发布了 message.inbound 事件（回显文本）
    let ev = recv_event(&mut rx, std::time::Duration::from_secs(1))
        .await
        .expect("plugin should publish an inbound event on send");
    assert_eq!(ev.event_type, event_types::MESSAGE_INBOUND);
    assert_eq!(ev.source, "__EASYBOT_NAME__");
    assert_eq!(ev.data["text"], "hello");

    host.disconnect(&mut adapter).await.unwrap();
    assert_eq!(adapter.state(), AdapterState::Stopped);
}
"#;

/// `README.md`：黄金路径（构建 → 本地联调 → 测试 → 发布）。
pub const README: &str = r#"# __EASYBOT_DISPLAY_NAME__

__EASYBOT_DESCRIPTION__

EasyBot 插件：一个自定义 IM 适配器，通过 `easybot plugin` 市场安装或手动放入宿主
`plugins/` 目录加载。

## 快速开始

```bash
# 1. 构建（git 依赖 SDK，首次拉取后缓存，秒级迭代）
cargo build --release

# 2. 测试（单元 + PluginTestHost 离线，无需启动真实网关）
cargo test

# 3. 本地联调：装入宿主 plugins 目录（dev 环境自动加载）
mkdir -p ~/.easybot/plugins/__EASYBOT_NAME__
cp target/release/lib__EASYBOT_CRATE_NAME__.{so,dylib} ~/.easybot/plugins/__EASYBOT_NAME__/
cp plugin.yaml ~/.easybot/plugins/__EASYBOT_NAME__/

# 4. 在宿主配置中启用（若平台需要凭据）
#   adapters:
#     __EASYBOT_NAME__:
#       enabled: true
#       token: "${PLATFORM_TOKEN}"
```

## 发布

发布前在宿主侧登记发布者公钥后，推送 tag 即触发 `.github/workflows/plugin-publish.yml`
（交叉编译 6 target + gitleaks 扫描 + ed25519 签名 + 发布到 GitHub Releases）：

```bash
# 1. 生成发布者密钥对（私钥存 GitHub Actions secret `PUBLISHER_PRIVATE_KEY`）
easybot-plugin-sign gen-keypair

# 2. 登记公钥：把 PUBLIC_KEY 那行提交给 EasyBot 官方 trusted_publishers（PR 审核）

# 3. 打 tag 推送
git tag v0.1.0 && git push origin v0.1.0
```

用户侧安装：

```bash
easybot plugin install __EASYBOT_NAME__
```

## 项目结构

```
.
├── Cargo.toml                  # cdylib + SDK git 依赖（tag 由脚手架按当前 EasyBot 版本生成）
├── src/lib.rs                  # PlatformAdapter 骨架（TODO 占位，按平台补全）
├── plugin.yaml                 # 插件清单（name / sdk_version / author）
├── tests/unit.rs               # 单元测试：身份 / 能力 / 状态
├── tests/host_test.rs          # PluginTestHost 集成测试：宿主模拟
└── .github/workflows/
    └── plugin-publish.yml      # 发布者 CI 模板（6 target + 签名 + Release）
```
"#;

/// `.gitignore`。
pub const GITIGNORE: &str = r#"/target
.DS_Store
"#;

/// `LICENSE`：MIT（作者占位，脚手架生成后按需替换）。
pub const LICENSE: &str = r#"MIT License

Copyright (c) 2026 __EASYBOT_AUTHOR__

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"#;

/// 发布者 CI 模板（copy 进生成工程）。内容与主仓
/// `.github/workflows/plugin-publish.yml` 保持一致（include_str! 保证同步）。
pub const PLUGIN_PUBLISH_YML: &str = include_str!("../../.github/workflows/plugin-publish.yml");
