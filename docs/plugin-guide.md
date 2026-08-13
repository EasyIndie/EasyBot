# EasyBot 插件开发完整指南

> 完整参考：生命周期、capability 声明、消息收发/媒体/交互/编辑、事件发布 vs 轮询、配置、错误分类、心跳、资源上限、ABI 纪律、调试。
> 快速上手（15 分钟教程）见 [`docs/plugin-quickstart.md`](plugin-quickstart.md)；方法论见 [`docs/plugin-methodology.md`](plugin-methodology.md)。

## 架构与加载流程

```
plugins/<plugin-name>/
├── plugin.yaml          # 插件清单（必需）
└── lib<plugin>.so       # 编译的动态库（.dylib / .dll）
```

1. `PluginLoader` 扫描插件目录，读取 `plugin.yaml`
2. `dlopen` 加载动态库
3. 校验 `easybot_abi_version()` 与宿主 `EASYBOT_PLUGIN_ABI_VERSION` 一致（不一致 → `AbiVersionMismatch`）
4. 调用 `easybot_plugin_create()` 创建适配器实例
5. 提取平台元信息（`platform_name` / `display_name`）
6. 生成 `AdapterFactory`，注册到 `AdapterRegistry`
7. 有 `plugin.sig.json` 则对库文件重新验签（生产模式强制；失败 → `SignatureVerificationFailed`）
8. 有 `enabled: false` 则标记 `DisabledPlugin` 不加载

> 宿主生产模式对**未签名**插件直接拒绝（见 `docs/SECURITY.md`）。

---

## 项目结构（脚手架产物）

```text
Cargo.toml                  # cdylib + SDK git tag 依赖 + release profile（panic=abort）
.cargo/config.toml          # 构建配置（[patch] 属 Cargo.toml，不写这里）
src/lib.rs                  # PlatformAdapter 骨架 + declare_plugin!
plugin.yaml                 # 清单
tests/unit.rs               # 单元测试（离线）
tests/host_test.rs          # PluginTestHost 集成测试（离线）
.github/workflows/plugin-publish.yml  # 发布者 CI 模板
```

---

## plugin.yaml 清单参考

| 字段 | 类型 | 必需 | 说明 |
|------|------|------|------|
| `name` | string | ✅ | 平台唯一标识（kebab-case，须与 `platform_name()` 一致），如 `"my-adapter"` |
| `display_name` | string | ❌ | 人类可读名字，默认用 `display_name()` |
| `description` | string | ❌ | 功能描述 |
| `version` | string | ❌ | 插件版本号 |
| `sdk_version` | u32 | ✅ | SDK ABI 版本，必须与编译所用 SDK 的 `EASYBOT_PLUGIN_ABI_VERSION` 一致 |
| `author` | string | ❌ | 作者信息 |
| `library` | string | ❌ | 动态库文件名；默认按平台名推导 |
| `enabled` | bool | ❌ | 缺省启用；`false` 时不加载（卸载/禁用语义见方法论「启停语义」） |

> 市场安装时由宿主根据 `easybot-plugin.json` 元数据合成 `plugin.yaml`；手动安装则自己写。

---

## ABI 版本管理

`PluginLoader` 调用插件的 `easybot_abi_version()` 检查与宿主 `EASYBOT_PLUGIN_ABI_VERSION` 是否一致。

当前 ABI 版本：**1**（`sdk_version: 1`）

| 版本 | 变更说明 |
|------|---------|
| 1 | 初始版本，包含 `PlatformAdapter` 核心接口 |

不匹配会被拒绝：

```
ERROR plugin::loader: Failed to load plugin: ABI version mismatch: plugin uses v1, host expects v2
```

**ABI 纪律**：`sdk_version` 必须等于编译所用 SDK 的常量；升级 SDK 大版本 = 重新发布插件。宿主更新主程序时 `check_plugin_compatibility` 会阻止不兼容插件（`updater/precheck.rs`）。

---

## declare_plugin! 宏

`declare_plugin!` 生成三个 C ABI 函数。**每个插件必须且只能调用一次**：

```rust
declare_plugin!(MyAdapter, MyAdapter::new);
```

| 导出函数 | 作用 |
|---------|------|
| `easybot_abi_version()` | 返回 `EASYBOT_PLUGIN_ABI_VERSION` |
| `easybot_plugin_create()` | 创建适配器实例，返回 `*mut c_void` |
| `easybot_plugin_destroy(ptr)` | 销毁适配器实例（幂等，接受空指针） |

**安全说明**：通过 `Box<Box<dyn PlatformAdapter>>` 双层装箱把 Rust 胖指针压缩为 C 瘦指针（64 bits）跨 FFI 传递。宏生成的函数已带 `#[allow(missing_docs)]`（FFI 入口是实现细节）。

**`panic = "abort"` 是硬要求**（脚手架 release profile 已设）：跨 FFI 边界不得 unwind，cdylib 内 panic 直接终止进程。

---

## PlatformAdapter 方法参考

### 必需实现

| 方法 | 签名 | 说明 |
|------|------|------|
| `platform_name()` | `fn platform_name(&self) -> &str` | 唯一平台标识（kebab-case），如 `"my-adapter"` |
| `display_name()` | `fn display_name(&self) -> &str` | 人类可读名称 |
| `capabilities()` | `fn capabilities(&self) -> &[Capability]` | 能力列表 |
| `init()` | `async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError>` | 校验配置、存凭据，**不建网络连接** |
| `connect()` | `async fn connect(&mut self) -> Result<ConnectResult, GatewayError>` | 建立连接、启动后台任务 |
| `disconnect()` | `async fn disconnect(&mut self) -> Result<(), GatewayError>` | 停止后台任务、释放连接 |
| `state()` | `fn state(&self) -> AdapterState` | 当前状态 |
| `health()` | `async fn health(&self) -> HealthReport` | 健康检查 |
| `send()` | `async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError>` | 发送文本消息 |
| `get_chat_info()` | `async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError>` | 获取聊天信息 |
| `runtime_config()` | `fn runtime_config(&self) -> AdapterRuntimeConfig` | 运行时配置状态 |
| `status_summary()` | `fn status_summary(&self) -> AdapterStatusSummary` | 状态摘要 |

### 辅助方法（有默认实现，通常无需覆盖）

| 方法 | 说明 |
|------|------|
| `is_connected()` | 是否处于 `Connected` |
| `heartbeat_age_ms()` | 上次存活心跳距今毫秒数（默认 `None`=不跟踪）；覆盖后应转调存储的 `Heartbeat::age_ms()` |
| `heartbeat_success_age_ms()` | 上次**成功收到消息**距今毫秒数；默认回落 `heartbeat_age_ms()`，覆盖后可在「活着但收不到消息」时上报 `Degraded` |
| `heartbeat_failure_count()` | 连续失败次数（`Heartbeat::record_failure` 累计，`beat_success` 清零）；覆盖后可检测「卡死的重试循环」→ `Down` |
| `health_status()` | 综合连接状态 + 心跳算健康等级（`Healthy` / `Degraded` / `Down`） |

### 可选覆盖（有默认实现）

| 方法 | 说明 |
|------|------|
| `retry_transport()` | **纯传输重启**（不重新鉴权）。取消旧后台任务后直接重启。默认 `Ok(false)` 回退完整 stop+start。`connect()` 含网络鉴权调用的适配器（Telegram/Discord/飞书）应覆盖为 `Ok(true)` |
| `set_event_bus()` | 注入事件总线（收到平台消息时 publish 用） |
| `send_media()` | 发送媒体消息（图片/音频/视频/文件） |
| `send_interactive()` | 发送交互消息（含 `InlineKeyboard`） |
| `send_typing()` | 输入指示器 |
| `edit_message()` | 编辑消息 |
| `delete_message()` | 删除消息 |
| `send_draft()` | 流式草稿 |
| `list_chats()` | 列出聊天 |
| `enrich_source()` | 富化会话来源信息 |
| `answer_callback_query()` | 应答内联键盘回调（`AnswerCallbackParams`） |
| `cursor_state()` / `restore_cursor_state()` | 滚动游标（如 Telegram `last_offset`）——重启后交还，由 `AdapterManager` 保存/恢复 |

---

## 生命周期与状态机

```
     ┌──────────────┐
     │   Created    │  initial state
     └──────┬───────┘
            │ init(config)
            v
     ┌──────────────┐
     │   Starting   │  config validated（无阻塞：只校验、存配置）
     └──────┬───────┘
            │ connect()
            v
     ┌──────────────┐
     │  Connecting  │  正在连平台
     └──────┬───────┘
            │ connected
            v
     ┌──────────────┐
     │  Connected   │  运行中，可收发
     └──────┬───────┘
            │ disconnect()
            v
     ┌──────────────┐
     │   Stopped    │  已清理
     └──────────────┘

Recovery flows（AdapterManager 健康监测自动管理）:
  Connected --(心跳过期 > 120s)--> [Tier 1: retry_transport()] --Ok(true)--> Connected
  Connected --(Tier 1 耗尽 / 适配器被移除)--> [Tier 2: reconnect_adapter()] --> Connecting --> Connected
  Connected --(永久鉴权错误)--> Failed
  Failed --(退避重试：完整 stop+start)--> Connecting --> Connected
  any --stop()--> Stopped
```

- **无阻塞初始化**：`init()` 只做校验和配置存储，网络连接在 `connect()` 中建立。
- **游标持久化**：用 `cursor_state()` / `restore_cursor_state()` 提供滚动游标，`AdapterManager` 在 stop/restart 时保存并恢复，重启不丢消息、不重复消费。
- **心跳语义**：`Heartbeat::beat()` 表示「后台任务存活且正在重试」，不要求消息成功。见 methodology「心跳」。

---

## Capability 声明

`capabilities()` 返回 `&[Capability]`：

```rust
fn capabilities(&self) -> &[Capability] {
    &[
        Capability { name: CapabilityName::Text, supported: true, limits: None },
        Capability { name: CapabilityName::Interactive, supported: false, limits: None },
    ]
}
```

`CapabilityName` 枚举：`Text / Image / Audio / Video / Document / Interactive / Streaming / Voice / Markdown / Html / CodeBlock / Thread / Topic / Group / ChatList / MessageEdit / MessageDelete / TypingIndicator`。

宿主据此路由消息与展示。**如实声明**——声明了 `MessageEdit` 却在 `edit_message()` 报能力不支持，会导致上游调用失败。

---

## 事件发布 vs 轮询

插件通过 `set_event_bus` 保存的 `Arc<EventBus>` 把平台入站事件发布给宿主。**发布的对象是 `GatewayEvent`**（事件类型常量见 SDK prelude 的 `event_types`）：

| 常量 | 值 |
|---|---|
| `event_types::MESSAGE_INBOUND` | `"message.inbound"` |
| `event_types::MESSAGE_SENT` | `"message.sent"` |
| `event_types::MESSAGE_FAILED` | `"message.failed"` |
| `event_types::ADAPTER_CONNECTED` | `"adapter.connected"` |
| `event_types::ADAPTER_DISCONNECTED` | `"adapter.disconnected"` |
| `event_types::ADAPTER_ERROR` | `"adapter.error"` |
| `event_types::ADAPTER_RECONNECTING` | `"adapter.reconnecting"` |
| `event_types::ADAPTER_RECONNECTED` | `"adapter.reconnected"` |
| `event_types::CALLBACK_RECEIVED` | `"callback.received"` |

```rust
fn set_event_bus(&mut self, bus: Arc<EventBus>) {
    self.event_bus = Some(bus);
}

// 收到平台消息时：
if let Some(bus) = &self.event_bus {
    bus.publish(GatewayEvent::new(
        event_types::MESSAGE_INBOUND,
        self.platform_name(),
        serde_json::json!({
            "chat_id": platform_msg.chat_id,
            "text": platform_msg.text,
        }),
    ));
}
```

**两种收消息模式**：

1. **事件推送**（平台主动推 WebSocket / webhook）→ 在回调里 `publish`。适合 Telegram、飞书、Discord。
2. **长轮询**（平台无推送）→ `connect()` 里 `tokio::spawn` 一个轮询任务，每轮把新消息 `publish`。适合 QQ、微信。轮询任务必须有并发上限（见 methodology）。

> 事件总线是 tokio `broadcast` **有界通道（cap 256）**：慢消费者 `Lagged` 丢事件是预期行为，插件发布方无需处理。

---

## 配置

### 插件侧（plugin.yaml）

`name` / `sdk_version` 必须正确（见上文清单表）。

### 宿主侧（gateway.yaml）

插件注册后就是一个普通适配器，凭据/参数放 `adapters:` 段：

```yaml
adapters:
  my-adapter:
    enabled: true
    token: "${MY_PLATFORM_TOKEN}"    # 从 .env 或环境变量读取
    base_url: "https://api.example.com"
    extra: {}
```

`AdapterConfig` 字段：`enabled: Option<bool>`、`token`、`api_key`、`base_url`（自定义 API 基础 URL，测试/代理用）、`extra: serde_json::Value`。**凭据字段 Debug 已脱敏**（`***REDACTED***`）。

凭据优先级：`export` / Docker `environment:` > `.env` 文件（`~/.easybot/.env`，chmod 600）。

### 插件市场段（gateway.yaml `plugins:`）

安装/信任/注册源相关配置（安装市场插件后由 `easybot plugin install` 管理；改动需重启生效，见 methodology「热重载语义」）：

```yaml
plugins:
  directory: ~/.easybot/plugins
  auto_load: true
  verify_signatures: true
  allow_untrusted: false
  trusted_publishers: []
  registries:
    - kind: github
      owner: EasyIndie
      repo: EasyBot-Plugins
```

---

## 错误分类

`GatewayError` 是唯一的错误类型，**分类影响健康监测的重试/停用决策**（详见 methodology）：

| 场景 | 用哪个变体 |
|------|-----------|
| reqwest `send()` 网络层失败 | `GatewayError::Transient(...)` —— **必须**，否则可能被误判为永久停用 |
| 业务失败（平台返回错误、消息被拒） | `GatewayError::SendError(...)` |
| 鉴权/凭据被拒 | `GatewayError::AuthFailed / Unauthorized / Forbidden`（→ 永久，立即 Failed） |
| 暂未支持的能力 | `GatewayError::capability_not_supported("method")` |
| 请求超时 | `GatewayError::RequestTimeout(...)`（→ 瞬态） |
| 被限流 | `GatewayError::RateLimited(...)`（→ 瞬态） |

`connect()` 失败用 `ConnectResult::failed(msg, kind)` 标记：网络 → `ConnectErrorKind::Transient`，凭据被拒 → `Permanent`，未知 → `None` 回退启发式。

---

## 心跳

```rust
struct MyAdapter {
    heartbeat: Heartbeat,   // 需要返回 heartbeat_age_ms 时持有
    // ...
}
```

- 后台轮询/重连任务的**每一轮**（包括错误路径）调用 `heartbeat.beat()`——表示「任务还活着，在重试」。
- 真正收到消息/一轮成功时调用 `heartbeat.beat_success()`；错误路径调用 `heartbeat.record_failure()`。
- `heartbeat_age_ms()` 过期（> 120s）→ 健康监测器介入重连；`heartbeat_success_age_ms()` 过期 → `Degraded`（活着但消息流被阻塞，如 WiFi 断）。
- **禁止**用独立定时器无条件 beat——那会让「任务已死」也显得活着，健康监测失效。

---

## 资源上限（与内置适配器同标准）

| 资源 | 约束 |
|------|------|
| 缓存 | 必须带大小上限或 TTL 淘汰（参考内置 `CHAT_TYPE_CACHE_LIMIT` / `ADMIN_CACHE_LIMIT` 模式） |
| 后台任务 | 所有 `tokio::spawn` 循环须有 `Semaphore` 或 `JoinSet` 并发上限 |
| 网络 | 发送带超时，避免悬挂 |
| 日志 | 用 `tracing`（进宿主 `/logs` 环形缓冲），不要 print |

---

## 调试技巧

### 日志

```bash
easybot --debug                          # 宿主 debug 日志
RUST_LOG=my_adapter=debug easybot        # 只开插件 crate 的日志
```

插件加载日志示例：

```
INFO PluginLoader: Loaded plugin 'my-adapter' (My Adapter) from /Users/alice/.easybot/plugins/my-adapter
INFO AdapterManager: Registered plugin adapter: my-adapter (My Adapter)
```

### 常见错误

| 错误 | 原因 | 解决 |
|------|------|------|
| `Library not found` | `plugin.yaml` 的 `library` 路径不对 | 检查文件存在、文件名匹配（`lib<name>.so`） |
| `ABI version mismatch` | 插件用旧版 SDK 编译 | 用当前 SDK 重编，`sdk_version` 同步 |
| `Symbol not found` | 插件未调用 `declare_plugin!` | 添加宏调用 |
| `Platform conflict` | 平台名已被占用 | 改 `platform_name()` |
| `Manifest not found` | 插件目录缺 `plugin.yaml` | 创建清单 |
| `Signature verification failed` | 生产模式签名不匹配 | 用 `plugin install` 重装（带签名）或 `allow_untrusted` |
| 装不上/dll 加载失败（Windows） | 缺 `vcruntime140.dll` | 发布时用 `crt-static`；宿主补装 VC runtime |

### LLDB/gdb

```bash
lldb target/debug/easybot
breakpoint set --name easybot_plugin_create
run --dir ~/.easybot
```

### 平台 API 排错三步法

1. 看错误分类：`Transient`（网络，会重试）/ `AuthFailed`（凭据）/ `SendError`（业务）
2. 看健康状态：`easybot plugin inspect <name>` / `/api/v1/plugins`
3. 看日志流：`easybot --debug` / `/api/v1/logs`

---

## 完整示例

- 入门（教程对照）：[`plugins/example-hello-adapter/`](../plugins/example-hello-adapter/) —— echo 适配器 + PluginTestHost 测试
- 高级参考（真实 HTTP + 鉴权 + 媒体/交互 + wiremock）：`plugins/example-slack-plugin/`
- 内置适配器：`crates/easybot-adapter-{telegram,discord,feishu,qq,wechat}/` 是最好的一手参考
- MockAdapter 源码：`tests/plugins/mock-adapter/src/lib.rs`
