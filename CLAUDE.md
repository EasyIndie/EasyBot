# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# Build everything
cargo build

# Build with all features
cargo build --features full

# Run all tests
cargo test

# Run tests in a specific crate
cargo test -p easybot-core
cargo test -p easybot-adapter-telegram

# Run a single test
cargo test -p easybot-core test_get_or_create -- --exact

# Run only unit tests (no doc tests)
cargo test --lib

# Check compilation without producing artifacts
cargo check

# Run the service (for testing)
cargo run -- --debug

# Initialize config directory
cargo run -- --init --dir /tmp/easybot-test

# Lint
cargo clippy --all-targets

# Format
cargo fmt
```

## Architecture Overview

EasyBot is an independent **IM Gateway** service connecting multiple instant messaging platforms (Telegram, Discord, 飞书/Lark, QQ, WeChat) and exposing a unified REST API + WebSocket for third-party clients. Written in Rust with a tokio + axum stack.

### Three-Layer Architecture

```
                                External Clients
                                      ↕
┌───────────────── API Layer (easybot-api) ─────────────────┐
│  axum HTTP server · REST routes · WebSocket event push    │
│  ApiError newtype for IntoResponse                        │
└─────────────────────────┬─────────────────────────────────┘
                          ↕
┌────────────── Core Logic (easybot-core) ─────────────────┐
│  EventBus        SessionManager    AdapterManager        │
│  (broadcast)     (DashMap store)   (registry + lifecycle) │
│  ApiKeyManager   ConfigLoader      DeliveryRouter (TBD)   │
└─────────────────────────┬─────────────────────────────────┘
                          ↕
┌─────────── Adapter Layer (easybot-adapter-*) ────────────┐
│  TelegramAdapter  (implements PlatformAdapter trait)      │
│  DiscordAdapter   (Gateway WebSocket)                      │
│  FeishuAdapter    (WebSocket 事件订阅)                           │
│  QQAdapter        (统一 QQBot 鉴权 + Gateway WebSocket)         │
│  WeChatAdapter (wecom)  (企业微信 REST API + 群机器人 Webhook) │
│  WeChatAdapter (wechat) (个人微信 iLink Bot API 长轮询)         │
└──────────────────────────────────────────────────────────┘
```

### Crate Layout (workspace)

| Crate | Purpose |
|-------|---------|
| `bin/` | Binary entry: CLI args, component wiring, signal handling |
| `crates/easybot-core` | Core library: types, event bus, sessions, adapter management, auth, config, storage (SQLite/PostgreSQL) |
| `crates/easybot-api` | API layer: axum server, REST routes, WebSocket, error responses, Prometheus metrics, rate limiting |
| `crates/easybot-adapter-telegram` | Telegram Bot API adapter |
| `crates/easybot-adapter-discord` | Discord Bot API / Gateway adapter |
| `crates/easybot-adapter-feishu` | 飞书/Lark 适配器（REST API + WebSocket 事件订阅） |
| `crates/easybot-adapter-qq` | QQ 机器人适配器（统一 QQBot 鉴权 + Gateway WebSocket） |
| `crates/easybot-adapter-wechat` | 企业微信 (WeCom) 适配器（应用消息推送 + 群机器人 Webhook） |
| `crates/easybot-plugin-sdk` | Re-exports core types for third-party plugin devs |

### Core Types (`easybot-core/src/types/`)

- **`adapter.rs`** — `PlatformAdapter` trait (the central abstraction every IM connector must implement): `init()`, `connect()`, `disconnect()`, `send()`, `send_media()`, `get_chat_info()`, plus capability declaration
- **`message.rs`** — `InboundMessage`, `OutboundMessage`, `SendTextParams`, `SendResult`, `MediaAttachment`, `InlineKeyboard`, `CallbackEvent`
- **`session.rs`** — `Session`, `SessionSource`, `ResetPolicy`; session key is `platform:chatId[:threadId]`
- **`event.rs`** — `GatewayEvent` with event type constants (`message.inbound`, `adapter.connected`, etc.)
- **`error.rs`** — `GatewayError` enum with error codes, HTTP status mapping, `BoxFuture` type alias
- **`config.rs`** — `GatewayConfig` matching YAML structure: `ServerConfig`, `ApiConfig`, `StorageConfig`, `AdapterConfig`

### Configuration Directory

User-level config stored at `~/.easybot/` (macOS/Linux) or `%APPDATA%\easybot\` (Windows). Resolution priority: CLI `--dir` > `EASYBOT_HOME` env var > `~/.easybot/` (legacy) > platform standard dir.

```
~/.easybot/
├── gateway.yaml              # Base config (version-controlled)
├── gateway.local.yaml        # Local overrides (.gitignore)
├── .env                      # Secrets (chmod 600)
├── data/gateway.db           # SQLite database (auto-created)
└── logs/                     # Log files
```

Config supports `${VAR_NAME}` for environment variable substitution and merges `gateway.local.yaml` on top of `gateway.yaml`.

### Adapter Lifecycle

```
init(config) → connect() → send()/send_media()/... → disconnect()
   ↓              ↓
 Created →   Starting → Connected → Reconnecting → Failed → Stopped
```

The `AdapterRegistry` holds factory functions keyed by platform name. `AdapterManager::start_all()` iterates enabled adapters from config, creates them through the registry, calls init then connect. Built-in adapters are registered at startup in `bin/main.rs`.

### API Routes (base path: `/api/v1`)

| Path | Method | Handler |
|------|--------|---------|
| `/health` | GET | Health check (connected adapters, session count) |
| `/adapters` | GET | List all adapters with status |
| `/adapters/{platform}/start` | POST | Start an adapter |
| `/adapters/{platform}/stop` | POST | Stop an adapter |
| `/adapters/{platform}/status` | GET | Adapter health detail |
| `/messages/send` | POST | Send message to a chat (`target: "platform:chatId"`) |
| `/messages/batch-send` | POST | Send to multiple targets |
| `/messages/{id}` | PUT | Edit message |
| `/messages/{id}` | DELETE | Delete message |
| `/messages` | GET | Message history (supports `?platform=` filter) |
| `/sessions` | GET | List active sessions |
| `/sessions/{key}` | GET | Get session details |
| `/sessions/{key}` | DELETE | Delete session |
| `/chats/{platform}` | GET | List chats for platform |
| `/chats/{platform}/{chat_id}` | GET | Get chat info |
| `/config` | GET | Get current config |
| `/config` | PUT | Update config (hot-reload) |
| `/ws` | GET | WebSocket real-time event stream (需先发送 `{"token":"..."}` 认证) |
| `/metrics` | GET | Prometheus metrics |

### Implementation Roadmap

| Phase | Scope | Status |
|-------|-------|--------|
| **P1 MVP** | Core types, PlatformAdapter trait, Telegram adapter, REST API, config loading, cross-platform paths | ✅ Done |
| **P2 Bidirectional** | Event bus, WebSocket push, webhooks, inbound message handling, session persistence, message edit/delete, adapter lifecycle events | 100% ✅ |
| **P3 Multi-platform** | Telegram ✅, Discord ✅, **飞书/Lark** ✅, **QQ** ✅ (群消息已验证, 频道 TODO), **企业微信(wecom)** ❌ (待验证), **个人微信(wechat)** 💻 (代码就绪待验证) — 五个平台 + 媒体发送 | 80% ✅ |
| **P4 Production** | API key auth (Argon2), rate limiting, hot-reload, graceful shutdown, PostgreSQL, Prometheus, Docker, TTL retention | 80% ✅ (均已完成，仅剩生产环境打磨) |
| **P5 Plugin System** | Plugin SDK, dynamic library loading, plugin registry | ❌ Not started |

### 不可退让的设计约束

- **必须同时支持 Docker 部署和独立运行**。任何功能迭代不得引入仅 Docker 可用的能力，也不得引入仅裸机可用的能力。核心功能的配置、运行、调试路径在两种模式中必须一致。测试默认在独立运行模式下执行。

### Key Patterns

- **Error handling**: Use `GatewayError` enum, convert to API via `ApiError` newtype in `easybot-api::response`
- **Adapter creation**: Register factory in `AdapterRegistry`, let `AdapterManager` handle lifecycle
- **Event bus**: Publish via `EventBus::publish()`, subscribe via `EventBus::subscribe()` — tokio broadcast channels under the hood
- **Session key format**: `"{platform}:{chatId}"` or `"{platform}:{chatId}:{threadId}"`
- **Target format** (API): `"{platform}:{chatId}"` — parsed by `parse_target()` in messages route
- **Config precedence**: YAML → `.local.yaml` merge → env var substitution (`${VAR_NAME}`)
