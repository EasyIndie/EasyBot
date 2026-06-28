# EasyBot TODO — 待办事项清单

> 最后更新: 2026-06-28

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

### 前端优化（2026-06-28）
- [x] **Sessions Tab 闪烁** — 增量 DOM 更新（`data-session-key` 属性 diff）
- [x] **Messages Tab 切换时空列表** — AbortController + 重置 cursor
- [x] **Metrics 刷新闪烁** — 刷新时跳过 loading spinner
- [x] **按钮文字折行** — `white-space: nowrap`
- [x] **首页简化** — 移除快速开始和平台区块
- [x] **登录页导航** — EasyBot 标题点击返回首页

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

## 审计完成

两轮安全审计已完成（Round 1: 30 项 / Round 2: 20 项）。所有发现项均已修复并合入代码库。审计记录已归档（可从 Git 历史查看）。

| 维度 | Round 1 | Round 2 | 变化 |
|------|:------:|:------:|:----:|
| 代码质量与架构 | 8.0 | 8.0 | — |
| 安全 | 5.5 | 6.5 | +1.0 ⬆️ |
| 测试覆盖与质量 | 6.5 | 7.0 | +0.5 ⬆️ |
| 性能与可靠性 | 7.0 | 7.5 | +0.5 ⬆️ |
| 文档与可维护性 | 7.5 | 8.0 | +0.5 ⬆️ |
| 依赖与供应链 | 8.0 | 8.0 | — |
| **综合** | **7.1** | **7.5** | **+0.4 ⬆️** |

> 下一阶段关注 TLS/HTTPS 和 RBAC 权限模型的覆盖。
