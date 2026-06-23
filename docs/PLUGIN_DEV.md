# EasyBot 插件开发指南

## 概述

EasyBot 支持通过动态库（`.so` / `.dylib` / `.dll`）加载第三方 IM 适配器插件。
插件开发者只需依赖 `easybot-plugin-sdk` crate 并实现 `PlatformAdapter` trait，
即可为 EasyBot 添加新的 IM 平台支持——无需修改主仓库代码、无需 fork。

### 架构图

```
┌─────────────────────────────────────────┐
│  EasyBot Host                           │
│  ┌──────────┐  ┌────────────────────┐   │
│  │PluginLoader├─→│AdapterRegistry    │   │
│  └─────┬────┘  └────────────────────┘   │
│        │           ↕                     │
│        │    ┌─────────────┐              │
│        └───→│AdapterManager│              │
│             └─────────────┘              │
│                    ↕                      │
│          ┌─────────────────┐             │
│          │   PlatformAdapter             │
│          │   (插件动态库)    │             │
│          └─────────────────┘             │
└─────────────────────────────────────────┘
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

> **注意**: `crate-type = ["cdylib"]` 是必需的——它告诉 Rust 编译器生成 C 兼容的动态链接库（`.so`/`.dylib`/`.dll`）。

### 3. 实现适配器

```rust
// src/lib.rs
use easybot_plugin_sdk::prelude::*;

struct MyAdapter {
    name: String,
    state: AdapterState,
}

impl MyAdapter {
    fn new() -> Self {
        Self {
            name: "my-platform".into(),
            state: AdapterState::Created,
        }
    }
}

#[async_trait]
impl PlatformAdapter for MyAdapter {
    fn platform_name(&self) -> &str {
        &self.name
    }

    fn display_name(&self) -> &str {
        "My Platform"
    }

    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::new(CapabilityName::SendText, true),
            Capability::new(CapabilityName::SendMedia, false),
        ]
    }

    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
        // 验证配置（token 等）
        if config.token.is_none() {
            return Ok(InitResult {
                ok: false,
                error: Some("token is required".into()),
            });
        }

        self.state = AdapterState::Starting;
        Ok(InitResult { ok: true, error: None })
    }

    async fn connect(&mut self) -> Result<ConnectResult, GatewayError> {
        // 建立连接（WebSocket / 长轮询等）
        self.state = AdapterState::Connected;
        Ok(ConnectResult {
            ok: true,
            error: None,
            bot_info: None, // 可选：填充 BotInfo
        })
    }

    async fn disconnect(&mut self) -> Result<(), GatewayError> {
        // 断开连接，清理资源
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
                HealthStatus::Unhealthy
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
        Ok(SendResult {
            success: true,
            message_id: Some("msg-1".into()),
            timestamp: None,
            error: None,
            error_code: None,
            retryable: false,
        })
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
            display_name: "My Platform".into(),
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

产物位于 `target/release/libmy_adapter.so`（或 `.dylib`/`.dll`）。

### 5. 创建插件清单

在插件目录中创建 `plugin.yaml`：

```yaml
# plugin.yaml
name: "my-platform"
display_name: "My Platform"
description: "My custom IM platform adapter"
version: "1.0.0"
sdk_version: 1
author: "Your Name"
library: "libmy_adapter.so"  # 相对于本目录
```

### 6. 部署

```bash
# 1. 创建插件目录
mkdir -p ~/.easybot/plugins/my-platform

# 2. 复制动态库和清单
cp target/release/libmy_adapter.so ~/.easybot/plugins/my-platform/
cp plugin.yaml ~/.easybot/plugins/my-platform/

# 3. 启用插件（在 gateway.yaml 中配置）
```

### 7. 配置

在 `~/.easybot/gateway.yaml` 中添加适配器配置，敏感信息（token、密钥）建议通过 `.env` 文件管理：

```yaml
adapters:
  my-platform:
    enabled: true
    token: "${MY_PLATFORM_TOKEN}"   # 从 .env 或环境变量读取
    api_key: null
    base_url: "https://api.my-platform.com"
    extra:
      option1: "value1"
```

然后在 `~/.easybot/.env` 中设置对应的值（`.env` 不会被提交到版本控制）：

```bash
# ~/.easybot/.env
MY_PLATFORM_TOKEN=your-actual-token
```

环境变量优先级（从高到低）：
1. `export MY_PLATFORM_TOKEN=xxx`（Shell / Docker environment:）
2. `~/.easybot/.env` 文件中的值

> **提示**: 运行 `easybot --init` 会自动创建 `.env.example` 模板文件，列出所有已知的环境变量。复制为 `.env` 后填入实际值即可。

---

## plugin.yaml 清单参考

| 字段 | 类型 | 必需 | 说明 |
|------|------|------|------|
| `name` | string | ✅ | 平台唯一标识，如 `"telegram"` |
| `display_name` | string | ❌ | 人类可读的名字，默认使用 adapter 返回的 `display_name()` |
| `description` | string | ❌ | 功能描述 |
| `version` | string | ❌ | 插件版本号 |
| `sdk_version` | u32 | ❌ | SDK ABI 版本，默认当前版本（通常为 1） |
| `author` | string | ❌ | 作者信息 |
| `library` | string | ❌ | 动态库文件名。默认根据平台名推导：`lib{name}.so` / `{name}.dylib` / `{name}.dll` |

### 完整示例

```yaml
name: "slack"
display_name: "Slack"
description: "Slack workspace integration"
version: "2.1.0"
sdk_version: 1
author: "EasyBot Contributors"
library: "libslack_adapter.so"
```

---

## ABI 版本管理

`PluginLoader` 在加载时调用插件的 `easybot_abi_version()` 函数，
检查与主机的 `EASYBOT_PLUGIN_ABI_VERSION` 是否一致。

当前 ABI 版本：**1**

| 版本 | 变更说明 |
|------|---------|
| 1 | 初始版本，包含 `PlatformAdapter` 核心接口 |

当 `PlatformAdapter` trait 发生不兼容变更时（新增必需方法、修改方法签名等），
ABI 版本将递增。版本不匹配的插件会被拒绝加载，错误日志中会包含明确的版本信息：

```
ERROR plugin::loader: Failed to load plugin: ABI version mismatch: plugin uses v1, host expects v2
```

---

## declare_plugin! 宏

`declare_plugin!` 宏生成三个 C ABI 函数。**每个插件必须且只能调用一次**。

```rust
declare_plugin!(MyAdapter, MyAdapter::new);
```

| 导出函数 | 作用 |
|---------|------|
| `easybot_abi_version()` | 返回 `EASYBOT_PLUGIN_ABI_VERSION` 常量 |
| `easybot_plugin_create()` | 创建适配器实例，返回 `*mut c_void` |
| `easybot_plugin_destroy(ptr)` | 销毁适配器实例，释放内存 |

**安全说明**：
- `easybot_plugin_create` 通过 `Box<Box<dyn PlatformAdapter>>` 双层装箱，将 Rust 胖指针（128 bits）压缩为 C 瘦指针（64 bits），以便跨 FFI 边界传递
- `easybot_plugin_destroy` 接受空指针（会安全跳过），支持幂等销毁

---

## PlatformAdapter 方法参考

### 必需实现

| 方法 | 签名 | 说明 |
|------|------|------|
| `platform_name()` | `fn platform_name(&self) -> &str` | 返回唯一平台标识，如 `"my-platform"` |
| `display_name()` | `fn display_name(&self) -> &str` | 返回人类可读名称 |
| `capabilities()` | `fn capabilities(&self) -> &[Capability]` | 返回能力列表 |
| `init()` | `async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError>` | 初始化，验证配置但不建立网络连接 |
| `connect()` | `async fn connect(&mut self) -> Result<ConnectResult, GatewayError>` | 建立连接并开始接收消息 |
| `disconnect()` | `async fn disconnect(&mut self) -> Result<(), GatewayError>` | 断开连接，清理资源 |
| `state()` | `fn state(&self) -> AdapterState` | 返回当前状态 |
| `health()` | `async fn health(&self) -> HealthReport` | 健康检查 |
| `send()` | `async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError>` | 发送文本消息 |
| `get_chat_info()` | `async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError>` | 获取聊天信息 |
| `runtime_config()` | `fn runtime_config(&self) -> AdapterRuntimeConfig` | 返回运行时配置状态 |
| `status_summary()` | `fn status_summary(&self) -> AdapterStatusSummary` | 返回适配器状态摘要 |

### 辅助方法（有默认实现，通常无需覆盖）

| 方法 | 说明 |
|------|------|
| `is_connected()` | 检查适配器是否处于 Connected 状态 |
| `heartbeat_age_ms()` | 返回上次心跳距今的毫秒数（用于 General Health Monitor） |
| `health_status()` | 综合连接状态和心跳延迟计算健康等级 (Healthy/Degraded/Unhealthy) |

### 可选覆盖（默认返回 capability_not_supported 错误）

| 方法 | 说明 |
|------|------|
| `send_media()` | 发送媒体消息（图片/音频/视频/文件） |
| `send_interactive()` | 发送交互式消息（含 InlineKeyboard） |
| `send_typing()` | 发送输入指示器 |
| `edit_message()` | 编辑消息 |
| `delete_message()` | 删除消息 |
| `send_draft()` | 发送流式草稿（AI 回复逐步生成） |
| `list_chats()` | 列出聊天列表 |
| `set_event_bus()` | 设置事件总线（用于接收平台消息推送） |

---

## 事件发布

要让插件向 EasyBot 事件总线推送消息（例如接收到的平台消息），
插件需要保存 `EventBus` 引用并调用 `publish()`：

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
                    "author": platform_msg.author,
                }),
            ));
        }
    }
}
```

---

## 生命周期与状态机

```
     ┌─────────┐
     │ Created │  ← 插件被构建时的初始状态
     └────┬────┘
          │ init(config)
          v
     ┌─────────┐
     │ Starting│  ← 配置已验证，准备连接
     └────┬────┘
          │ connect()
          v
     ┌───────────┐
     │ Connecting│  ← 正在建立网络连接
     └─────┬─────┘
           │ (连接成功)
           v
     ┌───────────┐
     │ Connected │  ← 正常运行，可收发消息
     └─────┬─────┘
           │ disconnect()
           v
     ┌─────────┐
     │ Stopped │  ← 已断开，资源已清理
     └─────────┘

     Connected ──(连接断开)──→ Reconnecting ──→ Connecting ──→ Connected
     Connected ──(严重错误)──→ Failed
     Failed ──(重试恢复)──→ Connecting ──→ Connected
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
            enabled: true,
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

参考 `crates/easybot-adapter-feishu/tests/send_mock.rs` 使用 wiremock 模拟 HTTP API：

```rust
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_send_success() {
    let mock_server = MockServer::start().await;

    // Mock 平台 API 端点
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
        message: OutboundMessage { text: "hello".into(), parse_mode: ParseMode::None },
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
easybot --debug
```

或设置环境变量：

```bash
EASYBOT_LOG=debug easybot
```

插件加载日志示例：

```
2026-06-15T12:00:00.123456Z  INFO PluginLoader: Loaded plugin 'my-platform' (My Platform) from /Users/alice/.easybot/plugins/my-platform
2026-06-15T12:00:00.123789Z  INFO AdapterManager: Registered plugin adapter: my-platform (My Platform)
```

### 常见错误

| 错误 | 原因 | 解决 |
|------|------|------|
| `Library not found` | `plugin.yaml` 中的 `library` 路径不对 | 检查路径，确认文件存在 |
| `ABI version mismatch` | 插件使用旧版 SDK 编译 | 用最新 SDK 重新编译 |
| `Symbol not found` | 插件未使用 `declare_plugin!` 宏 | 添加宏调用 |
| `Platform conflict` | 平台名已被占用 | 修改 `platform_name()` 返回值 |
| `Manifest not found` | 插件目录缺少 `plugin.yaml` | 创建清单文件 |

### LLDB/gdb 调试

```bash
# 设置断点
lldb target/debug/easybot
breakpoint set --name easybot_plugin_create
run --dir ~/.easybot
```

---

## 最佳实践

1. **平台名唯一** — `platform_name()` 应使用小写字母、连字符，如 `"my-platform"`
2. **能力声明准确** — 在 `capabilities()` 中如实声明支持的能力，不要过度承诺
3. **状态更新及时** — `init()` / `connect()` / `disconnect()` 必须正确更新 `state`
4. **无阻塞初始化** — `init()` 只做校验和配置存储，网络连接在 `connect()` 中进行
5. **日志丰富** — 使用 `tracing::info!()` / `tracing::warn!()` / `tracing::error!()` 记录关键事件
6. **配置校验严格** — 启动时校验所有必需参数，给出清晰的错误信息
7. **重试策略** — 网络错误应有重试机制，避免瞬时故障导致适配器永久失效
8. **线程安全** — `PlatformAdapter` 要求 `Send + Sync`，内部状态需使用线程安全容器

---

## 完整示例

查看 `tests/plugins/mock-adapter/` 这是一个最小可运行的插件示例：

- **[MockAdapter source](../tests/plugins/mock-adapter/src/lib.rs)** — 完整实现
- **[plugin.yaml](../tests/plugins/mock-adapter/plugin.yaml)** — 清单文件
- **[Integration test](../tests/integration/src/lib.rs)** — 插件加载与生命周期测试

### MockAdapter 关键代码

```rust
// tests/plugins/mock-adapter/src/lib.rs
use easybot_plugin_sdk::prelude::*;

struct MockAdapter { /* ... */ }
impl PlatformAdapter for MockAdapter { /* ... */ }
declare_plugin!(MockAdapter, MockAdapter::new);
```

编译：

```bash
cargo build -p mock-adapter
cargo test -p integration-tests
```
