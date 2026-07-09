# EasyBot 用户手册

> **统一多平台即时消息网关** — 一套 API，连接所有 IM 平台。

---

## 目录

1. [产品概述](#1-产品概述)
2. [快速入门：5 分钟上手](#2-快速入门5-分钟上手)
3. [安装指南](#3-安装指南)
4. [配置指南](#4-配置指南)
5. [平台接入指南](#5-平台接入指南)
6. [服务管理与运维](#6-服务管理与运维)
7. [API 使用指南](#7-api-使用指南)
8. [WebSocket 实时事件](#8-websocket-实时事件)
9. [管理后台](#9-管理后台)
10. [生产部署](#10-生产部署)
11. [常见问题](#11-常见问题)
12. [故障排查](#12-故障排查)

---

## 1. 产品概述

### 1.1 这是什么？

**EasyBot** 是一个轻量、高性能的即时消息网关服务。它将多个 IM 平台的 API 差异封装在统一的接口背后，让你只需学习**一套 API**，就能向 Telegram、Discord、飞书、QQ 和微信发送和接收消息。

```
┌─────────────────────────────────────────┐
│           你的应用 / 服务                  │
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
| **消息发送** | 通过 REST API 向任意平台发送文本、图片、文件、交互消息 |
| **消息接收** | 通过 WebSocket 实时接收各平台的入站消息 |
| **消息管理** | 编辑、删除已发送消息（平台支持范围内） |
| **批量发送** | 一次请求向多个平台/会话发送相同消息 |
| **会话管理** | 自动追踪跨平台会话，统一管理 |
| **适配器热插拔** | 运行时启停任意平台适配器，不影响其他平台 |
| **热重载配置** | 修改配置文件后自动生效，无需重启服务 |
| **插件扩展** | 通过动态库加载第三方适配器，无需 fork 主仓库 |

### 1.3 适用场景

- 🤖 **统一机器人后台** — 一个机器人服务同时服务 Telegram、Discord、QQ 用户
- 📢 **消息广播** — 运营公告一次发送到所有平台
- 🔄 **消息桥接** — 跨平台消息转发（例如 QQ 群 ↔ Discord 频道）
- 📊 **数据分析** — 集中采集多平台消息数据
- 🔌 **SaaS 集成** — 为你的 SaaS 产品添加多平台 IM 通知能力

---

## 2. 快速入门：5 分钟上手

以 Docker Compose 部署 + Telegram 平台为例，体验完整流程。

### 2.1 前置条件

- [Docker](https://docs.docker.com/get-docker/) + [Docker Compose](https://docs.docker.com/compose/install/)（推荐）
- 或从 [GitHub Releases](https://github.com/EasyIndie/EasyBot/releases) 下载预编译二进制（无需 Rust 环境）
- 或 [Rust 工具链](https://rustup.rs/) 1.94+（源码构建）
- 一个 Telegram 账号

### 2.2 创建 Telegram Bot

1. 在 Telegram 中搜索 [@BotFather](https://t.me/BotFather)，发送 `/newbot`
2. 按提示设置 bot 名称和用户名
3. BotFather 会返回一个 **HTTP API Token**，格式如 `123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11`
4. **保存这个 Token**，下一步会用到

### 2.3 启动 EasyBot

```bash
# 1. 克隆项目
git clone https://github.com/EasyIndie/EasyBot.git
cd EasyBot

# 2. 复制环境变量模板
cp .env.example .env

# 3. 编辑 .env，填入 Telegram Token（取消注释并填写）
#    用 vim 或其他编辑器打开 .env:
#    TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11
vim .env

# 4. 启动
docker compose up -d

# 5. 验证运行状态
curl http://localhost:8080/api/v1/health
```

如果一切正常，你会看到类似这样的响应：

```json
{
  "status": "ok",
  "version": "0.0.14",
  "uptime_seconds": 12,
  "adapters": {
    "telegram": "connected"
  },
  "sessions": 0
}
```

### 2.4 发送第一条消息

```bash
# 获取 API Key（首次运行自动生成，保存在 data/.dev_api_key）
EASYBOX_API_KEY=$(cat ~/.easybot/data/.dev_api_key)

# 发送消息到你自己
# target 格式: "telegram:{你的Telegram用户ID}"
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $EASYBOX_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "telegram:123456789",
    "text": "🎉 Hello from EasyBot!"
  }'
```

> 💡 **不知道怎么获取 Telegram 用户 ID？** 给刚创建的 Bot 发一条消息，然后访问 `https://api.telegram.org/bot<你的Token>/getUpdates`，在返回的 JSON 中找 `from.id` 字段。

---

## 3. 安装指南

EasyBot 支持三种部署方式，根据你的场景选择最适合的一种。

### 3.1 Docker Compose 部署（推荐）

适合生产环境和日常开发，一键启动全部依赖。

```bash
# 基础部署（EasyBot + SQLite）
cp .env.example .env
vim .env          # 填入你的平台令牌
docker compose up -d

# 带 PostgreSQL（生产推荐）
docker compose --profile postgres up -d

# 带 PostgreSQL + Prometheus 监控
docker compose --profile postgres --profile monitoring up -d
```

**可用配置文件说明：**

| 配置文件 | 挂载路径 | 作用 |
|---------|---------|------|
| `docker-compose.yml` | 根目录 | 主编排文件 |
| `.env`（项目根目录） | — | Docker Compose 自动加载其中的环境变量 |
| `prometheus.yml` | 根目录 | Prometheus 采集配置（使用 `--profile monitoring` 时生效） |

### 3.2 Docker 单容器部署

```bash
# 构建镜像
docker build -t easybot .

# 通过环境变量传入令牌运行
docker run -d \
  --name easybot \
  -p 8080:8080 \
  -e TELEGRAM_BOT_TOKEN="your_token_here" \
  -e EASYBOT_HOME=/var/lib/easybot \
  -v easybot_data:/var/lib/easybot/data \
  easybot
```

**Docker 环境变量说明：**

| 环境变量 | 说明 |
|---------|------|
| `TELEGRAM_BOT_TOKEN` | Telegram Bot Token |
| `DISCORD_BOT_TOKEN` | Discord Bot Token |
| `FEISHU_APP_ID` | 飞书 App ID |
| `FEISHU_APP_SECRET` | 飞书 App Secret |
| `QQ_APP_ID` | QQ 机器人 App ID |
| `QQ_CLIENT_SECRET` | QQ 机器人 Client Secret |
| `EASYBOT_HOME` | 数据目录（默认 `/var/lib/easybot`） |
| `EASYBOT_ADMIN_PASSWORD` | 管理后台密码（必设） |
| `EASYBOT_ALLOW_PLAINTEXT` | 生产环境跳过 TLS 检查（`true`，仅开发环境使用） |

> 📌 **安全建议**：生产环境使用 Docker secrets 或 bind mount 替代环境变量传递令牌。`docker inspect` 可查看容器的环境变量。

### 3.3 从源码构建

适合需要定制编译选项或贡献代码的场景。

```bash
# 1. 安装 Rust 工具链
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup install 1.94

# 2. 安装 Protobuf 编译器（飞书适配器依赖）
# macOS
brew install protobuf
# Ubuntu / Debian
sudo apt-get install protobuf-compiler
# Arch Linux
sudo pacman -S protobuf

# 3. 克隆并构建
git clone https://github.com/EasyIndie/EasyBot.git
cd EasyBot
cargo build --release

# 4. 初始化配置
./target/release/easybot --init --dir ~/.easybot

# 5. 编辑令牌
vim ~/.easybot/.env

# 6. 启动
./target/release/easybot --debug
```

**构建选项：**

| 命令 | 说明 |
|------|------|
| `cargo build --release` | 全量构建（所有 5 个平台） |
| `cargo build --release --no-default-features --features "adapter-telegram,adapter-discord"` | 仅构建指定适配器，减小二进制体积 |
| `cargo build --release --features "default,plugin-system"` | 全量构建 + 插件系统支持 |
| `cargo build --release --no-default-features` | 最小构建（无适配器，仅 API 网关） |

编译后的二进制文件位于 `target/release/easybot`。

### 3.4 预编译二进制

适合不想安装 Rust 工具链或需要快速上手的场景。从 GitHub Releases 直接下载对应平台的二进制文件即可运行。

```bash
# 1. 从 GitHub Releases 下载对应平台的二进制
# 访问 https://github.com/EasyIndie/EasyBot/releases
# 选择最新版本，下载对应平台的二进制文件

# Linux (x86_64)
curl -LO https://github.com/EasyIndie/EasyBot/releases/download/v0.0.14/easybot-x86_64-unknown-linux-musl

# Linux (ARM64, 如树莓派)
curl -LO https://github.com/EasyIndie/EasyBot/releases/download/v0.0.14/easybot-aarch64-unknown-linux-musl

# macOS (Intel)
curl -LO https://github.com/EasyIndie/EasyBot/releases/download/v0.0.14/easybot-x86_64-apple-darwin

# macOS (Apple Silicon M1/M2/M3/M4)
curl -LO https://github.com/EasyIndie/EasyBot/releases/download/v0.0.14/easybot-aarch64-apple-darwin

# Windows (x86_64)
curl -LO https://github.com/EasyIndie/EasyBot/releases/download/v0.0.14/easybot-x86_64-pc-windows-msvc.exe

# 2. 添加执行权限（Linux/macOS）
chmod +x easybot-*

# 3. （可选）验证文件完整性
curl -LO https://github.com/EasyIndie/EasyBot/releases/download/v0.0.14/checksums.txt
sha256sum -c checksums.txt --ignore-missing    # Linux
shasum -a 256 -c checksums.txt --ignore-missing # macOS

# 4. 初始化配置目录
./easybot-x86_64-unknown-linux-musl --init --dir ~/.easybot

# 5. 配置平台令牌
vim ~/.easybot/.env

# 6. 启动
./easybot-x86_64-unknown-linux-musl --debug
```

**使用提示：**

- 建议将重命名的二进制文件（如 `easybot`）加入 `$PATH` 方便调用
- Linux 二进制为静态编译（musl），不依赖系统动态库，可在绝大多数 Linux 发行版上直接运行
- Windows 用户下载 `.exe` 后缀文件，直接在命令行中执行
- 后续版本升级只需替换二进制文件，配置和数据目录不受影响

**各平台二进制说明：**

| 文件 | 平台 | 架构 | 静态编译 |
|------|------|------|---------|
| `easybot-x86_64-unknown-linux-musl` | Linux | x86_64 | ✅ (musl) |
| `easybot-aarch64-unknown-linux-musl` | Linux (ARM) | ARM64 | ✅ (musl) |
| `easybot-x86_64-apple-darwin` | macOS | Intel | — |
| `easybot-aarch64-apple-darwin` | macOS | Apple Silicon | — |
| `easybot-x86_64-pc-windows-msvc.exe` | Windows | x86_64 | — |
| `easybot-aarch64-pc-windows-msvc.exe` | Windows | ARM64 | — |

---

## 4. 配置指南

### 4.1 配置目录结构

运行 `easybot --init` 后自动创建以下目录结构：

```
~/.easybot/
├── gateway.yaml               # 基础配置（版本控制，一般无需修改）
├── gateway.local.yaml         # 本地覆盖配置（不上传版本控制）
├── .env                       # 令牌文件（chmod 600，唯一需要编辑的文件）
├── data/
│   ├── gateway.db             # SQLite 数据库（自动创建）
│   ├── .dev_api_key           # 开发 API Key（自动生成，chmod 600）
│   └── media_cache/           # 媒体文件缓存
├── logs/                      # 日志文件（配置 logging.output=file 时使用）
├── plugins/                   # 第三方适配器动态库
├── certs/                     # TLS 证书（可选）
├── secrets/                   # 密钥存储（可选）
├── easybot.sh                 # 服务管理脚本（Linux/macOS）
└── manage-service.ps1         # 服务管理脚本（Windows）
```

### 4.2 配置流程

```
第一步：初始化        第二步：填入令牌        第三步：启动
easybot --init  ──→  编辑 .env          ──→  easybot --debug
                        │
                        ▼
                 系统自动检测已设置令牌的平台，
                 自动启用对应适配器
```

> ✅ **无需手动编辑 gateway.yaml**。在 `.env` 中设置令牌即可自动启用对应平台适配器。

### 4.3 环境变量（.env）

`.env` 是唯一需要编辑的配置文件。所有变量默认被注释掉，取消注释并填入值即可启用对应平台。

```bash
# Telegram Bot Token（从 @BotFather 获取）
TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11

# Discord Bot Token
DISCORD_BOT_TOKEN=your_discord_bot_token

# 飞书/Lark 应用凭据
FEISHU_APP_ID=cli_xxxxxxxxxxxx
FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxx

# QQ 机器人凭据
QQ_APP_ID=your_app_id
QQ_CLIENT_SECRET=your_client_secret

# 个人微信：扫码登录，无需环境变量
```

**环境变量优先级（从高到低）：**

1. Shell `export` 或 Docker `environment:` 定义的值
2. `.env` 文件中定义的值
3. `gateway.yaml` 中的默认值

### 4.4 gateway.yaml 完整配置参考

基础配置 `gateway.yaml` 通常无需修改，以下列出所有可配置项供参考：

```yaml
# ── 服务器 ─────────────────────────────────
server:
  host: "127.0.0.1"          # 监听地址（生产环境改 0.0.0.0）
  port: 8080                  # 监听端口
  # adminPassword: ""         # 管理后台密码（建议使用 EASYBOT_ADMIN_PASSWORD 环境变量）
  tls:
    enabled: false            # 是否启用 TLS（生产环境推荐启用或使用反向代理）
    certFile: ""              # TLS 证书路径
    keyFile: ""               # TLS 私钥路径

# ── API ─────────────────────────────────────
api:
  basePath: "/api/v1"         # API 基础路径
  # rawPayloadEnabled: false  # 是否在 WebSocket 事件中透传平台原始 payload
  websocket:
    enabled: true             # 启用 WebSocket
    maxClients: 1000          # 最大 WebSocket 连接数
    heartbeatInterval: 30     # WebSocket 心跳间隔（秒）
  metrics:
    enabled: true             # Prometheus 指标
    path: "/metrics"
  rateLimit:
    enabled: true             # 速率限制
    requestsPerMinute: 60     # 每分钟允许的请求数
    burstSize: 10             # 突发峰值

# ── 存储 ────────────────────────────────────
storage:
  storageType: "sqlite"       # sqlite / postgres
  path: ""                    # 数据库路径（空=自动）
  # connectionString: ""      # PostgreSQL 连接字符串
  # poolSize: 10              # PostgreSQL 连接池大小
  retention:
    messageTtlDays: 90        # 消息保留天数
    sessionTtlDays: 365       # 会话保留天数
    cleanupIntervalSecs: 3600 # 清理间隔

# ── 日志 ────────────────────────────────────
logging:
  level: "info"               # debug / info / warn / error
  format: "text"              # text / json（生产推荐 json）
  output: "stdout"            # stdout / file（文件输出到 logs/ 目录）

# ── Webhook ─────────────────────────────────
# webhooks:
#   - name: "my-service"
#     url: "https://example.com/webhook"
#     secret: "your-secret"
#     events:
#       - "message.inbound"
#       - "adapter.connected"
#     platforms:
#       - telegram
```

### 4.5 本地覆盖配置（gateway.local.yaml）

用于覆盖 `gateway.yaml` 中的特定值，**不上传版本控制**（已在 `.gitignore` 中）。

典型用途：

```yaml
# ~/.easybot/gateway.local.yaml
# 覆盖基础配置，无需修改 gateway.yaml

server:
  host: "0.0.0.0"             # 覆盖监听地址

adapters:
  telegram:
    apiUrl: "https://api.telegram.org"  # 自定义 Telegram API 地址（通过代理）
  qq:
    sandbox: true             # QQ 沙箱模式
```

### 4.6 配置优先级（完整链路）

```
1. CLI 参数:    --dir 指定配置目录
2. 环境变量:    EASYBOT_HOME 或 EASYBOT_* 系列
3. 本地覆盖:    gateway.local.yaml 合并到基础配置
4. 基础配置:    gateway.yaml
5. 环境变量替换: 配置中 ${VAR_NAME} 被替换
6. .env 文件:    在配置目录加载
7. 默认值:      代码内建默认值
```

### 4.7 热重载

修改 `gateway.yaml`（或 `gateway.local.yaml`）后无需重启服务。EasyBot 每 **60 秒**自动检查配置文件变更并热加载。

```bash
# 修改配置后等待最多 60 秒即可生效
# 或手动调用 API 强制重载
curl -X PUT http://localhost:8080/api/v1/config \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"logging": {"level": "debug"}}'
```

---

## 5. 平台接入指南

### 5.1 Telegram

#### 前置条件
- 一个 Telegram 账号
- 能访问 [@BotFather](https://t.me/BotFather)

#### 接入步骤

**Step 1：创建 Bot**

1. 在 Telegram 中搜索 [@BotFather](https://t.me/BotFather)
2. 发送 `/newbot` 命令
3. 设置 Bot 显示名称（如 `My EasyBot`）
4. 设置 Bot 用户名（必须以 `bot` 结尾，如 `my_easy_bot`）
5. BotFather 回复中给出 **Token**，保存它

**Step 2：配置 EasyBot**

```bash
# 在 .env 中添加
TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11
```

**Step 3：启动验证**

启动 EasyBot 后，日志应显示：

```
[INFO] easybot_adapter_telegram::adapter Telegram adapter started
```

#### 可选配置

```yaml
# gateway.local.yaml
adapters:
  telegram:
    apiUrl: "https://api.telegram.org"  # 自定义 API 地址（通过代理时使用）
```

#### 发送消息

```bash
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "telegram:123456789",
    "text": "Hello from EasyBot!"
  }'
```

> `target` 格式：`telegram:{ChatID}`。ChatID 请参考 [2.4 发送第一条消息](#24-发送第一条消息) 获取方式。

---

### 5.2 Discord

#### 前置条件
- 一个 Discord 账号
- 一个 Discord 服务器（用于测试）

#### 接入步骤

**Step 1：创建应用**

1. 访问 [Discord Developer Portal](https://discord.com/developers/applications)
2. 点击 **New Application**，输入应用名称
3. 进入 **Bot** 页面
4. 点击 **Add Bot** → **Yes, do it!**
5. 在 **Token** 区域点击 **Reset Token** → **Copy**（保存好）

**Step 2：启用 Privileged Gateway Intents**

在 Bot 页面，向下滚动到 **Privileged Gateway Intents**：

- ✅ **MESSAGE CONTENT INTENT** — 必须启用，否则无法接收消息内容
- ✅ **SERVER MEMBERS INTENT** — 建议启用
- ✅ **PRESENCE INTENT** — 可选

**Step 3：邀请 Bot 到服务器**

1. 在 **OAuth2** → **URL Generator** 页面
2. Scopes: 勾选 `bot`
3. Bot Permissions: 勾选 `Send Messages`, `Read Message History`, `Send Messages in Threads`
4. 复制底部生成的 URL，在浏览器中打开
5. 选择你的服务器并授权

**Step 4：配置 EasyBot**

```bash
# 在 .env 中添加
DISCORD_BOT_TOKEN=your_discord_bot_token
```

#### 发送消息

```bash
# target 格式: "discord:{ChannelID}"
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "discord:123456789012345678",
    "text": "Hello from EasyBot!"
  }'
```

> 💡 **获取 Channel ID：** Discord 中右键频道 → **复制频道 ID**（需在设置中启用"开发者模式"）。

---

### 5.3 飞书 / Lark

#### 前置条件
- 一个飞书账号
- 飞书开放平台开发者权限

#### 接入步骤

**Step 1：创建应用**

1. 访问 [飞书开放平台](https://open.feishu.cn/app)
2. 点击 **创建应用** → 输入应用名称 → 选择应用类型（推荐**企业自建应用**）
3. 创建成功后进入应用详情页

**Step 2：获取凭据**

1. 进入 **凭证与基础信息** 页面
2. 记录 **App ID**（格式如 `cli_xxxxxxxxxxxx`）
3. 记录 **App Secret**（点击显示或重置，保存好）

**Step 3：配置权限**

进入 **权限管理** 页面，至少添加：

- `im:message` — 消息读写权限
- `im:resource` — 获取消息中的资源（图片、文件）
- `contact:user.base` — 读取用户信息

> 部分权限需要企业管理员审批。

**Step 4：配置事件订阅**

进入 **事件与回调** → **事件配置**：

1. 添加事件：`接收消息（im.message.receive_v1）`
2. 订阅方式选择 **WebSocket**（或 Webhook）
3. 确保事件状态为**已订阅**

**Step 5：发布应用**

1. 进入 **版本管理与发布**
2. 创建版本 → 填写更新说明 → **保存**
3. 点击 **申请发布**
4. 等待管理员审批通过

**Step 6：配置 EasyBot**

```bash
# 在 .env 中添加
FEISHU_APP_ID=cli_xxxxxxxxxxxx
FEISHU_APP_SECRET=your_app_secret
```

#### 发送消息

```bash
# target 格式: "feishu:{ChatID}"
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "feishu:oc_xxxxxxxxxxxxx",
    "text": "Hello from EasyBot!"
  }'
```

> 💡 **获取 Chat ID：** 飞书群聊设置 → **群设置** → **更多** → **复制群 ID**。

---

### 5.4 QQ

#### 前置条件
- QQ 开放平台账号（需企业认证）
- 一个 QQ 机器人机器人

#### 接入步骤

**Step 1：创建机器人**

1. 访问 [QQ 开放平台](https://bot.q.qq.com/open/#/bot)
2. 点击 **创建机器人**
3. 填写机器人信息，通过审核后进入管理页面

**Step 2：获取凭据**

1. 在机器人管理页面进入 **开发** → **基础配置**
2. 记录 **BotAppID**（即 App ID）
3. 点击 **Token（机器人令牌）** → 重置并复制 **Client Secret**

**Step 3：配置沙箱环境（可选，推荐开发阶段使用）**

在基础配置页面下方，**沙箱配置**，添加测试人员的 QQ 号。

**Step 4：配置 EasyBot**

```bash
# 在 .env 中添加
QQ_APP_ID=your_app_id           # BotAppID
QQ_CLIENT_SECRET=your_secret    # Token（机器人令牌）
```

如需启用沙箱模式：

```yaml
# gateway.local.yaml
adapters:
  qq:
    sandbox: true
```

#### 发送消息

```bash
# 发送到 QQ 群（群聊）
# target 格式: "qq:group:{GroupID}"
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "qq:group:123456",
    "text": "Hello from EasyBot!"
  }'

# 发送到 QQ 频道（频道消息）
# target 格式: "qq:channel:{ChannelID}"
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "qq:channel:123456",
    "text": "Hello from EasyBot!"
  }'

# 发送给 QQ 好友（私聊）
# target 格式: "qq:user:{UserID}"
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "qq:user:123456",
    "text": "Hello from EasyBot!"
  }'
```

> 💡 **支持的会话类型：** QQ 适配器自动识别群聊、频道、私聊三种会话类型，无需手动指定。

---

### 5.5 个人微信

#### 前置条件
- 一个个人微信号
- iLink Bot（第三方微信桥接服务）

#### 接入步骤

**Step 1：了解 iLink Bot**

EasyBot 的个人微信适配器基于 [iLink Bot](https://www.ilinkbot.com/) 实现。iLink Bot 是一个第三方微信桥接服务，通过扫码登录使个人微信号可通过 API 控制。

> ⚠️ **注意：** 个人微信适配器使用 iLink Bot 的**长轮询**方式与微信通信。受微信官方限制，此方式可能不稳定，建议仅用于个人辅助用途。

**Step 2：注册 iLink Bot**

1. 访问 iLink Bot 官网注册账号
2. 阅读其文档完成设备绑定

**Step 3：启动并扫码**

个人微信适配器**无需环境变量**。启动 EasyBot 后：

1. 查看日志中的 QR 码
2. 使用微信扫描二维码授权登录
3. 登录成功后适配器自动启用

```bash
# 启动后查看扫码信息
easybot --debug

# 日志应显示类似:
# [INFO] easybot_adapter_wechat::adapter 个人微信适配器启动，请扫描屏幕二维码登录
```

#### 发送消息

```bash
# target 格式: "wechat:wxid_xxxxxxxx"
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "target": "wechat:wxid_xxxxxxxx",
    "text": "Hello from EasyBot!"
  }'
```

> 💡 **获取 wxid：** 收到用户消息后，通过 WebSocket 事件中的 `chat_id` 字段获取。

#### 已知限制

| 功能 | 支持情况 |
|------|---------|
| 发送文本消息 | ✅ |
| 接收消息 | ✅ |
| 发送图片/文件 | ✅ |
| 编辑/删除消息 | ❌ 平台不支持 |
| 交互消息（按钮） | ❌ 平台不支持 |
| 获取聊天列表 | ❌ 平台不支持 |

---

## 6. 服务管理与运维

### 6.1 命令行参数

```bash
easybot [OPTIONS]

选项:
  -c, --config <FILE>    配置文件路径（优先级高于 --dir）
      --dir <DIR>        配置目录路径（默认 ~/.easybot/ 或平台标准目录）
      --init             初始化配置目录并退出
  -d, --debug            调试模式（启用 DEBUG 级别日志）
  -h, --help             显示帮助信息
  -V, --version          显示版本号
```

### 6.2 使用 Makefile 管理

```bash
# 显示所有可用命令
make

# 编译并启动（开发模式）
make run

# 在隔离目录中全新初始化 + 启动（不影响 ~/.easybot/）
make run-fresh

# 热重载模式：代码变更自动重编重启
make watch

# 运行验收测试
make verify
make test
```

### 6.3 安装为系统服务

初始化配置时，`easybot --init` 会自动生成服务管理脚本。

#### Linux（systemd）

```bash
# 安装为 systemd 服务
cd ~/.easybot && sudo ./easybot.sh install

# 查看服务状态
sudo ./easybot.sh status

# 查看日志
sudo ./easybot.sh logs

# 卸载服务
sudo ./easybot.sh uninstall
```

#### macOS（launchd）

```bash
# 安装为 launchd 服务
cd ~/.easybot && ./easybot.sh install

# 查看状态
./easybot.sh status

# 查看日志
./easybot.sh logs

# 卸载
./easybot.sh uninstall
```

#### Windows（Windows Service）

```powershell
# 以管理员身份运行 PowerShell
cd ~/.easybot
PowerShell -ExecutionPolicy Bypass -File manage-service.ps1 install
PowerShell -ExecutionPolicy Bypass -File manage-service.ps1 status
PowerShell -ExecutionPolicy Bypass -File manage-service.ps1 uninstall
```

### 6.4 健康检查

EasyBot 暴露 `/api/v1/health` 端点，返回服务运行状态：

```bash
curl http://localhost:8080/api/v1/health
```

```json
{
  "status": "ok",
  "version": "0.0.14",
  "uptime_seconds": 3600,
  "adapters": {
    "telegram": "connected",
    "discord": "connected",
    "qq": "disconnected"
  },
  "sessions": 42
}
```

Docker 内置了 healthcheck，每 30 秒自动检查一次。

### 6.5 查看适配器状态

```bash
curl http://localhost:8080/api/v1/adapters \
  -H "Authorization: Bearer $API_KEY"
```

```json
[
  {
    "platform": "telegram",
    "status": "connected",
    "uptime_seconds": 3600,
    "messages_sent": 128,
    "messages_received": 256
  },
  {
    "platform": "discord",
    "status": "connected",
    "uptime_seconds": 3500,
    "messages_sent": 64,
    "messages_received": 128
  },
  {
    "platform": "qq",
    "status": "disconnected",
    "error": "credentials not configured"
  }
]
```

---

## 7. API 使用指南

### 7.1 获取 API Key

首次启动时，EasyBot 自动生成一个开发用 API Key，保存在 `data/.dev_api_key`：

```bash
# 查看自动生成的 API Key
cat ~/.easybot/data/.dev_api_key
```

通过管理后台也可创建和管理更多 API Key（见 [9. 管理后台](#9-管理后台)）。

所有 API 请求需携带 `Authorization: Bearer <api-key>` 请求头。

### 7.2 REST API 完整参考

所有 API 路径以 `/api/v1` 为前缀。

#### 🏥 健康与状态

| 路径 | 方法 | 说明 |
|------|------|------|
| `/health` | GET | 健康检查（无需认证） |
| `/system` | GET | 系统信息（CPU、内存） |

#### 📦 适配器管理

| 路径 | 方法 | 说明 |
|------|------|------|
| `/adapters` | GET | 列出所有适配器及状态 |
| `/adapters/{platform}/start` | POST | 启动指定适配器 |
| `/adapters/{platform}/stop` | POST | 停止指定适配器 |
| `/adapters/{platform}/status` | GET | 适配器详细状态 |

#### 💬 消息操作

| 路径 | 方法 | 说明 |
|------|------|------|
| `/messages/send` | POST | 发送消息 |
| `/messages/batch-send` | POST | 批量发送消息 |
| `/messages/{id}` | PUT | 编辑消息 |
| `/messages/{id}` | DELETE | 删除消息 |
| `/messages` | GET | 查询消息历史（支持 `?platform=` 过滤） |

**发送消息请求体：**

```json
{
  "target": "telegram:123456789",
  "text": "Hello!",
  "parseMode": "markdown",
  "media": {
    "type": "image",
    "url": "https://example.com/image.jpg"
  },
  "keyboard": {
    "inline": [
      [{"text": "按钮1", "data": "btn1"}],
      [{"text": "按钮2", "url": "https://example.com"}]
    ]
  }
}
```

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `target` | string | ✅ | `{platform}:{chatId}` 格式 |
| `text` | string | — | 消息文本（`text` 和 `media` 至少一个） |
| `parseMode` | string | — | 解析模式: `markdown` / `html` |
| `media` | object | — | 媒体附件（type: image/audio/video/file） |
| `keyboard` | object | — | 内联键盘/按钮 |

**`target` 格式一览：**

| 平台 | 格式 | 示例 |
|------|------|------|
| Telegram | `telegram:{ChatID}` | `telegram:123456789` |
| Discord | `discord:{ChannelID}` | `discord:123456789012345678` |
| 飞书 | `feishu:{ChatID}` | `feishu:oc_xxxxxxxxxxxxx` |
| QQ 群聊 | `qq:group:{GroupID}` | `qq:group:123456` |
| QQ 频道 | `qq:channel:{ChannelID}` | `qq:channel:123456` |
| QQ 私聊 | `qq:user:{UserID}` | `qq:user:123456` |
| 微信 | `wechat:{wxid}` | `wechat:wxid_xxxxxxxx` |

#### 🔗 会话管理

| 路径 | 方法 | 说明 |
|------|------|------|
| `/sessions` | GET | 活跃会话列表 |
| `/sessions/{key}` | GET | 会话详情 |
| `/sessions/{key}` | DELETE | 删除会话 |

#### 👥 聊天信息

| 路径 | 方法 | 说明 |
|------|------|------|
| `/chats/{platform}` | GET | 获取平台聊天列表 |
| `/chats/{platform}/{chat_id}` | GET | 获取聊天详情 |

#### ⚙️ 配置

| 路径 | 方法 | 说明 |
|------|------|------|
| `/config` | GET | 获取当前运行时配置 |
| `/config` | PUT | 热更新配置（修改后即时生效） |

#### 🔑 API Keys

| 路径 | 方法 | 说明 |
|------|------|------|
| `/api-keys/types` | GET | 查看 API Key 类型列表 |
| `/api-keys/{id}` | DELETE | 吊销 API Key |
| `/api-keys/{id}/purge` | DELETE | 彻底删除 API Key |

#### 📊 监控和工具

| 路径 | 方法 | 说明 |
|------|------|------|
| `/metrics` | GET | Prometheus 指标 |
| `/logs` | GET | 实时日志流（环形缓冲区，最近 5000 条） |
| `/swagger` | GET | Swagger UI（OpenAPI 文档浏览器） |
| `/openapi.json` | GET | OpenAPI 3.1 JSON schema |

---

## 8. WebSocket 实时事件

### 8.1 连接

WebSocket 端点：`ws://host:8080/api/v1/ws`

EasyBot 使用**JSON 帧认证**（而非 HTTP 请求头），因为部分 WebSocket 客户端不支持自定义 HTTP 头。

```javascript
// JavaScript 示例
const ws = new WebSocket('ws://localhost:8080/api/v1/ws');

ws.onopen = () => {
  // 连接成功后发送认证帧
  ws.send(JSON.stringify({ token: 'your-api-key' }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);
  console.log('收到事件:', msg);
};
```

### 8.2 事件格式

所有事件统一格式：

```json
{
  "type": "event",
  "event": "message.inbound",
  "data": {
    "platform": "telegram",
    "chat_id": "123456789",
    "text": "Hello!",
    "from_user": {"id": 987654, "name": "Alice"},
    "timestamp": "2026-07-09T11:00:00Z"
  },
  "seq": 1,
  "timestamp": 1720512000000
}
```

### 8.3 事件类型

#### 消息事件

| 事件 | 说明 |
|------|------|
| `message.inbound` | 收到新消息 |
| `message.sent` | 消息已发送 |
| `message.edited` | 消息已编辑 |
| `message.deleted` | 消息已删除 |

#### 适配器事件

| 事件 | 说明 |
|------|------|
| `adapter.connected` | 适配器已连接 |
| `adapter.disconnected` | 适配器已断开 |
| `adapter.reconnecting` | 适配器正在重连 |
| `adapter.failed` | 适配器连接失败 |

#### 网关事件

| 事件 | 说明 |
|------|------|
| `gateway.started` | 网关启动完成 |
| `gateway.stopping` | 网关正在关闭 |
| `gateway.config.reloaded` | 配置已热重载 |

### 8.4 事件数据字段

**`message.inbound` 数据结构：**

```json
{
  "platform": "telegram",
  "chat_id": "123456",
  "chat_type": "group",
  "message_id": "msg_xxx",
  "thread_id": null,
  "from_user": {
    "id": "987654",
    "name": "Alice",
    "username": "alice123"
  },
  "text": "Hello from Telegram!",
  "mentions": [],
  "reply_to": null,
  "timestamp": "2026-07-09T11:00:00Z",
  "metadata": {}
}
```

### 8.5 原始 Payload 透传

如果配置中 `api.rawPayloadEnabled: true`（或设置环境变量 `EASYBOT_RAW_PAYLOAD_ENABLED=true`），WebSocket 事件还会包含 `metadata.raw_payload` 字段，里面是 IM 平台返回的完整原始 JSON。**此功能仅用于调试，默认关闭。**

---

## 9. 管理后台

### 9.1 访问

启动 EasyBot 后，浏览器打开：

```
http://localhost:8080/admin
```

### 9.2 登录

首次使用需设置管理后台密码：

```bash
# 通过环境变量设置
export EASYBOT_ADMIN_PASSWORD=your_secure_password
easybot --debug

# 或在 gateway.yaml 中设置
# server:
#   adminPassword: "your_secure_password"
```

> ⚠️ **密码设置提醒**：启动时若未检测到密码，日志会输出警告。管理后台登录将持续被拒绝直到设置密码。

### 9.3 功能介绍

管理后台提供以下功能：

- **适配器概览** — 查看所有平台适配器的实时状态
- **API Key 管理** — 创建、吊销、管理 API 密钥
- **实时日志** — 查看最近的运行日志（环形缓冲，最多 5000 条）
- **系统信息** — CPU、内存、运行时间
- **配置查看** — 当前生效的配置

---

## 10. 生产部署

### 10.1 生产环境 Checklist

| 检查项 | 说明 | 必选 |
|--------|------|------|
| ✅ 密钥管理 | 使用环境变量或 Docker secrets 注入令牌，不硬编码 | ✅ |
| ✅ 管理后台密码 | 设置 `EASYBOT_ADMIN_PASSWORD` 环境变量 | ✅ |
| ✅ 数据库 | 生产环境推荐 PostgreSQL | 推荐 |
| ✅ TLS | 配置网关 TLS 或使用反向代理（Nginx / Caddy） | ✅ |
| ✅ 监听地址 | `server.host` 改为 `0.0.0.0` 或指定内网地址 | ✅ |
| ✅ 资源限制 | Docker 部署时设置 CPU/内存上限 | 推荐 |
| ✅ 日志格式 | 使用 JSON 格式输出，方便日志采集系统 | 推荐 |
| ✅ 监控 | 启用 Prometheus 指标采集 | 推荐 |
| ✅ 安全审计 | 参考 `docs/SECURITY_AUDIT.md` | 推荐 |

### 10.2 使用 PostgreSQL

```bash
# 使用 Docker Compose 一键启动
docker compose --profile postgres up -d

# 或手动配置 PostgreSQL
```

配置 `gateway.local.yaml`：

```yaml
storage:
  storageType: "postgres"
  connectionString: "postgresql://user:password@host:5432/easybot"
  poolSize: 10
```

环境变量方式：

```bash
export DATABASE_URL=postgresql://user:password@host:5432/easybot
```

> PostgreSQL 首次启动时会自动执行数据库迁移，无需手动建表。

### 10.3 反向代理 + TLS

推荐使用 Nginx 或 Caddy 作为反向代理，终止 TLS 并转发到 EasyBot：

```nginx
# Nginx 配置示例
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

        # WebSocket 支持
        proxy_read_timeout 86400s;
    }
}
```

### 10.4 监控与告警

#### Prometheus 指标

EasyBot 在 `/api/v1/metrics` 暴露 Prometheus 格式指标：

| 指标 | 类型 | 说明 |
|------|------|------|
| `easybot_messages_sent_total` | Counter | 消息发送总数（标签: platform） |
| `easybot_messages_received_total` | Counter | 消息接收总数（标签: platform） |
| `easybot_adapters_connected` | Gauge | 当前已连接的适配器数 |
| `easybot_sessions_active` | Gauge | 当前活跃会话数 |
| `easybot_http_requests_total` | Counter | HTTP 请求总数（标签: method, path, status） |
| `easybot_http_request_duration_seconds` | Histogram | 请求耗时分布 |

Docker Compose 使用 `--profile monitoring` 即可启动预配置的 Prometheus。

### 10.5 Docker 资源限制（生产推荐）

```yaml
# docker-compose.yml 中已包含的资源配置
deploy:
  resources:
    limits:
      cpus: "2"           # CPU 上限
      memory: 512M        # 内存上限
    reservations:
      cpus: "0.5"         # CPU 预留
      memory: 128M        # 内存预留
```

### 10.6 安全加固

#### 容器安全

Docker 容器已配置以下加固措施：

- **删除所有 capabilities**：`cap_drop: ALL`
- **防止权限提升**：`no-new-privileges: true`
- **只读根文件系统**：仅 `/tmp` 和数据卷可写
- **网络隔离**：前端（互联网可达）和后端（数据库）分离

#### 密钥安全

- 生产环境使用 Docker secrets 或 bind mount 替代环境变量传入令牌
- API Key 文件权限自动设置为 600
- `.env` 文件默认 chmod 600
- 日志中所有密钥/令牌输出时自动掩码

---

## 11. 常见问题

### 11.1 Docker 构建太慢？

```bash
# 使用构建缓存加速重复构建
# Dockerfile 已配置缓存挂载

# 如果不需要所有平台，调整 build.rs 或使用按需构建：
cargo build --no-default-features --features "adapter-telegram"
```

### 11.2 启动后某些适配器未连接？

检查以下事项：

1. **环境变量是否正确设置？** — 确认 `.env` 中变量名无误且未被注释
2. **令牌是否有效？** — 令牌过期或未正确复制
3. **日志输出什么？** — 使用 `--debug` 模式启动查看详细日志
4. **QQ 沙箱模式** — 开发阶段建议开启沙箱模式

### 11.3 如何调试问题？

```bash
# 启用 debug 级别日志
easybot --debug

# 或通过配置文件
# logging:
#   level: "debug"

# 查看实时日志
curl http://localhost:8080/api/v1/logs -H "Authorization: Bearer $API_KEY"

# 查看适配器状态
curl http://localhost:8080/api/v1/adapters -H "Authorization: Bearer $API_KEY"
```

### 11.4 如何更换 SQLite 为 PostgreSQL？

```yaml
# 修改 gateway.local.yaml
storage:
  storageType: "postgres"
  connectionString: "postgresql://user:password@host:5432/easybot"
```

数据不会自动迁移，需要手动从 SQLite 导出导入。

### 11.5 如何更新配置而不重启？

EasyBot 每 60 秒自动检测配置文件变更。或直接调用 API：

```bash
curl -X PUT http://localhost:8080/api/v1/config \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"logging": {"level": "debug"}}'
```

### 11.6 WebSocket 连接不稳定？

```yaml
# 调整 gateway.local.yaml
api:
  websocket:
    maxClients: 1000        # 增大最大连接数
    heartbeatInterval: 15   # 缩短心跳间隔（默认 30 秒）
```

### 11.7 已启动但仍收不到消息？

1. **检查适配器状态** — 确保显示为 `connected`
2. **确认 WebSocket 已认证** — 连接后发送 `{"token":"..."}` 认证帧
3. **确认 WebSocket 认证成功** — 认证失败会收到错误帧
4. **检查平台侧权限** — 飞书、QQ 需要应用发布审批；Discord 需要 Gateway Intents

---

## 12. 故障排查

### 12.1 日志解读

EasyBot 有五个日志级别，从低到高：

| 级别 | 颜色 | 用途 |
|------|------|------|
| `TRACE` | 灰色 | 最详细调试，如心跳包 |
| `DEBUG` | 蓝色 | 调试信息，如消息收发 |
| `INFO` | 绿色 | 正常运行时信息 |
| `WARN` | 黄色 | 需要注意但不影响运行 |
| `ERROR` | 红色 | 错误，需要处理 |

#### 常见日志模式

**正常心跳（TRACE）** — 每 30 秒左右出现一次，说明适配器连接稳定：

```
[TRACE] easybot_adapter_qq::gateway QQ heartbeat ack
[TRACE] easybot_adapter_discord::gateway Discord heartbeat ack
```

**适配器启动（INFO）：**

```
[INFO] easybot_core::adapter::manager Starting adapter: telegram
[INFO] easybot_adapter_telegram::adapter Telegram adapter started
```

**消息收发（DEBUG）：**

```
[DEBUG] easybot_core::bus Publishing event: message.inbound (telegram)
[DEBUG] easybot_core::adapter::manager Sending message via telegram
```

**自动重连（INFO/WARN）— 正常行为，适配器会自动恢复：**

```
[INFO] easybot_adapter_qq::gateway QQ reconnect requested
[INFO] easybot_adapter_qq::gateway QQ Gateway: connecting to wss://api.sgroup.qq.com/websocket
[INFO] easybot_adapter_qq::gateway QQ Gateway connected
[INFO] easybot_adapter_qq::gateway QQ Gateway resumed
```

### 12.2 常见错误及解决

| 错误 | 原因 | 解决方法 |
|------|------|---------|
| `token invalid or expired (11244)` | QQ Token 过期 | 自动刷新（已内置重试逻辑，如超过 1 次需到 QQ 开放平台重新生成） |
| `401 Unauthorized` | API Key 无效 | 检查 `data/.dev_api_key` 或重新创建 |
| `DISCORD_... PrivilegedGatewayIntent` | Discord 未启用 Gateway Intents | 在开发者后台 Bot 页面开启 MESSAGE CONTENT INTENT |
| `适配器启动失败` | 平台凭据无效 | 检查 `.env` 中的令牌、重新生成 |
| `SQLite 初始化失败` | 磁盘空间不足或权限错误 | 检查配置目录权限和磁盘空间 |
| `PostgreSQL 连接失败` | 数据库地址/密码错误 | 检查 `connectionString` |
| `生产环境必须启用 TLS` | Release build 未配置 TLS | 设置 `EASYBOT_ALLOW_PLAINTEXT=true`（开发）或配置 TLS/反向代理 |

### 12.3 如何获取帮助

- **GitHub Issues**: [github.com/EasyIndie/EasyBot/issues](https://github.com/EasyIndie/EasyBot/issues)
- **项目文档**: `docs/` 目录和 [README.md](../README.md)
- **日志文件**: 启动时使用 `--debug` 获取完整日志
- **提交 Bug 报告**: 请附带日志、配置（掩码后的）和复现步骤

---

## 附录

### A. 快速参考卡

```bash
# ── 启动 ─────────────────────────────────
easybot --init                    # 首次：初始化配置
vim ~/.easybot/.env              # 填入令牌
easybot --debug                   # 开发模式启动
docker compose up -d              # Docker 部署

# ── 消息发送 ────────────────────────────
API_KEY=$(cat ~/.easybot/data/.dev_api_key)

curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"target": "telegram:123456", "text": "Hello"}'

# ── 适配器管理 ──────────────────────────
curl http://localhost:8080/api/v1/adapters \
  -H "Authorization: Bearer $API_KEY"

# ── 健康检查 ────────────────────────────
curl http://localhost:8080/api/v1/health

# ── WebSocket 监听 ─────────────────────
wscat -c ws://localhost:8080/api/v1/ws
> {"token": "YOUR_API_KEY"}
# 等待事件...
```

### B. 平台凭据获取速查

| 平台 | 需要什么 | 去哪获取 |
|------|---------|---------|
| Telegram | Bot Token | [@BotFather](https://t.me/BotFather) |
| Discord | Bot Token | [Discord Developer Portal](https://discord.com/developers/applications) |
| 飞书 | App ID + App Secret | [飞书开放平台](https://open.feishu.cn/app) |
| QQ | App ID + Client Secret | [QQ 开放平台](https://bot.q.qq.com/open/#/bot) |
| 微信 | 无需配置 | 启动后扫码登录 |

---

*最后更新：2026-07-09 · EasyBot v0.0.14*
