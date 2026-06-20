# Telegram 适配器验证指南

验证范围：`easybot-adapter-telegram` crate。

## 平台机器人与凭证获取

### Telegram Bot 创建步骤

1. 打开 Telegram，搜索 [@BotFather](https://t.me/BotFather)（官方机器人创建工具）
2. 发送 `/newbot` 命令
3. 按提示输入：
   - Bot 名称（如 `MyEasyBot`）— 显示用名称
   - Bot 用户名（如 `MyEasyBot_123_bot`）— 必须唯一，以 `bot` 结尾
4. 创建成功后，BotFather 会返回 **Bot Token**，格式如：
   ```
   1234567890:ABC-DEF1234ghIkl-zyx57W2v1u123ew11
   ```
5. (可选) 通过 BotFather 的 `/setprivacy` 命令关闭 Privacy Mode，让机器人能看到群聊中所有消息（否则只能看到 @mention 的消息）

### 获取测试 Chat ID

有两种方式获取你的 Telegram 用户/群组的 chatId：

**方式 A：通过消息 API**
```bash
# 启动服务后，向机器人发一条消息，然后查询
export TELEGRAM_BOT_TOKEN="你的token"
cargo run -- --debug
# 在另一个终端：
curl -s http://127.0.0.1:8080/api/v1/messages?platform=telegram | jq '.messages[0].chat_id'
```

**方式 B：通过 @userinfobot**
1. 搜索 [@userinfobot](https://t.me/userinfobot)
2. 发送任意消息，它会返回你的 ID
3. 群聊 ID 通常以 `-` 开头（负数）

### 环境变量配置

```bash
export TELEGRAM_BOT_TOKEN="1234567890:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"
```

## 当前测试现状

| 测试名 | 类型 | 依赖网络 | 需要真实 Token |
|--------|------|---------|---------------|
| `test_init_without_token` | 单元测试 | ❌ | ❌ |
| `test_init_and_connect_without_real_token` | 集成测试 | ✅（预期失败） | ❌ |
| `test_send_message_mocked` | 集成测试 | ✅（预期失败） | ❌ |
| `test_convert_message` | 纯单元测试 | ❌ | ❌ |

## 验证方法

### 1. 纯离线单元测试（最快）

```bash
# 只跑不依赖网络的测试
cargo test -p easybot-adapter-telegram -- test_convert_message --exact --nocapture
cargo test -p easybot-adapter-telegram -- test_init_without_token --exact
```

### 2. 全部单元测试（需要外网，不需要 Token）

```bash
cargo test -p easybot-adapter-telegram
```

`test_init_and_connect_without_real_token` 和 `test_send_message_mocked` 会向 `api.telegram.org` 发送真实 HTTP 请求，但使用无效 token，预期 API 返回错误并断言失败结果。不会挂死，但可能因网络超时耗时稍长。

### 3. 端到端验证——真实 Bot

需要：一个来自 [@BotFather](https://t.me/BotFather) 的 Telegram Bot Token。

#### 3.1 配置 Token

```bash
# 方法 A：环境变量
export TELEGRAM_BOT_TOKEN="123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"

# 方法 B：编辑本地配置覆盖（不上传版本控制）
cat >> ~/.easybot/gateway.local.yaml << 'EOF'
adapters:
  telegram:
    enabled: true
    token: "${TELEGRAM_BOT_TOKEN}"
EOF
```

#### 3.2 启动服务

```bash
cargo run -- --debug
```

预期日志输出：

```
INFO  Telegram adapter connected: YourBot (@your_bot_username)
```

#### 3.3 验证健康状态

```bash
curl -s http://127.0.0.1:8080/api/v1/health | jq .
```

预期返回中 `adapters` 字段包含 `telegram` 状态为 `Connected`。

#### 3.4 向 Bot 发送消息并查看

在 Telegram 中向你的 Bot 发送一条消息，然后：

```bash
curl -s http://127.0.0.1:8080/api/v1/messages | jq .
```

预期能看到收到消息的记录。

> 获取你的 chatId：发送消息后查看 `/api/v1/messages` 响应中的 `chat_id`；或使用 `@userinfobot` 查询。

#### 3.5 通过 API 回复消息

```bash
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "Hello from EasyBot API!"}' | jq .
```

预期返回 `{"success": true, "message_id": "..."}`，Telegram 中收到 Bot 的回复。

#### 3.6 验证更多消息格式

```bash
# Markdown 格式
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "*bold* _italic_ `code`", "parse_mode": "Markdown"}' | jq .

# HTML 格式
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "<b>bold</b> <i>italic</i>", "parse_mode": "HTML"}' | jq .

# 回复已有消息
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "这是回复", "reply_to": "<messageId>"}' | jq .
```

#### 3.7 验证消息编辑/删除

```bash
# 先发一条消息
MSG=$(curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "待编辑的消息"}')
MSG_ID=$(echo "$MSG" | jq -r '.message_id')

# 编辑消息
curl -s -X PUT "http://127.0.0.1:8080/api/v1/messages/$MSG_ID" \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "编辑后的消息"}' | jq .

# 删除消息
curl -s -X DELETE "http://127.0.0.1:8080/api/v1/messages/$MSG_ID" \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>"}' | jq .
```

#### 3.8 验证 API Key 鉴权（如果已启用）

```bash
# 如果 gateway.yaml 中 api.key 已配置，请求需要携带 Authorization 头
curl -s -H "Authorization: Bearer <your-api-key>" http://127.0.0.1:8080/api/v1/health | jq .
```

### 4. 运行参数验证

```bash
# 查看帮助
cargo run -- --help

# 初始化配置目录
cargo run -- --init --dir /tmp/easybot-test

# 重复初始化（应提示"already initialized"）
cargo run -- --init --dir /tmp/easybot-test

# 查看版本
cargo run -- --version
```

### 5. 适配器管理 API

```bash
# 查看适配器列表
curl -s -H "Authorization: Bearer <your-api-key>" http://127.0.0.1:8080/api/v1/adapters | jq .

# 查看适配器状态
curl -s -H "Authorization: Bearer <your-api-key>" http://127.0.0.1:8080/api/v1/adapters/telegram/status | jq .

# 停止适配器
curl -s -X POST -H "Authorization: Bearer <your-api-key>" http://127.0.0.1:8080/api/v1/adapters/telegram/stop | jq .

# 重启适配器
curl -s -X POST -H "Authorization: Bearer <your-api-key>" http://127.0.0.1:8080/api/v1/adapters/telegram/start | jq .
```

### 6. 消息双向收发验证

> 所有 API 请求需要携带 `Authorization: Bearer <your-api-key>` header。

#### 6.1 出站消息（API → Telegram）

```bash
# 6.1.1 纯文本（parse_mode 默认 none，不需要显式指定）
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "Hello from EasyBot!"}' | jq .

# 6.1.2 Markdown 格式（API 接收小写枚举值）
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "*bold* _italic_ `code`", "parse_mode": "markdown"}' | jq .

# 6.1.3 HTML 格式
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "<b>bold</b> <i>italic</i>", "parse_mode": "html"}' | jq .

# 6.1.4 回复已有消息
MSG_ID=$(curl -s http://127.0.0.1:8080/api/v1/messages?platform=telegram&limit=1 \
  | jq -r '.messages[0].raw_data.id')
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d "{\"target\": \"telegram:<chatId>\", \"text\": \"回复消息\", \"reply_to\": \"$MSG_ID\"}" | jq .

# 6.1.5 平台特有 metadata（合并到 Telegram API 请求 body）
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "无预览", "metadata": {"disable_web_page_preview": true}}' | jq .

# 6.1.6 批量发送
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/batch-send \
  -H "Content-Type: application/json" \
  -d '{"targets": ["telegram:<chatId>"], "text": "批量消息", "parse_mode": "none"}' | jq .
```

**预期返回格式：**

```json
// 发送成功
{"id": "4", "messageId": "4", "status": "sent", "timestamp": 1781487256000}

// 发送失败
{"id": null, "messageId": null, "status": "failed",
 "error": "Internal error: Telegram API error: ..."}
```

#### 6.2 入站消息（Telegram → API）

在 Telegram 中向 Bot 发消息后，通过以下方式查看：

```bash
# 查看消息历史（按平台筛选）
curl -s "http://127.0.0.1:8080/api/v1/messages?platform=telegram" | jq '.messages[].raw_data'

# 查看最近一条消息的完整字段
curl -s "http://127.0.0.1:8080/api/v1/messages?platform=telegram&limit=1" | jq '.messages[0].raw_data'
```

**入站消息 `raw_data` 字段映射：**

| 字段 | 来源 | 示例 |
|------|------|------|
| `id` | Telegram `message_id` | `"11"` |
| `platform` | 硬编码 | `"telegram"` |
| `chat_id` | `chat.id` | `"5668266914"` |
| `chat_name` | `chat.title ?? chat.first_name` | `"joker"` |
| `chat_type` | `chat.chat_type` 映射 | `"Dm"` / `"Group"` / `"Channel"` |
| `text` | `text ?? caption` | `"/start 开启旅程"` |
| `author.id` | `from.id` | `"5668266914"` |
| `author.name` | `from.first_name` | `"joker"` |
| `author.is_bot` | `from.is_bot` | `false` |
| `timestamp` | `date * 1000`（毫秒） | `1781487534000` |
| `command` | 检测 `/cmd args` 模式 | `{"name":"start","args":"开启旅程"}` |
| `reply_to` | `reply_to_message` | `{"message_id":"9","text":"批量消息测试"}` |
| `is_group` | `chat.chat_type != "private"` | `false` |

#### 6.3 消息编辑和删除

```bash
# 发一条消息
MSG_ID=$(curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "原始内容", "parse_mode": "none"}' | jq -r '.messageId')

# 编辑
curl -s -X PUT "http://127.0.0.1:8080/api/v1/messages/$MSG_ID" \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>", "text": "编辑后的内容"}' | jq .

# 删除
curl -s -X DELETE "http://127.0.0.1:8080/api/v1/messages/$MSG_ID" \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:<chatId>"}' | jq .
```

#### 6.4 WebSocket 实时推送

```bash
# 安装 wscat
npm install -g wscat

# 连接 WebSocket（需要 Authorization header）
wscat -c "ws://localhost:8080/api/v1/ws" \
  -H "Authorization: Bearer <your-api-key>"
```

连接后，发送 auth token：
```json
{"token": "<your-api-key>"}
```

预期收到 `{"type": "auth_ok"}`。之后在 Telegram 中收发消息时，会实时推送事件：

```json
{"type": "event", "event": "message.inbound", "data": {...}, "seq": 1, "timestamp": ...}
{"type": "event", "event": "message.sent",    "data": {...}, "seq": 2, "timestamp": ...}
```

#### 6.5 错误场景

| 场景 | 请求 | 预期响应 |
|------|------|---------|
| 无效 target 格式 | `"target": "invalid"` | `INVALID_REQUEST` |
| 不存在的 platform | `"target": "unknown:123"` | `ADAPTER_NOT_CONNECTED` |
| 无效 chatId | `"target": "telegram:-1"` | `"chat not found"` |
| 未认证 | 不传 Authorization header | 401 `AUTH_FAILED` |
| 无效 API Key | `Authorization: Bearer wrong_key` | 401 `AUTH_FAILED` |

## 关键实现细节

| 属性 | 值 |
|------|-----|
| 连接方式 | **长轮询（Long Polling）**，不支持 Webhook |
| API 基础 URL | `https://api.telegram.org/bot`（硬编码，不可配置） |
| 轮询超时 | 30 秒，HTTP 客户端超时 40 秒 |
| getUpdates 参数 | `offset`, `timeout: 30`, `allowed_updates: ["message"]` |
| 默认 parse_mode | `None`（纯文本，不触发 MarkdownV2 转义） |
| API parse_mode 枚举值 | 小写：`"none"` / `"markdown"` / `"html"` |
| Telegram parse_mode 映射 | `markdown` → `MarkdownV2`，`html` → `HTML` |
| 出站消息持久化 | `message.sent` 事件 → MessagePersister → SQLite |
| 入站消息持久化 | `message.inbound` 事件 → MessagePersister → SQLite |
| WebSocket 事件推送 | 7 种事件类型，100ms 发送超时，50 次连续丢弃断开 |
| 认证方式 | HTTP Header `Authorization: Bearer <api-key>` |
| 能力声明 | Text、Image、Audio、Video、Document、Interactive、Markdown、HTML、Group、TypingIndicator、MessageEdit、MessageDelete |
| 不支持的能力 | ChatList、Streaming、list_chats() |

## 后续改进建议

- [ ] 引入 `wiremock` 或 `mockito` 为 Telegram API 编写 mock 测试，消除网络依赖
  ```bash
  cargo add --dev wiremock -p easybot-adapter-telegram
  ```
- [ ] 将 `TELEGRAM_API` 常量改为可配置项（通过 `AdapterConfig.extra`），方便测试时指向 mock server
- [ ] 增加更多消息类型的 convert 测试（图片、视频、文档）
- [ ] 为 AdapterManager 补充更多测试（start_all、stop_all、list_statuses 等）
  - 已添加：`test_stop_updates_status_cache`、`test_start_passes_config_to_adapter`
- [ ] API 路由层集成测试（启动 Server → HTTP 调用 adapter start/stop/消息接口）
  - 已修复：`test_cli_short_flags` 端口冲突问题，`test_openapi_has_security_scheme` 随机端口
- [ ] WebSocket 推送的集成测试（需要 Node.js `ws` 或 Python `websockets` 支持）
