# EasyBot 审计修复计划

> 审计历史: [Round 1](#round-1-已完成-2026-06-24-早前) (30 项 · ✅ 全部完成) → **Round 2** (20 项 · ✅ 19/20 完成)
>
> ⚠️ **状态更新**: Round 2 几乎所有修复已应用（19/20），仅 P2-4 QQ/WeChat 适配器大文件拆分待最终编译验证。**当前最新状态见 [TODO.md](TODO.md)。**
>
> 关联文档: [TODO.md](TODO.md)

---

## Round 2 · 进度总览

> 基于 2026-06-24 第二轮全面审计 | 20 项新发现 (N1–N20)
>
> ⚠️ 本文档描述各修复项的技术方案。**实际修复已应用（19/20），详细状态见 [TODO.md](TODO.md) Round 2 章节。**

| 优先级 | 数量 | 状态 | 预计总工时 |
|--------|:----:|------|:----------:|
| **P0 紧急** | 5 | ✅ 已完成 | ~10h |
| **P1 高优先级** | 8 | ✅ 已完成 | ~14h |
| **P2 中优先级** | 4 | ✅ 3/4 (仅拆分待验证) | ~16h |
| **P3 低优先级** | 3 | ✅ 已完成 | ~12h |
| **合计** | **20** | **19/20** | **~52h** |

---

## Round 2 · P0 紧急修复

> 严重安全漏洞 + Crash 风险 + 数据安全风险

### ⬜ P0-1. 实现 API Key 权限检查中间件 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/server.rs` + 新建 `crates/easybot-core/src/auth/permissions.rs` |
| **审计 ID** | N8 |
| **严重性** | 🔴 Critical — 任何 API key 可执行任何操作 |
| **工时** | 4h |

**问题**: 所有受保护路由均通过 `auth_middleware` 验证 Bearer token，但 `AuthInfo` 注入到 request extensions 后**从未被检查**。拥有任意 API key 的攻击者可执行全部特权操作（启停适配器、删改配置、发送消息、删除 session）。

**修复方案**:

```rust
// crates/easybot-core/src/auth/permissions.rs (新建)

/// 权限位标志
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    MessagesRead,
    MessagesSend,
    AdaptersRead,
    AdaptersManage,   // start / stop
    ConfigRead,
    ConfigWrite,
    SessionsRead,
    SessionsManage,   // delete
    WebSocketConnect,
}

/// 检查 AuthInfo 是否持有所需权限
pub fn require_permission(auth: &AuthInfo, required: Permission) -> Result<(), GatewayError> {
    if auth.permissions.contains(&"*".to_string())
        || auth.permissions.contains(&format!("{:?}", required).to_lowercase())
    {
        Ok(())
    } else {
        Err(GatewayError::forbidden(format!(
            "需要权限 {:?}", required
        )))
    }
}
```

```rust
// crates/easybot-api/src/server.rs — 在路由层添加权限中间件

use easybot_core::auth::permissions::{Permission, require_permission};

fn permission_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth = request.extensions().get::<AuthInfo>()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // 根据路由路径推断所需权限
    let required = match request.uri().path() {
        p if p.starts_with("/api/v1/adapters/") && request.method() == Method::POST
            => Permission::AdaptersManage,
        p if p.starts_with("/api/v1/config") && request.method() == Method::PUT
            => Permission::ConfigWrite,
        p if p.starts_with("/api/v1/config")
            => Permission::ConfigRead,
        p if p.starts_with("/api/v1/messages/send")
            => Permission::MessagesSend,
        p if p.starts_with("/api/v1/messages")
            => Permission::MessagesRead,
        p if p.starts_with("/api/v1/sessions") && request.method() == Method::DELETE
            => Permission::SessionsManage,
        p if p.starts_with("/api/v1/sessions")
            => Permission::SessionsRead,
        _ => return Ok(()),  // 无特殊权限要求
    };
    require_permission(auth, required)
        .map_err(|_| StatusCode::FORBIDDEN)
}
```

**验收标准**:
1. 创建一个仅 `messages:send` 权限的 key，调用 `POST /adapters/{platform}/start` 返回 403
2. 创建一个 `*` 权限的 key，所有端点正常返回
3. 无认证 key 的请求仍返回 401（不变）

---

### ⬜ P0-2. 消除 Feishu 适配器 connect() 中 unwrap() panic 风险 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-feishu/src/lib.rs` 第 466 行 |
| **审计 ID** | N1 |
| **严重性** | 🔴 Critical — 生产环境 panic |
| **工时** | 15 分钟 |

**问题**: `self.config.as_ref().unwrap()` — 若 `connect()` 在 `init()` 之前被调用, `config` 为 `None`，服务直接 panic 崩溃。

**当前代码**:
```rust
let config = self.config.as_ref().unwrap();
```

**修复方案**:
```rust
let config = self.config.as_ref()
    .ok_or_else(|| GatewayError::internal("connect() called before init() — config not set"))?;
```

**验收标准**: 注释掉 `init()` 调用，`connect()` 应返回 `Err` 而非 panic。

---

### ⬜ P0-3. 消除 batch-send 中 Arc::try_unwrap() panic 风险 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/routes/messages.rs` 第 283 行 |
| **审计 ID** | N2 |
| **严重性** | 🔴 Critical — 生产环境 panic |
| **工时** | 20 分钟 |

**问题**: `Arc::try_unwrap(results).unwrap().into_inner()` — 若 join_all 后 Arc 引用计数不为 1（如某 task handle 泄漏），直接 panic。

**当前代码**:
```rust
let final_results = Arc::try_unwrap(results).unwrap().into_inner();
```

**修复方案** — 用 `Arc::get_mut` 安全解包:
```rust
let mut results = Arc::try_unwrap(results)
    .map(|m| m.into_inner())
    .unwrap_or_else(|arc| (*arc).clone());
```

或更简洁地，直接收集到 Vec 而非 Arc<Mutex<Vec>>:
```rust
// 改用 tokio::task::JoinSet + 直接收集
let mut join_set = tokio::task::JoinSet::new();
for target in &req.targets {
    join_set.spawn(/* ... */);
}
let mut final_results = Vec::new();
while let Some(result) = join_set.join_next().await {
    final_results.push(result.unwrap_or_else(|e| /* error result */));
}
```

**验收标准**: batch-send 功能正常，不再有 `try_unwrap` panic 路径。

---

### ⬜ P0-4. 用 QueryBuilder 替代 AssertSqlSafe + format! 拼接 SQL 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-core/src/storage/sqlite.rs` 第 239-257, 412-445 行 + `postgres.rs` 第 206-223, 370-412 行 |
| **审计 ID** | N9 |
| **严重性** | 🔴 Critical — SQL 注入风险 |
| **工时** | 3h |

**问题**: SQLite 和 PostgreSQL 后端使用 `format!(" LIMIT {}", limit)` + `AssertSqlSafe` 绕过了 sqlx 编译时查询检查。`AssertSqlSafe` 是 `#[doc(hidden)]` 的 sqlx 内部 API。虽然 `limit` 和 `offset` 当前是 `usize`（安全），但这个模式极其脆弱——任何人后续添加字符串参数并按此模式拼接即可引入 SQL 注入。

**当前代码**:
```rust
sql.push_str(&format!(" LIMIT {}", limit));
let mut query = sqlx::query(AssertSqlSafe(sql.as_str()));
```

**修复方案** — 使用 `sqlx::QueryBuilder`:
```rust
use sqlx::QueryBuilder;

let mut builder = QueryBuilder::new(base_sql);
if let Some(limit) = limit {
    builder.push(" LIMIT ").push_bind(limit as i64);
}
if let Some(offset) = offset {
    builder.push(" OFFSET ").push_bind(offset as i64);
}
let query = builder.build();
```

**验收标准**: 
1. 所有 `AssertSqlSafe` 引用从代码库中移除
2. 现有存储测试通过
3. `cargo clippy` 无新增警告

---

### ⬜ P0-5. 生产环境限制 CORS 为白名单 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/server.rs` 第 244 行 |
| **审计 ID** | N10 |
| **严重性** | 🔴 High — CSRF 风险 |
| **工时** | 2h |

**问题**: `CorsLayer::permissive()` 允许任意 origin 发起认证请求。虽然 API 使用 Bearer token（非 cookie），但 `Access-Control-Allow-Credentials: true` 下任何网页均可向网关发送认证请求。

**当前代码**:
```rust
let cors = CorsLayer::permissive();
```

**修复方案**:
```rust
let cors = if cfg!(debug_assertions) {
    CorsLayer::permissive()
} else {
    CorsLayer::new()
        .allow_origin(config.cors_allowed_origins())  // 从 config 读取
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
};
```

同时在 `GatewayConfig` 中新增:
```rust
#[serde(default = "default_cors_origins")]
pub cors_allowed_origins: Vec<String>,

fn default_cors_origins() -> Vec<String> {
    vec!["http://localhost:3000".into()]  // 默认仅本地
}
```

**验收标准**:
1. Debug 模式保持 permissive（开发体验不变）
2. Release 模式下，未在 `cors_allowed_origins` 中的 origin 收到 CORS 错误
3. 配置热重载后 CORS 策略即时生效

---

## Round 2 · P1 高优先级

> 功能性缺陷 + DoS 加固 + 可靠性风险

### ⬜ P1-1. 修复 Workspace Lint 继承 — 强制 unsafe_code = "deny"

| 属性 | 内容 |
|------|------|
| **文件** | 所有子 crate 的 `Cargo.toml` |
| **审计 ID** | N3 |
| **严重性** | High — 安全检查未生效 |
| **工时** | 30 分钟 |

**问题**: 根 `Cargo.toml` 第 96 行声明 `unsafe_code = "deny"`，但零个子 crate 包含 `[lints]\nworkspace = true`。实际编译时 `unsafe_code` 处于默认 `allow` 级别，lint 规则完全无效。

**修复方案**: 在每个 crate 的 `Cargo.toml` 中添加:
```toml
[lints]
workspace = true
```

需修改的文件 (14 个):
- `bin/Cargo.toml`
- `crates/easybot-core/Cargo.toml`
- `crates/easybot-api/Cargo.toml`
- `crates/easybot-adapter-telegram/Cargo.toml`
- `crates/easybot-adapter-discord/Cargo.toml`
- `crates/easybot-adapter-feishu/Cargo.toml`
- `crates/easybot-adapter-qq/Cargo.toml`
- `crates/easybot-adapter-wechat/Cargo.toml`
- `crates/easybot-plugin-sdk/Cargo.toml`
- `tests/integration/Cargo.toml`
- `tests/e2e/Cargo.toml`
- `tests/fixtures/Cargo.toml`
- `tests/plugins/mock-adapter/Cargo.toml`

**可能冲突**: 现有测试代码中的 `unsafe { env::set_var(...) }` 调用（config 测试 + adapter manager 测试）会触发编译错误。这些需要保留但加上 `#[allow(unsafe_code)]` 注解。

**验收标准**: `cargo clippy --workspace` 无 unsafe_code 警告（测试中的 env::set_var 除外）。

---

### ⬜ P1-2. 添加 HTTP 请求体大小限制

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/server.rs` |
| **审计 ID** | N11 |
| **严重性** | High — DoS |
| **工时** | 30 分钟 |

**问题**: `tower-http` 依赖已包含 `limit` feature，但未应用 `RequestBodyLimitLayer`。攻击者可发送任意大小的 JSON payload 耗尽内存。

**修复方案**:
```rust
use tower_http::limit::RequestBodyLimitLayer;

// 在 api_routes 的 ServiceBuilder 中添加
.layer(RequestBodyLimitLayer::new(256 * 1024))  // 256 KiB
```

**例外**: WebSocket upgrade 路由不需要 body limit（它不读 body）。

**验收标准**: `curl -X POST -d @5mb.json localhost:8080/api/v1/messages/send` 返回 413 Payload Too Large。

---

### ⬜ P1-3. 添加 WebSocket 消息帧大小限制

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/routes/ws.rs` 第 91 行附近 |
| **审计 ID** | N12 |
| **严重性** | High — DoS |
| **工时** | 30 分钟 |

**问题**: WS 帧反序列化前无大小检查，攻击者可发送 MB 级文本帧耗尽内存。

**修复方案**:
```rust
const MAX_FRAME_SIZE: usize = 64 * 1024;  // 64 KiB

// 在 handle_client_frame 的 Text 分支前
Message::Text(text) => {
    if text.len() > MAX_FRAME_SIZE {
        tracing::warn!(size = text.len(), "WebSocket frame too large, closing");
        return Ok(CloseReason::from((StatusCode::POLICY_VIOLATION, "Frame too large")));
    }
    // ... 原有处理逻辑
}
```

**验收标准**: 发送 >64 KiB 的 WS 帧，连接被关闭（POLICY_VIOLATION）。

---

### ⬜ P1-4. QQ 适配器 std::sync::Mutex → tokio::sync::Mutex

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-qq/src/lib.rs` 第 11, 37, 42, 91-93, 103-106, 123 行 |
| **审计 ID** | N4 |
| **严重性** | High — 阻塞 tokio worker 线程 |
| **工时** | 1h |

**问题**: `QqTokenStore` 使用 `Arc<std::sync::Mutex>` 在 async 上下文中。虽然当前争夺极低，但 `std::sync::Mutex::lock()` 阻塞 tokio worker 线程，违反 async 最佳实践。`parking_lot` 已是 workspace 依赖。

**修复方案**:
```rust
// 改前
use std::sync::Mutex;
type QqTokenStore = Arc<Mutex<Option<QqToken>>>;

// 改后
use parking_lot::Mutex as AsyncMutex;  // parking_lot 已在 workspace 依赖中
type QqTokenStore = Arc<AsyncMutex<Option<QqToken>>>;
```

`parking_lot::Mutex` 的优势：无中毒（poisoning），更轻量的锁实现，不会阻塞线程调度器。若需要 `.await` 点期间持有锁，改用 `tokio::sync::Mutex`。

**验收标准**: QQ 适配器现有测试通过，`cargo clippy` 无新增警告。

---

### ⬜ P1-5. SessionManager store 操作失败时添加日志

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-core/src/session/manager.rs` 第 72, 92, 110, 173 行 |
| **审计 ID** | N5 |
| **严重性** | High — 数据丢失无感知 |
| **工时** | 20 分钟 |

**问题**: session 增删持久化错误被 `let _ =` 静默吞没。若数据库不可用，session 静默丢失且无任何观测信号。

**当前代码**:
```rust
let _ = store.upsert_session(&session).await;
let _ = store.delete_session(key).await;
```

**修复方案**:
```rust
if let Err(e) = store.upsert_session(&session).await {
    tracing::warn!(?e, key = %session.key, "持久化 session 失败");
}
if let Err(e) = store.delete_session(key).await {
    tracing::warn!(?e, key, "删除 session 失败");
}
```

**验收标准**: 数据库不可用时日志中出现 warn 级别记录。

---

### ⬜ P1-6. /metrics 端点添加认证

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/server.rs` 第 137-141 行 |
| **审计 ID** | N13 |
| **严重性** | Medium — 信息泄露 |
| **工时** | 30 分钟 |

**问题**: Prometheus `/metrics` 端点公开无认证，暴露平台名、连接状态、消息量等内部信息。

**修复方案** — 选项 A (推荐): 移到认证路由下:
```rust
let api_routes = Router::new()
    // ... 其他路由
    .route("/metrics", get(metrics_handler));  // 移入 api_routes
```

**选项 B**: 保持公开但加 IP 白名单:
```rust
let metrics_routes = Router::new()
    .route("/metrics", get(metrics_handler))
    .layer(IpWhitelistLayer::new(config.metrics_allowed_ips));
```

**验收标准**: 无 token 请求 `/metrics` 返回 401（若选 A）。

---

### ⬜ P1-7. 修复 X-Forwarded-For 信任链

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/middleware/rate_limit.rs` 第 176-187 行 |
| **审计 ID** | N14 |
| **严重性** | Medium — 限流绕过 |
| **工时** | 30 分钟 |

**问题**: Rate limiter 取 `X-Forwarded-For` 链中的**第一个** IP（可被客户端伪造），而非**最后一个**（可信反向代理添加的）。

**当前代码**:
```rust
let ip = req.headers().get("x-forwarded-for")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.split(',').next().map(|s| s.trim().to_string()))
    .unwrap_or_else(|| /* socket addr */);
```

**修复方案**:
```rust
let ip = req.headers().get("x-forwarded-for")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.split(',').next_back()  // 取最后一个（可信代理添加的）
        .map(|s| s.trim().to_string()))
    .unwrap_or_else(|| /* socket addr */);
```

**验收标准**: 伪造的 `X-Forwarded-For` 头中第一个 IP 不被用于限流计数。

---

### ⬜ P1-8. Feishu SDK 版本审计 + 固定

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-feishu/Cargo.toml` 第 16 行 |
| **审计 ID** | N15 |
| **严重性** | Medium — 供应链风险 |
| **工时** | 2h |

**问题**: `larksuite-oapi-sdk-rs = "0.1"` 是极早期版本，可能存在未发现的安全漏洞（尤其是 WebSocket token 处理）。

**修复方案**:
1. 审计 `larksuite-oapi-sdk-rs` 源码中的安全关键路径（token 获取/刷新、WebSocket 重连、TLS 处理）
2. 固定到具体 patch 版本:
```toml
larksuite-oapi-sdk-rs = { version = "0.1", features = ["ws"] }
# 或固定: larksuite-oapi-sdk-rs = { version = "=0.1.X", features = ["ws"] }
```
3. 在 `deny.toml` 中添加对飞书 SDK 的特别关注注释

**验收标准**: 飞书适配器 E2E 测试通过，`cargo deny check` 无新增告警。

---

## Round 2 · P2 中优先级

> 可维护性 + 一致性改进

### ⬜ P2-1. webhook 序列化失败静默修复

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-core/src/webhook/mod.rs` 第 171 行 |
| **审计 ID** | N6 |
| **严重性** | Medium — 静默数据丢失 |
| **工时** | 15 分钟 |

**问题**: `serde_json::from_slice(&payload_bytes).unwrap_or_default()` — 若反序列化失败（bug），webhook 收到 `{}` 且无任何日志。

**修复方案**:
```rust
let payload_json: serde_json::Value = match serde_json::from_slice(&payload_bytes) {
    Ok(v) => v,
    Err(e) => {
        tracing::warn!(?e, "webhook payload 反序列化失败，使用空对象");
        serde_json::Value::default()
    }
};
```

**验收标准**: 代码逻辑不变，仅添加日志。

---

### ⬜ P2-2. 适配器 HTTP Client 类型统一为 OnceLock

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-feishu/src/lib.rs`, `crates/easybot-adapter-qq/src/lib.rs` |
| **审计 ID** | N16 |
| **严重性** | Medium — 不一致 |
| **工时** | 1.5h |

**问题**: Feishu 和 QQ 适配器使用 `Option<reqwest::Client>`，而 Telegram 和 Discord 已使用 `OnceLock<reqwest::Client>`。后者提供线程安全的懒初始化。

**修复方案**: 将 Feishu 和 QQ 的 `http_client` 字段从 `Option<reqwest::Client>` 改为 `OnceLock<reqwest::Client>`，初始化处用 `OnceLock::get_or_init()`。

```rust
// 改前
http_client: Option<reqwest::Client>,

// 改后
http_client: OnceLock<reqwest::Client>,

// 初始化
adapter.http_client.get_or_init(|| {
    reqwest::Client::builder().timeout(...).build().unwrap()
});
```

**验收标准**: 飞书/QQ 适配器现有测试通过。

---

### ⬜ P2-3. WeChat 适配器构造函数统一

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-wechat/src/lib.rs` + `bin/src/main.rs:655-669` |
| **审计 ID** | N17 |
| **严重性** | Medium — 不一致 |
| **工时** | 30 分钟 |

**问题**: WeChat 适配器使用独立构造函数 `new_with_event_bus(eb)`，所有其他适配器用 `new()` + `set_event_bus(eb)`。

**修复方案**: 移除 `new_with_event_bus()`，在 `WeChatAdapter` 上添加 `set_event_bus()` 方法，与其他 4 个适配器对齐。

**验收标准**: WeChat 适配器 E2E 测试通过。

---

### ⬜ P2-4. 拆分 QQ/WeChat 适配器大文件

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-qq/src/lib.rs` (2232 行), `crates/easybot-adapter-wechat/src/lib.rs` (2262 行) |
| **审计 ID** | N7 |
| **严重性** | Medium — 可维护性 |
| **工时** | 12h |

**目标结构** (以 QQ 为例):
```
crates/easybot-adapter-qq/src/
├── lib.rs              # ~200 行: struct + PlatformAdapter impl 骨架
├── types.rs            # 已存在 — 保持不变
├── gateway.rs          # WebSocket 网关事件循环
├── api.rs              # HTTP API 调用 (send/edit/delete/interactive)
├── message.rs          # 消息格式转换 (QQ ↔ InboundMessage)
├── auth.rs             # Token 管理 (QqTokenStore 从 lib.rs 移出)
└── tests/              # (可选) 集成测试
```

WeChat 同理拆分。

**策略**: 逐模块提取，不改变逻辑，确保每次提取后测试通过再继续。

**验收标准**: 所有 QQ/WeChat 测试通过，每个子模块 <800 行。

---

## Round 2 · P3 低优先级

> 长期改进 + 锦上添花

### ⬜ P3-1. 适配器 Capability 声明去重

| 属性 | 内容 |
|------|------|
| **文件** | 5 个适配器 `lib.rs` 各自的 `new()` 函数 |
| **审计 ID** | N18 |
| **严重性** | Low — 代码重复 |
| **工时** | 2h |

**问题**: 每个适配器在 `new()` 中重复构建相同的 `Vec<Capability>` 列表。

**修复方案** — 添加宏:
```rust
// crates/easybot-core/src/types/adapter.rs

macro_rules! capabilities {
    ($(($name:ident, $supported:expr $(, limits: $limits:expr)?)),* $(,)?) => {
        vec![
            $(
                Capability {
                    name: CapabilityName::$name,
                    supported: $supported,
                    limits: $({
                        let l: Option<serde_json::Value> = $limits;
                        l
                    })?,
                },
            )*
        ]
    };
}
```

使用:
```rust
let capabilities = capabilities![
    (Text, true),
    (Image, true, limits: Some(json!({"max_size_mb": 50}))),
    (Audio, false),
    // ...
];
```

**验收标准**: 编译通过，适配器功能不变。

---

### ⬜ P3-2. bin/main.rs 适配器注册宏统一

| 属性 | 内容 |
|------|------|
| **文件** | `bin/src/main.rs` 第 537-687 行 |
| **审计 ID** | N19 |
| **严重性** | Low — 代码重复 |
| **工时** | 3h |

**问题**: 6 个 feature-gated 适配器注册块几乎完全相同（仅平台名、feature flag、env var 名称不同）。

**修复方案** — 提取为 `register_adapter!` 宏:
```rust
macro_rules! register_adapter {
    ($registry:expr, $feature:literal, $cfg:ident, $module:ident, $platform:literal, $env_vars:expr) => {
        #[cfg(feature = $feature)]
        {
            let adapter = $module::$cfg::new();
            adapter.set_event_bus($registry.event_bus());
            match adapter.init(config.clone()).await {
                Ok(()) => $registry.register(
                    $platform.to_string(),
                    Arc::new(move |_| Box::new(adapter)),
                ),
                Err(e) => tracing::warn!("{} adapter init failed: {}", $platform, e),
            }
        }
    };
}
```

**验收标准**: 编译通过，适配器注册行为不变。

---

### ⬜ P3-3. 完善 Plugin 沙箱文档

| 属性 | 内容 |
|------|------|
| **文件** | `SECURITY.md` + `crates/easybot-core/src/plugin/loader.rs` 顶部文档 |
| **审计 ID** | N20 |
| **严重性** | Low — 文档 |
| **工时** | 1h |

**问题**: 插件系统无沙箱（原生 .so 可执行任意代码），虽已在 loader.rs 注释中简短提及，但未在面向用户的 SECURITY.md 中明确说明。

**修复方案**: 在 `SECURITY.md` 中新增 "Plugin Security" 章节:
```markdown
## Plugin Security

### Native Code Execution

⚠️ **插件以原生共享库 (.so/.dylib/.dll) 形式在 EasyBot 进程中运行。**

- 插件无沙箱隔离，拥有完整进程内存访问权限
- 恶意插件可读取所有适配器凭据、修改消息、访问数据库
- **仅加载来自可信来源的插件**
- 考虑对发布版插件实施代码签名验证（未来增强）

### Best Practices

1. 在隔离环境中测试第三方插件
2. 监控插件 CPU/内存使用
3. 使用 ABI 版本检查防止不兼容插件加载
```

同时在 `loader.rs` 顶部模块文档中补充:
```rust
//! # Security Note
//!
//! Plugins are loaded as native shared libraries with full process access.
//! See `SECURITY.md` for security guidelines.
```

**验收标准**: SECURITY.md 包含 Plugin Security 章节。

---

## 附录 A · Round 2 修复检查清单 (按文件)

| 文件 | P0 | P1 | P2 | P3 |
|------|:--:|:--:|:--:|:--:|
| `crates/easybot-core/src/auth/permissions.rs` (新建) | P0-1 | — | — | — |
| `crates/easybot-api/src/server.rs` | P0-1, P0-5 | P1-2, P1-6 | — | — |
| `crates/easybot-adapter-feishu/src/lib.rs` | P0-2 | P1-8 | P2-2 | — |
| `crates/easybot-api/src/routes/messages.rs` | P0-3 | — | — | — |
| `crates/easybot-core/src/storage/sqlite.rs` | P0-4 | — | — | — |
| `crates/easybot-core/src/storage/postgres.rs` | P0-4 | — | — | — |
| 14 个 `Cargo.toml` | — | P1-1 | — | — |
| `crates/easybot-api/src/routes/ws.rs` | — | P1-3 | — | — |
| `crates/easybot-adapter-qq/src/lib.rs` | — | P1-4 | P2-4 | — |
| `crates/easybot-core/src/session/manager.rs` | — | P1-5 | — | — |
| `crates/easybot-api/src/middleware/rate_limit.rs` | — | P1-7 | — | — |
| `crates/easybot-adapter-feishu/Cargo.toml` | — | P1-8 | — | — |
| `crates/easybot-core/src/webhook/mod.rs` | — | — | P2-1 | — |
| `crates/easybot-adapter-wechat/src/lib.rs` | — | — | P2-3, P2-4 | — |
| `crates/easybot-core/src/types/adapter.rs` | — | — | — | P3-1 |
| `bin/src/main.rs` | — | — | — | P3-2 |
| `SECURITY.md` | — | — | — | P3-3 |
| `crates/easybot-core/src/plugin/loader.rs` | — | — | — | P3-3 |

---

## 附录 B · Round 2 审计原始发现

以下为 Round 2 新发现清单（20 项），与 Round 1 无重叠。

### 代码质量 (N1–N7)
| ID | 严重性 | 简述 | 文件:行号 |
|----|--------|------|-----------|
| N1 | Critical | Feishu `config.as_ref().unwrap()` panic 风险 | `feishu/src/lib.rs:466` |
| N2 | Critical | `Arc::try_unwrap().unwrap()` panic 风险 | `api/src/routes/messages.rs:283` |
| N3 | High | Workspace lint 未继承，unsafe_code=deny 无效 | 14 个 Cargo.toml |
| N4 | High | QQ `std::sync::Mutex` 在 async 上下文 | `qq/src/lib.rs:11` |
| N5 | High | SessionManager store 错误静默吞没 | `core/src/session/manager.rs:72,92,110,173` |
| N6 | Medium | webhook serialize 失败无日志 | `core/src/webhook/mod.rs:171` |
| N7 | Medium | QQ/WeChat 适配器文件过大 (2200+ 行) | `qq/src/lib.rs`, `wechat/src/lib.rs` |

### 安全 (N8–N15)
| ID | 严重性 | 简述 | 文件:行号 |
|----|--------|------|-----------|
| N8 | Critical | 无 API 权限检查，任何 key 可执行任何操作 | `api/src/server.rs` |
| N9 | Critical | AssertSqlSafe + format! SQL 拼接绕过编译检查 | `storage/sqlite.rs`, `postgres.rs` |
| N10 | Critical | CorsLayer::permissive() 任意源可达 | `api/src/server.rs:244` |
| N11 | High | 无 HTTP 请求体大小限制 | `api/src/server.rs` |
| N12 | High | 无 WebSocket 消息帧大小限制 | `api/src/routes/ws.rs` |
| N13 | Medium | /metrics 端点无认证 | `api/src/server.rs:137-141` |
| N14 | Medium | X-Forwarded-For 信任链错误 (取首非尾) | `api/src/middleware/rate_limit.rs:176` |
| N15 | Medium | Feishu SDK v0.1 极早期版本 | `feishu/Cargo.toml:16` |

### 架构/一致性 (N16–N20)
| ID | 严重性 | 简述 | 文件 |
|----|--------|------|------|
| N16 | Medium | HTTP client 类型不一致 (Option vs OnceLock) | feishu, qq |
| N17 | Medium | WeChat 构造函数不一致 (new_with_event_bus) | wechat, bin |
| N18 | Low | 5 个适配器 Capability 声明重复 | 5 个 adapters |
| N19 | Low | 6 个 feature-gated 注册块几乎相同 | bin/main.rs:537-687 |
| N20 | Low | 插件沙箱风险未在 SECURITY.md 中说明 | SECURITY.md |

---

## Round 1 (已完成 · 2026-06-24 早前)

<details>
<summary>30 项修复 · ✅ 全部完成 · 点击展开</summary>

### ✅ P0-1. Dev API Key 明文写入日志 🔴 ✅

| 属性 | 内容 |
|------|------|
| **文件** | `bin/src/main.rs` 第 236 行 |
| **严重性** | 严重 — 凭据泄露 |
| **工时** | 5 分钟 |
| **状态** | ✅ 已完成 (commit 39d5d79) |

**问题**: `--debug` 模式下创建的 dev API key（权限 `["*"]`）完整写入 tracing 日志，Docker logs / journald / CI 输出均可被读取。

**当前代码**:
```rust
Ok((id, key)) => tracing::info!("Dev API Key created: id={}, key={}", id, key),
```

**修复方案**: 仅日志 key ID 或 key 前 8 位前缀:
```rust
Ok((id, key)) => tracing::info!(
    "Dev API Key created: id={}, key_prefix={}...",
    id,
    &key[..key.len().min(8)]
),
```

**验收标准**: 启动 `cargo run -- --debug`，日志中不再出现完整 key。

---

### ✅ P0-2. WebSocket 连接数限制未生效 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/routes/ws.rs` (handler 函数) + `crates/easybot-core/src/types/config.rs` (WebSocketConfig) |
| **严重性** | 严重 — 无限制 DoS |
| **工时** | 1h |

**问题**: `WebSocketConfig.max_clients` 字段已定义但从未被读取。无任何代码限制 WebSocket 并发连接数。

**修复方案**: 在 `ws_handler` 中引入 `tokio::sync::Semaphore`:
```rust
// AppState 中新增字段
ws_semaphore: Arc<Semaphore>,

// 在 ws_handler 中，upgrade 之前:
let permit = state.ws_semaphore
    .clone()
    .try_acquire_owned()
    .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "too many WebSocket connections"))?;
// permit 随连接生命周期持有，drop 时自动释放
```

**验收标准**: 达到 `max_clients` 后新连接返回 503。

---

### ✅ P0-3. 数据库存储路径穿越 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `bin/src/main.rs` 第 116-120 行 |
| **严重性** | 严重 — 任意文件系统写入 |
| **工时** | 30 分钟 |

**问题**: `config.storage.path` 来自 YAML 配置文件，直接用于 SQLite 数据库路径，未校验 `..` 或绝对路径。

**修复方案**: 添加路径校验:
```rust
let db_path = if !config.storage.path.is_empty() {
    let p = std::path::PathBuf::from(&config.storage.path);
    // 拒绝含 .. 的路径
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        anyhow::bail!("storage.path 不得包含 '..' 组件");
    }
    // 相对路径解析到 home dir 下
    if p.is_relative() {
        paths.home.join(p)
    } else {
        p  // 绝对路径需显式确认
    }
} else {
    paths.db_path.clone()
};
```

**验收标准**: 配置 `storage.path: "../../etc/passwd"` 启动报错退出。

---

### ✅ P0-4. 插件库路径穿越 → 任意代码执行 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-core/src/plugin/manifest.rs:45-47` + `loader.rs:242` |
| **严重性** | 严重 — 任意代码执行 |
| **工时** | 30 分钟 |

**问题**: `plugin.yaml` 的 `library` 字段与插件目录 `join` 后直接传给 `libloading::Library::new()`，恶意 YAML 可加载系统共享库。

**修复方案**: 解析后 canonicalize 并验证在插件目录内:
```rust
pub fn library_path(&self, plugin_dir: &Path) -> Result<PathBuf, GatewayError> {
    let lib = self.library.as_deref().unwrap_or("libplugin.so");
    let lib_path = plugin_dir.join(lib);
    // 规范化并验证不逃逸
    let canonical = lib_path.canonicalize().map_err(|e| {
        GatewayError::config(format!("插件库路径无效: {}: {}", lib_path.display(), e))
    })?;
    let canonical_dir = plugin_dir.canonicalize().map_err(|e| {
        GatewayError::config(format!("插件目录无效: {}: {}", plugin_dir.display(), e))
    })?;
    if !canonical.starts_with(&canonical_dir) {
        return Err(GatewayError::config(format!(
            "插件库路径逃逸: {}", lib_path.display()
        )));
    }
    Ok(canonical)
}
```

**验收标准**: 创建含 `library: ../../../usr/lib/libc.so.6` 的 plugin.yaml，加载报错。

---

### ✅ P0-5. PostgreSQL 连接字符串明文日志 🔴

| 属性 | 内容 |
|------|------|
| **文件** | `bin/src/main.rs` 第 143 行 |
| **严重性** | 高危 — 数据库凭据泄露 |
| **工时** | 15 分钟 |

**问题**: `tracing::info!("PostgreSQL storage initialized: {}", conn_str)` 可能包含密码。

**修复方案**: 日志前脱敏:
```rust
let safe_conn = redact_password_in_conn_str(&conn_str);
tracing::info!("PostgreSQL storage initialized: {}", safe_conn);

fn redact_password_in_conn_str(s: &str) -> String {
    // 匹配 postgresql://user:password@host/db 格式，替换密码为 ***
    // 或更简单: 仅日志 host + db 部分
    s.split('@').last().unwrap_or(s).to_string()
}
```

**验收标准**: 日志中的 PG 连接信息不含密码。

---

## P1 · 高优先级 (本迭代)

> 功能性缺陷 + 可靠性风险 + 安全加固

### ✅ P1-1. Rate Limiter IP 映射永不释放导致内存泄漏

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/middleware/rate_limit.rs` |
| **严重性** | 高危 |
| **工时** | 2h |

**问题**: `DashMap<String, Arc<RwLock<SlidingWindow>>>` 中 IP 条目插入后永不删除，每个新 IP 永久消耗内存。

**修复方案**: 添加后台清理任务，每 5 分钟扫描并删除超过 5 分钟未活跃的条目:
```rust
// 在 RateLimiter 中新增方法
pub fn start_cleanup(self: &Arc<Self>) {
    let this = Arc::downgrade(self);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(300)).await;
            let Some(me) = this.upgrade() else { break };
            me.windows.retain(|_, w| {
                w.try_read().map(|w| w.last_access.elapsed() < Duration::from_secs(300))
                    .unwrap_or(false)
            });
        }
    });
}
```

**验收标准**: 大量不同 IP 请求后，内存不持续增长。

---

### ✅ P1-2. SessionManager::get_or_create 竞态条件

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-core/src/session/manager.rs` 第 61-94 行 |
| **严重性** | 高危 |
| **工时** | 1h |

**问题**: check-then-act 模式（先 `get` 再 `insert`）导致并发创建时返回过期 session。

**修复方案**: 用 `DashMap::entry()` 原子化:
```rust
pub fn get_or_create(&self, key: &str, source: SessionSource) -> Session {
    let entry = self.sessions.entry(key.to_string());
    let session = entry.or_insert_with(|| Session::new(key, source));
    // 更新 updated_at（通过 clone + replace）
    let mut s = session.clone();
    s.updated_at = Utc::now();
    entry.replace_entry(s.clone());
    s
}
```

**验收标准**: 现有并发测试 `test_concurrent_session_access` 依然通过。

---

### ✅ P1-3. 应用层 TLS 实现或强制反向代理声明

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/server.rs` 第 80-89 行 |
| **严重性** | 高危 |
| **工时** | 4h |

**问题**: TLS 配置存在但未接入应用层，注释称"由反向代理处理"，但项目可独立运行。

**修复方案 (推荐)**: 非 debug 模式下若无 TLS 配置则拒绝启动:
```rust
if !self.config.tls.enabled && !cfg!(debug_assertions) {
    tracing::warn!(
        "TLS未启用！生产环境请启用 TLS 或使用反向代理。\n\
         设置 tls.enabled=true 或设置 EASYBOT_ALLOW_PLAINTEXT=true 确认风险"
    );
    if std::env::var("EASYBOT_ALLOW_PLAINTEXT").is_err() {
        anyhow::bail!("生产环境必须启用 TLS，或设置 EASYBOT_ALLOW_PLAINTEXT=true");
    }
}
```

**长期方案**: 接入 `axum_server::tls_rustls::TlsRustlsServer`。

**验收标准**: 非 debug 模式无 TLS 配置时启动报错。

---

### ✅ P1-4. 配置热重载端点缺少输入验证

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/routes/config.rs` 第 32-72 行 |
| **严重性** | 高危 |
| **工时** | 2h |

**问题**: `PUT /api/v1/config` 接受任意 JSON merge，API key 泄露后可修改存储路径/关闭限流。

**修复方案**: 合并后对关键字段做安全校验:
```rust
// merge 之后，apply 之前
if let Some(path) = new_config.storage.path.as_ref() {
    if Path::new(path).components().any(|c| c == Component::ParentDir) {
        return Err(ApiError(GatewayError::validation("storage.path 包含非法组件")));
    }
}
// 禁止在生产环境通过 API 关闭限流
if let Some(false) = new_config.rate_limit.enabled {
    tracing::warn!("尝试通过 API 关闭限流，已拒绝");
    return Err(ApiError(GatewayError::forbidden("不可通过 API 关闭限流")));
}
```

**验收标准**: 通过 API 提交路径穿越配置返回 400。

---

### ✅ P1-5. batch-send 无最大目标数限制

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/routes/messages.rs` 第 91-155 行 |
| **严重性** | 中高危 — DoS |
| **工时** | 30 分钟 |

**问题**: 无上限的 targets 可耗尽 tokio 任务预算。

**修复方案**: 添加硬限制:
```rust
const MAX_BATCH_TARGETS: usize = 100;

if request.targets.len() > MAX_BATCH_TARGETS {
    return Err(ApiError(GatewayError::validation(format!(
        "最多支持 {} 个目标，当前 {} 个", MAX_BATCH_TARGETS, request.targets.len()
    ))));
}
```

**验收标准**: 发送 101 个 targets 返回 400。

---

### ✅ P1-6. Docker 容器以 root 运行

| 属性 | 内容 |
|------|------|
| **文件** | `Dockerfile` |
| **严重性** | 中危 |
| **工时** | 1h |

**修复方案**:
```dockerfile
RUN useradd -r -m -s /bin/bash easybot \
    && mkdir -p /var/lib/easybot/data /var/lib/easybot/logs /etc/easybot/plugins \
    && chown -R easybot:easybot /var/lib/easybot /etc/easybot
USER easybot
EXPOSE 8080
```

**验收标准**: `docker exec ... whoami` 输出 `easybot`。

---

### ✅ P1-7. E2E Mock 断言从 `expect(0..)` 改为 `expect(1)`

| 属性 | 内容 |
|------|------|
| **文件** | `tests/e2e/tests/*.rs` (5 个文件) |
| **严重性** | 中危 — 测试可靠性 |
| **工时** | 2h |

**问题**: 所有发送路径 mock 用 `expect(0..)`，若代码修改导致 API 未被调用，测试静默通过。

**修复方案**: 逐文件将发送路径 mock 改为 `expect(1)`:
```rust
// 改前
mock_server.register(Mock::given(method("POST"))
    .and(path("/sendMessage"))
    .respond_with(ResponseTemplate::new(200))
    .expect(0..))
// 改后
mock_server.register(Mock::given(method("POST"))
    .and(path("/sendMessage"))
    .respond_with(ResponseTemplate::new(200))
    .expect(1))
```

**验收标准**: 注释掉适配器 `send()` 调用，E2E 测试失败。

---

### ✅ P1-8. Feishu E2E 认证失败测试断言修复

| 属性 | 内容 |
|------|------|
| **文件** | `tests/e2e/tests/feishu.rs` (`test_e2e_feishu_auth_failure`) |
| **严重性** | 中危 |
| **工时** | 30 分钟 |

**问题**: 认证失败时仅打印信息，不 `assert!`:
```rust
// 当前: 即使 connected=true 也静默通过
if status.is_success() || status.is_server_error() { /* ... */ }
```

**修复方案**:
```rust
// 认证失败应断言 adapter 未连接
let status = adapter.get_status().await;
assert!(!status.connected, "auth 失败时 adapter 不应连接: {:?}", status);
```

**验收标准**: 提供无效 token 时测试明确失败。

---

## P2 · 中优先级 (下个迭代)

> 可观测性 + 可靠性 + 测试补齐

### ✅ P2-1. Prometheus Metrics 仪器化

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/metrics.rs` + `server.rs` + 各适配器 |
| **严重性** | 中 |
| **工时** | 6h |
| **状态** | ✅ 已完成 |

**问题**: 6 个注册的指标中 5 个从未被填充（`http_requests_total`, `http_request_duration_seconds`, `messages_inbound_total`, `messages_outbound_total`, `adapter_status`）。

**修复方案**:
1. 在路由层添加 Tower 中间件，计数 HTTP 请求并观测耗时
2. 在各适配器 `send()`/`send_media()` 成功/失败时增减对应 counter
3. 在 adapter connect/disconnect 事件中更新 `adapter_status` gauge

**验收标准**: `GET /metrics` 返回非零值的指标。

---

### P2-2. EventBus `subscribe_many` 忙轮询优化

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-core/src/bus/event_bus.rs` 第 80-122 行 |
| **严重性** | 中 |
| **工时** | 3h |

**问题**: `try_recv()` + 10ms sleep 忙轮询消耗 CPU。

**修复方案**: 用 `tokio::select!` 替代:
```rust
pub async fn subscribe_many(&self, event_types: &[&str]) -> Vec<GatewayEvent> {
    let mut receivers: Vec<_> = event_types.iter().filter_map(|t| {
        Some((*t, self.channels.get(t)?.subscribe()))
    }).collect();
    // 等待第一个事件到达
    loop {
        let mut futures: Vec<_> = receivers.iter_mut().map(|(et, rx)| {
            Box::pin(async { (*et, rx.recv().await) })
        }).collect();
        tokio::select! {
            result = futures::future::select_all(futures) => {
                // 处理到达的事件，继续循环
                return vec![result.0.1.unwrap_or_default()];
            }
        }
    }
}
```

**验收标准**: 空闲时 CPU 使用率降低。

---

### P2-3. GatewayError 内部错误信息脱敏

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-core/src/types/error.rs` |
| **严重性** | 中 |
| **工时** | 2h |

**问题**: `to_api_error()` 将 `self.to_string()`（含文件路径/SQL 错误）直接返回客户端。

**修复方案**: 区分内部/外部消息:
```rust
impl GatewayError {
    pub fn external_message(&self) -> &str {
        match self {
            Self::Internal(_) => "内部服务器错误",
            Self::StorageError(_) => "存储错误",
            Self::ConfigError(_) => "配置错误",
            // ... 其余保留原消息
            _ => &self.to_string(),
        }
    }
}
```

**验收标准**: API 错误响应中不含实体文件路径。

---

### P2-4. Rate Limiting 应用到 Public Routes

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-api/src/server.rs` 第 113-125 行 |
| **严重性** | 中 |
| **工时** | 1h |

**问题**: health/metrics/swagger 端点无速率限制。

**修复方案**: 为 `public_routes` 添加独立的 RateLimitLayer（更宽松的限制，如 60 req/min）:
```rust
let public_routes = Router::new()
    .merge(health_routes)
    .merge(metrics_routes)
    .layer(RateLimitLayer::new(rate_limiter.clone()).with_limit(60));
```

**验收标准**: 快速请求 `/health` 超过限制后返回 429。

---

### P2-5. MessagePersister 批处理 + 重试

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-core/src/session/message_persister.rs` |
| **严重性** | 中 |
| **工时** | 3h |

**问题**: 消息逐条写入，失败静默丢失，无重试。

**修复方案**: 引入缓冲批量写入:
```rust
struct MessagePersister {
    buffer: Arc<Mutex<Vec<InboundMessage>>>,
    flush_interval: Duration,  // 1s
    batch_size: usize,          // 50
    max_retries: u32,           // 3
}

impl MessagePersister {
    async fn flush_loop(&self, storage: Arc<dyn MessageStore>) {
        loop {
            tokio::time::sleep(self.flush_interval).await;
            let batch = {
                let mut buf = self.buffer.lock().await;
                std::mem::take(&mut *buf)
            };
            if batch.is_empty() { continue; }
            for attempt in 1..=self.max_retries {
                match storage.store_messages_batch(&batch).await {
                    Ok(_) => break,
                    Err(e) if attempt == self.max_retries => {
                        tracing::error!(?e, count=batch.len(),
                            "消息持久化失败，已丢弃 {} 条", batch.len());
                    }
                    Err(e) => {
                        tracing::warn!(?e, attempt, "消息持久化失败，重试");
                        tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                    }
                }
            }
        }
    }
}
```

**验收标准**: 数据库短暂不可用后消息恢复写入。

---

### P2-6. CHANGELOG.md 创建

| 属性 | 内容 |
|------|------|
| **文件** | `CHANGELOG.md` (新建) |
| **严重性** | 中 (release workflow 依赖) |
| **工时** | 1h |

**问题**: `.github/workflows/release.yml` 引用 `body_path: CHANGELOG.md` 但文件不存在。

**修复方案**: 基于 git 历史创建，采用 [Keep a Changelog](https://keepachangelog.com/) 格式。

**验收标准**: release workflow 正常运行。

---

### P2-7. CONTRIBUTING.md 创建

| 属性 | 内容 |
|------|------|
| **文件** | `CONTRIBUTING.md` (新建) |
| **严重性** | 中 |
| **工时** | 1h |

**内容要点**: 开发环境搭建、代码风格（fmt + clippy）、PR 流程、commit 约定、添加新适配器指南、测试运行方法。

---

### P2-8. SECURITY.md 创建

| 属性 | 内容 |
|------|------|
| **文件** | `SECURITY.md` (新建) |
| **严重性** | 中 |
| **工时** | 30 分钟 |

**内容要点**: 漏洞报告渠道、安全期望、支持的版本。

---

### P2-9. QQ 适配器 Mock 测试补充

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-qq/tests/send_mock.rs` |
| **严重性** | 中 |
| **工时** | 3h |

**缺失测试** (`send_media` 成功/失败, `send_interactive`, `edit_message`, `delete_message`, `rate_limit/429`, request body 验证):
- [ ] `test_send_media_image_success`
- [ ] `test_send_media_error`
- [ ] `test_send_interactive_success`
- [ ] `test_edit_message_success`
- [ ] `test_delete_message_success`
- [ ] `test_send_media_request_body`

**参考**: Telegram 的 `send_mock.rs` (29 tests) 是最佳范例。

---

### P2-10. Feishu send_media Mock 测试补充

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-feishu/tests/send_mock.rs` |
| **严重性** | 中 |
| **工时** | 2h |

**缺失测试**:
- [ ] `test_send_media_image_success`
- [ ] `test_send_media_no_url_or_data_error`
- [ ] `test_send_media_request_body`

---

## P3 · 低优先级 (积压)

> 长期改进 + 锦上添花

### ✅ P3-1. Cargo.toml 元数据补全

| 属性 | 内容 |
|------|------|
| **文件** | 根 `Cargo.toml` + 各子 crate `Cargo.toml` |
| **工时** | 1h |

```toml
[workspace.package]
description = "IM Gateway — 统一多平台即时通讯网关"
repository = "https://github.com/wangzhizhou/EasyBot"
documentation = "https://github.com/wangzhizhou/EasyBot"
readme = "README.md"
```

---

### ✅ P3-2. README 仓库 URL 修正

| 属性 | 内容 |
|------|------|
| **文件** | `README.md` |
| **工时** | 5 分钟 |

替换 `git clone https://github.com/your-org/easybot.git` 为真实仓库地址。

---

### ✅ P3-3. tungstenite 版本统一

| 属性 | 内容 |
|------|------|
| **文件** | `crates/easybot-adapter-qq/Cargo.toml` |
| **工时** | 1h |

QQ 适配器 `tokio-tungstenite` 从 0.26 升级到 0.29，消除重复版本。

---

### ✅ P3-4. CI 添加 Windows / macOS 构建验证

| 属性 | 内容 |
|------|------|
| **文件** | `.github/workflows/ci.yml` |
| **工时** | 2h |

在 `test-feature-matrix` 中加入 `windows-latest` 和 `macos-latest` runner（仅 check + build，不跑全量测试）。

---

### ✅ P3-5. CI Feature Matrix 加入飞书/QQ/微信

| 属性 | 内容 |
|------|------|
| **文件** | `.github/workflows/ci.yml` |
| **工时** | 30 分钟 |

当前 feature matrix 仅验证 telegram 和 discord 的独立编译，需加入 feishu/qq/wechat。

---

### ✅ P3-6. 移除过期的 cargo-audit 豁免

| 属性 | 内容 |
|------|------|
| **文件** | `.cargo/audit.toml` |
| **工时** | 10 分钟 |

移除 `RUSTSEC-2023-0071`（`rsa` 已不在依赖树中）。

---

### ✅ P3-7. Gateway 配置默认文件扩展

| 属性 | 内容 |
|------|------|
| **文件** | `gateway.yaml` |
| **工时** | 1h |

添加注释掉的 rate_limiting / metrics / webhook / plugins / ttl / adapter-specific 配置段，帮助用户发现可用选项。

---

## 附录 A · 修复检查清单 (按文件)

以下按文件组织，方便逐文件推进：

| 文件 | P0 | P1 | P2 | P3 |
|------|:--:|:--:|:--:|:--:|
| `bin/src/main.rs` | P0-1, P0-3, P0-5 | — | — | — |
| `crates/easybot-api/src/routes/ws.rs` | P0-2 | — | — | — |
| `crates/easybot-core/src/plugin/manifest.rs` | P0-4 | — | — | — |
| `crates/easybot-api/src/middleware/rate_limit.rs` | — | P1-1 | — | — |
| `crates/easybot-core/src/session/manager.rs` | — | P1-2 | — | — |
| `crates/easybot-api/src/server.rs` | — | P1-3 | P2-4 | — |
| `crates/easybot-api/src/routes/config.rs` | — | P1-4 | — | — |
| `crates/easybot-api/src/routes/messages.rs` | — | P1-5 | — | — |
| `Dockerfile` | — | P1-6 | — | — |
| `tests/e2e/tests/*.rs` | — | P1-7 | — | — |
| `tests/e2e/tests/feishu.rs` | — | P1-8 | — | — |
| `crates/easybot-api/src/metrics.rs` | — | — | P2-1 | — |
| `crates/easybot-core/src/bus/event_bus.rs` | — | — | P2-2 | — |
| `crates/easybot-core/src/types/error.rs` | — | — | P2-3 | — |
| `crates/easybot-core/src/session/message_persister.rs` | — | — | P2-5 | — |
| `CHANGELOG.md` | — | — | P2-6 | — |
| `CONTRIBUTING.md` | — | — | P2-7 | — |
| `SECURITY.md` | — | — | P2-8 | — |
| `crates/easybot-adapter-qq/tests/send_mock.rs` | — | — | P2-9 | — |
| `crates/easybot-adapter-feishu/tests/send_mock.rs` | — | — | P2-10 | — |
| 各 `Cargo.toml` | — | — | — | P3-1 |
| `README.md` | — | — | — | P3-2 |
| `crates/easybot-adapter-qq/Cargo.toml` | — | — | — | P3-3 |
| `.github/workflows/ci.yml` | — | — | — | P3-4, P3-5 |
| `.cargo/audit.toml` | — | — | — | P3-6 |
| `gateway.yaml` | — | — | — | P3-7 |

---

## 附录 B · 审计原始数据

以下为审计发现的完整清单，供交叉参考。

### 代码质量 (8.0/10)
| ID | 严重性 | 简述 |
|----|--------|------|
| C1 | Critical | Discord/QQ 适配器 WebSocket 连接前提前设置 Connected |
| C2 | Critical | AdapterManager 多锁 TOCTOU |
| H1 | High | AdapterState 热路径不必要的 clone |
| H2 | High | edit/delete 失败返回 Ok 而非 Err |
| H3 | High | set_event_bus 固有方法绕过 trait |
| H5 | High | SessionManager 双重 hash 查找 |
| M1 | Medium | EventBus 忙轮询 |
| M2 | Medium | QQ std::Mutex 在 async 上下文 |
| M3 | Medium | 事件类型 stringly-typed |
| M4 | Medium | 时间戳类型不一致 i64 vs String |
| L1-L6 | Low | 行内注释、魔数等 |

### 安全 (5.5/10)
| ID | 严重性 | 简述 |
|----|--------|------|
| CRIT-1 | Critical | Dev Key 日志泄露 |
| CRIT-2 | Critical | WS 连接限制未生效 |
| HIGH-1-5 | High | 路径穿越 x2, PG 日志, 缺 TLS, 配置热重载无验证 |
| MED-1-8 | Medium | 错误暴露, 无请求体限制, root 容器等 |
| LOW-1-10 | Low | CORS, metrics 无认证, 无安全头等 |

### 测试 (6.5/10)
| ID | 严重性 | 简述 |
|----|--------|------|
| 3.1 | Critical | 无入站消息 E2E 流程 |
| 3.4 | Critical | QQ mock 测试过少 |
| 3.5 | Critical | Feishu auth 断言无效 |
| 3.6 | High | expect(0..) 全部 mock |
| 3.9 | High | CI feature matrix 缺飞书/QQ/微信 |
| 3.11-3.18 | Med-Low | 各项测试缺口 |

### 性能/可靠性 (7.0/10)
| ID | 严重性 | 简述 |
|----|--------|------|
| 1.1 | Critical | Rate limiter 内存泄漏 |
| 1.2 | Critical | SessionManager 竞态 |
| 1.3 | Critical | Metrics 从未填充 |
| 1.4 | Critical | MessagePersister 无批处理/重试 |
| 1.5 | High | subscribe_many 轮询 |
| 2.x | Med | 重连轮询, 无 TCP keepalive 等 |

### 文档 (7.5/10)
| ID | 简述 |
|----|------|
| D1 | CHANGELOG.md 不存在（release 会失败） |
| D2 | CONTRIBUTING.md 不存在 |
| D3 | SECURITY.md 不存在 |
| D4 | Cargo.toml 元数据不完整 |
| D5 | README clone URL 占位符 |
| D6 | CLAUDE.md DeliveryRouter 标记 TBD |

### 依赖 (8.0/10)
| ID | 简述 |
|----|------|
| DEP1 | 移除过期 RUSTSEC-2023-0071 豁免 |
| DEP2 | tungstenite 版本分裂 (QQ 0.26 vs axum 0.29) |
| DEP3 | 考虑添加 cargo-deny check bans 到 CI |

</details>

---

> 最后更新: 2026-06-24 · Round 2 修复计划 (20 项新发现)
> Round 1 历史: 30 项 · ✅ 全部完成
