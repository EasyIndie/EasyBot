# OrzMC 接入实战（Minecraft 服务端 × 群消息与 $ 指令）

> 本文是 **EasyBot 的第一个完整外部客户端接入案例**，以开源 Minecraft 插件
> [OrzMC](https://github.com/OrzMC/OrzMCPlugin) 的真实生产接入为蓝本，演示：
> 部署 → API Key 与 Target 授权 → `easybot.yml` 配置 → REST 出站 + WebSocket 事件流
> → 群消息通知与 `$` 群指令的完整链路。
>
> 读者对象：想把自家服务（游戏服务器、监控告警、运维机器人）接入 EasyBot 的
> 应用开发者。文中所有协议细节均可直接照抄；协议本身的完整规范见
> [01 user-guide.md](01%20user-guide.md) 与 [10 message-idempotency.md](10%20message-idempotency.md)。

## 1. 接入总览

OrzMC 只消费 EasyBot 的四个能力，不部署任何额外的 IM 网关：

```
IM 平台 (QQ / Telegram / Discord / Feishu)
        │  EasyBot 各平台适配器（官方协议，出站长连接/轮询）
        ▼
   EasyBot 网关 ── WebSocket 事件流 ──► OrzMC（收 $ 指令 / 群消息）
        ▲                                   │
        └──────── REST send / batch-send ───┘（发通知 / 回复）

OrzMC 配置三要素：
  api_server  REST 基地址（默认 http://127.0.0.1:8080）
  ws_server   WebSocket 地址（默认 ws://127.0.0.1:8080）
  api_key     客服类 API Key（管理后台创建，见 §3）
```

OrzMC 侧不感知平台差异：入站归一为「消息文本 + 发送者角色 + 来源会话」，出站只分
「PUBLIC（公开群）/ PRIVATE（管理员私聊）」两类目标——平台差异全部由 EasyBot 屏蔽。

## 2. 前置条件

| 项 | 说明 |
|:--|:--|
| EasyBot | 已部署并可通过 `http://<host>:8080/api/v1/live` 探活（部署见 [01 user-guide.md](01%20user-guide.md)） |
| IM 平台应用 | 按 [01 user-guide.md §5](01%20user-guide.md) 在对应平台创建机器人并取得令牌（QQ `QQ_APP_ID/QQ_CLIENT_SECRET`、Telegram Bot Token、Discord Bot Token、飞书 `FEISHU_APP_ID/FEISHU_APP_SECRET`） |
| OrzMC | 1.0.x 任意版本（EasyBot 为外部进程、无需插件依赖声明；`easybot.yml` 未配置时仅群功能禁用，游戏本体不受影响） |

## 3. 管理后台：API Key 与 Target 授权

EasyBot 的鉴权模型分两层（详见 [01 user-guide.md §7.1](01%20user-guide.md)）：

1. **接口权限**：API Key 必须拥有 `messages:send`、`websocket:connect` 等接口权限。
   OrzMC 需要的最小接口权限：`MessagesSend`（send/batch-send）+ `WebSocketConnect`（事件流）；
   建议加 `MessagesRead`（投递查询/对账）。
2. **Target 授权（数据范围）**：对每个要收发的会话 `platform:chatId` 授予对应 action。
   OrzMC 的 `admin_group / player_group / admin_dm` 指向的每个会话都需要出现在授权列表，
   否则对应方向的收发会被服务端以 `403` 拒绝（WS 侧则静默丢弃事件）。

后台「创建 API Key → 配置 Target 授权」时，从活跃会话列表勾选目标并勾选 action 即可。
授权是**按稳定 `subject_id` 记录**的：轮换 Key 不影响已配置的数据范围。

## 4. OrzMC 配置（easybot.yml）

OrzMC 的 `config/easybot.yml`（或服务端插件目录下 `easybot.yml`）：

```yaml
config-version: 11
cmd_prompt_char: '$'          # 群指令前缀（默认 $：$w / $l / $r ...）

api_server: 'http://127.0.0.1:8080'   # EasyBot REST 基地址
ws_server:  'ws://127.0.0.1:8080'      # EasyBot WebSocket 地址（内部拼接 /api/v1/ws）
api_key:    'sk-xxxxxxxx'              # 客服类 API Key（§3）

platforms:
  qq:
    enabled: true
    admin_group:  'qq:group:<群 OpenID>'   # 管理群（群主/管理员发管理 $ 指令）
    player_group: ''                        # 玩家群（公开通知；留空降级 admin_group）
    admin_dm:     'qq:user:<用户 OpenID>'   # 管理员私聊（仅下行 PRIVATE 通知）
  telegram:
    enabled: true
    admin_group:  'telegram:<群 ChatID>'
    player_group: ''
    admin_dm:     'telegram:<管理员私聊 ChatID>'
  # discord / feishu 同理：target 一律是平台可直接投递的会话标识
```

### 4.1 会话标识 = 平台原生 ID（重要）

`admin_group / player_group / admin_dm` 的值**不是** EasyBot 分配的别名，而是
**平台可直接投递的会话标识**——服务端拿到 `target` 后只按首个冒号拆分
`platform:chatId`，`chatId` 原样交给对应平台适配器投递。各平台取值：

| 平台 | target 格式 | 说明 |
|:--|:--|:--|
| QQ | `qq:group:<GroupOpenID>` / `qq:channel:<ChannelID>` / `qq:user:<UserOpenID>` | **OpenID 体系**：填群号/QQ 号无效，需取机器人在平台侧分配的 openid（后台会话列表可查）；适配器按 `group:/channel:/user:` 前缀自动识别类型，无前缀时按事件缓存探测 |
| Telegram | `telegram:<ChatID>` | 群为负号 ID 或用户名；私聊为对应用户 ID |
| Discord | `discord:<ChannelID>` | 频道 ID（开发者模式复制）；私聊为 DM 频道 ID |
| 飞书 | `feishu:<ChatID>` | 群设置 → 复制群 ID |

### 4.2 路由语义

- **PUBLIC 消息**（玩家状态、白名单/TNT/安全事件公告）→ 遍历所有启用平台的
  `player_group`，为空则降级 `admin_group`；
- **PRIVATE 消息**（异常告警、管理员专属通知）→ 各平台 `admin_dm`；
- **入站门槛**：只处理来自 `admin_group` / `player_group` / `admin_dm` 的会话消息，
  其余会话事件在插件侧直接丢弃（fail-closed）。

## 5. 客户端协议（OrzMC 已实现，接入方可照抄）

### 5.1 出站：发送文本

`POST {api_server}/api/v1/messages/send`

```json
{ "target": "qq:group:<GroupOpenID>", "text": "服务器重启完成，耗时 12s" }
```

```http
Authorization: Bearer sk-xxxxxxxx
Idempotency-Key: <8-128 ASCII，见下>
Content-Type: application/json
```

成功响应（HTTP 200）：

```json
{ "id": "...", "status": "sent", "messageId": "...", "timestamp": 1780000000000 }
```

广播同一文本到多个会话用 `/api/v1/messages/batch-send`（`targets` 数组，上限 100；
返回逐 target 结果，单个失败不阻塞其余）。**重要**：`POST` 只能在带持久化
`Idempotency-Key` 时安全重试（超时后不得换新 Key 重发，按对账流程结案，
见 [10 message-idempotency.md](10%20message-idempotency.md)）。服务端消息平台调用最长
等待约 15s，批量约 30s——客户端超时预算必须大于该窗口。

### 5.2 出站：文本解析模式

请求体可选 `parse_mode`（`markdown` / `html` / `none`）。OrzMC 当前以纯文本发送
（不传该字段），需要富文本的客户端可自行启用并注意平台转义差异（Telegram 的
Markdown 需转义：`_` `*` `[` `]` `(` `)` `~` 反引号 `` ` `` `>` `#` `+` `-` `=` `|` `{` `}` `.` `!`）。

### 5.3 入站：WebSocket 事件流

连接地址：`{ws_server}/api/v1/ws`。帧协议：

| 阶段 | 客户端发 | 服务端回 |
|:--|:--|:--|
| 1 连接 | 建立 WS | `{"type":"auth_required","message":"..."}`（未认证提示） |
| 2 认证 | `{"token":"sk-xxxxxxxx"}` | `{"type":"auth_ok"}` 或 `{"type":"auth_failed",...}` |
| 3 心跳 | 收到 `{"type":"ping"}` 后回 `{"type":"pong"}` | 每 `heartbeat_interval_secs`（默认 30s）发 `{"type":"ping"}`；两倍间隔收不到 pong 即断开 |
| 4 事件 | — | `{"type":"event","event":"message.inbound","data":{...},"seq":N,"timestamp":N}` |

服务端限制：客户端帧限速 10 帧/秒、单帧 ≤64KB、认证需在连接后 10s 内完成（最多 5
次尝试）。事件按订阅 API Key 的 Target 授权过滤（未授权会话的事件静默丢弃），事件
带单调递增 `seq`——客户端可用它检测丢帧并决定是否需要重连补偿。

### 5.4 入站事件负载（message.inbound）

`data` 为平台归一化后的消息对象，关键字段：

| 字段 | 说明 | 示例 |
|:--|:--|:--|
| `platform` | 来源平台 | `"qq"` |
| `chat_id` | 来源会话标识（与出站 target 同构，可直接回发） | `"group:xxx"` 或 openid |
| `text` | 消息文本 | `"$w 张三"` |
| `sender.id` | 发送者平台 ID | `"<openid>"` |
| `sender.name` | 发送者显示名（群昵称/平台名） | `"服主"` |
| `sender.username` | 平台特有句柄（telegram @username 等，可选） | `"@admin"` |
| `sender.role` | 发送者角色：`Owner` / `Admin` / `Member` / `Bot` / `Anonymous`（见 §6） | `"Admin"` |
| `sender.is_bot` | 是否机器人账号 | `false` |

> **版本演进提示**：字段名以当前网关输出为准——早期版本（0.0.33，OrzMC 现网所连）
> 的 `sender` 曾用 `nickname`（显示名）与 `user_id`（平台 ID）；语义与上表 `name`/`id`
> 一致，升级对接时按本表字段读取即可。**`sender.role` 自始至终不变，是权限判定的唯一依据。**

**回复定向规则**：客户端把回复发回 `platform:chat_id`（同一次事件）即可——这正是
OrzMC 群指令一问一答的实现方式（`$w` 查询 → 原群回复）。

### 5.5 断线重连（生产要求）

EasyBot 侧对 WS 连接超时/无 pong 会主动断开；客户端应实现指数退避重连（OrzMC 用
5s 起步、上限 60s、加抖动，稳定连接 20s 后重置），并在重连后重新走 §5.3 的
认证流程。REST 侧使用 AsyncHttp 风格的重试时，只对可幂等请求重试并遵守 `429` 的
`Retry-After`。

## 6. 管理员与权限语义（决定谁能发 $ 管理指令）

EasyBot 在入站事件中统一输出 `sender.role`（取值 `Owner`/`Admin`/`Member`/`Bot`/`Anonymous`），判定完全来自平台官方数据：

| 平台 | Owner / Admin 判定来源 |
|:--|:--|
| QQ | 群消息事件自带 `author.member_role`（`owner`/`admin`） |
| Telegram | `getChatAdministrators` 的 `creator` / `administrator` |
| Discord | 群主经 `GET /guilds/{id}`；管理员用事件内 `member.permissions` 的 `ADMINISTRATOR` 位 |
| 飞书 | `GET /im/v1/chats/{id}` 的 `owner_id` 与 `user_manager_id_list` |

**私聊（DM）一律无角色**（平台侧不存在群角色概念）。对 OrzMC 的实际影响：

- 管理指令（`$a` 加白 / `$r` 移白 / `$d` 黑名单 / `$v` 审核 / `$p` 升降级 / `$b` 备份等）
  **只在管理群内可用**：命令处理层要求 `sender.role ∈ {Owner, Admin}`，否则拒绝；
- `admin_dm` 私聊会话用于**下行告警**（服务器异常、上游故障等 PRIVATE 通知）与入站
  门槛放行，不作为管理指令通道；
- 判定失败（role 缺失/未知）按非管理员处理（fail-closed）。

## 7. 端到端流程（最小可用验证）

1. 管理后台：创建 API Key，勾选 `MessagesSend / WebSocketConnect`，授权
   `admin_group` 与 `admin_dm` 两个会话；
2. 启动 EasyBot 与各平台适配器（设置令牌 → 自动启用）；
3. 启动 Minecraft 服务器加载 OrzMC，`config/easybot.yml` 填 §3/§4 的值；
4. 群内发 `$h` → 收到帮助菜单；发 `$w 玩家名` → 收到白名单状态回复（一问一答链路）；
5. 触发一条服务器事件（如玩家上线）→ 群收到通知（PUBLIC 下行链路）；
6. 故意停止 EasyBot → 插件 `$` 命令与通知失效但游戏本体不受影响，恢复后自动重连
   （OrzMC 内置 WS 重连与健康状态；游戏内 `/bot` 命令可查看连接/HTTP/投递健康明细）。

## 8. 已知限制与建议

- **QQ OpenID**：机器人只能感知 openid，无法把群号翻译成 openid；配置会话前先在
  后台会话列表/事件日志中取到 openid（群/用户各一份，且随应用隔离）。
- **飞书 WebSocket 集群单活**：同一飞书应用只随机推送到一个 WS 客户端——多 EasyBot
  实例时必须单实例独占该平台或每个实例注册独立飞书应用（见
  [01 user-guide.md](01%20user-guide.md)）。
- **单帧限制**：入站帧 ≤64KB；群消息文本有长度上限，超长请让 EasyBot 分段或客户端
  自截断（OrzMC 的格式化器会按平台阈值分段发送）。
- **幂等与投递对账**：生产环境应持久化 `Idempotency-Key`，超时后查询
  `/messages/deliveries` 结案，不要盲目重发。

## 附录：OrzMC 群指令一览（前缀 `$`，仅管理群可用）

| 指令 | 功能 | 指令 | 功能 |
|:--|:--|:--|:--|
| `$h` | 帮助 | `$l` | 在线列表 |
| `$w [玩家]` | 白名单查询 | `$a <玩家>` | 加白 |
| `$r <玩家>` | 移白 | `$d` | IP 黑名单管理 |
| `$v` | 审核队列 | `$p` | 升降级 |
| `$b` | 世界备份 | `$o` | 地图优化 |
| `$e <cmd>` | 控制台命令回显 | | |

完整功能清单见 OrzMC 仓库 `docs/features.md`；OrzMC 侧网关对接实现见
`src/main/java/.../infra/bot/`（`OrzEasyBot` / `HttpSender` / `InboundEventParser`）。
