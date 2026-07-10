# 平台能力矩阵

EasyBot 已接入 5 个 IM 平台，各平台在机器人能力上有本质差异。本文档记录各平台的群聊支持、消息收发能力、重连策略，以及连接协议。

---

## 群聊/频道支持

| 平台 | 群聊 | 频道 | 说明 |
|------|------|------|------|
| **Telegram** | ✅ Group / Supergroup | ✅ Channel | 群聊和频道均支持 |
| **Discord** | ✅ Guild（服务器） | — | 频道是 Guild 的子结构，机器人按权限访问 |
| **飞书** | ✅ 群聊 | — | 飞书频道不在机器人范畴内 |
| **QQ** | ✅ QQ 群 | ✅ QQ 频道 | 统一 QQBot 平台同时支持群和频道 |
| **个人微信** | ❌ | ❌ | iLink Bot API 仅支持一对一私聊 |

---

## 消息收发场景

支持群聊的 4 个平台，bot 面临三种入站消息场景：

| 平台 | ① 私信 (DM) | ② 群内 @机器人 | ③ 群内公共消息（非 @） |
|------|-------------|----------------|----------------------|
| **Telegram** | ✅ I/O | ✅ I/O | ⚠️ 需关闭 Privacy Mode |
| **Discord** | ✅ I/O | ✅ I/O | ⚠️ 需开启 Message Content Intent |
| **飞书** | ✅ I/O | ✅ I/O | ⚠️ 需 `im:message.group_msg` 敏感权限 |
| **QQ** | ✅ I/O | ✅ I/O | ⚠️ 需选择"获取群内全部消息" |
| **个人微信** | ✅ I/O | — | 不支持群聊 |

> I/O = 收发均可用。⚠️ 表示能力存在但需额外配置。

### 全量消息配置要求

所有支持群聊的平台均能接收群内非 @ 消息，但需额外配置：

| 平台 | 默认行为 | 所需配置 | 配置入口 |
|------|---------|---------|---------|
| **Telegram** | 仅 `/command` 和 @消息 | 关闭 Privacy Mode | [@BotFather](https://t.me/BotFather) → `/setprivacy` → Disable |
| **Discord** | 接收所有消息（但 `content` 可能为空） | 开启 Message Content Intent | [Developer Portal](https://discord.com/developers) → Bot → Privileged Gateway Intents |
| **飞书** | 仅 @消息 | 申请 `im:message.group_msg` 敏感权限 | 飞书开放平台 → 权限管理 → 开发 → 发布 |
| **QQ** | 取决于机器人设置（默认仅 @） | 选择"获取群内全部消息" | [QQ 开放平台](https://q.qq.com/) → 机器人设置 → 消息范围 |

---

## 详细说明

### Telegram

- **消息接收**: HTTP 长轮询（`getUpdates`），30s 超时
- **消息过滤**: 无过滤。所有消息（私信、群聊、频道）均作为 inbound 发布
- **全量群消息**: 通过 [@BotFather](https://t.me/BotFather) `/setprivacy` 关闭 Privacy Mode
- **Outbound**: Text / Image / Audio / Video / Document / Markdown / HTML / Inline Keyboard / Edit / Delete / Streaming Draft / Typing

### Discord

- **消息接收**: Gateway WebSocket（intents: `GUILD_MESSAGES`, `DIRECT_MESSAGES`, `MESSAGE_CONTENT`）
- **消息过滤**: 仅过滤 bot 自身消息（按 `author.id`）。不区分 @mention
- **全量群消息**: 默认即接收所有消息，需开启 Message Content Intent（否则 `content` 为空）
- **Outbound**: Text / Image / Audio / Video / Document / Markdown / Interactive (ActionRow + Button) / Edit / Delete / Streaming Draft / Typing

Discord Components 格式（ActionRow + Button）：
```json
{
  "flags": 32768,
  "components": [
    {
      "type": 1,
      "components": [
        { "type": 2, "custom_id": "btn_yes", "label": "是", "style": 1 },
        { "type": 2, "custom_id": "btn_no",  "label": "否", "style": 4 }
      ]
    }
  ]
}
```

### 飞书

- **消息接收**: WebSocket（larksuite SDK `ws_client`），订阅 `im.message.receive_v1` 事件
- **消息过滤**: 无过滤。私信（`p2p`）和群聊（`group`）消息均接收
- **集群模式**: 事件仅随机推送到**其中一个**连接客户端，多实例部署需注意此限制
- **全量群消息**: 需申请 `im:message.group_msg` 敏感权限（默认 `im:message.group_at_msg:readonly` 仅 @消息）
- **Outbound**: Text / Image / Audio / Video / Document / Interactive Card / Markdown / Edit / Delete

### QQ

> **API 注意**: C2C 私聊不支持 `msg_type: 2`（图文混合），发送图片需 `msg_type: 1`（纯图片）。编辑/删除仅限频道消息，C2C/群聊不支持。

- **消息接收**: Gateway WebSocket，intents: `AT_MESSAGE`、`C2C_MESSAGE`、`GROUP_AT_MESSAGE`
- **消息过滤**: 2026 新版支持三种群消息范围：
  - `C2C_MESSAGE`: 私信 — 全部接收
  - `AT_MESSAGE`: 频道消息 — 仅 @
  - `GROUP_AT_MESSAGE_CREATE`: 群聊 @消息（旧协议）
  - `GROUP_MESSAGE_CREATE`: 群聊全量消息（2026 新版）
- **Self-message 过滤**: 频道按 `author.id` 过滤自身；群聊无法过滤（平台无 bot 字段）
- **Outbound**: Text / Image / Markdown / Inline Keyboard (Interactive) / Edit / Delete（仅频道）/ Chat List（仅频道服务器）

### 个人微信（iLink Bot API）

- **消息接收**: HTTP 长轮询（`/ilink/bot/getupdates`），35s 超时，首次需扫码登录
- **群聊支持**: **不支持**
- **Outbound**: Text / Image / Audio / Video / Document（AES-128-ECB 加密 + CDN 上传）
- **不支持**: Edit / Delete / Interactive / Chat List

---

## Capability 声明汇总

| Capability | Telegram | Discord | 飞书 | QQ | 微信 |
|-----------|----------|---------|------|-----|------|
| Text | ✅ | ✅ | ✅ | ✅ | ✅ |
| Image | ✅ | ✅ | ✅ | ✅ | ✅ |
| Audio | ✅ | ✅ | ✅ | ❌ | ✅ |
| Video | ✅ | ✅ | ✅ | ❌ | ✅ |
| Document | ✅ | ✅ | ✅ | ❌ | ✅ |
| Interactive | ✅ | ✅ | ✅ | ✅ | ❌ |
| Markdown | ✅ | ✅ | ✅ | ✅ | ❌ |
| Html | ✅ | ❌ | ❌ | ❌ | ❌ |
| Group | ✅ | ✅ | ✅ | ✅ | ❌ |
| TypingIndicator | ✅ | ✅ | ❌ | ❌ | ❌ |
| MessageEdit | ✅ | ✅ | ✅ | ✅¹ | ❌ |
| MessageDelete | ✅ | ✅ | ✅ | ✅¹ | ❌ |
| Thread | ❌ | ❌ | ❌ | ⚠️² | ❌ |
| ChatList | ❌ | ✅ | ❌ | ⚠️³ | ❌ |
| Streaming | ✅ | ✅ | ❌ | ❌ | ❌ |

> ¹ QQ 编辑/删除仅限频道消息。² QQ Thread 入站未填充 `thread_id`，无实际线程路由。³ QQ ChatList 仅支持列出频道服务器（Guild）。

---

## 官方 API / SDK 参考

| 平台 | API / SDK | EasyBot 中的 Base URL |
|------|----------|----------------------|
| **Telegram** | [Telegram Bot API](https://core.telegram.org/bots/api) | `https://api.telegram.org/bot` |
| **Discord** | [Discord API v10](https://discord.com/developers/docs/intro) | `https://discord.com/api/v10` |
| **飞书** | [larksuite-oapi-sdk-rs](https://crates.io/crates/larksuite-oapi-sdk-rs) | `https://open.feishu.cn/open-apis` |
| **QQ** | [QQBot API](https://bot.q.qq.com/wiki/) | `https://api.sgroup.qq.com` (API) / `https://bots.qq.com` (Auth) |
| **个人微信** | [iLink Bot API](https://ilinkai.weixin.qq.com) | `https://ilinkai.weixin.qq.com` |

---

## 连接协议

| 平台 | 连接协议 | 入站消息推送方式 |
|------|---------|----------------|
| **Telegram** | HTTP Long Polling | `getUpdates` 轮询（30s 超时） |
| **Discord** | Gateway WebSocket | `MESSAGE_CREATE` Dispatch 事件 |
| **飞书** | WS (larksuite SDK) | `im.message.receive_v1` 事件订阅 |
| **QQ** | Gateway WebSocket | `AT_MESSAGE_CREATE` / `GROUP_AT_MESSAGE_CREATE` / `GROUP_MESSAGE_CREATE` / `C2C_MESSAGE_CREATE` |
| **个人微信** | HTTP Long Polling | `/ilink/bot/getupdates` 轮询（35s 超时） |

---

## 断线自动重连

所有适配器均具备断线自动重连能力。除各适配器内置重连循环外，`AdapterManager` 提供通用健康监控（每 30s 检查所有适配器心跳，检测到后台任务死亡后触发指数退避重连：5s → 10s → 30s → 60s → 120s → 300s 封顶）。

| 平台 | 自动重连 | 重连延迟 | 实现方式 |
|------|---------|---------|---------|
| **Telegram** | ✅ | 5s | 长轮询 `loop` 内捕获错误后 `sleep(5s)` 重试 + Heartbeat liveness 追踪 |
| **Discord** | ✅ | 5s | 外层 `loop` 包裹全流程（连接→Hello→Identify→Ready→事件循环），失败后 5s 重试 + Heartbeat liveness |
| **飞书** | ✅ | 5s / 120s+jitter | SDK `ws_client.start()` 内部无限重连：首次 5s，反复失败使用服务端 `ReconnectInterval`（默认 120s）+ 随机抖动（0-30s）+ 独立 Heartbeat ticker (30s) |
| **QQ** | ✅ | 5-30s | 外层 `loop` 包裹全流程，每次重连前刷新 `access_token`。连接失败 5s，Token/URL 获取失败 30s + Heartbeat liveness |
| **个人微信** | ✅ | 5s | 长轮询 `loop` 内错误后 `sleep(5s)` 重试。连续 10 次失败视为 Session 过期，退出需重新扫码登录 + Heartbeat liveness |

> 所有重连循环响应 cancel 信号（`POST /adapters/{platform}/stop`），主动停止后不会继续重连。通用健康监控通过 EventBus 发布 `adapter.reconnecting`、`adapter.reconnected`、`adapter.reconnect_failed` 事件。

### 重连指标对比

| 平台 | 心跳/保活 | 重连触发条件 | 最长恢复时间 |
|------|----------|-------------|-------------|
| **Telegram** | 无（轮询即保活） | 网络 / API 错误 | ~35s（30s + 5s） |
| **Discord** | Gateway 指定间隔 | WS close / 连接错误 | ~5s |
| **飞书** | Ping/Pong（默认 120s） | WS close / 连接错误 | 5s（首）/ ~150s（反复） |
| **QQ** | Gateway 管理 | WS close / Token 过期 | 5-30s |
| **个人微信** | 无（轮询即保活） | 网络 / API 错误 / Session 过期 | ~40s（35s + 5s） |
