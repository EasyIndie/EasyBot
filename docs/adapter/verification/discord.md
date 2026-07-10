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

### 平台机器人与凭证获取

#### Discord Bot 创建步骤（完整流程）

1. **创建 Application**
   - 打开 [Discord Developer Portal](https://discord.com/developers/applications)
   - 点击 **New Application**，输入名称（如 `MyEasyBot`）
   - 创建后会自动进入应用设置页

2. **创建 Bot 并获取 Token**
   - 左侧导航 → **Bot**
   - 点击 **Add Bot** → 确认
   - 在 **Token** 区域点击 **Reset Token** → 复制 token（格式示例：`YOUR_BOT_TOKEN_HERE.xxxxxxxxxxxxx`）
   - ⚠️ **务必开启 Message Content Intent**（开关在 TOKEN 下方），否则收不到消息内容

3. **邀请 Bot 到测试服务器**
   - 左侧导航 → **OAuth2 → URL Generator**
   - **Scopes**: 勾选 `bot`
   - **Bot Permissions**: 勾选以下权限：
     - `Send Messages`
     - `Read Messages/View Channels`
     - `Read Message History`
   - 复制页面底部生成的 **URL**
   - 在浏览器中打开该 URL → 选择你的测试服务器 → 授权

4. **开启开发者模式**（用于获取 ID）
   - Discord 应用 → 左下角齿轮 ⚙️ → **Advanced**
   - 打开 **Developer Mode**
   - 右键任意频道、用户或服务器 → **Copy ID**

#### 获取测试用的 channelId

1. 在 Discord 中进入你的测试服务器
2. 右键目标文字频道 → **复制 ID**（频道 ID 通常是纯数字，如 `1101910610033250468`）
3. 或者：在频道中发一条消息，通过 API 查询：

```bash
curl -s -H "Authorization: Bearer <your-api-key>" \
  "http://localhost:8080/api/v1/messages?platform=discord" | jq '.messages[0].chat_id'
```

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

> 注意：Discord 默认包含在 `default` feature 中，无需手动指定。
> ```bash
> cargo run -- --debug
> ```
> 如果希望通过最小构建测试 Discord，可使用：`cargo run --features adapter-discord -- --debug`

### 常见调试问题

| 问题 | 原因 | 解决方法 |
|------|------|---------|
| Gateway 连接失败 `invalid peer certificate` | rustls 未配置 CryptoProvider | 已修复（使用 aws-lc-rs + webpki-roots） |
| 用户消息收不到 | Message Content Intent 未开启 | 在 Bot 设置页面打开开关 |
| 自身消息也收到 | 未过滤 bot 自身消息 | 已修复（按 author.id == bot_user_id 过滤） |
| DELETE 消息返回 500 | 204 No Content 无法解析 JSON | 已修复（直接处理空响应） |
| 消息内容为空 | Message Content Intent 未开启 | 检查 Bot 页面 intent 开关 |

## 验证结果

### 已验证功能清单

| 验证项 | 状态 | 说明 |
|--------|------|------|
| **适配器管理** | | |
| 自动启动 | ✅ | REST API 验证 token → Gateway WebSocket 连接 |
| 停止适配器 | ✅ | `POST /adapters/discord/stop` → `Stopped`, `connected: false` |
| 重启适配器 | ✅ | `POST /adapters/discord/start` → `Connected`, `connected: true` |
| **出站消息** | | |
| 纯文本发送 | ✅ | `POST /messages/send` → Discord 中收到消息 |
| 消息编辑 | ✅ | `PUT /messages/{id}` → `ok: true` |
| 消息删除 | ✅ | `DELETE /messages/{id}` → `ok: true`（修复 204 空 body 解析后） |
| Typing Indicator | ✅ | `POST /channels/{id}/typing`（通过适配器实现） |
| **入站消息** | | |
| DM 消息接收 | ✅ | 用户"烛龙一现"的私信正确解析（修复 `bot` 字段后） |
| 频道消息接收 | ✅ | `is_group: true`, `chat_type: Group` |
| 自身消息过滤 | ✅ | `convert_message` 返回 `None` 当 `author.id == bot_user_id` |
| **WebSocket 推送** | | |
| 出站事件推送 | ✅ | `message.sent` 事件实时推送到 WS 客户端 |
| 入站事件推送 | ✅ | `message.inbound` 事件实时推送到 WS 客户端 |
| **错误处理** | | |
| 无效 channelId | ✅ | 返回 `"Unknown Channel" (404)` |
| 未认证请求 | ✅ | 返回 401 `AUTH_FAILED` |
| 无效 API Key | ✅ | 返回 401 `AUTH_FAILED` |

### 验证中发现并修复的问题

| Bug | 修复 | 文件 |
|-----|------|------|
| `tokio-tungstenite` TLS 未编译 | 添加 `__rustls-tls` + `connect` features | `Cargo.toml` |
| Rustls 0.23 CryptoProvider 未配置 | 调用 `aws_lc_rs::default_provider().install_default()` | `lib.rs` |
| Gateway 证书验证失败 | 手动 TLS 连接器 + `webpki-roots` 根证书 | `lib.rs` |
| 用户消息 MESSAGE_CREATE 解析失败 | `DiscordUser.bot` 改为 `Option<bool>` + `#[serde(default)]` | `types.rs` |
| DELETE 返回 204 No Content 解析失败 | 重写 `delete_message` 直接处理空响应 | `lib.rs` |

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

> `reply_to` 字段已映射到 Discord API 的 `message_reference`，支持回复消息引用。

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
| 自动重连 | ✅ 外层 loop 无限重试（5s 延迟，支持 cancel 信号） |
| parse_mode 支持 | ❌ 忽略（Discord 不支持，客户端自行渲染 Markdown） |
| 入站消息过滤 | 按 `author.id == bot_user_id` 过滤自身消息 |
| 能力声明 | Text、Image、Audio、Video、Document、Interactive、Markdown、Group、TypingIndicator、MessageEdit、MessageDelete、ChatList、Streaming |
| 不支持的能力 | Html |
| 能力限制 | Image/Audio/Video/Document: 最大 8MB；Interactive: 最多 25 个按钮 |

## 与 Telegram 适配器的关键差异

| 特性 | Telegram | Discord |
|------|---------|---------|
| 连接方式 | 长轮询（getUpdates） | Gateway WebSocket |
| 心跳 | 无（HTTP 轮询天然无心跳） | Gateway 心跳（Hello 指定间隔） |
| 自动重连 | 轮询失败 5 秒后重试 | ✅ 外层 loop 无限重连（5s） |
| parse_mode | 支持 `markdown`/`html`/`none` | 不支持（直接发纯文本） |
| reply_to 支持 | ✅ 完整（`reply_to_message_id`） | ✅ `message_reference` API |
| chat_name 来源 | `chat.title` 或 `chat.first_name` | 查询时不可用（需额外 API 查询） |
| 自身消息过滤 | ❌ 未实现（无 bot_user_id 检查） | ✅ 按 `author.id` 过滤 |
| 发送 typing | ✅ `sendChatAction` | ✅ `POST /channels/{id}/typing` |

## 后续改进建议

- [ ] 引入 `wiremock` 或 `mockito` 为 Discord REST API 编写 mock 测试
- [x] ~~实现 `reply_to` 字段到 Discord `message_reference` API 参数的映射~~ — 已完成
- [x] ~~重构 `gateway_shard_loop` 与 `handle_gateway_event` 的双路径问题（`MessageCreate` 在生产路径中被拦截，`handle_gateway_event` 中的分支为死代码——已清理死代码分支，`MessageCreate` 仅在外层 `gateway_shard_loop` 处理）~~
- [ ] 支持更多 Intents 从配置读取（当前硬编码）
- [x] ~~补充 `chat_name` 的获取（从 GuildCreate/GuildUpdate 事件缓存 guild 名称；DM 使用 author.name）~~
- [ ] 增加更多 Gateway 事件类型的处理（MESSAGE_UPDATE、MESSAGE_DELETE 等）
