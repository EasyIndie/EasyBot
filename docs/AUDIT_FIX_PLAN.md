# EasyBot 审计修复计划

> 基于 2026-06-24 全面审计报告 | 6 个维度 · 60+ 发现 → 30 项可执行修复
>
> 关联文档: [TODO.md](TODO.md) · [审计报告](AUDIT_FIX_PLAN.md#附录-审计原始数据)

---

## 进度总览

| 优先级 | 数量 | 状态 | 预计总工时 |
|--------|:----:|------|:----------:|
| **P0 紧急** | 5 | ✅ 已完成 | ~2h |
| **P1 高优先级** | 8 | ✅ 已完成 | ~12h |
| **P2 中优先级** | 10 | ✅ 已完成 | ~20h |
| **P3 低优先级** | 7 | ✅ 已完成 | ~16h |
| **合计** | **30** | **✅ 全部完成** | **~50h** |

---

## P0 · 紧急修复 (本周必须完成)

> 严重安全漏洞 + 数据安全风险，修复成本极低

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

---

> 最后更新: 2026-06-24 · 基于 2026-06-24 全面审计
