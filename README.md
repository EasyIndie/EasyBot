<h1 align="center">
  <br>
  EasyBot
  <br>
</h1>

<p align="center">
  <strong>IM Gateway — 统一多平台即时消息网关</strong>
</p>

<p align="center">
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.94+-de5c43?logo=rust" alt="Rust"></a>
  <a href="./LICENSE"><img src="https://img.shields.io/badge/license-GPLv3-blue" alt="License"></a>
  <a href="#"><img src="https://img.shields.io/badge/Platform-Telegram%20|%20Discord%20|%20%E9%A3%9E%E4%B9%A6%20|%20QQ%20|%20%E5%BE%AE%E4%BF%A1-green" alt="Platforms"></a>
</p>

## 📖 项目简介

**EasyBot** 是一个轻量、高性能的 **IM Gateway**（即时消息网关）服务，使用 Rust 编写。

它连接多个即时通讯平台（Telegram、Discord、飞书、QQ、微信），将不同平台的 API 差异封装在统一的 **`PlatformAdapter`** trait 背后，对外暴露一致的 **REST API** + **WebSocket** 接口。

**一句话：一个网关，连接所有 IM 平台。** 无论你用的是哪个聊天软件，通过 EasyBot 都能用同一套 API 收发消息。

### 核心特性

- 🔌 **五平台支持** — Telegram、Discord、飞书/Lark、QQ、个人微信
- 🧩 **插件系统** — 通过动态库加载第三方适配器，无需 fork 主仓库
- 🔄 **双向通信** — REST API 发送消息 + WebSocket 实时接收消息推送
- 🔐 **API 密钥认证** — 基于 Argon2 的安全认证机制
- 📦 **多存储后端** — SQLite（默认）/ PostgreSQL
- ⚡ **高性能异步** — tokio + axum 栈，全异步非阻塞
- 🐳 **Docker 就绪** — 多阶段构建，一行命令部署
- 📊 **可观测性** — Prometheus 指标、结构化日志、健康检查
- ⚙️ **热重载配置** — 运行时更新配置无需重启

---

## 🏗 架构设计

```
                                External Clients (REST / WebSocket)
                                      ↕
┌───────────────── API Layer (easybot-api) ─────────────────┐
│  axum HTTP server · REST routes · WebSocket event push    │
│  ApiError · Prometheus metrics · Rate Limiting · OpenAPI  │
└─────────────────────────┬─────────────────────────────────┘
                          ↕
┌────────────── Core Logic (easybot-core) ─────────────────┐
│  EventBus        SessionManager    AdapterManager        │
│  (broadcast)     (DashMap store)   (registry + lifecycle)│
│  ApiKeyManager   ConfigLoader      DeliveryRouter        │
│  Storage         Webhook           PluginLoader          │
└─────────────────────────┬─────────────────────────────────┘
                          ↕
┌─────────── Adapter Layer (easybot-adapter-*) ────────────┐
│  Telegram  │  Discord  │  飞书  │  QQ  │  微信          │
│  (每个适配器实现 PlatformAdapter trait)                     │
└─────────────────────────────────────────────────────────┘
```

### 三层架构

| 层 | Crate | 职责 |
|----|-------|------|
| **API 层** | `easybot-api` | axum HTTP 服务器、REST 路由、WebSocket 推送、Prometheus 指标、速率限制、OpenAPI 文档 |
| **核心层** | `easybot-core` | 事件总线、会话管理、适配器管理、API 鉴权、配置加载、存储（SQLite/PostgreSQL）、插件加载 |
| **适配器层** | `easybot-adapter-*` | 各平台 SDK 封装，每个适配器独立 crate，通过 `PlatformAdapter` trait 接入 |

---

## 🔌 支持平台

| 平台 | Crate | 连接方式 | 状态 |
|------|-------|---------|------|
| <img src="https://img.shields.io/badge/Telegram-2CA5E0?logo=telegram" height="20"> | `easybot-adapter-telegram` | Bot API (getUpdates 长轮询) | ✅ 已验证 |
| <img src="https://img.shields.io/badge/Discord-5865F2?logo=discord" height="20"> | `easybot-adapter-discord` | Gateway WebSocket | ✅ 已验证 |
| <img src="https://img.shields.io/badge/%E9%A3%9E%E4%B9%A6-3370FF?logo=feishu" height="20"> | `easybot-adapter-feishu` | REST API + WebSocket 事件订阅 | ✅ 已完成 |
| <img src="https://img.shields.io/badge/QQ-1E80FF?logo=tencentqq" height="20"> | `easybot-adapter-qq` | 统一 QQBot 鉴权 + Gateway WebSocket | ✅ 已验证（群聊/私聊/频道全场景） |
| <img src="https://img.shields.io/badge/%E5%BE%AE%E4%BF%A1-07C160?logo=wechat" height="20"> | `easybot-adapter-wechat` | iLink Bot API 长轮询 | ✅ 已完成 |

---

## 🚀 快速开始

### 前置条件

- Rust 1.94+
- Protobuf 编译器（用于飞书 SDK 编译）

```bash
# macOS
brew install protobuf

# Ubuntu / Debian
sudo apt-get install protobuf-compiler

# Arch Linux
sudo pacman -S protobuf
```

### 从源码构建

```bash
# 克隆项目
git clone https://github.com/EasyIndie/EasyBot.git
cd easybot

# 默认构建（包含全部 5 个平台适配器）
cargo build

# 仅编译指定适配器（可节省编译时间）
cargo build --no-default-features --features "adapter-telegram,adapter-discord"

# 全量构建 + 插件系统
cargo build --features "full,plugin-system"

# 启动服务
cargo run -- --debug
```

### 初始化配置

```bash
# 初始化配置目录（自动创建 gateway.yaml + .env）
cargo run -- --init --dir ~/.easybot

# 编辑 .env，取消注释并填入你要启用的平台令牌
vim ~/.easybot/.env

# 启动 — 系统自动检测已设置令牌的平台并启用对应适配器
cargo run -- --debug
```

> 💡 **无需手动编辑 gateway.yaml** — 在 `.env` 中设置令牌即可自动启用对应平台适配器。

### Docker 部署

```bash
# 构建镜像
docker build -t easybot .

# 运行（通过环境变量传入令牌，自动启用对应适配器）
docker run -p 8080:8080 \
  -e TELEGRAM_BOT_TOKEN="your_token_here" \
  easybot
```

### Docker Compose 部署

```bash
# 复制环境变量模板并填入令牌
cp .env.example .env && vim .env

# 启动（系统自动检测已设置令牌的平台）
docker compose up -d

# 启动 + PostgreSQL + Prometheus 监控
docker compose --profile postgres --profile monitoring up -d

# 查看状态
curl http://localhost:8080/health
```

---

## ⚙️ 配置

### 配置目录结构

```
~/.easybot/
├── gateway.yaml              # 基础配置（版本控制，一般无需修改）
├── gateway.local.yaml        # 本地覆盖配置（可选，高级用途）
├── .env                      # 令牌文件（chmod 600，编辑此文件即可启用适配器）
├── data/
│   ├── gateway.db            # SQLite 数据库（自动创建）
│   └── media_cache/          # 媒体文件缓存
├── logs/                     # 日志文件
├── plugins/                  # 第三方适配器插件
├── certs/                    # TLS 证书（可选）
└── secrets/                  # 密钥存储（可选，chmod 600）
```

### 配置优先级

1. 命令行参数：`--dir` > `EASYBOT_HOME` 环境变量 > 平台标准目录
2. 环境变量优先级：`export` / Docker `environment:` > `.env` 文件
3. 配置合并：`gateway.local.yaml` 覆盖 `gateway.yaml`
4. 环境变量替换：配置支持 `${VAR_NAME}` 语法

### 核心配置示例

```yaml
# gateway.yaml — 默认配置，一般无需修改
server:
  host: "127.0.0.1"        # 监听地址
  port: 8080               # 监听端口
  tls:
    enabled: false          # TLS 默认关闭（生产环境推荐反向代理）

api:
  basePath: "/api/v1"
  websocket:
    enabled: true           # 启用 WebSocket
    maxClients: 1000        # 最大连接数
    heartbeatInterval: 30   # 心跳间隔（秒）
  # rateLimit:
  #   enabled: true          # 速率限制（默认开启）
  #   requestsPerMinute: 60 # 每分钟最大请求数
  #   burstSize: 10         # 突发峰值

storage:
  storageType: "sqlite"       # sqlite / postgres

logging:
  level: "info"             # debug / info / warn / error
  format: "text"            # text / json
  output: "stdout"          # stdout / file
```

```bash
# .env — 唯一需要编辑的文件，设置令牌即自动启用对应平台
TELEGRAM_BOT_TOKEN=123456:ABC-DEF...
DISCORD_BOT_TOKEN=your_token
FEISHU_APP_ID=cli_xxx
FEISHU_APP_SECRET=your_secret
QQ_APP_ID=your_app_id
QQ_CLIENT_SECRET=your_secret
# WECHAT_BOT_TOKEN=optional  # 个人微信可不设令牌，扫码登录
```

> 各平台所需的环境变量参见 `.env.example`。高级用户可通过 `gateway.local.yaml` 覆盖默认值或显式禁用某平台。

---

## 📡 API 参考

所有 API 路径以 `/api/v1` 为前缀。请求需携带 `Authorization: Bearer <api-key>` 头。

| 路径 | 方法 | 说明 |
|------|------|------|
| `/health` | GET | 健康检查（已连接适配器、会话数） |
| `/adapters` | GET | 列出所有适配器及状态 |
| `/adapters/{platform}/start` | POST | 启动适配器 |
| `/adapters/{platform}/stop` | POST | 停止适配器 |
| `/adapters/{platform}/status` | GET | 适配器健康详情 |
| `/messages/send` | POST | 发送消息（`target: "platform:chatId"`） |
| `/messages/batch-send` | POST | 批量发送 |
| `/messages/{message_id}` | PUT | 编辑消息 |
| `/messages/{message_id}` | DELETE | 删除消息 |
| `/messages` | GET | 消息历史（支持 `?platform=` 过滤） |
| `/sessions` | GET | 活跃会话列表 |
| `/sessions/{key}` | GET | 会话详情 |
| `/sessions/{key}` | DELETE | 删除会话 |
| `/chats/{platform}` | GET | 获取平台聊天列表 |
| `/chats/{platform}/{chat_id}` | GET | 获取聊天详情 |
| `/config` | GET | 获取当前配置 |
| `/config` | PUT | 热更新配置 |
| `/ws` | GET | WebSocket 实时事件流（需 `Authorization` 头 + 连接后发送 `{"token":"..."}`） |
| `/metrics` | GET | Prometheus 指标 |
| `/swagger` | GET | Swagger UI (OpenAPI 文档浏览器) |
| `/openapi.json` | GET | OpenAPI 3.1 JSON schema |

### WebSocket 事件

WebSocket 端点有**两层认证**：
1. **HTTP 升级请求**必须携带 `Authorization: Bearer <api-key>` 头
2. 连接成功后，发送 JSON 认证帧：

```json
{"token": "your-api-key"}
```

连接成功后，会收到实时事件推送，例如：

```json
{"type": "message.inbound", "data": {"platform": "telegram", "text": "hello", ...}}
{"type": "adapter.connected", "data": {"platform": "telegram", ...}}
{"type": "adapter.disconnected", "data": {"platform": "discord", ...}}
```

---

## 🧩 插件系统

EasyBot 支持通过动态库加载第三方 IM 适配器插件。

```rust
// 插件开发者只需依赖 easybot-plugin-sdk 并实现 PlatformAdapter trait
use easybot_plugin_sdk::*;

struct MyAdapter;

#[async_trait]
impl PlatformAdapter for MyAdapter {
    fn platform_name(&self) -> &str { "my-platform" }
    async fn send(&self, msg: OutboundMessage) -> Result<SendResult, GatewayError> {
        // 实现消息发送逻辑
    }
    // ...
}
```

详见 [插件开发指南](docs/PLUGIN_DEV.md)。

---

## 📦 项目结构

```
easybot/
├── bin/                          # 二进制入口
│   └── src/main.rs              # CLI 参数、组件组装、信号处理
├── crates/
│   ├── easybot-core/            # 核心库
│   │   ├── src/adapter/         # 适配器管理（manager, registry）
│   │   ├── src/auth/            # API 密钥认证（Argon2）
│   │   ├── src/bus/             # 事件总线（tokio broadcast）
│   │   ├── src/config/          # 配置加载（YAML + 环境变量）
│   │   ├── src/plugin/          # 插件系统（动态库加载）
│   │   ├── src/session/         # 会话管理
│   │   ├── src/storage/         # 存储（SQLite / PostgreSQL）
│   │   ├── src/types/           # 核心数据模型
│   │   └── src/webhook/         # Webhook 支持
│   ├── easybot-api/             # API 层
│   │   ├── src/server.rs        # 服务器组装（路由注册、中间件）
│   │   ├── src/lib.rs           # AppState 定义
│   │   ├── src/routes/          # 路由处理（含 WebSocket）
│   │   ├── src/middleware/       # 中间件（rate_limit）
│   │   ├── src/response.rs      # 统一响应格式
│   │   ├── src/metrics.rs       # Prometheus 指标
│   │   └── src/openapi.rs       # OpenAPI 文档定义
│   ├── easybot-adapter-telegram/ # Telegram 适配器
│   ├── easybot-adapter-discord/  # Discord 适配器
│   ├── easybot-adapter-feishu/   # 飞书适配器
│   ├── easybot-adapter-qq/       # QQ 适配器
│   ├── easybot-adapter-wechat/   # 微信适配器
│   └── easybot-plugin-sdk/      # 插件 SDK
├── tests/
│   ├── integration/              # 集成测试
│   ├── e2e/                      # 端到端测试
│   ├── plugins/                  # 插件测试（mock-adapter）
│   └── fixtures/                 # 共享测试 fixture
├── docs/                        # 文档
├── scripts/                     # 工具脚本
├── Dockerfile                   # Docker 多阶段构建
├── docker-compose.yml           # Docker Compose 部署
├── prometheus.yml               # Prometheus 配置
└── gateway.yaml                 # 默认配置
```

---

## 🛠 开发指南

### 构建选项

```bash
# 默认构建（包含全部 5 个平台适配器）
cargo build

# 指定适配器子集（可节省编译时间）
cargo build --no-default-features --features "adapter-telegram,adapter-discord"

# 全量构建 + 插件系统
cargo build --features "full,plugin-system"

# 最小构建（无适配器）
cargo build --no-default-features

# Release 构建（全量）
cargo build --release --features full
```

### 运行测试

```bash
# 全部测试
cargo test --workspace --features "full,plugin-system"

# 指定 crate 测试
cargo test -p easybot-core
cargo test -p easybot-api

# 集成测试（需要先编译 mock-adapter）
cargo build -p mock-adapter && cargo test -p integration-tests

# 配置/环境相关测试
cargo test -p easybot-core config::tests
```

### 代码规范

```bash
# Lint 检查
cargo clippy --all-targets --features "full,plugin-system"

# 格式化
cargo fmt
```

### 适配器生命周期

```
init(config) → connect() → send()/send_media()/... → disconnect()
    ↓              ↓
 Created → Starting → Connecting → Connected → Reconnecting → Failed → Stopped
```

### 实现路线图

| 阶段 | 范围 | 状态 |
|------|------|------|
| **P1 MVP** | 核心类型、PlatformAdapter trait、Telegram 适配器、REST API、配置加载、跨平台路径 | ✅ 完成 |
| **P2 双向通信** | 事件总线、WebSocket 推送、Webhooks、入站消息处理、会话持久化、消息编辑/删除、适配器生命周期事件 | ✅ 完成 |
| **P3 多平台** | Telegram ✅、Discord ✅（含 send_interactive + list_chats）、**飞书/Lark** ✅、**QQ** ✅（含 send_interactive + list_chats）、**个人微信** ✅（iLink Bot API 已验证；edit/delete/send_interactive/list_chats 平台不支持）— 五平台全部完成 | ✅ 完成 |
| **P4 生产就绪** | API 密钥认证（Argon2）、速率限制、热重载、优雅关闭、PostgreSQL、Prometheus、Docker、TTL 保留、健康监控 + 自动重连、send_draft 流式传输、健康运行时间 | ✅ 95% |
| **P5 插件系统** | 插件 SDK、动态库加载、插件注册、加载器测试、开发者文档 | ✅ 完成 |

> **P4 未完成项**: 权限模型 RBAC（暂缓）、TLS 仅配置层未在应用层处理（暂缓）。

---

## 🐳 部署

### 生产部署注意事项

1. **密钥管理** — 使用环境变量或 `.env` 注入 API Token，切勿硬编码
2. **数据库** — 生产环境推荐使用 PostgreSQL（`docker compose --profile postgres up -d`）
3. **TLS** — 配置网关 TLS 或使用反向代理（Nginx / Caddy）
4. **监控** — 启用 Prometheus 指标采集，配合 Grafana 可视化
5. **日志** — 使用 JSON 格式输出，方便日志系统采集
6. **资源限制** — Docker 部署时设置 CPU/内存限制

---

## 📄 许可证

本项目采用 **GNU General Public License v3.0**。详见 [LICENSE](./LICENSE) 文件。

### GPLv3 核心要求

- 你可以自由使用、修改、分发本软件
- 如果分发修改后的版本，**必须开源**并以 GPLv3 许可
- 如果以商业方式分发（包括 SaaS），需提供源码访问
- 保留版权声明和许可声明

---

## 🌟 致谢

- [tokio](https://tokio.rs) — 异步运行时
- [axum](https://github.com/tokio-rs/axum) — Web 框架
- 各 IM 平台官方 Bot API / SDK
- 所有贡献者 ❤️
