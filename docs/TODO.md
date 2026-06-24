# EasyBot TODO — 待办事项清单

> 最后更新: 2026-06-24
> 基于 docs/rust-implementation-plan.md 分阶段计划

---

## 进度总览

| 阶段 | 状态 | 说明 |
|------|------|------|
| **P1 MVP** | 100% ✅ | 核心类型、Telegram 适配器、REST API、配置加载 |
| **P2 Bidirectional** | 100% ✅ | 事件总线、WebSocket 推送、入站消息、消息编辑/删除 |
| **P3 Multi-platform** | 100% ✅ | 五平台适配器全部完成（微信受限于 iLink Bot API） |
| **P4 Production** | 95% ✅ | 仅 TLS/HTTPS 和 RBAC 暂缓，其余全部完成 |
| **P5 Plugin System** | 100% ✅ | Plugin SDK、动态加载、开发者文档 |

---

## 已完成项 (本轮开发)

### WeChat 适配器
- [x] **edit_message** — ❌ 平台不支持（iLink Bot API 仅 7 个端点）
- [x] **delete_message** — ❌ 平台不支持
- [x] **send_interactive** — ❌ 平台不支持（仅 5 种消息类型，无 keyboard/button）
- [x] **list_chats** — ❌ 平台不支持（无聊天列表端点）

### QQ 适配器
- [x] **send_interactive** — InlineKeyboard → QQ MessageKeyboard 映射
- [x] **list_chats** — GET /users/@me/guilds，支持 chat_type 过滤
- [x] **C2C/频道消息实机验证** — Gateway WebSocket 连接正常，GROUP_MESSAGE_CREATE + C2C_MESSAGE_CREATE 入站消息均成功接收，@mention 检测正确

### Discord 适配器
- [x] **send_media** — Image/Audio/Video/Document，base64 + URL 下载双模式
- [x] **send_interactive** — Message Components (ActionRow + Button)
- [x] **list_chats** — GET /users/@me/guilds + /users/@me/channels

### 跨平台 / 基础设施
- [x] **send_draft** 流式草稿 — Telegram (sendMessage/editMessageText) + Discord (POST/PATCH)
- [x] **通用健康轮询 + 自动重连** — AdapterManager.start_health_monitor()，5 适配器 Heartbeat 集成，指数退避
- [x] **Health 端点启动时间** — AppState.started_at → uptime 秒级
- [x] **WeChat panic!() 修复** — 2 处 assert!(matches!(...)) 替换
- [x] **AdapterManager 状态缓存修复** — list_statuses()/get_status() 实时查询 adapter.status_summary()

---

## 暂缓项

| 项目 | 文件 | 原因 |
|------|------|------|
| **TLS/HTTPS** | `crates/easybot-api/src/server.rs` | TlsConfig 结构体存在但未接入应用层，可通过反向代理规避 |
| **RBAC 权限模型** | `crates/easybot-core/src/auth/` | 多用户场景才需要，单用户部署不影响功能 |

---

## 平台限制 (无法实现)

| 平台 | 限制项 | 原因 |
|------|--------|------|
| **个人微信** | edit_message | iLink Bot API 无编辑端点 |
| **个人微信** | delete_message | iLink Bot API 无撤回端点 |
| **个人微信** | send_interactive | 仅 5 种消息类型，无 keyboard/button |
| **个人微信** | list_chats | 无聊天列表端点 |
| **个人微信** | 群聊支持 | 入站消息可识别群聊（通过 `group_id`），发送群聊消息取决于 iLink Bot API 权限 |
| **飞书** | ChatList | 平台 API 限制 |
| **飞书** | Streaming | 平台 API 限制 |
| **飞书** | TypingIndicator | 平台 API 限制 |
| **QQ** | Audio/Video/Document | 平台 API 限制 |
| **QQ** | Streaming | 平台 API 限制 |
| **Telegram** | ChatList | 平台 API 限制 |
| **Telegram** | Thread | 平台 API 限制 |

---

## 审计发现 · 修复计划

### Round 2 (当前) — 2026-06-24 第二轮审计

> 第二轮全面审计（6 维度）→ 20 项新发现 (N1–N20)
>
> 详见 **[AUDIT_FIX_PLAN.md](AUDIT_FIX_PLAN.md)** — Round 2 修复计划

| 优先级 | 数量 | 状态 | 预计工时 |
|--------|:----:|------|:--------:|
| P0 紧急 | 5 | ✅ 已完成 | ~10h |
| P1 高 | 8 | ✅ 已完成 | ~14h |
| P2 中 | 4 | ✅ 3/4 (拆分待编译) | ~16h |
| P3 低 | 3 | ✅ 已完成 | ~12h |
| **合计** | **20** | **19/20** | **~52h** |

#### Round 2 修复概览

| ID | 优先级 | 简述 | 文件 |
|----|:------:|------|------|
| P0-1 (N8) | 🔴 | ✅ API Key 权限检查中间件 | `api/server.rs` + `auth/permissions.rs` (新建) |
| P0-2 (N1) | 🔴 | ✅ Feishu unwrap() panic 风险 | `feishu/src/lib.rs:466` |
| P0-3 (N2) | 🔴 | ✅ Arc::try_unwrap panic 风险 | `api/routes/messages.rs:283` |
| P0-4 (N9) | 🔴 | ✅ AssertSqlSafe + format! SQL 拼接 | `storage/sqlite.rs` + `postgres.rs` |
| P0-5 (N10) | 🔴 | ✅ CORS permissive 生产加固 | `api/server.rs:244` |
| P1-1 (N3) | 🟠 | ✅ Workspace lint 继承修复 | 13 个 Cargo.toml |
| P1-2 (N11) | 🟠 | ✅ HTTP 请求体大小限制 | `api/server.rs` |
| P1-3 (N12) | 🟠 | ✅ WebSocket 帧大小限制 | `api/routes/ws.rs` |
| P1-4 (N4) | 🟠 | ✅ QQ std::Mutex → parking_lot | `qq/src/lib.rs` |
| P1-5 (N5) | 🟠 | ✅ SessionManager 存储日志 | `session/manager.rs` |
| P1-6 (N13) | 🟠 | ✅ /metrics 端点认证 | `api/server.rs` |
| P1-7 (N14) | 🟠 | ✅ X-Forwarded-For 信任链修复 | `middleware/rate_limit.rs` |
| P1-8 (N15) | 🟠 | ✅ Feishu SDK 版本审计 | `feishu/Cargo.toml` |
| P2-1 (N6) | 🟡 | ✅ webhook serialize 失败日志 | `webhook/mod.rs` |
| P2-2 (N16) | 🟡 | ✅ HTTP Client 类型统一 OnceLock | `feishu` + `qq` + `wechat` |
| P2-3 (N17) | 🟡 | ✅ WeChat 构造函数统一 | `wechat` + `bin/main.rs` |
| P2-4 (N7) | 🟡 | ⏸️ QQ/WeChat 适配器拆分 | `qq/src/` (✅ 已完成: auth.rs, gateway.rs, types.rs) + `wechat/src/` (⏸️ crypto.rs 已拆分，其余待验证) |
| P3-1 (N18) | 🟢 | ✅ Capability 声明宏去重 | 2/5 适配器 (Telegram + Discord) |
| P3-2 (N19) | 🟢 | ✅ bin/main.rs 注册宏统一 | `bin/src/main.rs` |
| P3-3 (N20) | 🟢 | ✅ Plugin 沙箱文档化 | `SECURITY.md` + `plugin/loader.rs` |

### Round 1 (已完成) — 2026-06-24 早前

> 2026-06-24 首轮审计（6 维度 · 60+ 发现）→ 30 项可执行修复
>
> 详见 **[AUDIT_FIX_PLAN.md](AUDIT_FIX_PLAN.md)** — Round 1 历史记录

| 优先级 | 数量 | 状态 |
|--------|:----:|------|
| P0 紧急 | 5 | ✅ 已完成 |
| P1 高 | 8 | ✅ 已完成 |
| P2 中 | 10 | ✅ 已完成 |
| P3 低 | 7 | ✅ 已完成 |
| **合计** | **30** | **✅ 全部完成** |

### 审计评分汇总

| 维度 | Round 1 | Round 2 | 变化 |
|------|:------:|:------:|:----:|
| 代码质量与架构 | 8.0 | 8.0 | — |
| 安全 | 5.5 | 6.5 | +1.0 ⬆️ |
| 测试覆盖与质量 | 6.5 | 7.0 | +0.5 ⬆️ |
| 性能与可靠性 | 7.0 | 7.5 | +0.5 ⬆️ |
| 文档与可维护性 | 7.5 | 8.0 | +0.5 ⬆️ |
| 依赖与供应链 | 8.0 | 8.0 | — |
| **综合** | **7.1** | **7.5** | **+0.4 ⬆️** |

> 目标: Round 2 P0–P1 完成后安全预计达到 7.5+

## 技术债务

（已纳入 [AUDIT_FIX_PLAN.md](AUDIT_FIX_PLAN.md) 跟踪）
