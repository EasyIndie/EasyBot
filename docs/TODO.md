# EasyBot TODO — 待办事项清单

> 最后更新: 2026-06-21
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
| **个人微信** | 群聊支持 | 仅一对一私聊 |
| **飞书** | ChatList | 平台 API 限制 |
| **飞书** | Streaming | 平台 API 限制 |
| **飞书** | TypingIndicator | 平台 API 限制 |
| **QQ** | Audio/Video/Document | 平台 API 限制 |
| **QQ** | Streaming | 平台 API 限制 |
| **Telegram** | ChatList | 平台 API 限制 |
| **Telegram** | Thread | 平台 API 限制 |

---

## 技术债务

（无 — 所有已知技术债务已解决）
