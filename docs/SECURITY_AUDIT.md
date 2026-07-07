# EasyBot 安全审计报告 & 修复手册

> **日期:** 2026-07-06 | **修复数:** 48 项 | **上游阻塞:** 3 项

本文档记录了对 EasyBot 全量代码的安全审计结果、修复方案及通用安全实践，供团队后续开发和审计参考。

---

## 目录

- [上游阻塞问题（3 项）](#上游阻塞问题)
- [修复方法论](#修复方法论)
  - [认证凭据类](#认证凭据类)
  - [暴力破解防护类](#暴力破解防护类)
  - [注入与验证类](#注入与验证类)
  - [速率限制类](#速率限制类)
  - [日志安全类](#日志安全类)
  - [错误处理类](#错误处理类)
  - [基础设施类](#基础设施类)
- [通用原则总结](#通用原则总结)

---

## 上游阻塞问题

以下 3 项需要上游 crate 作者配合修改，EasyBot 自身代码无法解决。每次依赖更新时检查进展。

### 1. ring 0.17.x 维护模式

- **影响范围:** `Cargo.lock` 中 `ring` 0.17.14
- **风险:** `ring` 0.17.x 系列已进入维护模式，未来安全漏洞可能无法及时修复。Rust TLS 生态正迁移到 `aws-lc-rs`
- **阻塞原因:** `rustls`、`reqwest`、`tokio-tungstenite` 等核心依赖尚未完成迁移
- **跟进方式:**
  - 关注 [rustls aws-lc-rs migration](https://github.com/rustls/rustls) 进度
  - 当 rustls 默认使用 aws-lc-rs 时，`ring` 将作为间接依赖自动消失

### 2. larksuite-oapi-sdk-rs 锁定旧版 tokio-tungstenite 0.26.2

- **影响范围:** `Cargo.lock` 中 `tokio-tungstenite` 0.26.2（飞书适配器 SDK 传递依赖）
- **风险:** `rustls-native-certs` 证书验证不一致；导致多版本共存
- **阻塞原因:** `larksuite-oapi-sdk-rs` 0.1.2 固定依赖老版本，无法在 workspace 级别覆盖
- **跟进方式:**
  - 向 [larksuite/oapi-sdk-rust](https://github.com/larksuite/oapi-sdk-rust) 提交 PR 升级 tungstenite 到 0.29.x
  - 短期方案：vendor 一份 patched 版本
  - `deny.toml` 已配置 `skip-tree` 临时放行

### 3. 重复安全 crate 版本

- **影响范围:** `sha2`（0.10.9 + 0.11.0）、`tungstenite`（0.26.2 + 0.29.0）、`rand`（3 个版本）、`webpki-roots`（0.26.11 + 1.0.8）、`digest`/`crypto-common`/`block-buffer`（各 2 个版本）
- **风险:** 每个重复版本扩大攻击面，增加二进制体积
- **阻塞原因:** 受飞书 SDK（问题 #2）阻塞
- **跟进方式:** 解决 #2 后大部分重复会自动消除；其余可通过 `cargo tree -i <crate>` 追踪并提 PR 给上游

---

## 修复方法论

### 认证凭据类

#### 默认密码移除

- **受影响代码:** `default_admin_password()` 返回 `"easybot"`
- **修复:** 返回空字符串 `""`，空密码时登录始终被拒绝；启动时打印配置指引
- **适用模式:** 永远不要设默认密码。生产环境强制显式配置，未配置时安全失败（fail-secure）

#### 恒定时间字符串比对

- **受影响代码:** `body.password != state.admin_password` 存在时序侧信道
- **修复:** 手写 `constant_time_eq(a: &[u8], b: &[u8]) -> bool`，用 XOR 累积差异位
- **适用模式:** 所有密码/令牌比对必须用恒定时间算法。也可以使用 Argon2 哈希比对（已有基础设施）

#### 凭据脱敏序列化

- **受影响代码:** `AdapterConfig` 的 `Debug` derive 会直接打印 `token`/`api_key`
- **修复:** 手写 `impl std::fmt::Debug for AdapterConfig`，将凭据字段替换为 `"***REDACTED***"`
- **适用模式:** 任何包含凭据的结构体必须手动实现 Debug，或使用 `secrecy::Secret<T>` 包装

#### 配置 API 凭据隐藏

- **受影响代码:** `GET /config` 返回完整的 `GatewayConfig`，含所有适配器 token
- **修复:** 在序列化后的 `serde_json::Value` 上执行 `sanitize_config_for_api()`，递归移除 `token`/`api_key`/`adminPassword`
- **适用模式:** 公开 API 返回的配置必须是"安全视图"；敏感字段在序列化层统一剥离

---

### 暴力破解防护类

#### 登录端点速率限制

- **受影响代码:** `/admin/login` 无任何速率限制
- **修复:** 添加专用 `RateLimiter`（5 次/分钟，突发 2），通过 `route_layer` 只作用于 admin 路由组
- **适用模式:** 所有认证端点必须有比常规 API 更严格的速率限制

#### WebSocket 认证爆破防护

- **受影响代码:** 单连接可无限次尝试认证（Argon2 验证 CPU 密集）
- **修复:**
  - 最大 5 次尝试，超限断开
  - 失败后 500ms 延迟（`tokio::time::sleep`）
  - 10 秒认证超时
  - 每连接帧速率限制 10 fps
- **适用模式:** 认证端点 = 次数上限 + 递增延迟 + 超时关闭。三者缺一不可

---

### 注入与验证类

#### SSRF 防护

- **受影响代码:** Webhook URL 和媒体下载 URL 无验证，可指向内部服务
- **修复:** `validate_url_for_ssrf()` 函数，拦截非 HTTP(S) scheme 和已知危险 host（`localhost`、`127.0.0.1`、`169.254.169.254` AWS 元数据、`metadata.google.internal` GCP 元数据）
- **调用点:** 配置加载时、API 修改时、媒体下载时
- **适用模式:** 所有用户可控的外部 URL 必须通过统一的 SSRF 验证函数

#### XSS 防护

- **受影响代码:** admin.js 中 `innerHTML = '...' + e.message` 将错误消息直接插入 DOM
- **修复:** 所有外部数据插入 DOM 前包裹 `escapeHtml()`；CSP 移除 `'unsafe-inline'`，添加 `frame-ancestors 'none'`
- **适用模式:** 任何用户/外部数据插入 DOM 前必须 escape；CSP 应尽可能收紧

#### API 输入验证

| 字段 | 约束 |
|------|------|
| 消息文本 | ≤ 16KB 字符 |
| metadata JSON | ≤ 64KB |
| API Key 名称 | ≤ 128 字符，仅 `[a-zA-Z0-9\-_ ]` |
| API Key 权限 | 白名单验证 |
| API Key 数量 | ≤ 100 |
| 会话 metadata | ≤ 4KB |

- **适用模式:** 所有外部输入必须有长度、字符集、数量三重约束

#### 会话 Key 分隔符注入

- **受影响代码:** `format!("{}:{}:{}", platform, chat_id, tid)` 若字段含冒号会破坏 key 结构
- **修复:** 各字段先 `replace(':', '_')` 再拼接
- **适用模式:** 任何用分隔符拼接的 key/标识符，必须先 sanitize 分隔符

---

### 速率限制类

#### 突发逻辑缺陷修复

- **受影响代码:** 允许突发时记录时间戳 → 攻击者可维持 `burst_size * 60` 额外请求/分钟
- **修复:** 突发通过时**不**记录时间戳
- **适用模式:** 突发是"容忍"不是"配额" — 不应消耗额度，也不应创造新额度

#### X-Forwarded-For 防伪造

- **受影响代码:** 无条件信任 XFF 头 → 攻击者可伪造 IP 绕过限流
- **修复:** 只信任来自已知代理 IP（`TRUSTED_PROXY_CIDRS`）的 XFF 头；非受信代理回退到 `ConnectInfo` 直连 IP
- **适用模式:** 限流 IP 提取 = 受信代理检查 → XFF 最右 IP；否则用直连 IP

#### 桶数量上限

- **受影响代码:** DashMap 无限增长 → IP 欺骗导致内存耗尽
- **修复:** `MAX_BUCKETS = 100_000`，超限时驱逐 LRU 条目
- **适用模式:** 任何按客户端/IP 分片的缓存必须设上限 + 淘汰策略

---

### 日志安全类

#### 敏感数据日志脱敏

- **受影响代码:** 多处 DEBUG/INFO 日志输出完整凭据（AES key hex、QR token、HTTP payload）
- **修复:**
  - 微信 AES key: 删除 `aes_key_hex` 和 `first_16_key` 日志字段
  - 微信 QR token: INFO → DEBUG
  - Discord send_media: 删除 `payload_text` 和 `content` 日志
  - WebSocket 客户端帧: 只记长度不记内容
- **适用模式:** DEBUG 级别也不应记录完整凭据；加密密钥一律不记；用户消息内容只用 `trace!`

#### TraceLayer Authorization 头部

- **受影响代码:** `TraceLayer::new_for_http()` 默认记录完整请求头
- **修复:** 简化为 `DefaultMakeSpan` + `DefaultOnRequest`
- **适用模式:** 生产环境 HTTP 日志只记 method + path + status，不记 header 值

#### 审计日志

- **受影响代码:** 零认证事件审计
- **修复:** 添加 `tracing::info!("AUDIT: ...")` 模式，覆盖管理员登录、API Key 创建/吊销
- **适用模式:** 所有认证、授权、凭据操作必须产生结构化审计日志，使用 `AUDIT:` 前缀便于检索

---

### 错误处理类

#### Panic 防护

- **受影响代码:** 多处 `.unwrap()` / `.expect()` 在生产环境会直接 crash
- **修复:**
  - RwLock: `unwrap()` → `unwrap_or_else(|e| e.into_inner())`（中毒恢复）
  - YAML 映射: `as_mapping_mut().unwrap()` → `let Some(m) = ... else { return }`
  - HTTP client: `expect()` → `unwrap_or_else(|| fallback)`
  - HeaderValue: `unwrap()` → `unwrap_or_else(|_| static_fallback)`
  - HMAC: `expect()` → `match { Ok => ..., Err => { warn!; None } }`
- **适用模式:** 公共 API/后台任务路径不允许 unwrap；用 `unwrap_or_else` + `tracing::error!` 兜底

#### 适配器重连限制

- **受影响代码:** 无限制重连循环（token 过期后永久重试）
- **修复:** `MAX_TOTAL_RECONNECT_ATTEMPTS = 20`，超限后停止重连
- **适用模式:** 所有自动重试必须有上限；达到上限后进入 terminal 状态，要求运维介入

#### 统一错误消息

- **受影响代码:** API Key 验证失败区分 "revoked" / "expired" / "invalid" → 用户枚举
- **修复:** 三者统一返回 `"Invalid API key"`
- **适用模式:** 认证失败的对外消息必须统一；内部区分用日志实现

---

### 基础设施类

#### Docker 强化

| 措施 | 配置 |
|------|------|
| 权限最小化 | `cap_drop: [ALL]` + `security_opt: [no-new-privileges:true]` |
| 只读根文件系统 | `read_only: true` + `tmpfs: [/tmp]` |
| 资源限制 | CPU 2 核 / 内存 512M |
| 网络隔离 | `frontend`（公网） + `backend`（internal） |
| 凭据强制 | PostgreSQL 密码使用 `${POSTGRES_PASSWORD:?err}` 强制设置 |
| 镜像固定 | Prometheus `latest` → `v2.55.0`；PostgreSQL 固定 digest |
| 文件所有权 | `COPY --chown=easybot:easybot` |
| 健康检查 | 独立 Dockerfile 添加 `HEALTHCHECK` |

#### 安全头部

- **添加:** `X-Frame-Options: DENY`、`X-Content-Type-Options: nosniff`、`Strict-Transport-Security: max-age=31536000; includeSubDomains`
- **CSP:** 移除 `'unsafe-inline'`，添加 `frame-ancestors 'none'`

#### CORS 运行时标志

- **受影响代码:** `cfg!(debug_assertions)` 导致 debug build 意外部署时 CORS 全开
- **修复:** 改为 `EASYBOT_DEBUG_CORS` 环境变量运行时检查
- **适用模式:** 不要用编译时标志控制安全策略；用环境变量或配置文件

#### 权限中间件路径匹配

- **受影响代码:** `p.contains("/config")` 子串匹配不精确
- **修复:** 先 `strip_prefix("/api/v1")` 获取路由路径，再用 `starts_with` 或精确 `==` 匹配
- **适用模式:** 权限检查的路径匹配必须基于精确的路由路径，不可是子串匹配

#### .env 文件权限检查

- **修复:** Unix 平台加载 `.env` 后检查权限，`mode & 0o077 != 0` 时发出 warning
- **适用模式:** 敏感文件的文件系统权限应在启动时验证

#### API Key 存储

- **修复:** admin.js 中 `localStorage` → `sessionStorage`（标签页关闭时自动清除）
- **适用模式:** 敏感 token 首选 HttpOnly cookie；次选 sessionStorage；禁用 localStorage

---

## 通用原则总结

1. **Fail-Secure:** 任何安全配置缺失时应拒绝操作，而不是回退到不安全默认值
2. **Defense in Depth:** 每个攻击面叠加多层防护（如登录：恒定时间比对 + 速率限制 + 审计日志）
3. **最小暴露:** API 响应不返回凭据、日志不记录凭据、Debug 不打印凭据
4. **输入验证:** 长度 + 字符集 + 数量 + 格式，四重验证
5. **错误统一:** 认证失败的错误消息对外一致，内部日志区分不同失败原因

---

## 涉及文件索引

| 文件 | 涉及修复 |
|------|---------|
| `crates/easybot-core/src/types/config.rs` | 默认密码移除 |
| `crates/easybot-core/src/types/adapter.rs` | Debug 脱敏 |
| `crates/easybot-core/src/types/session.rs` | Key 分隔符 sanitize |
| `crates/easybot-core/src/config/mod.rs` | SSRF 验证、.env 权限检查、merge_configs panic 防护 |
| `crates/easybot-core/src/auth/api_key.rs` | 统一错误消息 |
| `crates/easybot-core/src/bus/event_bus.rs` | 发布审计追踪、subscribe_many 文档 |
| `crates/easybot-core/src/session/manager.rs` | metadata 大小限制 |
| `crates/easybot-core/src/adapter/manager.rs` | 重连上限 |
| `crates/easybot-core/src/webhook/mod.rs` | HMAC panic 防护 |
| `crates/easybot-api/src/server.rs` | CSP、安全头部、CORS 运行时、TraceLayer、权限中间件、admin 限流 |
| `crates/easybot-api/src/routes/admin.rs` | 恒定时间比对、审计日志、权限验证、名称验证、Key 上限 |
| `crates/easybot-api/src/routes/config.rs` | 凭据脱敏、字段白名单、Webhook SSRF |
| `crates/easybot-api/src/routes/messages.rs` | 长度验证、metadata 剥离、错误清理 |
| `crates/easybot-api/src/routes/ws.rs` | 认证爆破防护、帧限流、日志脱敏 |
| `crates/easybot-api/src/middleware/rate_limit.rs` | 突发修复、XFF 受信代理、桶上限 |
| `crates/easybot-api/src/log_collector.rs` | RwLock 中毒恢复 |
| `crates/easybot-api/templates/js/admin.js` | sessionStorage、escapeHtml |
| `crates/easybot-adapter-wechat/src/lib.rs` | QR token 日志、AES key 日志、unwrap 防护 |
| `crates/easybot-adapter-wechat/src/crypto.rs` | SSRF 防护 |
| `crates/easybot-adapter-discord/src/lib.rs` | API 错误截断、WARN 日志清理 |
| `crates/easybot-adapter-feishu/src/lib.rs` | expect 防护、签名文档 |
| `crates/easybot-adapter-qq/src/lib.rs` | 401 精确匹配 |
| `bin/src/main.rs` | 密钥文件输出、admin 密码空值告警 |
| `Dockerfile` | HEALTHCHECK、chown |
| `Dockerfile.release` | HEALTHCHECK、chown |
| `docker-compose.yml` | 全面强化 |
| `deny.toml` | sources 检查、版本去重 |
