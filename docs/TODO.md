# EasyBot TODO — 待办事项清单

> 最后更新: 2026-06-21
> 基于 docs/rust-implementation-plan.md 分阶段计划, 标注 P3 (85%) / P4 (75%) 全部未完成项

---

## P3: 多平台补完 (目标: 100%)

### Discord 适配器 (`crates/easybot-adapter-discord/src/lib.rs`)

- [x] **send_media** — 发送图片/音频/视频/文件
  - ✅ 已完成: 支持 base64 数据和 URL 下载两种模式, multipart/form-data 上传
  - 文件: `crates/easybot-adapter-discord/src/lib.rs`

- [ ] **send_interactive** — 交互式按钮/键盘消息
  - Discord 支持 Message Components (buttons, select menus)
  - 参考: 飞书适配器的 `send_interactive` 实现
  - 文件: `crates/easybot-adapter-discord/src/lib.rs`

- [ ] **list_chats** — 列出可用频道/服务器
  - Discord API: `GET /users/@me/guilds` + `GET /guilds/{guild.id}/channels`
  - 文件: `crates/easybot-adapter-discord/src/lib.rs`

### 微信适配器 (`crates/easybot-adapter-wechat/src/lib.rs`)

- [ ] **edit_message** — 编辑已发送消息
  - 需要确认 iLink Bot API 是否支持消息编辑端点
  - 文件: `crates/easybot-adapter-wechat/src/lib.rs`

- [ ] **delete_message** — 撤回/删除消息
  - 需要确认 iLink Bot API 是否支持消息删除端点
  - 文件: `crates/easybot-adapter-wechat/src/lib.rs`

- [ ] **send_interactive** — 交互式消息
  - 需要确认 iLink Bot API 的交互式消息格式
  - 文件: `crates/easybot-adapter-wechat/src/lib.rs`

- [ ] **list_chats** — 实际返回聊天列表
  - 当前返回空 Vec (第 828 行)
  - 文件: `crates/easybot-adapter-wechat/src/lib.rs`

### QQ 适配器 (`crates/easybot-adapter-qq/src/lib.rs`)

- [ ] **send_interactive** — 交互式按钮/键盘消息
  - QQ Bot API 支持 MessageKeyboard (rows of buttons)
  - 文件: `crates/easybot-adapter-qq/src/lib.rs`

- [ ] **list_chats** — 实际返回聊天列表
  - 当前返回空 Vec (第 1161 行)
  - QQ Bot API: `GET /users/@me/guilds` + `GET /guilds/{guild_id}/channels`
  - 文件: `crates/easybot-adapter-qq/src/lib.rs`

### 通用 / 跨平台

- [ ] **list_chats 统一** — 确保所有 5 个适配器都实际实现 `list_chats` (当前 Discord/QQ/WeChat 返回空 Vec)

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

- [ ] **send_draft** — 流式草稿发送
  - trait 方法已定义在 `crates/easybot-core/src/types/adapter.rs`
  - 无任何适配器实现该方法
  - Telegram 支持: 使用 Bot API `editMessageText` + `disable_web_page_preview` 模拟流式输出
  - Discord 支持: 使用 `PATCH /channels/{channel.id}/messages/{message.id}` 编辑草稿
  - 建议先从 Telegram 开始实现

### 健康与可靠性

- [ ] **通用适配器健康轮询 + 自动重连**
  - 文件: `crates/easybot-core/src/adapter/manager.rs`
  - 当前状态: Discord Gateway 有自己的重连循环 (5s 重试), 其他适配器无通用重连机制
  - 需求: AdapterManager 提供定时 `health()` 检查, 健康状态异常时自动触发重连
  - 参考: Discord 的 `auto-reconnect` 实现 (commit `d963f2d`)

- [ ] **Health 端点记录启动时间**
  - 文件: `crates/easybot-api/src/routes/health.rs:56`
  - 第 56 行有 `// TODO: 记录启动时间`
  - 需要在 AppState 中记录 `started_at: chrono::DateTime<Utc>`, 在 health 响应中返回 uptime

### TLS / HTTPS

- [ ] **TLS 应用层处理**
  - 文件: `crates/easybot-api/src/server.rs`
  - 当前状态: `TlsConfig` 结构体存在, `tls.enabled` 标志存在, 但证书/密钥文件只在注释中文档化
  - 需要: 使用 axum 的 `axum_server::tls_rustls::RustlsConfig` 在 `server.rs` 中实际加载证书
  - 需要同时支持 HTTP 和 HTTPS 端口 (或强制重定向)

### 测试与验证环境

- [ ] **QQ C2C/频道消息实机验证**
  - 代码已实现 (GROUP_MESSAGE_CREATE, C2C_MESSAGE_CREATE 等 dispatch 测试通过)
  - 需要在真实 QQ Bot 环境中验证 C2C 私聊和频道消息收发

---

## 技术债务 / 代码质量

- [ ] **微信适配器 `panic!()` 处理** — `crates/easybot-adapter-wechat/src/lib.rs` 第 1202、1237 行
  - 测试辅助函数中 match 的 else 分支直接调用 `panic!()`
  - 应替换为 `Result` 返回或 `unreachable!()` 宏

---

## 优先级建议

| 优先级 | 项目 | 理由 |
|----------|------|---------|
| 🔴 高 | Discord `send_media` | 计划文档明确要求的核心功能, Discord 用户高频需求 |
| 🔴 高 | 通用健康轮询 + 自动重连 | 直接影响生产可用性, 当前仅 Discord 有重连 |
| 🟡 中 | 微信 `edit_message` / `delete_message` | 两个基础消息管理功能, 完整 CRUD 闭环 |
| 🟡 中 | QQ `send_interactive` | 交互式消息是常见聊天机器人需求 |
| 🟡 中 | TLS/HTTPS | 生产环境安全需求, 但可通过反向代理暂时规避 |
| 🟡 中 | Discord `send_interactive` + `list_chats` | 完善 Discord 适配器功能矩阵 |
| 🟢 低 | 权限模型 RBAC | 多用户场景才需要, 单用户部署不影响功能 |
| 🟢 低 | `send_draft` 流式草稿 | 高级功能, 目前无实际使用场景 |
| 🟢 低 | `list_chats` 微信/QQ | 管理端功能, 不影响核心消息收发 |
| 🟢 低 | Health 启动时间 TODO | 小改进, 不影响功能 |
