# QQ 适配器验证指南

验证范围：`easybot-adapter-qq` crate。

## 当前测试现状

| 测试名 | 类型 | 依赖网络 | 需要真实凭证 |
|--------|------|---------|-------------|
| `test_create_adapter` | 单元测试 | ❌ | ❌ |
| `test_capabilities` | 单元测试 | ❌ | ❌ |
| `test_status_summary` | 单元测试 | ❌ | ❌ |
| `test_bot_token_uninitialized` | 单元测试 | ❌ | ❌ |
| `test_token_store_new_needs_refresh` | 单元测试 | ❌ | ❌ |
| `test_token_store_get_uninitialized_returns_err` | 单元测试 | ❌ | ❌ |
| `test_token_store_clone` | 单元测试 | ❌ | ❌ |
| `test_init_missing_config` | 单元测试 | ❌ | ❌ |
| `test_init_valid_config` | 单元测试 | ❌ | ❌ |
| `test_qq_user_deserialize_with_bot_field` | 类型反序列化 | ❌ | ❌ |
| `test_qq_user_deserialize_without_bot_field` | 类型反序列化 | ❌ | ❌ |
| `test_channel_message_event_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_channel_message_event_without_guild_id` | 类型反序列化 | ❌ | ❌ |
| `test_group_message_event_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_c2c_message_event_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_c2c_message_event_without_content` | 类型反序列化 | ❌ | ❌ |
| `test_send_message_request_serialize` | 类型序列化 | ❌ | ❌ |
| `test_send_message_response_deserialize` | 类型反序列化 | ❌ | ❌ |

## 前置条件

### 获取 QQ 机器人凭证（QQ 开放平台统一机器人）

1. 登录 [QQ 开放平台](https://q.qq.com/)
2. 创建或选择一个机器人应用
3. 获取 **BotAppID** 和 **AppSecret**

### 配置

```bash
export QQ_APP_ID="你的BotAppID"
export QQ_CLIENT_SECRET="你的AppSecret"
```

```yaml
# ~/.easybot/gateway.local.yaml
adapters:
  qq:
    enabled: true
    token: "${QQ_CLIENT_SECRET}"
    extra:
      app_id: "${QQ_APP_ID}"
```

启动命令：
```bash
cargo run --features full -- --debug
```

### 获取测试用的频道 ID

将机器人添加到你的 QQ 频道或群聊，然后通过 WebSocket Gateway 或 Webhook 接收消息来获取 channelId。

## 验证结果

### 已验证功能清单

| 验证项 | 状态 | 说明 |
|--------|------|------|
| **鉴权（本次新增）** | | |
| `getAppAccessToken` | ✅ | 通过 AppID + clientSecret 获取 access_token |
| REST API `QQBot {token}` 鉴权 | ✅ | `/users/@me` 认证成功 |
| Gateway Identify `QQBot {token}` | ✅ | Gateway Ready 事件收到 |
| Token 定时刷新 | ✅ | 每 3500s 自动刷新 |
| **适配器管理** | | |
| 自动启动 | ✅ | REST API 认证 → Gateway WebSocket 连接 |
| 停止适配器 | ✅ | `POST /adapters/qq/stop` → `Stopped`, `connected: false` |
| 重启适配器 | ✅ | `POST /adapters/qq/start` → `Connected`, `connected: true` |
| **出站消息** | | |
| 群聊消息发送（被动回复） | ✅ | 通过 `reply_to` 传 `msg_id`，使用 `/v2/groups/{openid}/messages` |
| 频道消息发送 | ⬜ TODO | 需提供 QQ 频道测试环境验证（见 TODO） |
| 主动消息发送 | ❌ | QQ 限制（需特殊权限），需通过被动回复方式 |
| **入站消息** | | |
| 群聊 @消息接收 | ✅ | `GROUP_AT_MESSAGE_CREATE` 成功解析存储 |
| 频道 @消息接收 | ⬜ TODO | 代码已实现 `AT_MESSAGE_CREATE` 解析，需端到端验证 |
| C2C 私聊消息接收 | ⬜ TODO | 代码已实现 `C2C_MESSAGE_CREATE` 解析，未验证 |
| 自身消息过滤 | ❌ | 群消息不含 `bot` 字段，需另寻方案 |
| **连接方式** | | |
| Gateway WebSocket | ✅ | 使用 native-tls (系统 CA) |

### 适配器修复清单

| 修复项 | 说明 | 文件 |
|--------|------|------|
| 增加 `QqTokenStore` | 新增 token 管理模块，支持 access_token 获取/刷新/缓存 | `lib.rs` |
| 更换鉴权方式 | REST API: `QQBot {token}`, Gateway: `QQBot {token}` | `lib.rs` |
| 修复 `QqUser.bot` 字段 | 旧响应含 `bot` 字段，新统一平台不含，改为 `Option<bool>` | `types.rs` |
| TLS 修正 | 使用 `native-tls` 替代 `rustls + webpki-roots`（GlobalSign 新 CA 不在 webpki-roots 中） | `Cargo.toml`, `lib.rs` |
| Gateway WebSocket 连接 | 手动建立 DNS→TCP→TLS→WebSocket 连接，支持 `native-tls` | `lib.rs` |
| 配置更新 | `token` 字段现在存储 `clientSecret`（AppSecret），env var 改为 `QQ_CLIENT_SECRET` | `gateway.local.yaml` |

### 验证中发现并修复的问题

| 问题 | 修复 | 文件 |
|------|------|------|
| `Bot {appid}.{token}` 鉴权 401 | 更换为 `getAppAccessToken` + `QQBot {access_token}` | `lib.rs` |
| `/users/@me` JSON 不含 `bot` 字段反序列化失败 | `QqUser.bot` 改为 `Option<bool>` | `types.rs` |
| Gateway rustls 证书验证失败 | 改用 `native-tls`（系统 CA，支持 GlobalSign Atlas R3 CA） | `Cargo.toml`, `lib.rs` |
| `GROUP_AT_MESSAGE_CREATE` 缺少 `channel_id` 字段解析失败 | 拆分为 `QqChannelMessageEvent` / `QqGroupMessageEvent` / `QqC2cMessageEvent` 三种消息结构 | `types.rs` |
| 群聊消息发送使用 `/channels/` 路由返回"频道不存在" | 新增 `try_send()` 自动降级到 `/v2/groups/{openid}/messages` | `lib.rs` |
| 主动消息"无权限"错误 | `send()` 增加 `msg_id` 参数支持被动回复 | `lib.rs` |

## 测试方法

### 1. 纯离线单元测试

```bash
cargo test -p easybot-adapter-qq
```

### 2. 端到端验证

#### 2.1 启动服务

```bash
export QQ_APP_ID="你的BotAppID"
export QQ_CLIENT_SECRET="你的AppSecret"
cargo run --features full -- --debug
```

预期日志：
```
INFO  QQ access token refreshed, expires in 7200s
INFO  QQ adapter connected: YourBot (id=...)
INFO  QQ Gateway ready
```

#### 2.2 适配器管理

```bash
API_KEY=$(curl -s http://127.0.0.1:8080/api/v1/health | jq -r '...')

# 查看所有适配器
curl -s -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters | jq .

# 查看 QQ 状态
curl -s -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/qq/status | jq .

# 停止
curl -s -X POST -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/qq/stop | jq .

# 启动
curl -s -X POST -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/qq/start | jq .
```

### 3. 发送消息

```bash
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"target": "qq:<channelId>", "text": "Hello from EasyBot!", "msg_type": 0}' | jq .
```

> 注意：QQ 频道消息发送需要机器人已在目标频道中，并且需要具备对应权限。

## 关键实现细节

| 属性 | 值 |
|------|-----|
| API 地址 | `https://api.sgroup.qq.com` |
| 鉴权地址 | `https://bots.qq.com/app/getAppAccessToken` |
| REST API 鉴权 | `Authorization: QQBot {access_token}` |
| Gateway Identiy | `"token": "QQBot {access_token}"` |
| Token 有效期 | 7200 秒，提前 60 秒触发刷新 |
| 连接方式 | Gateway WebSocket (`wss://api.sgroup.qq.com/websocket`) |
| TLS 方案 | `native-tls`（系统 CA 证书） |
| 支持的消息类型 | Text (0), Image (2), Markdown |
| 支持的能力 | Text, Image, Markdown, Group, Thread, MessageEdit, MessageDelete |
| 默认 Intents | `AT_MESSAGE | C2C_MESSAGE | GROUP_AT_MESSAGE` |
| 自动重连 | ✅ 外层循环自动重连（同时刷新 token） |
| Token 定时刷新 | ✅ Gateway 事件循环中每 3500s 刷新一次 |

## 后续改进建议

- [ ] **QQ 频道双向消息验证** — 目前只验证了群聊（`GROUP_AT_MESSAGE_CREATE`）。频道消息（`AT_MESSAGE_CREATE`）的解析已实现但未端到端验证，需要将机器人添加到一个 QQ 频道中进行测试
- [ ] 添加入站消息的 `chat_name` 字段填充
- [ ] 补充 `list_chats` 实现（当前返回空列表）
- [ ] 考虑 Docker Alpine 环境下 `native-tls` 需要 OpenSSL 支持
- [ ] 添加更多 Gateway 事件处理（MESSAGE_DELETE、GROUP_AT_MESSAGE_CREATE 等）
