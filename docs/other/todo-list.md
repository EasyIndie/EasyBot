# EasyBot TODO — 待办事项清单

> 最后更新: 2026-08-14

---

## 进度总览

| 阶段 | 状态 | 说明 |
|------|------|------|
| **P1 MVP** | 100% ✅ | 核心类型、Telegram 适配器、REST API、配置加载 |
| **P2 Bidirectional** | 100% ✅ | 事件总线、WebSocket 推送、入站消息、消息编辑/删除 |
| **P3 Multi-platform** | 100% ✅ | 五平台适配器全部完成（微信受限于 iLink Bot API） |
| **P4 Production engineering** | 100% ✅ | TLS 由固定镜像的 Caddy 终止；API Key 已实施细粒度权限、审计、配额与商用发布门禁。真实上线仍需外部证据验收 |
| **P5 Plugin System** | 100% ✅ | Plugin SDK、动态加载、开发者文档 |
| **P6 Plugin Market** | 100% ✅ | GitHub Releases 分发、ed25519 签名、多注册表 Taps、信任语义、脚手架/DX、plugin-publish.yml |

---

## 已完成项 (当前开发)

### 资源耗尽审计与修复（2026-07-07）
- [x] **SQLite WAL 文件增长** — 新增后台 WAL checkpoint 任务，`PRAGMA wal_checkpoint(TRUNCATE)` 按 TTL 周期运行
- [x] **Webhook 并发保护** — Semaphore 上限 16 并发分发，防止事件洪水压垮运行时
- [x] **SessionBridge 任务泄漏** — 移除每条入站消息 spawn 2 个 tokio 任务，改为内联执行
- [x] **SessionManager 内存清理** — 新增 `prune_expired()` 方法，按 TTL 周期同步清理 DashMap 过期会话
- [x] **QQ chat_types 缓存** — 4 处插入点加 10,000 条上限
- [x] **Telegram admin_cache 缓存** — 插入点加 5,000 条上限
- [x] **Discord guild_owner_cache 缓存** — 2 处插入点加 5,000 条上限
- [x] **飞书 role_cache TTL** — 缓存读取时检查 30s 过期，到期自动移除重新查询

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
- [x] **通用健康轮询 + 自动重连** — AdapterManager.start_health_monitor()，5 适配器 Heartbeat 集成，分级响应（传输重试→完整重连→慢重试）+ 错误分类
- [x] **Health 端点启动时间** — AppState.started_at → uptime 秒级
- [x] **WeChat panic!() 修复** — 2 处 assert!(matches!(...)) 替换
- [x] **AdapterManager 状态缓存修复** — list_statuses()/get_status() 实时查询 adapter.status_summary()

---

## 架构边界与外部上线项

| 项目 | 文件 | 原因 |
|------|------|------|
| **应用进程内 TLS** | `crates/easybot-api/src/server.rs` | 明确不作为生产 TLS 终止点；`deploy/Caddyfile`/云负载均衡负责证书、HSTS 与 WebSocket 代理 |
| **进程内多租户隔离** | `crates/easybot-core/src/auth/` | 当前采用“每客户独立实例与数据库”；不能把互不信任客户放进同一实例 |
| **真实商业运营证据** | `commercial/evidence.env.example` | 法务、支付/开票、告警送达、容量、迁移、备份恢复及回滚必须在真实环境验收，代码库不能自行证明 |
| **Windows U2 升级真机验证** | `docs/other/windows-upgrade-verify.md` | 分离辅助脚本两步替换的运行时锁语义（marker OK/TIMEOUT、`!`/空格路径、回滚拒绝）已通过 Windows 真机 + NSSM 验收（2026-08-14：U1、U2 分离交换机制、场景 C/E/F、A·B 前置路径）；场景 A/B/D/G 完整端到端作为 v0.0.37 发版后回归项 |

---

## 平台限制 (无法实现)

| 平台 | 限制项 | 原因 |
|------|--------|------|
| **个人微信** | edit_message | iLink Bot API 无编辑端点 |
| **个人微信** | delete_message | iLink Bot API 无撤回端点 |
| **个人微信** | send_interactive | 仅 5 种消息类型，无 keyboard/button |
| **个人微信** | list_chats | 无聊天列表端点 |
| **个人微信** | 群聊支持 | 入站消息可识别群聊（通过 `group_id`），发送群聊消息取决于 iLink Bot API 权限 |
| **个人微信** | context_token 协议级刷新 | iLink Bot API 无官方刷新端点，采用保守降级路径（过期仍试一次 + 失败兜底重登） |
| **飞书** | ChatList | 平台 API 限制 |
| **飞书** | Streaming | 平台 API 限制 |
| **飞书** | TypingIndicator | 平台 API 限制 |
| **飞书** | sticker 出站 | 无官方贴纸发送通道，`send_media(Sticker)` → `CapabilityNotSupported` |
| **QQ** | Audio/Video/Document | 平台 API 限制 |
| **QQ** | Streaming | 平台 API 限制 |
| **Telegram** | ChatList | 平台 API 限制 |
| **Telegram** | Thread | 平台 API 限制 |

---

## 待办 — 适配器协议审查遗留缺口（2026-08-12）

五平台适配器协议审查（Telegram/Discord/飞书/QQ/微信）后整理的功能缺口。下表中"平台限制"项已在"平台限制"节列出，此处仅登记可实现或需决策的项目。

| 平台 | 缺口 | 状态 | 说明 |
|------|------|------|------|
| **Telegram** | album 出站（media group 多图） | 待实现 | `SendMediaParams` 目前仅支持单媒体出站，需扩展媒体模型支持一次发送多图 |
| **飞书** | 视频封面上传 | 待决策 | 飞书视频消息支持 `cover` 参数，但 `SendMediaParams` 无封面字段承载；是否扩展媒体模型需决策 |
| **QQ** | 公/私域机器人自动检测 | 待实现 | 无官方端点可运行时探测，目前公域机器人需手动 `config.extra.intents` |
| **Discord** | >2500 guild 单分片拒连 | 待实现 | `/gateway/bot` 失败时回退单分片，超大服务器仍可能被 4010 拒连；大集群需完整分片协商 |

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

> 下一阶段关注真实预生产/生产证据、支付与财税系统接入，以及需要横向扩容时的外部共享配额和租户隔离。
