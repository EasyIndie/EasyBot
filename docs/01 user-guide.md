# EasyBot 用户手册

> **统一多平台即时消息网关** — 一套 API 连接所有 IM 平台。

---

## 1. 产品概述

### 1.1 这是什么？

**EasyBot** 是一个轻量、高性能的即时消息网关服务。它将多个 IM 平台的 API 差异封装在统一的接口背后，只需学习**一套 API**，即可向 Telegram、Discord、飞书、QQ 和微信收发消息。

```
┌─────────────────────────────────────────┐
│           你的应用 / 服务                 │
│      (REST API / WebSocket 客户端)        │
└──────────────────┬──────────────────────┘
                   ↕
┌────────── EasyBot Gateway ─────────────┐
│  统一 API 层 → 核心引擎 → 平台适配器     │
└──┬──────┬──────┬──────┬──────┬─────────┘
   ↕      ↕      ↕      ↕      ↕
 Telegram Discord 飞书   QQ    微信
```

### 1.2 核心能力

| 能力 | 说明 |
|------|------|
| **消息收发** | REST API 发送 + WebSocket 实时接收，支持文本/图片/文件/交互消息 |
| **消息管理** | 编辑、删除已发送消息（平台支持范围内） |
| **批量发送** | 一次请求向多个平台/会话发送相同消息 |
| **会话管理** | 自动追踪跨平台会话，统一管理 |
| **适配器热插拔** | 运行时启停任意平台适配器，不影响其他平台 |
| **热重载配置** | 修改配置文件后每 60 秒自动生效，无需重启 |
| **插件扩展** | 通过动态库加载第三方适配器，无需 fork 主仓库 |

### 1.3 适用场景

- **统一机器人后台** — 一个机器人同时服务 Telegram、Discord、QQ 用户
- **消息广播** — 运营公告一次发送到所有平台
- **消息桥接** — 跨平台消息转发（例如 QQ 群 ↔ Discord 频道）
- **数据分析** — 集中采集多平台消息数据
- **SaaS 集成** — 为 SaaS 产品添加多平台 IM 通知能力

---

## 2. 快速入门：5 分钟上手

### 2.1 前置条件

- [Docker](https://docs.docker.com/get-docker/) + [Docker Compose](https://docs.docker.com/compose/install/)（推荐）
- 或从 [GitHub Releases](https://github.com/EasyIndie/EasyBot/releases) 下载预编译二进制
- 或 [Rust 工具链](https://rustup.rs/) 1.81+（源码构建）
- 一个 Telegram 账号

### 2.2 创建 Telegram Bot

1. 在 Telegram 搜索 [@BotFather](https://t.me/BotFather)，发送 `/newbot`
2. 按提示设置 bot 名称和用户名
3. BotFather 返回 HTTP API Token，格式如 `123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11`
4. **保存此 Token**

### 2.3 启动 EasyBot

```bash
# 克隆项目
git clone https://github.com/EasyIndie/EasyBot.git
cd EasyBot

# 复制环境变量模板
cp .env.example .env

# 编辑 .env，填入 Telegram Token
vim .env

# 启动
docker compose up -d

# 验证运行状态
curl http://localhost:8080/api/v1/health
```

成功响应示例：

```json
{
  "status": "healthy",
  "version": "0.0.14",
  "uptime": 12,
  "adapters": { "total": 1, "connected": 1 },
  "sessions": { "active": 0 }
}
```

### 2.4 发送第一条消息

```bash
# 获取 API Key（首次运行自动生成）
API_KEY=$(cat ~/.easybot/data/.dev_api_key)

# 发送消息
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "telegram:123456789",
    "text": "🎉 Hello from EasyBot!"
  }'
```

> 💡 **获取 Telegram 用户 ID：** 给 Bot 发一条消息，访问 `https://api.telegram.org/bot<你的Token>/getUpdates`，在返回 JSON 中找 `from.id`。

---

## 3. 安装指南

### 3.1 Docker Compose（推荐）

```bash
cp .env.example .env
vim .env                           # 填入令牌
docker compose up -d

# 带 PostgreSQL
docker compose --profile postgres up -d

# 带 PostgreSQL + Prometheus
docker compose --profile postgres --profile monitoring up -d
```

### 3.2 Docker 单容器

```bash
docker build -t easybot .
docker run -d \
  --name easybot \
  -p 8080:8080 \
  -e TELEGRAM_BOT_TOKEN="your_token" \
  -e EASYBOT_HOME=/var/lib/easybot \
  -v easybot_data:/var/lib/easybot/data \
  easybot
```

**环境变量一览：**

| 变量 | 说明 |
|------|------|
| `TELEGRAM_BOT_TOKEN` | Telegram Bot Token |
| `DISCORD_BOT_TOKEN` | Discord Bot Token |
| `FEISHU_APP_ID` | 飞书 App ID |
| `FEISHU_APP_SECRET` | 飞书 App Secret |
| `QQ_APP_ID` | QQ 机器人 App ID |
| `QQ_CLIENT_SECRET` | QQ 机器人 Client Secret |
| `EASYBOT_HOME` | 数据目录（默认 `/var/lib/easybot`） |
| `EASYBOT_ADMIN_PASSWORD` | 管理后台密码（**必设**） |
| `EASYBOT_ALLOW_PLAINTEXT` | 生产环境跳过 TLS 检查 |

### 3.3 从源码构建

```bash
# 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 安装 Protobuf 编译器（飞书适配器依赖）
# macOS: brew install protobuf
# Ubuntu: sudo apt-get install protobuf-compiler

# 克隆并构建
git clone https://github.com/EasyIndie/EasyBot.git
cd EasyBot
cargo build --release

# 初始化和启动
./target/release/easybot --init --dir ~/.easybot
vim ~/.easybot/.env
./target/release/easybot --debug
```

**构建选项：**

| 命令 | 说明 |
|------|------|
| `cargo build --release` | 全量构建（所有 5 平台） |
| `cargo build --release --no-default-features --features "adapter-telegram,adapter-discord"` | 仅构建指定适配器 |
| `cargo build --release --no-default-features` | 最小构建（无适配器） |

### 3.4 预编译二进制

从 [GitHub Releases](https://github.com/EasyIndie/EasyBot/releases) 下载对应平台的二进制：

```bash
# Linux (x86_64)
curl -LO https://github.com/EasyIndie/EasyBot/releases/download/v0.0.14/easybot-x86_64-unknown-linux-musl

# macOS (Apple Silicon)
curl -LO https://github.com/EasyIndie/EasyBot/releases/download/v0.0.14/easybot-aarch64-apple-darwin

chmod +x easybot-*
./easybot-x86_64-unknown-linux-musl --init --dir ~/.easybot
vim ~/.easybot/.env
./easybot-x86_64-unknown-linux-musl --debug
```

> Linux 二进制为 musl 静态编译，不依赖系统动态库。
> 升级只需替换二进制，配置和数据目录不受影响。

---

## 4. 配置指南

### 4.1 目录结构

`easybot --init` 自动创建：

```
~/.easybot/
├── gateway.yaml                # 基础配置（不包含密钥）
├── gateway.local.yaml          # 本地覆盖（.gitignore）
├── .env                        # 令牌文件（chmod 600）
├── data/
│   ├── gateway.db              # SQLite 数据库（自动创建）
│   ├── .dev_api_key            # 开发 API Key（自动生成）
│   └── media_cache/            # 媒体文件缓存
├── logs/                       # 日志文件
├── plugins/                    # 第三方适配器动态库
├── certs/                      # TLS 证书（可选）
├── easybot.sh                  # 服务管理脚本（Linux/macOS）
└── manage-service.ps1          # 服务管理脚本（Windows）
```

### 4.2 配置流程

```
easybot --init  ──→  编辑 .env          ──→  easybot --debug
                      （填入令牌）
```

系统自动检测已设置令牌的平台，自动启用对应适配器。

### 4.3 环境变量（.env）

```bash
# Telegram Bot Token
TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11

# Discord Bot Token
DISCORD_BOT_TOKEN=your_discord_bot_token

# 飞书凭据
FEISHU_APP_ID=cli_xxxxxxxxxxxx
FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxx

# QQ 机器人凭据
QQ_APP_ID=your_app_id
QQ_CLIENT_SECRET=your_client_secret

# 微信 — 扫码登录，无需环境变量
```

**优先级：** Shell export / Docker `environment:` > `.env` 文件 > `gateway.yaml` 默认值

### 4.4 完整配置参考（gateway.yaml）

```yaml
# ── 服务器 ──
server:
  host: "127.0.0.1"              # 监听地址（生产环境改 0.0.0.0）
  port: 8080                     # 监听端口
  adminPassword: ""              # 管理后台密码（建议用 EASYBOT_ADMIN_PASSWORD 环境变量）
  tls:
    enabled: false               # TLS 终止建议由反向代理处理
    certFile: ""
    keyFile: ""

# ── API ──
api:
  basePath: "/api/v1"
  rawPayloadEnabled: false       # WebSocket 事件中是否透传平台原始 payload
  websocket:
    enabled: true
    maxClients: 1000
    heartbeatInterval: 30        # 心跳间隔（秒）
  metrics:
    enabled: true
    path: "/metrics"
  rateLimit:
    enabled: true
    requestsPerMinute: 60
    burstSize: 10

# ── 存储 ──
storage:
  storageType: "sqlite"          # sqlite / postgres
  path: ""                       # 数据库路径（空 = 自动）
  connectionString: ""           # PostgreSQL 连接字符串
  poolSize: 10                   # PostgreSQL 连接池大小
  retention:
    messageTtlDays: 90           # 消息保留天数
    sessionTtlDays: 365          # 会话保留天数
    cleanupIntervalSecs: 3600    # 清理间隔

# ── 日志 ──
logging:
  level: "info"                  # debug / info / warn / error
  format: "text"                 # text / json
  output: "stdout"               # stdout / file

# ── Webhook ──
# webhooks:
#   - name: "my-service"
#     url: "https://example.com/webhook"
#     secret: "your-secret"
#     events: ["message.inbound"]
#     platforms: ["telegram"]
```

### 4.5 本地覆盖（gateway.local.yaml）

```yaml
# 覆盖基础配置，无需修改 gateway.yaml
server:
  host: "0.0.0.0"

adapters:
  telegram:
    apiUrl: "https://api.telegram.org"  # 自定义 Telegram API 地址
  qq:
    sandbox: true                        # QQ 沙箱模式
```

### 4.6 配置优先级

```
CLI 参数 (--dir) → 环境变量 (EASYBOT_HOME) → gateway.local.yaml → gateway.yaml
→ ${VAR_NAME} 替换 → .env 文件 → 代码内建默认值
```

### 4.7 热重载

修改配置文件后，EasyBot **每 60 秒**自动检测并热加载。也可手动触发：

```bash
curl -X PUT http://localhost:8080/api/v1/config \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"logging": {"level": "debug"}}'
```

---

## 5. 平台接入指南

### 5.1 Telegram

**接入步骤：** 通过 [@BotFather](https://t.me/BotFather) 创建 Bot → 获取 Token → 写入 `.env`

```bash
TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11
```

**发送消息：** `target: "telegram:{ChatID}"`

> 可选配置：`adapters.telegram.apiUrl`（自定义 API 地址，用于代理场景）

### 5.2 Discord

**接入步骤：**
1. [Discord Developer Portal](https://discord.com/developers/applications) → New Application → Bot
2. 启用 **MESSAGE CONTENT INTENT**
3. OAuth2 URL Generator → Scopes: `bot` → Permissions: `Send Messages`, `Read Message History`
4. 邀请 Bot 到服务器

```bash
DISCORD_BOT_TOKEN=your_discord_bot_token
```

**发送消息：** `target: "discord:{ChannelID}"`

> 获取 Channel ID：Discord 设置中启用"开发者模式"后，右键频道 → 复制频道 ID

### 5.3 飞书

**接入步骤：**
1. [飞书开放平台](https://open.feishu.cn/app) → 创建企业自建应用
2. 获取 App ID + App Secret
3. 添加权限：`im:message`、`im:resource`、`contact:user.base`
4. 配置事件订阅：`im.message.receive_v1`（WebSocket）
5. 发布应用并等待审批

```bash
FEISHU_APP_ID=cli_xxxxxxxxxxxx
FEISHU_APP_SECRET=your_app_secret
```

**发送消息：** `target: "feishu:{ChatID}"`

> 获取 Chat ID：飞书群聊设置 → 群设置 → 更多 → 复制群 ID

### 5.4 QQ

**接入步骤：**
1. [QQ 开放平台](https://bot.q.qq.com/) → 创建机器人
2. 获取 BotAppID 和 Client Secret
3. （开发阶段）在沙箱配置中添加测试 QQ 号

```bash
QQ_APP_ID=your_app_id
QQ_CLIENT_SECRET=your_secret
```

**发送消息：**

| 会话类型 | target 格式 | 示例 |
|---------|------------|------|
| 群聊 | `qq:group:{GroupID}` | `qq:group:123456` |
| 频道 | `qq:channel:{ChannelID}` | `qq:channel:123456` |
| 私聊 | `qq:user:{UserID}` | `qq:user:123456` |

> 适配器自动识别会话类型，无需手动指定。

### 5.5 个人微信

基于 [iLink Bot](https://www.ilinkbot.com/) 实现，**无需环境变量**，启动后扫码登录。

```bash
# 查看日志中的扫码信息
easybot --debug
# [INFO] easybot_adapter_wechat::adapter 个人微信适配器启动，请扫描屏幕二维码登录
```

**发送消息：** `target: "wechat:{wxid}"`

> ⚠️ 个人微信适配器使用 iLink Bot 长轮询，仅支持一对一私聊。受微信官方限制，可能不稳定。

| 功能 | 支持 |
|------|------|
| 收发文本 | ✅ |
| 图片/文件 | ✅ |
| 编辑/删除 | ❌ 平台不支持 |
| 交互按钮 | ❌ 平台不支持 |

---

## 6. 服务管理与运维

### 6.1 命令行参数

```bash
easybot [OPTIONS]

选项:
  -c, --config <FILE>   配置文件路径
      --dir <DIR>       配置目录路径（默认 ~/.easybot/）
      --init            初始化配置目录并退出
  -d, --debug           调试模式（DEBUG 级别日志）
  -V, --version         显示版本号
  -h, --help            显示帮助信息
```

### 6.2 Makefile 命令

```bash
make           # 显示帮助
make run       # 编译 + 调试启动
make run-fresh # 隔离目录全新启动
make watch     # 自动重编重启（需 cargo-watch）
make test      # cargo test --workspace
make verify    # 完整 CI 检查
```

### 6.3 安装为系统服务

**Linux（systemd）：**
```bash
cd ~/.easybot && sudo ./easybot.sh install
sudo ./easybot.sh status
sudo ./easybot.sh logs
```

**macOS（launchd）：**
```bash
cd ~/.easybot && ./easybot.sh install
./easybot.sh status
```

**Windows（Service）：**
```powershell
cd ~/.easybot
PowerShell -ExecutionPolicy Bypass -File manage-service.ps1 install
```

### 6.4 健康检查

```bash
curl http://localhost:8080/api/v1/health
```

```json
{
  "status": "healthy",
  "version": "0.0.14",
  "uptime": 3600,
  "adapters": { "total": 5, "connected": 3 },
  "sessions": { "active": 42 }
}
```

- `status`: `healthy`（≥1 适配器已连接）/ `degraded`（无适配器连接）
- `adapters.total`: 注册适配器总数
- `adapters.connected`: 已成功连接的适配器数

### 6.5 查看适配器状态

```bash
curl http://localhost:8080/api/v1/adapters \
  -H "Authorization: Bearer $API_KEY"
```

```json
{
  "adapters": [
    {
      "platform": "telegram",
      "display_name": "Telegram",
      "status": "Connected",
      "connected": true
    },
    {
      "platform": "discord",
      "display_name": "Discord",
      "status": "Failed",
      "connected": false
    }
  ]
}
```

详细状态（含统计指标）：
```bash
curl http://localhost:8080/api/v1/adapters/telegram/status \
  -H "Authorization: Bearer $API_KEY"
```

```json
{
  "platform": "telegram",
  "display_name": "Telegram",
  "state": "Connected",
  "connected": true,
  "health": "Healthy",
  "uptime": 3600,
  "messages_in": 256,
  "messages_out": 128,
  "errors": 0,
  "last_error": null
}
```

---

## 7. API 使用指南

### 7.1 获取 API Key

首次启动时自动生成开发用 API Key，保存在 `~/.easybot/data/.dev_api_key`。所有 API 请求需携带 `Authorization: Bearer <api-key>` 请求头。

### 7.2 REST API 完整参考

所有 API 路径以 `/api/v1` 为前缀。

| 路径 | 方法 | 说明 |
|------|------|------|
| `/health` | GET | 健康检查（无需认证） |
| `/system` | GET | 系统信息（CPU、内存） |
| `/adapters` | GET | 适配器列表及状态 |
| `/adapters/{platform}/start` | POST | 启动适配器 |
| `/adapters/{platform}/stop` | POST | 停止适配器 |
| `/adapters/{platform}/status` | GET | 适配器详细状态 |
| `/messages/send` | POST | 发送消息 |
| `/messages/batch-send` | POST | 批量发送 |
| `/messages/{id}` | PUT | 编辑消息 |
| `/messages/{id}` | DELETE | 删除消息 |
| `/messages` | GET | 消息历史 |
| `/sessions` | GET | 会话列表 |
| `/sessions/{key}` | GET | 会话详情 |
| `/sessions/{key}` | DELETE | 删除会话 |
| `/chats/{platform}` | GET | 聊天列表 |
| `/chats/{platform}/{chat_id}` | GET | 聊天详情 |
| `/config` | GET/PUT | 获取/更新配置 |
| `/api-keys` | GET/POST | 列出/创建 API Key |
| `/api-keys/types` | GET | 查看 API Key 类型 |
| `/api-keys/{id}` | DELETE | 吊销 API Key |
| `/api-keys/{id}/purge` | DELETE | 彻底删除 API Key |
| `/metrics` | GET | Prometheus 指标 |
| `/logs` | GET | 实时日志流（环形缓冲 5000 条） |
| `/swagger` | GET | Swagger UI |
| `/openapi.json` | GET | OpenAPI 3.1 Schema |
| `/ws` | GET | WebSocket 实时事件流 |

### 7.3 发送消息

```json
POST /api/v1/messages/send
Authorization: Bearer <api-key>
Content-Type: application/json

{
  "target": "telegram:123456789",
  "text": "Hello!",
  "parse_mode": "markdown",
  "media": {
    "media_type": "Image",
    "url": "https://example.com/image.jpg",
    "caption": "图片说明"
  },
  "keyboard": {
    "rows": [
      {"buttons": [{"text": "按钮1", "callback_data": "btn1"}]}
    ]
  },
  "reply_to": "original_msg_id"
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `target` | string | `{platform}:{chatId}` 格式（必填） |
| `text` | string | 消息文本（必填，除非有 media） |
| `parse_mode` | string | `markdown` / `html` / `none` |
| `media` | object | 媒体附件（media_type: Image/Audio/Video/Document） |
| `keyboard` | object | 行内键盘按钮 |
| `reply_to` | string | 被回复消息 ID |
| `metadata` | object | 平台特有参数 |

**target 格式：**

| 平台 | 格式 | 示例 |
|------|------|------|
| Telegram | `telegram:{ChatID}` | `telegram:123456789` |
| Discord | `discord:{ChannelID}` | `discord:123456789012345678` |
| 飞书 | `feishu:{ChatID}` | `feishu:oc_xxxxxxxxxxxxx` |
| QQ 群聊 | `qq:group:{GroupID}` | `qq:group:123456` |
| QQ 频道 | `qq:channel:{ChannelID}` | `qq:channel:123456` |
| QQ 私聊 | `qq:user:{UserID}` | `qq:user:123456` |
| 微信 | `wechat:{wxid}` | `wechat:wxid_xxxxxxxx` |

**响应：**
```json
{
  "id": "msg_abc123",
  "status": "sent",
  "messageId": "msg_telegram_98765",
  "timestamp": 1718000000000
}
```

### 7.4 批量发送

```json
POST /api/v1/messages/batch-send

{
  "targets": ["telegram:123456", "discord:789012"],
  "text": "群发公告",
  "parse_mode": "markdown"
}
```

**响应：**
```json
{
  "total": 2,
  "results": {
    "telegram:123456": { "status": "sent", "messageId": "msg_1" },
    "discord:789012": { "status": "failed", "error": "chat not found" }
  }
}
```

### 7.5 编辑与删除消息

```bash
# 编辑消息
curl -X PUT http://localhost:8080/api/v1/messages/{message_id} \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:123456", "text": "修改后的消息"}'

# 删除消息
curl -X DELETE http://localhost:8080/api/v1/messages/{message_id} \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:123456"}'
```

> 编辑/删除能力因平台而异（微信不支持，QQ 仅频道消息支持）。

---

## 8. WebSocket 实时事件

### 8.1 连接

```javascript
const ws = new WebSocket('ws://localhost:8080/api/v1/ws');

ws.onopen = () => {
  // 连接成功后发送 JSON 帧认证
  ws.send(JSON.stringify({ token: 'your-api-key' }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  console.log('收到事件:', msg);
};
```

认证成功后客户端需回复心跳 Ping：
```javascript
// 收到服务器 Ping 后回复 Pong
ws.addEventListener('message', (e) => {
  const msg = JSON.parse(e.data);
  if (msg.type === 'ping') {
    ws.send(JSON.stringify({ type: 'pong' }));
  }
});
```

### 8.2 事件格式

```json
{
  "type": "event",
  "event": "message.inbound",
  "data": {
    "platform": "telegram",
    "chat_id": "123456789",
    "text": "Hello!",
    "sender": { "id": "987654", "name": "Alice", "is_bot": false },
    "timestamp": "2026-07-09T11:00:00Z"
  },
  "seq": 1,
  "timestamp": 1720512000000
}
```

### 8.3 事件类型

| 事件 | 说明 |
|------|------|
| `message.inbound` | 收到新消息 |
| `message.sent` | 消息已发送 |
| `message.failed` | 消息发送失败 |
| `adapter.connected` | 适配器已连接 |
| `adapter.disconnected` | 适配器已断开 |
| `adapter.reconnecting` | 适配器正在重连 |
| `adapter.reconnected` | 适配器重连成功 |
| `adapter.reconnect_failed` | 适配器重连失败 |
| `adapter.error` | 适配器异常 |
| `callback.received` | 收到按钮回调 |
| `gateway.started` | 网关启动完成 |
| `gateway.stopping` | 网关正在关闭 |
| `config.changed` | 配置已热重载 |

### 8.4 入站消息数据

```json
{
  "platform": "telegram",
  "chat_id": "123456",
  "chat_type": "Group",
  "message_id": "msg_xxx",
  "thread_id": null,
  "sender": {
    "id": "987654",
    "name": "Alice",
    "username": "alice123",
    "is_bot": false
  },
  "text": "Hello!",
  "mentions": null,
  "mentioned": null,
  "reply_to": null,
  "timestamp": 1720512000000,
  "metadata": null
}
```

### 8.5 原始 Payload 透传

设置 `api.rawPayloadEnabled: true`（或 `EASYBOT_RAW_PAYLOAD_ENABLED=true`），WebSocket 事件会附带 `metadata.raw_payload` 字段（平台原始 JSON）。**仅调试用，默认关闭。**

---

## 9. 管理后台

### 9.1 访问

```
http://localhost:8080/admin
```

### 9.2 设置密码

```bash
export EASYBOT_ADMIN_PASSWORD=your_secure_password
easybot --debug
```

> 未设置密码时管理后台持续拒绝登录，日志输出警告。

### 9.3 功能

- **适配器概览** — 所有平台适配器的实时状态
- **API Key 管理** — 创建、吊销、管理密钥
- **实时日志** — 最近 5000 条运行日志（环形缓冲）
- **系统信息** — CPU、内存、运行时间
- **配置查看** — 当前生效的配置

---

## 10. 生产部署

### 10.1 检查清单

| 检查项 | 说明 |
|--------|------|
| 密钥管理 | 使用环境变量或 Docker secrets，不硬编码 |
| 管理后台密码 | 设置 `EASYBOT_ADMIN_PASSWORD` |
| 数据库 | 生产推荐 PostgreSQL |
| TLS | 配置反向代理（Nginx / Caddy）终止 TLS |
| 监听地址 | `server.host` 改为 `0.0.0.0` |
| 资源限制 | Docker 设置 CPU/内存上限 |
| 日志格式 | 使用 JSON 格式输出 |
| 监控 | 启用 Prometheus 指标采集 |

### 10.2 PostgreSQL

```bash
docker compose --profile postgres up -d
```

```yaml
# gateway.local.yaml
storage:
  storageType: "postgres"
  connectionString: "postgresql://user:password@host:5432/easybot"
  poolSize: 10
```

> 首次启动自动执行数据库迁移，无需手动建表。

### 10.3 反向代理 + TLS

```nginx
server {
    listen 443 ssl;
    server_name easybot.example.com;

    ssl_certificate     /etc/ssl/certs/easybot.crt;
    ssl_certificate_key /etc/ssl/private/easybot.key;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_read_timeout 86400s;
    }
}
```

### 10.4 Prometheus 指标

端点：`/api/v1/metrics`（需认证）

| 指标 | 类型 | 说明 |
|------|------|------|
| `easybot_messages_sent_total` | Counter | 消息发送总数（标签: platform） |
| `easybot_messages_received_total` | Counter | 消息接收总数 |
| `easybot_adapters_connected` | Gauge | 已连接适配器数 |
| `easybot_sessions_active` | Gauge | 活跃会话数 |
| `easybot_http_requests_total` | Counter | HTTP 请求总数 |
| `easybot_http_request_duration_seconds` | Histogram | 请求耗时分布 |

### 10.5 Docker 资源限制

```yaml
deploy:
  resources:
    limits:
      cpus: "2"
      memory: 512M
```

### 10.6 安全加固

- 容器：`cap_drop: ALL`、只读根文件系统、`no-new-privileges`
- 密钥：文件权限 chmod 600、日志输出自动掩码
- 认证：Argon2 哈希存储 API Key

---

## 11. 常见问题

| 问题 | 解决方法 |
|------|----------|
| Docker 构建慢 | 利用缓存挂载，或使用预编译二进制 |
| 适配器未连接 | 检查 `.env` 变量名和令牌有效期，使用 `--debug` 查看日志 |
| 收不到消息 | 确认适配器状态为 `Connected`；确认 WebSocket 已认证；检查平台权限配置 |
| WebSocket 不稳定 | 缩短心跳间隔（`api.websocket.heartbeatInterval: 15`） |
| 切换 SQLite→PostgreSQL | 配置 `storage.storageType: "postgres"`，数据需手动迁移 |
| 更新配置不重启 | 修改文件后等待 60 秒自动生效，或调用 PUT `/api/v1/config` |

---

## 12. 故障排查

### 12.1 日志级别

| 级别 | 颜色 | 用途 |
|------|------|------|
| TRACE | 灰色 | 最详细调试（心跳包等） |
| DEBUG | 蓝色 | 消息收发信息 |
| INFO | 绿色 | 正常运行信息 |
| WARN | 黄色 | 需注意但不影响运行 |
| ERROR | 红色 | 需要处理的错误 |

### 12.2 常见错误

| 错误 | 原因 | 解决 |
|------|------|------|
| `token invalid or expired (11244)` | QQ Token 过期 | 自动刷新（超过 1 次需到 QQ 开放平台重新生成） |
| `401 Unauthorized` | API Key 无效 | 检查 `~/.easybot/data/.dev_api_key` |
| `PrivilegedGatewayIntent` | Discord 未启用 Gateway Intents | 开发者后台开启 MESSAGE CONTENT INTENT |
| 适配器启动失败 | 平台凭据无效 | 检查 `.env` 中的令牌 |
| 生产环境 TLS 检查失败 | Release build 未配置 TLS | 设置 `EASYBOT_ALLOW_PLAINTEXT=true` 或配置反向代理 |

### 12.3 调试命令

```bash
easybot --debug                              # 启用 DEBUG 日志
curl http://localhost:8080/api/v1/logs       # 查看实时日志
curl http://localhost:8080/api/v1/adapters   # 查看适配器状态
```

### 12.4 获取帮助

- **GitHub Issues**: [github.com/EasyIndie/EasyBot/issues](https://github.com/EasyIndie/EasyBot/issues)
- **项目文档**: `docs/` 目录和 [README.md](../README.md)
- **提交 Bug 报告**: 请附带日志、配置（掩码后）和复现步骤

---

## 附录

### A. 快速参考卡

```bash
# 启动
easybot --init                      # 首次初始化
vim ~/.easybot/.env                # 填入令牌
easybot --debug                     # 开发模式
docker compose up -d                # Docker 部署

# 消息发送
API_KEY=$(cat ~/.easybot/data/.dev_api_key)
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:123456", "text": "Hello"}'

# 适配器管理
curl http://localhost:8080/api/v1/adapters \
  -H "Authorization: Bearer $API_KEY"

# 健康检查
curl http://localhost:8080/api/v1/health

# WebSocket 监听
wscat -c ws://localhost:8080/api/v1/ws
> {"token": "YOUR_API_KEY"}
```

### B. 平台凭据速查

| 平台 | 需要什么 | 获取地址 |
|------|---------|----------|
| Telegram | Bot Token | [@BotFather](https://t.me/BotFather) |
| Discord | Bot Token | [Discord Developer Portal](https://discord.com/developers/applications) |
| 飞书 | App ID + App Secret | [飞书开放平台](https://open.feishu.cn/app) |
| QQ | App ID + Client Secret | [QQ 开放平台](https://bot.q.qq.com/) |
| 微信 | 扫码登录 | 启动后扫描屏幕二维码 |

---

*最后更新：2026-07-10 · EasyBot v0.0.14*
