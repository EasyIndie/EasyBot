# 平台能力矩阵

EasyBot 已接入 5 个 IM 平台，各平台在机器人能力上有本质差异。本文档记录各平台的群聊/频道支持情况，以及不同消息场景下的 inbound/outbound 收发能力。

## 机器人加入群聊/频道

| 平台 | 机器人可加入群聊 | 机器人可加入频道 | 说明 |
|---|---|---|---|
| **Telegram** | ✅ Group / Supergroup | ✅ Channel | 群聊和频道均支持，通过 BotFather 配置 |
| **Discord** | ✅ Guild（服务器） | — | Discord 频道是 Guild 的子结构，机器人加入服务器后按权限访问频道 |
| **飞书** | ✅ 群聊 | — | 飞书频道的产品形态不同于传统 IM 机器人范畴 |
| **QQ** | ✅ QQ 群 | ✅ QQ 频道 | 统一 QQBot 平台同时支持群和频道 |
| **个人微信** | ❌ 不支持 | ❌ 不支持 | iLink Bot API 仅支持一对一私聊 |

## 消息收发场景矩阵

对于支持群聊的 4 个平台，bot 面临三种 inbound 消息场景：

| 平台 | ① 私信消息 (DM) | ② 群内 @机器人 | ③ 群内公共消息（非 @） |
|---|---|---|---|
| **Telegram** | ✅ I/O | ✅ I/O | ⚠️ 需关闭 Privacy Mode（默认仅 `/command` 和 @消息）|
| **Discord** | ✅ I/O | ✅ I/O | ⚠️ 需开启 Message Content Intent（否则 `content` 为空）|
| **飞书** | ✅ I/O | ✅ I/O | ⚠️ 需 `im:message.group_msg` 敏感权限（默认仅 @消息）|
| **QQ** | ✅ I/O | ✅ I/O | ⚠️ 需在机器人设置中选择"获取群内全部消息"（默认仅 @消息）|
| **个人微信** | ✅ I/O | — 不支持群聊 | — 不支持群聊 |

> I/O = inbound 接收 + outbound 发送均可用。⚠️ 表示能力存在但需要额外配置/权限才能启用，标记为 ❌ 的是**平台原生限制**。

### 群聊全量消息配置要求

所有支持群聊的平台均能接收群内非 @ 消息，但需要额外配置。默认配置下各平台均只能接收 @机器人消息（或仅 `/command` 消息）：

| 平台 | 默认行为 | 全量消息所需配置 | 配置入口 |
|---|---|---|---|
| **Telegram** | 仅 `/command` 和 @消息（Privacy Mode 开启） | 关闭 Privacy Mode | [@BotFather](https://t.me/BotFather) `/setprivacy` → Disable |
| **Discord** | 所有消息（但 `content` 可能为空） | 开启 Message Content Intent | [Developer Portal](https://discord.com/developers) → Bot → Privileged Gateway Intents |
| **飞书** | 仅 @消息（`group_at_msg:readonly`） | 申请 `im:message.group_msg` 敏感权限 | 飞书开放平台 → 权限管理 → 审核 → 发布 |
| **QQ** | 取决于机器人设置（默认仅 @） | 选择"获取群内全部消息" | [QQ 开放平台](https://q.qq.com/) → 机器人设置 → 消息范围 |

## 各平台详细说明

### Telegram

- **消息接收方式**: HTTP 长轮询 (`getUpdates`)，30s 超时
- **消息过滤**: 无过滤。所有消息（私信、群聊、频道）均作为 inbound 发布。不区分 @mention
- **群聊全量消息**: 需通过 [@BotFather](https://t.me/BotFather) `/setprivacy` 关闭 Privacy Mode（默认开启时仅接收 `/command` 和 @消息）
- **Outbound 能力**: Text / Image / Audio / Video / Document / Markdown / HTML / Inline Keyboard / Message Edit / Delete

### Discord

- **消息接收方式**: Gateway WebSocket (intent: `GUILD_MESSAGES`, `DIRECT_MESSAGES`, `MESSAGE_CONTENT`)
- **消息过滤**: 仅过滤 bot 自身发送的消息（按 `author.id`）。群内所有消息均接收，不区分 @mention
- **群聊全量消息**: 默认即接收所有消息，需在 Bot 设置中开启 **Message Content Intent**（否则 `content` 字段为空）
- **Outbound 能力**: Text / Image / Audio / Video / Document / Markdown / Inline Keyboard (Interactive) / Message Edit / Delete / Streaming Draft

### 飞书

- **消息接收方式**: WebSocket (larksuite SDK `ws_client`)，订阅 `im.message.receive_v1` 事件
- **消息过滤**: 无过滤。私信 (`p2p`) 和群聊 (`group`) 消息均接收
- **群聊全量消息**: 需申请敏感权限 `im:message.group_msg`（获取群组中所有消息）。默认权限 `im:message.group_at_msg:readonly` 仅接收 @机器人消息。配置路径：飞书开放平台 → 权限管理 → 搜索 `group_msg` → 开通 → 发布版本并提交审核
- **Outbound 能力**: Text / Image / Audio / Video / Document / Interactive Card / Markdown / Message Edit / Delete

### QQ

- **消息接收方式**: Gateway WebSocket，intent 为 `AT_MESSAGE`、`C2C_MESSAGE`、`GROUP_AT_MESSAGE`
- **消息过滤**: 2026 年新版协议支持三种群消息范围（可在机器人设置中选择）：
  - `C2C_MESSAGE`: 私信消息 — 全部接收
  - `AT_MESSAGE`: 频道消息（`AT_MESSAGE_CREATE`）— 仅 @ 消息
  - `GROUP_AT_MESSAGE_CREATE`: 群聊 @消息（旧协议）— 仅 @ 消息，`mentioned: true`
  - `GROUP_MESSAGE_CREATE`: 群聊全量消息（2026 新版）— 全部接收，通过 `mentions[].is_you` 判断是否 @
- **Inbound 消息标记**: 新增 `mentioned` 字段：
  - 频道/旧版群 @ → `Some(true)`
  - 新版全量群消息 → `Some(bool)`（根据 `mentions.is_you` 判断）
  - C2C 私聊 → `None`
- **Self-message 过滤**: 频道消息按 `author.id` 过滤自身；群聊消息无法过滤自身（平台无 bot 字段）
- **Outbound 能力**: Text / Image / Markdown / Inline Keyboard (Interactive) / Message Edit / Delete / Chat List

### 个人微信 (iLink Bot API)

- **消息接收方式**: HTTP 长轮询 (`/ilink/bot/getupdates`)，首次需扫码登录
- **群聊支持**: **不支持**。iLink Bot API 仅提供一对一聊天能力
- **Outbound 能力**: Text 已实现。图片/音频/视频/文件发送依赖 AES-128-ECB 加密（尚未实现）。编辑/删除/交互式按钮/聊天列表 — iLink Bot API 不支持

## 各平台 Capability 声明汇总

| Capability | Telegram | Discord | 飞书 | QQ | WeChat |
|---|---|---|---|---|---|
| Text | ✅ | ✅ | ✅ | ✅ | ✅ |
| Image | ✅ | ✅ | ✅ | ✅ | ❌ |
| Audio | ✅ | ✅ | ✅ | ❌ | ❌ |
| Video | ✅ | ✅ | ✅ | ❌ | ❌ |
| Document | ✅ | ✅ | ✅ | ❌ | ❌ |
| Interactive | ✅ | ✅ | ✅ | ✅ | ❌ |
| Markdown | ✅ | ✅ | ✅ | ✅ | ❌ |
| Html | ✅ | ❌ | ❌ | ❌ | ❌ |
| Group | ✅ | ✅ | ✅ | ✅ | ❌ |
| TypingIndicator | ✅ | ✅ | ❌ | ❌ | ❌ |
| MessageEdit | ✅ | ✅ | ✅ | ✅ | ❌ |
| MessageDelete | ✅ | ✅ | ✅ | ✅ | ❌ |
| Thread | ❌ | ❌ | ❌ | ✅ | ❌ |
| ChatList | ❌ | ✅ | ❌ | ✅ | ❌ |
| Streaming | ✅ | ✅ | ❌ | ❌ | ❌ |

## 设计启示

1. **QQ 2026 年升级为全量群消息** — 2026 年新版 `GROUP_MESSAGE_CREATE` 事件支持接收全部群消息（非 @ 也可），机器人设置中可选择消息范围（全部 / @最近10条 / 仅 @）。旧版 `GROUP_AT_MESSAGE_CREATE` 仍兼容
2. **群聊全量消息需要额外配置** — 各平台均支持接收群内非 @ 消息，但都需要额外配置：Telegram 需关闭 Privacy Mode、Discord 需开启 Message Content Intent、飞书需申请 `im:message.group_msg` 敏感权限、QQ 需选择"获取群内全部消息"。默认配置下只有 Telegram/Discord/飞书 @消息 和 QQ @消息 可正常接收
3. **个人微信群聊限制** — 如果需要群聊能力，必须改用企业微信或其他方案；个人微信仅适合一对一对话场景
4. **Outbound 媒体差距** — QQ/WeChat 的媒体发送能力不完整（QQ 仅 Image、WeChat 仅 Text），WeChat 受限于 AES-128-ECB 加密。Discord 媒体发送已全面支持 (Image/Audio/Video/Document)

## 官方 API / SDK 参考

EasyBot 各适配器直接对接平台官方 API，未使用第三方封装 SDK（飞书除外）：

| 平台 | 官方 API / SDK | 文档入口 | EasyBot 中的 Base URL |
|---|---|---|---|
| **Telegram** | [Telegram Bot API](https://core.telegram.org/bots/api) | [Bot API docs](https://core.telegram.org/bots/api) | `https://api.telegram.org/bot` |
| **Discord** | [Discord API v10](https://discord.com/developers/docs/intro) | [Developer Portal](https://discord.com/developers/docs/intro) | `https://discord.com/api/v10` |
| **飞书** | [larksuite-oapi-sdk-rs](https://crates.io/crates/larksuite-oapi-sdk-rs) v0.1 + WebSocket | [飞书开放平台](https://open.feishu.cn/) | `https://open.feishu.cn/open-apis` |
| **QQ** | [QQBot API](https://bot.q.qq.com/wiki/) | [QQ 开放平台](https://q.qq.com/) | `https://api.sgroup.qq.com` (API) / `https://bots.qq.com` (Auth) |
| **个人微信** | [iLink Bot API](https://ilinkai.weixin.qq.com) | 腾讯官方协议文档 | `https://ilinkai.weixin.qq.com` |

### 各平台接入入口

| 平台 | 注册 / 创建 Bot | 凭证获取 |
|---|---|---|
| **Telegram** | [@BotFather](https://t.me/BotFather) 发送 `/newbot` | Bot Token（`12345:ABC-DEF...`） |
| **Discord** | [Discord Developer Portal](https://discord.com/developers/applications) → New Application → Bot | Bot Token + Message Content Intent |
| **飞书** | [飞书开放平台](https://open.feishu.cn/) → 创建企业自建应用 | App ID + App Secret |
| **QQ** | [QQ 开放平台](https://q.qq.com/) → 创建机器人 | BotAppID + AppSecret |
| **个人微信** | iLink Bot API（扫码登录，无需注册应用） | QR 码扫码获取 bot_token |

### 连接协议

| 平台 | 连接协议 | 入站消息推送方式 |
|---|---|---|
| **Telegram** | HTTP Long Polling | `getUpdates` 轮询（30s 超时） |
| **Discord** | Gateway WebSocket | `MESSAGE_CREATE` Dispatch 事件 |
| **飞书** | WS (larksuite SDK) | `im.message.receive_v1` 事件订阅 |
| **QQ** | Gateway WebSocket | `AT_MESSAGE_CREATE` / `GROUP_AT_MESSAGE_CREATE` / `C2C_MESSAGE_CREATE` Dispatch |
| **个人微信** | HTTP Long Polling | `/ilink/bot/getupdates` 轮询（35s 超时） |

### 断线自动重连

所有适配器均具备断线自动重连能力，网络抖动或服务端主动断开后无需人工干预。

除各适配器内置重连循环外，`AdapterManager` 提供**通用健康监控**（`start_health_monitor()`），每 30s 检查所有适配器状态，检测到后台任务死亡后触发指数退避重连（5s → 10s → 30s → 60s → 120s → 300s 封顶）。

| 平台 | 自动重连 | 重连延迟 | 实现方式 |
|---|---|---|---|
| **Telegram** | ✅ | 5s | 长轮询 `loop` 内捕获错误后 `sleep(5s)` 重试 + Heartbeat liveness 追踪 |
| **Discord** | ✅ | 5s | 外层 `loop` 包裹全流程（连接→Hello→Identify→Ready→事件循环），任意阶段失败后 5s 重试，支持 cancel 信号优雅退出 + Heartbeat liveness 追踪 |
| **飞书** | ✅ | 5s / 120s+jitter | SDK `ws_client.start()` 内部无限重连循环：首次失败 5s 快速重连，反复失败使用服务端下发的 `ReconnectInterval`（默认 120s）+ 随机抖动（0-30s）+ 独立 Heartbeat ticker (30s) |
| **QQ** | ✅ | 5-30s | 外层 `loop` 包裹全流程，每次重连前刷新 `access_token`。连接失败 5s 重试，Token/URL 获取失败 30s 重试 + Heartbeat liveness 追踪 |
| **个人微信** | ✅ | 5s | 长轮询 `loop` 内捕获错误后 `sleep(5s)` 重试。连续 10 次失败视为 Session 过期，清除凭据并退出（需重新扫码登录）+ Heartbeat liveness 追踪 |

> 所有适配器的重连循环均响应 cancel 信号（`POST /adapters/{platform}/stop`），不会在主动停止后继续重连。通用健康监控通过 EventBus 发布 `adapter.reconnecting`、`adapter.reconnected`、`adapter.reconnect_failed` 事件。

### 连接生命周期对比

| 平台 | 协议 | 心跳/保活 | 重连触发条件 | 最长恢复时间 |
|---|---|---|---|---|
| **Telegram** | HTTP Long Poll | 无（轮询即保活） | 网络错误 / API 错误 | ~35s（30s 超时 + 5s 重连） |
| **Discord** | WebSocket | Gateway Hello 指定间隔 | WS close / 连接错误 / 事件循环退出 | ~5s |
| **飞书** | WebSocket (SDK) | Ping/Pong（默认 120s） | WS close / 连接错误 | 5s（首次）/ ~150s（反复失败） |
| **QQ** | WebSocket | 由 Gateway 管理 | WS close / Token 过期 / URL 获取失败 | 5-30s |
| **个人微信** | HTTP Long Poll | 无（轮询即保活） | 网络错误 / API 错误 / Session 过期 | ~40s（35s 超时 + 5s 重连） |
