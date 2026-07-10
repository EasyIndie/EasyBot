# 适配器架构全面评审报告

> 生成日期: 2026-07-06 | 最后复核: 2026-07-10
> 范围: 全部五个适配器 + 核心层 (EventBus, AdapterManager, SessionManager)
> 
> ⚠️ **本文档是代码审查快照。已修复的问题保留原文供参考，在每节头部标记当前状态。**

---

### 此后已修复问题一览

以下问题是审查报告发现但**已在此后修复**的：

| 报告引用 | 问题 | 修复验证日期 |
|---|---|---|
| §4.1 | 飞书 `upload_media` base64 未解码 | ✅ 2026-07-10 确认已修复 |
| §2.1 | Telegram AdminCache 使用 `std::sync::Mutex::try_lock` | ✅ 2026-07-10 确认已改用 `AsyncMutex` |
| §5.4 | QQ Gateway 3500s 定时 token 刷新 (已移除) | ✅ 2026-07-10 确认已移除 |
| §6.1 | 微信每条消息同步磁盘 I/O (`save_sync_buf`) | ✅ 2026-07-10 确认已用 `spawn_blocking` |
| §6.2 | 微信非文本消息使用 `[图片]` 占位符 | ✅ 2026-07-10 确认已改为空文本 + `MediaAttachment` |
| §1.3/5.5 | `messages_in` 在 Telegram/Discord/飞书从未递增 | ✅ 2026-07-10 修复：传递 `Arc<AtomicU64>` 给后台任务并递增 |
| §6.1 (续) | 微信 `save_context_tokens` 同步写（2 处） | ✅ 2026-07-10 修复：改用 `spawn_blocking` |

> 注：微信 `save_context_tokens` 共 3 处调用，2 处（send/send_media 的 ret=-14 路径）已修复，
> 第 3 处（`longpoll_loop` 批量持久化）在先前的修复中已使用 `spawn_blocking`。

## 目录

1. [通用跨问题](#1-通用跨适配器问题)
2. [Telegram 适配器](#2-telegram-适配器)
3. [Discord 适配器](#3-discord-适配器)
4. [飞书适配器](#4-飞书适配器)
5. [QQ 适配器](#5-qq-适配器)
6. [微信适配器](#6-微信适配器)
7. [核心层性能评估](#7-核心层性能评估)
8. [优化优先级排序](#8-优化优先级排序)

---

## 1. 通用跨适配器问题

### 1.1 `publish_send_event` 函数在每个适配器中重复定义

每个适配器都定义了完全相同的 `publish_send_event` 函数，仅 `source` 参数（"telegram"、"discord" 等）不同：

- `adapter-telegram/src/lib.rs:457-477`
- `adapter-discord/src/lib.rs:447-467`
- `adapter-feishu/src/lib.rs:403-423`
- `adapter-qq/src/lib.rs:577-597`
- `adapter-wechat/src/lib.rs:674-694`

合计约 100 行完全重复的代码。

**建议**: 提取到 `easybot-core` 作为公共工具函数 `EventBus::publish_send_result()`。

### 1.2 `adapter.health()` 返回的 `HealthReport` 大半字段始终为 `None`

```rust
// 每个适配器的 health() 实现:
HealthReport {
    last_connected_at: None,  // 全部适配器都返回 None
    last_error_at: None,     // 全部适配器都返回 None
    last_error: None,        // 全部适配器都返回 None
    uptime: None,            // 全部适配器都返回 None
    ...
}
```

**影响**: 健康检查 API 返回的信息严重缺失，无法判断上次连接时间、错误历史等有效诊断数据。

**建议**: 在 PlatformAdapter trait 层面增加 `record_connection()`, `record_error()` 等方法，由 Heartbeat 统一管理时间戳追踪，避免每个适配器各自实现。

### 1.3 消息计数原子变量命名不一致

- Telegram: `messages_in: AtomicU64`, `messages_out: AtomicU64`
- Discord: 同上
- Feishu: 同上
- QQ: `messages_in: Arc<AtomicU64>`（注意QQ的`messages_in`包裹了Arc，因为需要跨线程共享给gateway_loop）
- WeChat: 同上

**问题**: QQ 和 WeChat 的 `messages_in` 是 `Arc<AtomicU64>`，而 Telegram/Discord/Feishu 是 `AtomicU64`。这反映了架构差异——某些适配器的后台任务需要递增入站计数，另一些不需要。说明缺乏统一的入站消息追踪模式。

### 1.4 角色/管理员缓存无淘汰策略

所有适配器都实现了某种形式的角色或管理员缓存：

| 适配器 | 缓存结构 | 淘汰策略 |
|---|---|---|
| Telegram | `HashMap<i64, Vec<(i64, SenderRole)>>` | 无（仅 chat_member 事件触发更新） |
| Discord | `HashMap<String, String>` (guild_id → owner_id) | 无 |
| Feishu | `HashMap<String, (SenderRole, Instant)>` + TTL (30s) | 有 TTL |
| QQ | `QqTokenStore` 有 Token TTL | 有 |

**影响**: Telegram 的 admin cache 在长期运行中可能积累大量群聊，且 `try_lock()` 在争用时会静默失败。

## 2. Telegram 适配器

**文件**: `crates/easybot-adapter-telegram/src/lib.rs` (1420 行)

### 2.1 ~~AdminCache 使用 `std::sync::Mutex` 和 `try_lock()`~~ 已修复

> ✅ **已于 2026-07-10 前修复。** 现在使用 `AsyncMutex`（tokio::sync::Mutex），不再有 `try_lock()` 静默跳过问题。

~~```rust
type AdminCache = Arc<Mutex<HashMap<i64, Vec<(i64, SenderRole)>>>>;
// ...
if let Ok(cache) = admin_cache.try_lock() {  // 争用时会静默跳过
```

**问题**: 在异步上下文中使用 `std::sync::Mutex` 配合 `try_lock()`，当锁被持有时不会等待而是静默跳过，导致缓存更新丢失或角色无法解析。如果 `resolve_sender_role` 和 `update_admin_cache` 同时在多个 polling 迭代中执行，try_lock 失败将导致角色回退到 `None`。

**建议**: 改用 `tokio::sync::Mutex` 或使用 DashMap 避免全局锁。~~

### 2.2 长轮询实现缺少 HTTP 429 处理

**影响**: Telegram API 对速率限制有明确的 `retry_after` 响应字段，但当前 `api_call` 方法未做任何特判，直接将 429 当作普通错误处理。虽然调用方返回的 `GatewayError` 可能会触发重试，但不会遵守 `retry_after` 延迟。

### 2.3 长轮询单线程处理消息

`polling_loop` 按顺序处理每次轮询返回的所有更新，每条消息依次调用 `resolve_sender_role(..., .await)` 和 `convert_message(..., .await)`。如果同时收到大量消息（例如 bot 加入大群后收到历史消息），处理会串行排队。

**建议**: 使用 `tokio::spawn` 或 `FuturesUnordered` 并行处理同一批次中的消息，同时在外部设置最大并发数限流。

### 2.4 `api_call` 方法重复检查 `result.ok` 和 `description`

这是 Telegram 适配器的核心 API 调用模式，但在 `poll_once` 中也有近似的反序列化逻辑——本质上是对同一 Telegram API 响应结构的处理在两处重复实现。

## 3. Discord 适配器

**文件**: `crates/easybot-adapter-discord/src/lib.rs` (1643 行)

### 3.1 Gateway 事件处理存在双路径代码

`gateway_shard_loop` 方法（第 290 行）内联处理 `MessageCreate` 事件，且包含了 guild owner 缓存的完整逻辑（包括 spawn 后台任务去获取未缓存的 owner）。而 `handle_gateway_event` 函数（第 402 行）也处理 `MessageCreate`，但没有 guild owner context。

```rust
// gateway_shard_loop 中的第 361 行：
Some(Ok(Event::MessageCreate(msg))) => {
    // 内联处理：会查 guild_owner_cache
    // 还会 spawn 后台任务去获取 owner
}

// 在 other 分支中：
other => {
    match handle_gateway_event(other, &event_bus, &bot_user_id, &heartbeat) {
        // 这个函数也处理 MessageCreate，但没 guild owner context
    }
}
```

**问题**: `Event::MessageCreate` 在主分支被完整处理，不会走到 `other` → `handle_gateway_event`。`handle_gateway_event` 中的 MessageCreate 分支实际上是死代码——永远不会被执行到。

**影响**: 低。但代码可维护性受影响，未来如果有人修改 `gateway_shard_loop` 里的 match，可能会意外让 MessageCreate 漏到 `handle_gateway_event`。

**建议**: 移除 `handle_gateway_event` 中的 MessageCreate 分支，或重构为单一事件处理入口。

### 3.2 每次遇到未缓存的 guild 就 spawn 新 tokio 任务

```rust
// gateway_shard_loop 第 325 行：
tokio::spawn(async move {
    // 获取 guild owner 并更新缓存
});
```

**问题**: 每次收到来自未缓存 guild 的消息时，都会创建一个新的 tokio 任务去获取 guild owner。在 bot 启动时（大量 guild 消息流入），这可能导致几百上千个短暂的空闲任务同时创建。此外，`reqwest::Client` 也在循环中创建（`let http_client = reqwest::Client::new();`），未复用连接池。

**建议**: 使用一个有限并发的工作队列来获取 guild owner，并复用已有的 HTTP 客户端。

### 3.3 API 方法只返回 JSON，未利用 twilight-model

当前 `api_call` 泛型方法直接解析 JSON 到目标类型 T，绕过了 twilight 的类型系统。Discord adapter 已经依赖 `twilight-gateway` 和 `twilight-model`，但 REST API 调用仍然裸用 `reqwest::Client` 解析 JSON。

**影响**: 低，但不如使用 twilight 的 HTTP client 那样获得内置的速率限制处理。

### 3.4 `send_media` 的 multipart 构建复杂

Discord 的 `payload_json` + `files[0]` multipart 上传模式需要手工构建 JSON payload 并确保 `attachments` 数组引用正确。已有一次轻微问题历史（代码中包含了关于 `payload_text` 的日志）。

## 4. 飞书适配器

**文件**: `crates/easybot-adapter-feishu/src/lib.rs` (1388 行)

### 4.1 ~~BUG: `upload_media` 中 base64 数据未解码~~ 已修复

> ✅ **已于 2026-07-10 前修复。** 现在正确调用 `base64::engine::general_purpose::STANDARD.decode(base64_data)`，不再使用 `base64_data.as_bytes().to_vec()`。

~~```rust
// FeishuAdapter::upload_media 第 988-996 行：
let file_data = if let Some(ref url) = media.url {
    // URL 下载 → OK
    resp.bytes().await?.to_vec()
} else if let Some(ref base64_data) = media.data {
    // ⚠️ BUG: 这里用的是 base64 字符串的 ASCII 字节，而不是解码后的二进制
    base64_data.as_bytes().to_vec()
};
```

**影响**: 当通过 `send_media` 使用 base64 data 发送媒体时，传送到飞书的是 base64 ASCII 文本字节，而不是原始二进制文件。飞书收到了一个文本文件而不是图片/视频，上传"成功"但展示异常或损坏。

**建议**: 使用 `base64::engine::general_purpose::STANDARD.decode(base64_data)` 解码。~~

### 4.2 两套独立的 token 管理系统

```rust
// 适配器实例本身的 token 管理:
access_token: tokio::sync::RwLock<Option<String>>,          // 用于 api_get/api_post 等
token_expires_at: tokio::sync::RwLock<i64>,

// WebSocket 后台任务中独立的 token 管理:
token_cache: Arc<Mutex<Option<(String, Instant)>>>,         // 用于角色解析
```

**问题**: `resolve_feishu_role` 和 `ensure_token` 都在刷同一个 `tenant_access_token`，但各自维护独立的缓存。这意味着 token 可能被重复刷新，且两处的刷新逻辑完全重复（都在调用 `auth/v3/tenant_access_token/internal`）。

**建议**: 合并为单一 token Store，通过 `Arc` 在适配器实例和 webSocket 任务间共享。

### 4.3 五个几乎完全相同的 `api_*` 方法

`api_get`、`api_post`、`api_patch`、`api_put`、`api_delete` 五个方法共享相同的 pattern：

```rust
let token = self.ensure_token().await?;
let client = self.client();
let url = format!("{}{}", self.api_base_url(), path);
let req = client.XXX(&url)  // 仅 HTTP 方法不同
    .header(...);
// ... 相同响应解析
```

**建议**: 合并为一个 `send_api_request` 方法，接受 `reqwest::Method` 参数（和 QQ 适配器的设计一样）。

### 4.4 WebSocket 事件回调过度的 Arc 克隆

飞书适配器使用 `EventDispatcher` 链式回调注册，每个 `on_event` 回调闭包都捕获了大量 `Arc` 克隆（`event_bus.clone()`、`feishu_http.clone()`、`role_cache.clone()`、`token_cache.clone()` 等），导致大量模板化代码。

```rust
// lib.rs 第 518-552 行
.on_event(EVENT_MESSAGE_RECEIVE_V1, {
    let eb = eb.clone();       // 5x Arc clone...
    let bot_id = ...;
    let secret = ...;
    let app_id = ...;
    let hc = ...;
    let bu = ...;
    let tc = ...;
    let rc = ...;
    move |event_data| {
        let eb = eb.clone();   // 再次克隆...
        // ...
    }
});
```

**建议**: 使用一个包含所有共享引用的结构体传递给回调（如 `FeishuEventContext`），避免逐一手工克隆。

### 4.5 capability 定义未使用宏

和 Telegram/Discord 不同，飞书的 `capabilities` 使用手动 `vec!` 逐个 push，代码量更大：

```rust
// Feishu: 手动构造 11 个 Capability 结构体
capabilities: vec![
    Capability { name: CapabilityName::Text, supported: true, limits: None },
    Capability { name: CapabilityName::Image, supported: true, limits: None },
    // ... 10 个
]

// Telegram: 使用宏，更简洁
capabilities: capabilities![
    (Text, true), (Image, true), // ...
]
```

**影响**: 极低。风格不一致但功能相同。

## 5. QQ 适配器

**文件**: `crates/easybot-adapter-qq/src/lib.rs` (1727 行)，`gateway.rs` (522 行)

### 5.1 `try_send` 在每次失败时级联尝试所有端点

```rust
async fn try_send(&self, chat_id: &str, body: &Value) -> Result<...> {
    // 1. 尝试频道端点 /channels/{chat_id}/messages
    // 2. 失败后尝试群聊端点 /v2/groups/{chat_id}/messages
    // 3. 失败后尝试 C2C 端点 /v2/users/{chat_id}/messages
}
```

**问题**: 对**每条发出的消息**，如果频道端点的 API 返回错误（即使是速率限制或鉴权错误，而非"频道不存在"），仍然会依次尝试群聊和私聊端点。这意味着：
- 速率限制场景会触发对三个端点的连续调用
- 鉴权错误同样会级联，直到三次都失败后才返回错误
- 没有及早短路：如果 `chat_id` 明确是一个频道 ID，但仍会尝试群聊和 C2C

**建议**: 在使用 `try_send` 前，增加关于 chat_id 格式/类型的预判断（例如通过 `base_url` 自动判断），或一个可选的 `chat_type` 参数来跳过某些端点。至少：对非 404/非"不存在的端点"错误（如 401、429、403）应该立即返回，不向下级联。

### 5.2 `send_media` 的错误嵌套过深（5 层 if-else）

`send_media` 方法（第 825-947 行）包含：
1. 外层 if `data.is_some()` → C2C 上传尝试
2. C2C 失败后 → `try_send` 尝试 msg_type 2
3. msg_type 2 失败且错误码 11255 → msg_type 1 降级
4. try_send 内部的端点级联
5. try_send 之间还有嵌套错误处理

**建议**: 将 send_media 的不同策略提取为独立方法，使用 `Strategy` 模式依次尝试。

### 5.3 `mime_to_file_type` 默认将非视频/音频全部映射为图片类型

```rust
fn mime_to_file_type(mime_type: &str) -> u32 {
    if mime_type.starts_with("image/") { 1 }
    else if mime_type.starts_with("video/") { 2 }
    else if mime_type.starts_with("audio/") { 3 }
    else { 1 }  // ← 文档和纯文件都被当作图片
}
```

**影响**: 文本文件、PDF 等发送时会被 QQ API 视为图片，可能导致上传失败或类型错误。虽然 `send_media` 中 `media.media_type` 字段可以用来做更精确的判断（不必依赖 MIME），但这个工具函数有潜在的误判。

### 5.4 ~~Gateway 循环中 token 刷新频率无效~~ 已修复

> ✅ **已于 2026-07-10 前修复。** 3500s 定时刷新已移除，现在完全依赖 401 自动重试机制。

~~gateway_loop 中有一个 `token_refresh_timer` 每 3500 秒刷新一次 token，但 `send_api_request` 已经实现了 401 自动重试。两者同时存在导致：
- 定时刷新可能发生在没有 API 请求的空闲期，浪费一次 HTTP 调用
- 定时刷新和按需刷新没有协调，可能在短时间内连续刷新两次

**建议**: 移除周期性定时刷新，仅依赖 401 重试机制。或者将 token 过期检查改为"后台自动刷新+按需使用"（如飞书的 `ensure_token` 模式）。~~

### 5.5 `messages_in: Arc<AtomicU64>` 在各适配器间不一致

```rust
messages_in: Arc<AtomicU64>,  // QQ 和 WeChat
// vs
messages_in: AtomicU64,       // Telegram, Discord, Feishu
```

**原因**: QQ 的 `messages_in` 需要从 `gateway_loop` 后台任务中递增。Telegram 的 `polling_loop` 没有递增 `messages_in`（代码中 `messages_in` 字段从未被写入——Telegram adapter 的 `messages_in` 永远为 0）。

**建议**: 所有适配器统一使用 `Arc<AtomicU64>`，并在核心 trait 增加 `on_message_received`/`on_message_sent` 生命周期方法，由 Manager 或统一 trait 自动计数，消除不一致。

## 6. 微信适配器

**文件**: `crates/easybot-adapter-wechat/src/lib.rs` (2257 行)，`crypto.rs` (298 行)

### 6.1 ~~性能瓶颈: 每次收到消息都同步写磁盘~~ 部分修复

> ⚠️ **部分修复：** `save_sync_buf` 已用 `spawn_blocking` 包裹（行 1437-1438），但 `save_context_tokens` 在 session 过期重试路径（行 1097）仍未使用 `spawn_blocking`，仍然在异步上下文中执行同步文件 I/O。

~~`longpoll_loop` 在每条入站消息处理路径上执行同步文件 I/O：

```rust
// longpoll_loop 第 1378-1387 行：
*sync_buf.write().await = new_buf.clone();
save_sync_buf(&new_buf);                  // ❌ 同步写 sync_buf.txt

// 每条消息循环内:
if let Some(ref ct) = msg.context_token {
    let mut tokens = context_tokens.write().await;
    tokens.insert(msg.from_user_id.clone(), ct.clone());
    save_context_tokens(&tokens);          // ❌ 同步写 context_tokens.json
}
```

**影响**: `save_sync_buf` 使用 `std::fs::write`（阻塞调用），而 `save_context_tokens` 调用的 `atomic_write_json` 虽然使用临时文件+rename，但也是同步写。在高频消息场景下（如群聊中大量消息），每个消息处理后都要等待两次磁盘写入完成才能处理下一条。这与 tokio 异步运行时的工作窃取模式不兼容（阻塞磁盘 I/O 会挂起 worker 线程）。

**建议**:
- 使用 `tokio::task::spawn_blocking` 将磁盘 I/O 移出异步热路径。
- 或使用内存 buffer + 节流写入（每 10 秒或每 50 条消息写一次磁盘，减少写入频率）。
- sync_buf 可用 `Arc<RwLock<...>>` + 后台定时持久化替代每次同步写入。~~

### 6.2 ~~`convert_message` 对非文本消息的降级处理~~ 已修复

> ✅ **已于 2026-07-10 前修复。** 非文本消息（图片/语音/文件/视频）不再使用 `[图片]` 占位文本，改为返回空字符串 (`String::new()`) 并填充 `MediaAttachment` 字段（行 1607-1640）。下游通过 `media` 字段和 `msg_type` 判断消息类型。

~~```rust
match item.item_type {
    1 => Some(t.text.clone()),
    2 => Some("[图片]".to_string()),
    3 => Some("[语音]".to_string()),
    // ...
}
```

**影响**: 收到图片/语音/视频消息时，`InboundMessage.text` 被设置为 `"[图片]"` 等占位字符串。这些占位字符串会被下游处理链（如 LLM 调用）当作真实文本处理，可能导致 AI 回复 "我收到了一张[图片]"。

**建议**: 使用 `Option::None`（或专门的 `media_only` 标记）替代占位文本，让下游逻辑能正确区分"纯媒体消息"和"文本消息"。~~

### 6.3 声明的能力与实际支持不匹配（已修复）

WeChat 适配器当前的能力声明为：Text、Image、Audio、Video、Document，均为真实支持。**Group 未声明**（无需额外代码，`capabilities` 列表中不含 `Group`），且 `get_chat_info` 始终返回 `ChatType::Dm`，`list_chats` 返回空。

> 之前版本曾错误声明 `Group: true`，已在清理后移除。

### 6.4 `send_text_http` 和 `send_media_http` 有大量重复模板

两个方法的 HTTP 调用、错误处理、重试逻辑完全相同（差异只有 body 构建方式）：

**建议**: 提取通用 HTTP 发送方法。

### 6.5 `auth_headers()` 中的 `x-wechat-uin` 在每条消息都生成新 UUID

```rust
let uin = uuid::Uuid::new_v4().as_u64_pair().0 as u32;
```

**影响**: 极微。每次发送和轮询都生成随机 Token 用于防重放，这是 iLink API 的要求，无法避免。

### 6.6 CDN 上传流程复杂但缺乏超时控制

整个 `upload_media_to_cdn` 方法（5 步：下载→AES→拿上传 URL→加密→上传）包含多个网络交互，但只在 `cdn_client` 上设置了 120 秒超时，`download_media` 依赖于默认超时。上传失败时的错误消息中将完整的 `ciphertext` 大小和 `aeskey_hex` 写入日志——存在密钥泄露风险。但代码默认级别是 `debug`，生产环境不一定开启，风险可控。

**建议**: 确保所有子步骤都有显式超时。

## 7. 核心层性能评估

### 7.1 EventBus: `subscribe_many` 使用轮询引入延迟

```rust
pub fn subscribe_many(&self, event_types: &[&str]) -> broadcast::Receiver<GatewayEvent> {
    // ...
    tokio::spawn(async move {
        loop {
            let mut had_data = false;
            for i in (0..receivers.len()).rev() {
                loop {
                    match receivers[i].try_recv() {
                        // 有数据则 drain
                    }
                }
            }
            if had_data {
                tokio::task::yield_now().await;
            } else {
                tokio::time::sleep(Duration::from_millis(MERGE_POLL_INTERVAL_MS)).await;
            }
        }
    });
}
```

**问题**: 当事件频率低时，每条事件的延迟增加约 20ms（从 publish 到 merge channel 可读的时间）。虽然对 IM 场景（通常延迟容忍度 > 500ms）不明显，但在高吞吐场景下可能成为瓶颈。

**已实施**:
- `MERGE_POLL_INTERVAL_MS` 已设为 20ms，无需进一步降低
- 如果延迟敏感度提高，可考虑直接不合并，让调用方分别订阅各事件类型

### 7.2 AdapterManager 的 `statuses` 缓存与适配器实时状态不一致

```rust
async fn get_status(&self, platform: &str) -> Option<AdapterStatusSummary> {
    // 优先查 pending_connections（从 statuses 缓存读）
    // 再查已连接适配器（实时调用 status_summary()）
    // 最后回退到 statuses 缓存
}
```

**问题**: 当适配器从 Connected → Failed（后台 task 失败），AdapterManager 的 `adapters` HashMap 中仍保留该适配器（后台 task 退出时不会自动从 managers 移除），导致 `get_status` 一直从实时适配器读取旧的状态，而失败事件可能已经被丢弃。

**建议**: 后台 task 退出时应当通过某种回调通知 Manager 更新状态（事件总线正在做这件事，但 Manager 本身没有监听）。

### 7.3 SessionManager 的富化路径是异步后写

```rust
// session/manager.rs 第 167 行
pub async fn update_source_fields(&self, key: &str, enriched: SessionSource) -> Option<Session> {
    // 更新内存 + 同步写持久化存储
}
```

每次富化都写一次存储层（SQLite/PostgreSQL）。在高频场景下，每一次 `enrich_source` 回调都产生一次数据库写操作。

**建议**: 富化仅更新内存 DashMap，持久化延迟到会话更新时批处理。

### 7.4 AdapterManager 大量使用 `RwLock` 保护的小型 HashMap

`adapters`、`statuses`、`configs`、`pending_connections` 分别使用独立的 `RwLock<HashMap<...>>`。在 `send_message` 路径上，每次调用获取读锁。考虑到适配器数量通常很少（5 个），且 `HashMap` 很小，这个开销可以忽略。

### 7.5 `reconnect_adapter` 的 60 秒等待是同步轮询

```rust
// 第 823-847 行
let deadline = Instant::now() + Duration::from_secs(60);
while Instant::now() < deadline {
    if !self.pending_connections.read().await.contains_key(platform) {
        // 检查状态
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
}
```

**影响**: 轮询间隙 500ms，在 60 秒超时内最多查询 120 次。这是合理的，无需优化。

---

## 8. 优化优先级排序

> 更新于 2026-07-10。已修复项标记为删除线，当前最高优先级无 P0/🔴 项。

| 优先级 | 问题 | 影响范围 | 修复难度 | 当前状态 |
|---|---|---|---|---|
| ~~🔴 P0~~ | ~~飞书 `upload_media` base64 未解码 (#4.1)~~ | ~~Feishu~~ | ~~1 行~~ | ✅ 已修复 |
| ~~🔴 P0~~ | ~~微信每条消息同步磁盘 I/O (#6.1)~~ | ~~WeChat~~ | ~~中等~~ | ✅ 已修复 (含 save_context_tokens) |
| 🟠 **P1** | QQ `try_send` 错误级联 (#5.1) | QQ | 低 | ❌ 未修复 |
| 🟠 **P1** | 飞书两套独立 token 管理系统 (#4.2) | Feishu | 低 | ❌ 未修复 |
| 🟡 **P2** | Discord gateway 事件双路径 (#3.1) | Discord | 低 | ❌ 未修复 |
| 🟡 **P2** | `publish_send_event` 重复代码 (#1.1) | 全部适配器 | 低 | ❌ 未修复 |
| 🟡 **P2** | 飞书 5 个 `api_*` 方法合并 (#4.3) | Feishu | 低 | ❌ 未修复 |
| 🟡 **P2** | EventBus `subscribe_many` 20ms 延迟 (#7.1) | Core | 低 | ❌ 未修复 |
| 🟢 **P3** | `HealthReport` 字段全部为 None (#1.2) | 全部 | 中等 | ❌ 未修复 |
| ~~🟢 P3~~ | ~~`messages_in` 从未递增 (Telegram/Discord/Feishu) (#1.3/5.5)~~ | ~~全部~~ | ~~低~~ | ✅ 已修复 |
| 🟢 **P3** | Adapter uptime 未追踪 | 全部 | 低 | ❌ 未修复 |
| 🟢 **P3** | 微信声明能力与实际不匹配 (#6.3) | WeChat | 低 | ❌ 未修复 |
| ⚪ **P4** | Discord 每次未缓存 guild 都 spawn tokio 任务 (#3.2) | Discord | 低 | ❌ 未修复 |
| ⚪ **P4** | QQ `mime_to_file_type` 默认图片 (#5.3) | QQ | 低 | ❌ 未修复 |
| ⚪ **P4** | 微信 CDN 密钥写 debug 日志 (#6.6) | WeChat | 低 | ❌ 未修复 |

---

## 总结

### 架构强项
- **Adapter trait 设计清晰**: `PlatformAdapter` 提供了完善的抽象，使得 5 个差异巨大的 IM 平台能统一接入
- **Heartbeat 机制优雅**: 统一的背景任务存活检测，被所有适配器采纳
- **无锁数据结构使用得当**: DashMap 在 SessionManager 和 EventBus 中的使用减少了锁争用
- **异步分离良好**: connect() 在背景执行，不阻塞 Manager 启动流程

### 曾需立即修复的缺陷 (现已修复)
1. ~~**飞书 base64 上传 bug**~~ — 已修复，现正确使用 `STANDARD.decode()`
2. ~~**微信同步磁盘 I/O**~~ — 已全部修复，`save_sync_buf` 和 `save_context_tokens` 均已使用 `spawn_blocking`
3. ~~**Telegram AdminCache try_lock**~~ — 已修复，改为 `AsyncMutex`
4. ~~**`messages_in` 从未递增**~~ — 已修复，Telegram/Discord/飞书三适配器均通过 `Arc<AtomicU64>` 传递并递增

### 架构层面的改进方向
1. **token 管理统一化**: 每个适配器都有自己的 token 刷新逻辑（飞书两套、QQ 两套），可以抽象出公共的 `TokenManager` 结构体
2. **缓存策略标准化**: 角色/管理员缓存的淘汰策略不一致（Telegram 无 TTL vs Feishu 30s TTL）
3. **API 调用模式提取**: 各适配器的 api_* 方法重复度极高，可以组合 `Method` 参数提取为通用模式
