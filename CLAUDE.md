# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# Build everything
cargo build

# Build with all features
cargo build --features full

# Build with plugin system
cargo build --features "full,plugin-system"

# Build plugin test adapter (required before integration tests)
cargo build -p mock-adapter

# Run all tests
cargo test

# Run all tests with plugin system
cargo test --features "full,plugin-system"

# Run tests in a specific crate
cargo test -p easybot-core
cargo test -p easybot-adapter-telegram

# Run integration tests (compile mock-adapter first)
cargo build -p mock-adapter && cargo test -p integration-tests

# Run config/env tests
cargo test -p easybot-core config::tests

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
│  DiscordAdapter   (Gateway WebSocket)                     │
│  FeishuAdapter    (WebSocket 事件订阅)                    │
│  QQAdapter        (统一 QQBot 鉴权 + Gateway WebSocket)   │
│  WeChatAdapter    (个人微信 iLink Bot API 长轮询)         │
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
| `crates/easybot-adapter-wechat` | 个人微信 (WeChat) 适配器（iLink Bot API 长轮询） |
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
├── .env                      # Secrets (chmod 600, loaded via dotenvy)
├── data/gateway.db           # SQLite database (auto-created)
└── logs/                     # Log files
```

Config supports `${VAR_NAME}` for environment variable substitution and merges `gateway.local.yaml` on top of `gateway.yaml`.
Environment variable priority: `export` / Docker `environment:` > `.env` file (loaded via `dotenvy::from_path`, does not override existing vars).
Run `easybot --init` to create `gateway.yaml` and `.env` (template with all known variables).
Adapters auto-enable via credential env var detection — no need to manually set `enabled: true` in YAML config.

### Adapter Lifecycle

```
init(config) → connect() → send()/send_media()/... → disconnect()
   ↓              ↓
 Created →   Starting → Connected → Reconnecting → Failed → Stopped
```

The `AdapterRegistry` holds factory functions keyed by platform name, each with declared credential environment variable names. `AdapterManager::start_all()` iterates registered adapters (not config entries), auto-detects credentials via env vars, and starts adapters whose credentials are present. `AdapterConfig.enabled` is `Option<bool>`: `None` auto-detects, `Some(true)` forces enable, `Some(false)` forces disable. Built-in adapters are registered at startup in `bin/main.rs`.

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
| **P3 Multi-platform** | Telegram ✅, Discord ✅, **飞书/Lark** ✅, **QQ** ✅ (群消息已验证, C2C/频道代码已实现待验证环境), **个人微信(wechat)** ✅ (iLink Bot API 已验证) — 五个平台 + 媒体发送 | 85% ✅ |
| **P4 Production** | API key auth (Argon2), rate limiting, hot-reload, graceful shutdown, PostgreSQL, Prometheus, Docker, TTL retention | 75% ✅ |

> **P3 未完成项**: Discord `send_media`/`send_interactive`、微信 `edit_message`/`delete_message`/`send_interactive`、所有适配器 `list_chats` 实际实现。
> **P4 未完成项**: 权限模型 RBAC (`auth/permissions.rs`)、`send_draft` 流式草稿、通用适配器健康轮询/自动重连（仅 Discord 实现了 Gateway 重连）、TLS 仅配置层未在应用层处理。
| **P5 Plugin System** | Plugin SDK, dynamic library loading, plugin registry, loader tests, developer docs | ✅ Done |

### 不可退让的设计约束

- **必须同时支持 Docker 部署和独立运行**。任何功能迭代不得引入仅 Docker 可用的能力，也不得引入仅裸机可用的能力。核心功能的配置、运行、调试路径在两种模式中必须一致。测试默认在独立运行模式下执行。

### Key Patterns

- **Error handling**: Use `GatewayError` enum, convert to API via `ApiError` newtype in `easybot-api::response`
- **Adapter creation**: Register factory in `AdapterRegistry`, let `AdapterManager` handle lifecycle
- **Event bus**: Publish via `EventBus::publish()`, subscribe via `EventBus::subscribe()` — tokio broadcast channels under the hood
- **Session key format**: `"{platform}:{chatId}"` or `"{platform}:{chatId}:{threadId}"`
- **Target format** (API): `"{platform}:{chatId}"` — parsed by `parse_target()` in messages route
- **Config precedence**: YAML → `.local.yaml` merge → env var substitution (`${VAR_NAME}`); env vars sourced from `export` > Docker env > `.env` file (loaded via `dotenvy::from_path` before config loading)
- **Env var loading**: Call `load_env(&EasyBotPaths)` at startup (in `bin/main.rs`) before `load_config()`; `.env` file lives at `{config_dir}/.env`; run `easybot --init` to generate `.env` template
- **Adapter auto-enable**: `start_all()` traverses registry, checks `credential_env_vars` per platform; adapters with credentials set auto-enable without needing `enabled: true` in YAML

## Known Gaps & TODO

Detailed tracking: see `docs/TODO.md` for the full prioritized checklist.

### P3: Multi-Platform (85% → target: 100%)

| Gap | Platform | File | Description |
|-----|----------|------|-------------|
| `send_media` | **Discord** | `crates/easybot-adapter-discord/src/lib.rs` | Send images/audio/video/files via Discord REST API |
| `send_interactive` | **Discord** | same | Inline keyboard / button messages |
| `list_chats` | **Discord** | same | List available guilds/channels |
| `edit_message` | **WeChat** | `crates/easybot-adapter-wechat/src/lib.rs` | Edit previously sent messages |
| `delete_message` | **WeChat** | same | Delete/recall messages |
| `send_interactive` | **WeChat** | same | Interactive button messages |
| `send_interactive` | **QQ** | `crates/easybot-adapter-qq/src/lib.rs` | Interactive button/keyboard messages |
| `list_chats` | **QQ / WeChat** | respective adapters | Return actual chat lists (currently empty vec) |

### P4: Production (75% → target: 100%)

| Gap | File | Description |
|-----|------|-------------|
| Permission model (RBAC) | `crates/easybot-core/src/auth/` (new `permissions.rs`) | Role-based access control with permission-check middleware |
| `send_draft` streaming | PlatformAdapter trait + adapters | Streaming draft send method (`send_draft` defined in trait, no adapter implements it) |
| Health poll + auto-reconnect | `crates/easybot-core/src/adapter/manager.rs` | Periodic `health()` checks for all adapters with automatic reconnect; currently only Discord Gateway has its own reconnect loop |
| TLS/HTTPS termination | `crates/easybot-api/src/server.rs` | TLS config exists but cert loading/serving not wired at application layer |
| Health: track start time | `crates/easybot-api/src/routes/health.rs:56` | Record and expose gateway process start time in health endpoint |
