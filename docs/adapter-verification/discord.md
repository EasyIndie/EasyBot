# Discord 适配器验证指南

验证范围：`easybot-adapter-discord` crate。

## 当前测试现状

| 测试名 | 类型 | 依赖网络 | 需要真实 Token |
|--------|------|---------|---------------|
| `test_init_without_token` | 单元测试 | ❌ | ❌ |
| `test_init_and_connect_without_real_token` | 集成测试 | ✅（预期失败） | ❌ |
| `test_send_message_mocked` | 集成测试 | ✅（预期失败） | ❌ |
| `test_convert_dm_message` | 纯单元测试 | ❌ | ❌ |
| `test_convert_guild_message` | 纯单元测试 | ❌ | ❌ |
| `test_convert_own_message_is_filtered` | 纯单元测试 | ❌ | ❌ |

## 前置条件

### 获取 Discord Bot Token

1. 打开 [Discord Developer Portal](https://discord.com/developers/applications)
2. 点击 **New Application**，输入名称
3. 左侧导航 → **Bot** → **Reset Token**，复制 token
4. 在同一页面开启 **Message Content Intent**（必须，否则收不到消息内容）
5. 左侧导航 → **OAuth2 → URL Generator**：
   - Scopes: 勾选 `bot`
   - Bot Permissions: 勾选 `Send Messages`、`Read Messages/View Channels`、`Read Message History`
   - 复制生成的 URL，在浏览器中打开，将 Bot 拉入你的测试服务器

### 配置

```bash
# 方法 A：环境变量
export DISCORD_BOT_TOKEN="你的bot_token"

# 方法 B：~/.easybot/gateway.local.yaml
cat >> ~/.easybot/gateway.local.yaml << 'EOF'
adapters:
  discord:
    enabled: true
    token: "${DISCORD_BOT_TOKEN}"
EOF
```

> 注意：Discord 不是默认启用的编译特性，需要手动指定：
> ```bash
> cargo run --features adapter-discord -- --debug
> ```
> 或者同时启用所有适配器：`cargo run --features full -- --debug`

### 获取测试用的 channelId

1. 在 Discord 中进入你的测试服务器
2. 右键目标文字频道 → **复制 ID**（需在开发者模式开启：Settings → Advanced → Developer Mode）
3. 或者：在频道中发一条消息，通过 API 查询：

```bash
curl -s -H "Authorization: Bearer <your-api-key>" \
  "http://localhost:8080/api/v1/messages?platform=discord" | jq '.messages[0].chat_id'
```

## 验证方法

### 1. 纯离线单元测试

```bash
cargo test -p easybot-adapter-discord
```

### 2. 全部单元测试（有网络即可）

```bash
cargo test -p easybot-adapter-discord
```

### 3. 端到端验证

#### 3.1 启动服务

```bash
export DISCORD_BOT_TOKEN="你的token"
cargo run --features adapter-discord -- --debug
```

预期日志：
```
INFO  Discord adapter connected: YourBot
INFO  Adapter 'discord' started (connected: true)
```

#### 3.2 适配器管理

```bash
# 查看状态
curl -s -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters | jq .
curl -s -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/discord/status | jq .

# 停止/启动
curl -s -X POST -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/discord/stop | jq .
curl -s -X POST -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/discord/start | jq .
```

#### 3.3 发送文本消息

```bash
# 纯文本（Discord 发送时不支持 parse_mode，客户端自行渲染 Markdown）
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"target": "discord:<channelId>", "text": "Hello from EasyBot!", "parse_mode": "none"}' | jq .
```

**预期**：Discord 频道中看到消息。注意 `parse_mode` 字段在 Discord 发送时被忽略（直接发 `content` 纯文本）。

#### 3.4 发送 typing indicator

```bash
# Discord 支持 typing indicator
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"target": "discord:<channelId>", "text": "/typing"}' > /dev/null
```

> 当前 API 没有直接暴露 typing endpoint，需通过适配器实现判断或手动调用。

#### 3.5 回复已有消息

```bash
# 获取消息 ID
MSG_ID=$(curl -s -H "Authorization: Bearer <key>" \
  "http://localhost:8080/api/v1/messages?platform=discord&limit=1" | jq -r '.messages[0].raw_data.id')

# 回复
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d "{\"target\": \"discord:<channelId>\", \"text\": \"回复消息\", \"reply_to\": \"$MSG_ID\"}" | jq .
```

> 注意：当前 Discord `send()` 实现中 `reply_to` 字段未被映射到 Discord API 的 `message_reference`。这是已知缺失功能。

#### 3.6 消息编辑

```bash
# 先发一条
MSG_ID=$(curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"target": "discord:<channelId>", "text": "原始内容"}' | jq -r '.messageId')

# 编辑
curl -s -X PUT "http://127.0.0.1:8080/api/v1/messages/$MSG_ID" \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"target": "discord:<channelId>", "text": "编辑后的内容"}' | jq .
```

**预期**: Discord 中消息内容被更新。

#### 3.7 消息删除

```bash
curl -s -X DELETE "http://127.0.0.1:8080/api/v1/messages/$MSG_ID" \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"target": "discord:<channelId>"}' | jq .
```

**预期**: Discord 中消息消失。

### 4. 入站消息验证

#### 4.1 私聊消息

向 Bot 发送一条私信（Direct Message），然后：

```bash
curl -s -H "Authorization: Bearer <key>" \
  "http://localhost:8080/api/v1/messages?platform=discord&limit=1" | jq '.messages[0].raw_data'
```

**预期字段**：
- `platform`: `"discord"`
- `chat_type`: `"Dm"`
- `is_group`: `false`
- `author.name`: 你的 Discord 用户名
- `text`: 你发送的内容

#### 4.2 频道消息

在已将 Bot 加入的频道中发消息，然后查询：

```bash
curl -s -H "Authorization: Bearer <key>" \
  "http://localhost:8080/api/v1/messages?platform=discord&limit=1" | jq '.messages[0].raw_data'
```

**预期字段**：
- `chat_type`: `"Group"`
- `is_group`: `true`
- `chat_id`: 频道 ID

#### 4.3 Bot 自消息过滤

通过 API 发送一条消息，确认它不会作为入站消息出现（Bot 自己的消息不会被 `MESSAGE_CREATE` 重新推送到 EventBus）。

### 5. WebSocket 实时推送

```bash
# 安装 wscat（如未安装）
npm install -g wscat

# 连接（需要 Authorization header）
wscat -c "ws://localhost:8080/api/v1/ws" \
  -H "Authorization: Bearer <key>"
```

连接后发送 auth：
```json
{"token": "<your-api-key>"}
```

在 Discord 中收发消息时，应看到 `message.inbound` 和 `message.sent` 事件推送。

### 6. 错误场景

| 场景 | 操作 | 预期 |
|------|------|------|
| 无效 target 格式 | `target: "invalid"` | `INVALID_REQUEST` |
| 不存在的 platform | `target: "unknown:123"` | `ADAPTER_NOT_CONNECTED` |
| 无效 channelId | `target: "discord:0"` | `"Internal error: ... 10003 Unknown Channel"` |
| 未认证请求 | 不传 Authorization | 401 `AUTH_FAILED` |
| 无效 API Key | `Bearer: wrong_key` | 401 `AUTH_FAILED` |

## 关键实现细节

| 属性 | 值 |
|------|-----|
| 连接方式 | **Gateway WebSocket**（`wss://gateway.discord.gg/?v=10&encoding=json`）|
| REST API | `https://discord.com/api/v10` |
| 鉴权方式 | HTTP Header `Authorization: Bot <token>` |
| 默认 Intents | `GUILD_MESSAGES \| DIRECT_MESSAGES \| MESSAGE_CONTENT` |
| 心跳间隔 | 由 Gateway Hello 事件指定 |
| 自动重连 | ❌ 未实现（loop 退出后不再重试） |
| parse_mode 支持 | ❌ 忽略（Discord 不支持，客户端自行渲染 Markdown） |
| 入站消息过滤 | 按 `author.id == bot_user_id` 过滤自身消息 |
| 能力声明 | Text、Markdown、Group、TypingIndicator、MessageEdit、MessageDelete |
| 不支持的能力 | Html、Interactive、Image、Audio、Video、Document、ChatList、Streaming |

## 与 Telegram 适配器的关键差异

| 特性 | Telegram | Discord |
|------|---------|---------|
| 连接方式 | 长轮询（getUpdates） | Gateway WebSocket |
| 心跳 | 无（HTTP 轮询天然无心跳） | Gateway 心跳（Hello 指定间隔） |
| 自动重连 | 轮询失败 5 秒后重试 | 未实现（需手动 restart） |
| parse_mode | 支持 `markdown`/`html`/`none` | 不支持（直接发纯文本） |
| reply_to 支持 | ✅ 完整（`reply_to_message_id`） | ❌ `message_reference` 未映射 |
| chat_name 来源 | `chat.title` 或 `chat.first_name` | 查询时不可用（需额外 API 查询） |
| 发送 typing | `sendChatAction` | `POST /channels/{id}/typing` ✅ |
| 自身消息过滤 | ❌ 未实现（无 bot_user_id 检查） | ✅ 按 `author.id` 过滤 |

## 后续改进建议

- [ ] 引入 `wiremock` 或 `mockito` 为 Discord REST API 编写 mock 测试
- [ ] 实现 `reply_to` 字段到 Discord `message_reference` API 参数的映射
- [ ] 实现 Gateway 自动重连（当前收到 RECONNECT/INVALID_SESSION 后 loop 退出）
- [ ] 支持更多 Intents 从配置读取（当前硬编码）
- [ ] 补充 `chat_name` 的获取（从 Gateway Ready 的 guild 信息或独立查询）
- [ ] 增加更多 Gateway 事件类型的处理（MESSAGE_UPDATE、MESSAGE_DELETE 等）
