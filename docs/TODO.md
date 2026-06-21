# EasyBot TODO — 待办事项清单

> 最后更新: 2026-06-21
> 基于 docs/rust-implementation-plan.md 分阶段计划, 标注 P3 (100%) / P4 (90%) 全部未完成项

---

## P3: 多平台补完 (目标: 100%)

### Discord 适配器 (`crates/easybot-adapter-discord/src/lib.rs`)

- [x] **send_media** — 发送图片/音频/视频/文件
  - ✅ 已完成: 支持 base64 数据和 URL 下载两种模式, multipart/form-data 上传
  - 文件: `crates/easybot-adapter-discord/src/lib.rs`

- [x] **send_interactive** — 交互式按钮/键盘消息
  - ✅ 已完成: 支持 Discord Message Components (ActionRow + Button), callback (Primary style=1) 和 URL (Link style=5) 两种按钮
  - 文件: `crates/easybot-adapter-discord/src/lib.rs`

- [x] **list_chats** — 列出可用频道/服务器
  - ✅ 已完成: 通过 GET /users/@me/guilds 获取服务器列表 + GET /users/@me/channels 获取 DM 频道, 支持 chat_type 过滤
  - 文件: `crates/easybot-adapter-discord/src/lib.rs`, `crates/easybot-adapter-discord/src/types.rs`

### 微信适配器 (`crates/easybot-adapter-wechat/src/lib.rs`)

- [x] **edit_message** — ❌ 平台不支持
  - iLink Bot API 无消息编辑端点（已确认：API 仅有 7 个端点）
  - 文件: `crates/easybot-adapter-wechat/src/lib.rs`

- [x] **delete_message** — ❌ 平台不支持
  - iLink Bot API 无消息撤回/删除端点
  - 文件: `crates/easybot-adapter-wechat/src/lib.rs`

- [x] **send_interactive** — ❌ 平台不支持
  - iLink Bot API 仅支持 5 种消息类型（文本/图片/语音/文件/视频），无 keyboard/button/component 类型
  - 文件: `crates/easybot-adapter-wechat/src/lib.rs`

- [x] **list_chats** — ❌ 平台不支持
  - iLink Bot API 不提供聊天列表端点
  - 文件: `crates/easybot-adapter-wechat/src/lib.rs`

### QQ 适配器 (`crates/easybot-adapter-qq/src/lib.rs`)

- [x] **send_interactive** — 交互式按钮/键盘消息
  - ✅ 已完成: 支持 InlineKeyboard → QQ MessageKeyboard 映射, callback (type=2) 和 URL (type=0) 两种按钮
  - 文件: `crates/easybot-adapter-qq/src/lib.rs`, `crates/easybot-adapter-qq/src/types.rs`

- [x] **list_chats** — 实际返回聊天列表
  - ✅ 已完成: 通过 GET /users/@me/guilds 获取频道服务器列表, 支持 chat_type 过滤
  - 文件: `crates/easybot-adapter-qq/src/lib.rs`, `crates/easybot-adapter-qq/src/types.rs`

### 通用 / 跨平台

- [x] **list_chats 统一** — 5 个适配器全部完成: Telegram (stub), Discord ✅, 飞书 (stub), QQ ✅, 微信 ❌ (API 不支持)

---

## P4: 生产级补完 (目标: 100%)

### 安全与权限

- [ ] **权限模型 RBAC** — 角色 + 权限检查中间件
  - 新建文件: `crates/easybot-core/src/auth/permissions.rs`
  - 定义角色枚举 (Admin, Operator, ReadOnly 等)
  - 定义权限位 (send_message, manage_adapters, read_config 等)
  - 在 `easybot-api` 中间件中集成权限检查
  - 参考: 计划文档 Phase 4 任务 4.2

### 流式消息

- [x] **send_draft** — 流式草稿发送
  - ✅ 已完成: trait 方法 `send_draft()` 已添加到 PlatformAdapter, Telegram 和 Discord 已实现
  - Telegram: sendMessage (新建) / editMessageText (更新) 双模式
  - Discord: POST /channels/{id}/messages (新建) / PATCH (更新) 双模式
  - 文件: `crates/easybot-core/src/types/adapter.rs`, `message.rs`, Telegram/Discord `lib.rs`

### 健康与可靠性

- [x] **通用适配器健康轮询 + 自动重连**
  - 文件: `crates/easybot-core/src/adapter/manager.rs`
  - ✅ 已完成: AdapterManager 提供 `start_health_monitor()` 定时 `health()` 检查（30s 间隔）
  - 所有 5 个适配器集成 `Heartbeat` liveness 追踪，`health()` 使用 `health_status()` 检测后台任务存活
  - 指数退避重连: 5s → 10s → 30s → 60s → 120s → 300s 封顶
  - 通过 `gateway.yaml` 日志和 EventBus 事件 (`adapter.reconnecting/reconnected/reconnect_failed`) 观测

- [x] **Health 端点记录启动时间**
  - ✅ 已完成: AppState 新增 `started_at: Instant` 字段, health 响应返回 `uptime` (秒级)
  - 文件: `crates/easybot-api/src/lib.rs`, `crates/easybot-api/src/routes/health.rs`

### TLS / HTTPS

- [ ] **TLS 应用层处理**
  - 文件: `crates/easybot-api/src/server.rs`
  - 当前状态: `TlsConfig` 结构体存在, `tls.enabled` 标志存在, 但证书/密钥文件只在注释中文档化
  - 需要: 使用 axum 的 `axum_server::tls_rustls::RustlsConfig` 在 `server.rs` 中实际加载证书
  - 需要同时支持 HTTP 和 HTTPS 端口 (或强制重定向)

### 测试与验证环境

- [x] **QQ C2C/频道消息实机验证**
  - ✅ 已完成 (2026-06-21): 实机环境验证通过 — Gateway WebSocket 连接正常, 群聊 `GROUP_MESSAGE_CREATE` 和私聊 `C2C_MESSAGE_CREATE` 入站消息均成功接收, @mention 检测正常, outbound 群聊/私聊发送正常, `list_chats` 正确返回群聊和私聊列表

---

## 技术债务 / 代码质量

- [x] **微信适配器 `panic!()` 处理** — `crates/easybot-adapter-wechat/src/lib.rs`
  - ✅ 已修复: `panic!()` → `assert!(matches!(...))` 断言宏, 零 panic 残留
  - 应替换为 `Result` 返回或 `unreachable!()` 宏

---

## 优先级建议

| 优先级 | 项目 | 理由 |
|----------|------|---------|
| 🔴 高 | Discord `send_media` | 计划文档明确要求的核心功能, Discord 用户高频需求 |
| 🔴 高 | 通用健康轮询 + 自动重连 | 直接影响生产可用性, 当前仅 Discord 有重连 |
| 🟡 中 | 微信 `edit_message` / `delete_message` | ❌ iLink Bot API 不支持, 已确认关闭 |
| 🟡 中 | QQ `send_interactive` | ✅ 已完成 |
| 🟡 中 | Discord `send_interactive` + `list_chats` | ✅ 已完成 |
| 🟡 中 | WeChat `send_interactive` + `list_chats` | ❌ iLink Bot API 不支持, 已确认关闭 |
| 🟡 中 | TLS/HTTPS | 生产环境安全需求, 但可通过反向代理暂时规避（暂缓） |
| 🟢 低 | 权限模型 RBAC | 多用户场景才需要, 单用户部署不影响功能（暂缓） |
| 🟢 低 | QQ `list_chats` | ✅ 已完成 |
| 🟢 低 | `send_draft` 流式草稿 | ✅ 已完成 (Telegram + Discord) |
| 🟢 低 | Health 端点启动时间 | ✅ 已完成 |
