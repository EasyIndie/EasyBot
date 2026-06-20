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
| **Telegram** | ✅ I/O | ✅ I/O | ✅ I/O |
| **Discord** | ✅ I/O | ✅ I/O | ✅ I/O |
| **飞书** | ✅ I/O | ✅ I/O | ✅ I/O |
| **QQ** | ✅ I/O | ✅ I/O | ❌ inbound（平台不推送） |
| **个人微信** | ✅ I/O | — 不支持群聊 | — 不支持群聊 |

> I/O = inbound 接收 + outbound 发送均可用。标记为 ❌ 的能力是**平台原生限制**，不在 EasyBot 适配器层面处理。

## 各平台详细说明

### Telegram

- **消息接收方式**: HTTP 长轮询 (`getUpdates`)，30s 超时
- **消息过滤**: 无过滤。所有消息（私信、群聊、频道）均作为 inbound 发布。不区分 @mention
- **Outbound 能力**: Text / Image / Audio / Video / Document / Markdown / HTML / Inline Keyboard / Message Edit / Delete

### Discord

- **消息接收方式**: Gateway WebSocket (intent: `GUILD_MESSAGES`, `DIRECT_MESSAGES`, `MESSAGE_CONTENT`)
- **消息过滤**: 仅过滤 bot 自身发送的消息（按 `author.id`）。群内所有消息均接收，不区分 @mention
- **Outbound 能力**: Text / Markdown / Message Edit / Delete。媒体发送（Image/Audio/Video/Document）和 Interactive 尚未实现

### 飞书

- **消息接收方式**: WebSocket (larksuite SDK `ws_client`)，订阅 `im.message.receive_v1` 事件
- **消息过滤**: 无过滤。私信 (`p2p`) 和群聊 (`group`) 消息均接收
- **Outbound 能力**: Text / Image / Audio / Video / Document / Interactive Card / Markdown / Message Edit / Delete

### QQ

- **消息接收方式**: Gateway WebSocket，intent 为 `AT_MESSAGE`、`C2C_MESSAGE`、`GROUP_AT_MESSAGE`
- **消息过滤**: QQ 平台**仅在用户 @机器人 时才推送消息**。这是 QQBot 平台的 intent 级别限制，非 adapter 过滤。
  - `C2C_MESSAGE`: 私信消息 — 全部接收
  - `AT_MESSAGE`: 频道 @消息 — 仅 @ 消息
  - `GROUP_AT_MESSAGE`: 群聊 @消息 — 仅 @ 消息
- **Self-message 过滤**: 频道消息按 `author.id` 过滤自身；群聊消息无法过滤自身（平台无 bot 字段）
- **Outbound 能力**: Text / Image / Markdown / Message Edit / Delete。音频/视频/文件发送、Interactive 尚未实现

### 个人微信 (iLink Bot API)

- **消息接收方式**: HTTP 长轮询 (`/ilink/bot/getupdates`)，首次需扫码登录
- **群聊支持**: **不支持**。iLink Bot API 仅提供一对一聊天能力
- **Outbound 能力**: Text 已实现。图片/音频/视频/文件发送依赖 AES-128-ECB 加密（尚未实现）

## 各平台 Capability 声明汇总

| Capability | Telegram | Discord | 飞书 | QQ | WeChat |
|---|---|---|---|---|---|
| Text | ✅ | ✅ | ✅ | ✅ | ✅ |
| Image | ✅ | ❌ | ✅ | ✅ | ❌ |
| Audio | ✅ | ❌ | ✅ | ❌ | ❌ |
| Video | ✅ | ❌ | ✅ | ❌ | ❌ |
| Document | ✅ | ❌ | ✅ | ❌ | ❌ |
| Interactive | ✅ | ❌ | ✅ | ❌ | ❌ |
| Markdown | ✅ | ✅ | ✅ | ✅ | ❌ |
| Html | ✅ | ❌ | ❌ | ❌ | ❌ |
| Group | ✅ | ✅ | ✅ | ✅ | ❌ |
| TypingIndicator | ✅ | ✅ | ❌ | ❌ | ❌ |
| MessageEdit | ✅ | ✅ | ✅ | ✅ | ❌ |
| MessageDelete | ✅ | ✅ | ✅ | ✅ | ❌ |
| Thread | ❌ | ❌ | ❌ | ✅ | ❌ |
| ChatList | ❌ | ❌ | ❌ | ❌ | ❌ |
| Streaming | ❌ | ❌ | ❌ | ❌ | ❌ |

## 设计启示

1. **QQ 是唯一 @mention-only 的平台** — 机器人无法感知群内公共对话，这对需要"监听群内所有消息"的 AI Agent 场景构成限制
2. **Telegram / Discord / 飞书 行为一致** — 这三个平台机器人均能接收群内全部消息，适配器层不做额外过滤，可视为"全量 inbound"模式
3. **个人微信群聊限制** — 如果需要群聊能力，必须改用企业微信或其他方案；个人微信仅适合一对一对话场景
4. **Outbound 媒体差距** — Discord 和 QQ/WeChat 的媒体发送能力不完整，主要受限于网络协议（Discord multipart / WeChat AES 加密）
