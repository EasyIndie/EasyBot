# EasyBot 插件开发指南

## 概述

EasyBot 支持通过动态库（`.so` / `.dylib` / `.dll`）加载第三方 IM 适配器插件。
插件开发者只需依赖 `easybot-plugin-sdk` crate 并实现 `PlatformAdapter` trait，
即可为 EasyBot 添加新的 IM 平台支持——无需修改主仓库代码、无需 fork。

### 架构

```
┌───────────────────────────────┐
│        EasyBot Host           │
│                               │
│  ┌────────────┐               │
│  │   Plugin   │  ┌──────────┐ │
│  │   Loader   │  │ Adapter  │ │
│  └──────┬─────┘  │ Registry │ │
│         │        └─────┬────┘ │
│         │              │      │
│         │    ┌─────────┴───┐  │
│         └───→│  Adapter    │  │ 
│              │  Manager    │  │ 
│              └──────┬──────┘  │ 
│                     │         │ 
│              ┌──────┴─────┐   │ 
│              │ Platform   │   │  
│              │ Adapter    │   │  
│              │(plugin .so)│   │  
│              └────────────┘   │  
└───────────────────────────────┘
```

### 加载流程

```
plugins/<plugin-name>/
├── plugin.yaml          # 插件清单（必需）
└── libplugin.so         # 编译的动态库
```

1. `PluginLoader` 扫描插件目录
2. 读取 `plugin.yaml` 清单
3. `dlopen` 加载动态库
4. 验证 `easybot_abi_version()` 与主机匹配
5. 调用 `easybot_plugin_create()` 创建适配器
6. 提取平台元信息（platform_name, display_name）
7. 生成 `AdapterFactory` 闭包，注册到 `AdapterRegistry`

---

## 快速开始

### 1. 创建插件项目

```bash
cargo new --lib my-adapter
cd my-adapter
```

### 2. 配置 Cargo.toml

```toml
[package]
name = "my-adapter"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
easybot-plugin-sdk = { path = "/path/to/easybot/crates/easybot-plugin-sdk" }
serde_json = "1"
async-trait = "0.1"
tokio = { version = "1", features = ["full"] }
```

> `crate-type = ["cdylib"]` 是必需的——告诉 Rust 编译器生成 C 兼容的动态链接库。

### 3. 实现适配器

```rust
// src/lib.rs
use easybot_plugin_sdk::prelude::*;

struct MyAdapter {
    name: String,
    display: String,
    state: AdapterState,
    token: Option<String>,
}

impl MyAdapter {
    fn new() -> Self {
        Self {
            name: "my-platform".into(),
            display: "My Platform".into(),
            state: AdapterState::Created,
            token: None,
        }
    }
}

#[async_trait]
impl PlatformAdapter for MyAdapter {
    // ── 元数据 ──

    fn platform_name(&self) -> &str {
        &self.name
    }

    fn display_name(&self) -> &str {
        &self.display
    }

    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::new(CapabilityName::SendText, true),
            Capability::new(CapabilityName::SendMedia, false),
        ]
    }

    // ── 生命周期 ──

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        self.token = config.token.clone();
        if self.token.is_none() {
            return Ok(InitResult {
                ok: false,
                error: Some("token is required".into()),
            });
        }
        self.state = AdapterState::Starting;
        Ok(InitResult { ok: true, error: None })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        // 建立 WebSocket / 长轮询连接
        self.state = AdapterState::Connected;
        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: None,
        })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        // 断开连接、清理资源
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
        // 调用平台 API 发送消息
        Ok(SendResult::ok("msg-1".into()))
    }

    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError> {
        Err(GatewayError::capability_not_supported("get_chat_info"))
    }

    fn runtime_config(&self) -> AdapterRuntimeConfig {
        AdapterRuntimeConfig {
            enabled: true,
            token_configured: self.token.is_some(),
            extra: serde_json::Value::Null,
        }
    }

    fn status_summary(&self) -> AdapterStatusSummary {
        AdapterStatusSummary {
            platform: self.name.clone(),
            display_name: self.display.clone(),
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

// ── FFI 导出（关键！） ──
declare_plugin!(MyAdapter, MyAdapter::new);
```

### 4. 编译

```bash
cargo build --release
```

产物位于 `target/release/libmy_adapter.so`（Linux）或 `.dylib`（macOS）/ `.dll`（Windows）。

### 5. 创建插件清单

```yaml
# plugin.yaml
name: "my-platform"
display_name: "My Platform"
description: "My custom IM platform adapter"
version: "1.0.0"
sdk_version: 1
author: "Your Name"
library: "libmy_adapter.so"     # 相对于本目录
```

### 6. 部署

```bash
# 1. 创建插件目录
mkdir -p ~/.easybot/plugins/my-platform

# 2. 复制动态库和清单
cp target/release/libmy_adapter.so ~/.easybot/plugins/my-platform/
cp plugin.yaml ~/.easybot/plugins/my-platform/
```

### 7. 配置

```yaml
# ~/.easybot/gateway.yaml
adapters:
  my-platform:
    enabled: true
    token: "${MY_PLATFORM_TOKEN}"    # 从 .env 或环境变量读取
    baseUrl: "https://api.my-platform.com"
    extra: {}
```

```bash
# ~/.easybot/.env
MY_PLATFORM_TOKEN=your-actual-token
```

> 凭据优先级：`export` / Docker `environment:` > `.env` 文件

---

## plugin.yaml 清单参考

| 字段 | 类型 | 必需 | 说明 |
|------|------|------|------|
| `name` | string | ✅ | 平台唯一标识，如 `"my-platform"` |
| `display_name` | string | ❌ | 人类可读的名字，默认使用 `display_name()` |
| `description` | string | ❌ | 功能描述 |
| `version` | string | ❌ | 插件版本号 |
| `sdk_version` | u32 | ❌ | SDK ABI 版本，默认当前版本（通常为 1） |
| `author` | string | ❌ | 作者信息 |
| `library` | string | ❌ | 动态库文件名。默认根据平台名推导 |

---

## ABI 版本管理

`PluginLoader` 在加载时调用插件的 `easybot_abi_version()` 函数，检查与主机的 `EASYBOT_PLUGIN_ABI_VERSION` 是否一致。

当前 ABI 版本：**1**

| 版本 | 变更说明 |
|------|---------|
| 1 | 初始版本，包含 `PlatformAdapter` 核心接口 |

版本不匹配的插件会被拒绝加载：

```
ERROR plugin::loader: Failed to load plugin: ABI version mismatch: plugin uses v1, host expects v2
```

---

## declare_plugin! 宏

`declare_plugin!` 宏生成三个 C ABI 函数。**每个插件必须且只能调用一次。**

```rust
declare_plugin!(MyAdapter, MyAdapter::new);
```

| 导出函数 | 作用 |
|---------|------|
| `easybot_abi_version()` | 返回 `EASYBOT_PLUGIN_ABI_VERSION` 常量 |
| `easybot_plugin_create()` | 创建适配器实例，返回 `*mut c_void` |
| `easybot_plugin_destroy(ptr)` | 销毁适配器实例 |

**安全说明：**
- 通过 `Box<Box<dyn PlatformAdapter>>` 双层装箱，将 Rust 胖指针压缩为 C 瘦指针（64 bits），以便跨 FFI 边界传递
- `easybot_plugin_destroy` 接受空指针（安全跳过），支持幂等销毁

---

## PlatformAdapter 方法参考

### 必需实现

| 方法 | 签名 | 说明 |
|------|------|------|
| `platform_name()` | `fn platform_name(&self) -> &str` | 唯一平台标识，如 `"my-platform"` |
| `display_name()` | `fn display_name(&self) -> &str` | 人类可读名称 |
| `capabilities()` | `fn capabilities(&self) -> &[Capability]` | 能力列表 |
| `init()` | `async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError>` | 初始化，验证配置但不建立网络连接 |
| `connect()` | `async fn connect(&mut self) -> Result<ConnectResult, GatewayError>` | 建立连接并开始接收消息 |
| `disconnect()` | `async fn disconnect(&mut self) -> Result<(), GatewayError>` | 断开连接，清理资源 |
| `state()` | `fn state(&self) -> AdapterState` | 当前状态 |
| `health()` | `async fn health(&self) -> HealthReport` | 健康检查 |
| `send()` | `async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError>` | 发送文本消息 |
| `get_chat_info()` | `async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError>` | 获取聊天信息 |
| `runtime_config()` | `fn runtime_config(&self) -> AdapterRuntimeConfig` | 运行时配置状态 |
| `status_summary()` | `fn status_summary(&self) -> AdapterStatusSummary` | 状态摘要 |

### 辅助方法（有默认实现，通常无需覆盖）

| 方法 | 说明 |
|------|------|
| `is_connected()` | 检查是否处于 Connected 状态 |
| `heartbeat_age_ms()` | 返回上次心跳距今的毫秒数 |
| `health_status()` | 综合连接状态和心跳延迟计算健康等级 |

### 可选覆盖（有默认实现）

| 方法 | 说明 |
|------|------|
| `retry_transport()` | **纯传输重启**（不鉴权）。取消旧后台任务后直接启动新任务。默认 `Ok(false)` 回退到完整 stop+start。`connect()` 含网络鉴权调用的适配器应覆盖为 `Ok(true)` |
| `set_event_bus()` | 设置事件总线（用于接收平台消息推送） |
| `send_media()` | 发送媒体消息（图片/音频/视频/文件） |
| `send_interactive()` | 发送交互式消息（含 InlineKeyboard） |
| `send_typing()` | 发送输入指示器 |
| `edit_message()` | 编辑消息 |
| `delete_message()` | 删除消息 |
| `send_draft()` | 发送流式草稿 |
| `list_chats()` | 列出聊天列表 |
| `enrich_source()` | 富化会话来源信息 |

---

## 事件发布

插件通过保存 `EventBus` 引用发布接收到的平台消息：

```rust
use easybot_plugin_sdk::prelude::*;

struct MyAdapter {
    event_bus: Option<Arc<EventBus>>,
    // ...
}

#[async_trait]
impl PlatformAdapter for MyAdapter {
    fn set_event_bus(&mut self, bus: Arc<EventBus>) {
        self.event_bus = Some(bus);
    }

    // 当从平台收到消息时：
    async fn handle_incoming_message(&self, platform_msg: ...) {
        if let Some(bus) = &self.event_bus {
            bus.publish(GatewayEvent::new(
                "message.inbound",
                "my-platform",
                serde_json::json!({
                    "chat_id": platform_msg.chat_id,
                    "text": platform_msg.text,
                    "sender": platform_msg.sender,
                }),
            ));
        }
    }
}
```

---

## 生命周期与状态机

```
     ┌──────────────┐
     │   Created    │  initial state
     └──────┬───────┘
            │ init(config)
            v
     ┌──────────────┐
     │   Starting   │  config validated
     └──────┬───────┘
            │ connect()
            v
     ┌──────────────┐
     │  Connecting  │  connecting to IM
     └──────┬───────┘
            │ connected
            v
     ┌──────────────┐
     │  Connected   │  running, can send/recv
     └──────┬───────┘
            │ disconnect()
            v
     ┌──────────────┐
     │  Stopped     │  cleaned up
     └──────────────┘

Recovery flows (auto, managed by AdapterManager health monitor):
  Connected --(heartbeat stale > 120s)--> [Tier 1: retry_transport()] --(Ok(true))--> Connected
  Connected --(Tier 1 exhausted / adapter removed)--> [Tier 2: reconnect_adapter()] --> Connecting --> Connected
  Connected --(permanent auth err)--> Failed
  Failed --(backoff retry via full stop+start)--> Connecting --> Connected
  any state --stop()--> Stopped
```

---

## 测试

### 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state_created() {
        let adapter = MyAdapter::new();
        assert_eq!(adapter.state(), AdapterState::Created);
    }

    #[tokio::test]
    async fn test_init_success() {
        let mut adapter = MyAdapter::new();
        let config = AdapterConfig {
            enabled: Some(true),
            token: Some("test-token".into()),
            ..Default::default()
        };
        let result = adapter.init(config).await.unwrap();
        assert!(result.ok);
        assert_eq!(adapter.state(), AdapterState::Starting);
    }
}
```

### 集成测试（wiremock）

参考 `crates/easybot-adapter-feishu/tests/send_mock.rs`：

```rust
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_send_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/api/send"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "message_id": "msg-1",
        })))
        .mount(&mock_server)
        .await;

    let config = AdapterConfig {
        token: Some("test".into()),
        base_url: Some(format!("http://127.0.0.1:{}", mock_server.address().port())),
        ..Default::default()
    };

    let mut adapter = MyAdapter::new();
    adapter.init(config).await.unwrap();

    let result = adapter.send(SendTextParams {
        chat_id: "test-chat".into(),
        message: OutboundMessage {
            text: "hello".into(),
            parse_mode: ParseMode::None,
        },
        reply_to: None,
        metadata: None,
    }).await.unwrap();

    assert!(result.success);
}
```

---

## 调试技巧

### 启用 DEBUG 日志

```bash
# 使用 --debug 命令行参数
easybot --debug
```

插件加载日志示例：
```
2026-07-10T12:00:00.123456Z  INFO PluginLoader: Loaded plugin 'my-platform' (My Platform) from /Users/alice/.easybot/plugins/my-platform
2026-07-10T12:00:00.123789Z  INFO AdapterManager: Registered plugin adapter: my-platform (My Platform)
```

### 常见错误

| 错误 | 原因 | 解决 |
|------|------|------|
| `Library not found` | `plugin.yaml` 中 `library` 路径不对 | 检查路径，确认文件存在 |
| `ABI version mismatch` | 插件使用旧版 SDK 编译 | 用最新 SDK 重新编译 |
| `Symbol not found` | 插件未使用 `declare_plugin!` 宏 | 添加宏调用 |
| `Platform conflict` | 平台名已被占用 | 修改 `platform_name()` 返回值 |
| `Manifest not found` | 插件目录缺少 `plugin.yaml` | 创建清单文件 |

### LLDB/gdb 调试

```bash
lldb target/debug/easybot
breakpoint set --name easybot_plugin_create
run --dir ~/.easybot
```

---

## 最佳实践

1. **平台名唯一** — `platform_name()` 应使用小写字母、连字符，如 `"my-platform"`
2. **能力声明准确** — 在 `capabilities()` 中如实声明支持的能力
3. **状态更新及时** — `init()` / `connect()` / `disconnect()` 必须正确更新 `state`
4. **无阻塞初始化** — `init()` 只做校验和配置存储，网络连接在 `connect()` 中进行
5. **日志丰富** — 使用 `tracing::info!()` / `tracing::warn!()` / `tracing::error!()` 记录关键事件
6. **重试策略** — 网络错误应有重试机制，避免瞬时故障导致永久失效
7. **线程安全** — `PlatformAdapter` 要求 `Send + Sync`，内部状态需使用线程安全容器

---

## 完整示例

- **MockAdapter 源码**: [`tests/plugins/mock-adapter/src/lib.rs`](../tests/plugins/mock-adapter/src/lib.rs)
- **集成测试**: [`tests/integration/src/lib.rs`](../tests/integration/src/lib.rs)

```bash
cargo build -p mock-adapter
cargo test -p integration-tests
```
