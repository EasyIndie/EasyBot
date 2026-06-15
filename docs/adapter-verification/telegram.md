# Telegram 适配器验证指南

验证范围：`easybot-adapter-telegram` crate。

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
curl -s http://127.0.0.1:8080/api/v1/adapters | jq .

# 查看适配器状态
curl -s http://127.0.0.1:8080/api/v1/adapters/telegram/status | jq .

# 停止适配器
curl -s -X POST http://127.0.0.1:8080/api/v1/adapters/telegram/stop | jq .

# 重启适配器
curl -s -X POST http://127.0.0.1:8080/api/v1/adapters/telegram/start | jq .
```

## 关键实现细节

| 属性 | 值 |
|------|-----|
| 连接方式 | **长轮询（Long Polling）**，不支持 Webhook |
| API 基础 URL | `https://api.telegram.org/bot`（硬编码，不可配置） |
| 轮询超时 | 30 秒，HTTP 客户端超时 40 秒 |
| 支持的 parse_mode | `MarkdownV2`、`HTML` |
| 能力声明 | Text、Image、Audio、Video、Document、Interactive、Markdown、HTML、Group、TypingIndicator、MessageEdit、MessageDelete |
| 不支持的能力 | ChatList、Streaming、send_media()、send_interactive()、list_chats() |

## 后续改进建议

- [ ] 引入 `wiremock` 或 `mockito` 为 Telegram API 编写 mock 测试，消除网络依赖
  ```bash
  cargo add --dev wiremock -p easybot-adapter-telegram
  ```
- [ ] 将 `TELEGRAM_API` 常量改为可配置项（通过 `AdapterConfig.extra`），方便测试时指向 mock server
- [ ] 补充 `send_media()` 实现（当前返回默认错误）
- [ ] 增加更多消息类型的 convert 测试（图片、视频、文档）
