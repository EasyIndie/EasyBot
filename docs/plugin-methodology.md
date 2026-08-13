# EasyBot 适配器插件方法论（编码规范）

> 插件作者必须遵守的标准，与内置适配器同一水平。这些约束不是「建议」——宿主长期运行的稳定性依赖它们（对齐 CLAUDE.md「资源管理指南」与「Key Patterns」）。快速上手见 [`plugin-quickstart.md`](plugin-quickstart.md)，API 参考见 [`plugin-guide.md`](plugin-guide.md)，安全边界见 [`docs/SECURITY.md`](SECURITY.md)。

---

## 1. 适配器设计原则

1. **平台名唯一** — `platform_name()` 用 kebab-case 小写（`"my-adapter"`），不得占用内置平台名（`telegram`/`discord`/`feishu`/`qq`/`wechat` 等）——`PluginLoader` 会拒绝平台冲突。
2. **能力声明准确** — `capabilities()` 如实声明；未实现的能力 `supported: false` 或 `capability_not_supported`。
3. **无阻塞初始化** — `init()` 只做校验和配置存储，**网络连接在 `connect()` 中建立**。`init()` 里做网络请求会拖慢启动。
4. **状态更新及时** — `init()` / `connect()` / `disconnect()` 必须正确推进 `AdapterState`，宿主健康监测依赖它。
5. **Send + Sync** — `PlatformAdapter` 要求 `Send + Sync`；内部可变状态用线程安全容器（`Mutex`/`RwLock`/`dashmap`）。
6. **日志用 tracing** — 插件日志自动进宿主 `/logs` 环形缓冲；用 `RUST_LOG=my_adapter=trace` 可细分。不要 `println!`。
7. **凭据永不明文** — 不硬编码 token；日志/错误消息里输出密钥必须掩码。`AdapterConfig` 的 `Debug` 已对 `token`/`api_key` 脱敏，但你自己打印 `token` 字段时要同样处理。

---

## 2. 错误分类（结构化优先，影响健康监测）

宿主健康监测器按 `GatewayError` 变体**结构化分类**决定重试还是永久停用：

- **永久（立即 Failed）**：`AuthFailed` / `Unauthorized` / `Forbidden`（凭据被拒）
- **瞬态（退避重试）**：`Transient` / `RequestTimeout` / `RateLimited`

启发式只兜底，且**网络信号优先于永久关键词**——不要把 `"X auth failed: <网络错误>"` 误判成永久。

**规则**：

| 场景 | 必须 |
|------|------|
| reqwest `send()` 网络层失败 | 用 `GatewayError::Transient(...)` 包裹，**不要**压成 `Internal(String)` |
| `connect()` 失败 | `ConnectResult::failed(msg, kind)`：网络 → `ConnectErrorKind::Transient`，凭据被拒 → `Permanent`，未知 → `None` |
| 业务失败 | `GatewayError::SendError(...)`（不触发重连） |
| 超时/限流 | `RequestTimeout` / `RateLimited`（瞬态） |

> 新适配器约定：`send()` 网络层失败必须 `Transient`；`connect()` 失败用 `ConnectResult::failed(error, kind)` 透传分类——它穿透 `retry_transport()` / `reconnect_adapter()` 的决策。

### 传输可注入

真实平台的 HTTP client 通过**构造器或 `init(config)` 注入**（`base_url` 指向平台 API，测试时指向 wiremock）。这让你能离线测协议交互，不依赖网络。

---

## 3. 心跳语义

心跳表示「**后台任务存活且正在重试**」，不要求消息一定成功。

| 方法 | 含义 |
|------|------|
| `Heartbeat::beat()` | 每轮后台循环（**含错误/重试路径**）都调，表示任务还活着 |
| `Heartbeat::beat_success()` | 真正收到消息 / 一轮轮询成功时调 |
| `Heartbeat::record_failure()` | 错误路径调（累计连续失败数） |

- `heartbeat_age_ms()` 过期（> 120s）→ `HealthStatus::Degraded` → 健康监测器介入。
- `heartbeat_success_age_ms()` 过期但 `beat()` 新鲜 → `Degraded`（任务活着但消息流被阻塞）。
- 连续失败过多（`heartbeat_failure_count()` 超阈值）但一直 `beat()` → 检测为「卡死重试循环」→ `Down`。

**禁止**使用独立定时器无条件 `beat()`——那会让「后台任务已死」也显得活着，健康监测完全失效（内置飞书适配器曾有此 bug，已修复）。

---

## 4. 缓存：必须带上限或 TTL

**所有适配器缓存必须有大小上限或 TTL 淘汰**，否则长期运行内存无界增长。

参考内置常量模式：`CHAT_TYPE_CACHE_LIMIT`（QQ 10K）/ `ADMIN_CACHE_LIMIT` / `GUILD_CACHE_LIMIT`（Discord 5K）/ Telegram 5K / 飞书 30s TTL。

```rust
const CHAT_TYPE_CACHE_LIMIT: usize = 10_000;
```

选择：`dashmap` + 上限 + 淘汰；或 TTL 过期清理；或 LRU。**三者至少满足其一。**

---

## 5. 并发上限

**所有 `tokio::spawn` 循环必须考虑并发上限**，用 `Semaphore` 或 `JoinSet`。

```rust
// 轮询任务：每轮最多同时处理 N 个请求
let sem = Arc::new(Semaphore::new(16));
```

内置参考：Webhook 并发 Semaphore 上限 16；WebSocket 最大客户端 1000。无上限的 spawn 循环在慢下游下会线程爆炸。

---

## 6. 消息与媒体

- `OutboundMessage` / `SendTextParams` 构造约定见 guide；文本发送走 `send()`。
- **media 按 chat type 区分**（QQ 教训）：同一份媒体代码不能对群聊/私聊/频道统一用一个 `msg_type`。以 QQ 为例：
  - **Channel**：`msg_type: 2`（rich media）
  - **Dm（C2C）**：`msg_type: 1`（image embed，v2 API 不支持 `2`）
  - **Group**：必须走文件上传 + `msg_type: 7`（media），v2 群聊端点的 `1/2` 均不可用
- 其它平台同样需要 `list_chats()` / `get_chat_info()` 返回的 `chat_type` 决定请求形态。

---

## 7. 原始 payload 仅进 metadata，不消费

各适配器在解析字段前，把平台原始 payload 序列化存入 `InboundMessage.metadata`——**仅调试用，不做任何二次处理/消费**。由宿主 `api.raw_payload_enabled`（默认 `false`）控制 WebSocket 事件中是否透传该字段。

插件侧同样：收到平台原始报文时，先存 `metadata`（供排障），再解析业务字段。**不要**把原始报文当业务数据用。

---

## 8. 启停与热重载语义

| 操作 | 语义 |
|------|------|
| `disable` | 写 `enabled: false` 并**立即** stop + unregister（`adapter_manager.stop(platform)`） |
| `enable` | 写回 `true`，**下次启动生效**（v1 不做热 dlopen） |
| 卸载 | stop + unregister 后删目录；`stop` 失败则拒绝卸载 |
| 孤儿数据 | 卸载/禁用后的会话/消息由现有 **TTL 保留策略**兜底，不级联删除 |
| 市场配置（registries/trusted_publishers）变更 | 需重启生效；dev 热加载仅指插件本体，不含市场源配置 |
| 插件升级后 ABI 不兼容 | `requires.easybot` / `sdk_version` 安装前拒；主程序更新时 `check_plugin_compatibility` 阻止 |

---

## 9. 安全边界（无沙箱，容器化兜底）

**插件没有沙箱**，以宿主进程相同权限运行。签名 / verified 徽标只证明「代码来自该发布者、未被篡改」，**不证明代码安全**（VS Code 验证徽章被滥用为信任信号的教训）。

- 不得硬编码/日志输出凭据（对齐 no-hardcoded-credentials 约束）。
- 安装第三方发布者插件前自行评估。
- 生产隔离用**容器化兜底**：把 EasyBot（含插件）跑在容器里，限制可写文件系统与网络（见 `docs/docker-compose.plugin-dev.yml` 与 `docs/SECURITY.md`）。

---

## 10. ABI 纪律

- `plugin.yaml` 的 `sdk_version` 必须等于编译所用 SDK 的 `EASYBOT_PLUGIN_ABI_VERSION`。
- 升级 SDK 到不兼容版本 = 重新发布插件（按 `sdkVersion` 重建）。
- 宿主侧 ABI 常量是 loader 自身那份（`plugin/loader.rs`），SDK 发布侧用 SDK 那份（`easybot-plugin-sdk/src/ffi.rs`），两处互相注释「同步」——发布时保持一致。
- `panic = "abort"`：跨 FFI 不得 unwind。
- **FFI 分配器契约（最深的坑）**：插件在进程内 dlopen，与宿主通过 FFI 收发
  String/Vec/Value 等**带堆所有权**的值（宿主构造→插件 Drop，或反向），两侧
  **必须共用同一全局分配器**。因此：
  - **宿主侧**：禁用自定义 `#[global_allocator]`（`bin/Cargo.toml` 已移除
    mimalloc 并注释警示，防止回归）。宿主 mimalloc + 插件系统 malloc → 交叉
    free → SIGABRT；插件各自静态链接 mimalloc → 进程内两套堆 → 析构死锁。
  - **宿主侧（`PluginLoader::create_adapter`）**：改为返回 `PluginAdapterProxy`——
    宿主只**借读**插件返回的 `Box<Box<dyn PlatformAdapter>>` 胖指针
    （`ptr::read` + `ManuallyDrop`，不取得所有权），代理 Drop 时调用插件的
    `easybot_plugin_destroy`——**谁分配谁释放**。即便两侧分配器不同也不交叉释放。
    旧实现宿主 `Box::from_raw` 后用自己的分配器释放插件内存，Linux musl-static
    宿主跨堆释放即 UB（macOS 两侧共享系统 libc 无感，故早期未暴露）。
  - **插件侧**：同样**不要**声明 `#[global_allocator]`，保持默认（= System）。
  - 完整诊断过程（症状/根因/验证）见独立样例仓库
    [`EasyIndie/easybot-hello-adapter`](https://github.com/EasyIndie/easybot-hello-adapter)
    的[插件开发指南 §8.1「FFI 分配器契约」](https://github.com/EasyIndie/easybot-hello-adapter/blob/main/docs/plugin-development-guide.md)。

---

## 11. 跨平台发布约束

- **Linux 插件必须 musl 静态编译**（宿主 musl-static，glibc `.so` 无法 dlopen）。
- macOS `MACOSX_DEPLOYMENT_TARGET` 须 ≤ 宿主（10.15/11.0）；Windows 可选 `crt-static`。
- 同一份源码，`plugin-publish.yml` 交叉编译 6 个 target 产物，安装端按宿主 triple 只下载匹配项。

---

## 12. 测试金字塔

| 层 | 工具 | 测什么 |
|---|---|---|
| 1. 单元 | `tests/unit.rs` | capability 声明、消息构造、错误分类逻辑 |
| 2. 宿主 | `PluginTestHost`（SDK `testing` feature） | connect/send/事件流，离线 |
| 3. 协议 | wiremock（`config_with_base_url` 指向 mock） | 平台 HTTP 协议交互 |
| 4. 端到端 | 真实平台凭据 + 真实 EasyBot | 全链路 |

方法论核心：**传输可注入**——HTTP client 经构造器/`init(config)` 注入，2/3 层不需要真实网络。
