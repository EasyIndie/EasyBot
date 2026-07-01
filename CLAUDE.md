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

### 模板文件构建系统（重要：避免改错文件）

`crates/easybot-api/build.rs` 在编译时从源文件生成 HTML 产物，产物不在 git 中跟踪（见 `.gitignore`）。

| 产物（生成） | 源文件（直接修改） | 修改方式 |
|-------------|-------------------|---------|
| `templates/admin.html` ❌ | `templates/admin_layout.html` + `templates/js/admin.js` + `templates/css/admin.css` + 图片 | 改 `admin_layout.html` / `admin.js` / `admin.css` |
| `templates/docs.html` ❌ | `templates/docs_layout.html` + `docs/*.md` + `templates/vendor/` | 改 `docs_layout.html` 或 `docs/` 下的 Markdown |
| `templates/home.html` ❌ | `templates/home_layout.html` + 图片 | 改 `home_layout.html` |

**规则：产物文件不要直接编辑，修改源文件后 `cargo build` 自动重新生成。** 产物文件已被 `.gitignore` 排除。

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
| `tests/integration` | Integration tests for plugin system |
| `tests/e2e` | End-to-end tests across adapters |
| `tests/plugins/mock-adapter` | Mock adapter for plugin system integration testing |
| `tests/fixtures` | Shared test fixtures for adapter crates |

### Core Types (`easybot-core/src/types/`)

- **`adapter.rs`** — `PlatformAdapter` trait (the central abstraction every IM connector must implement): `init()`, `connect()`, `disconnect()`, `send()`, `send_media()`, `get_chat_info()`, plus capability declaration
- **`message.rs`** — `InboundMessage`, `OutboundMessage`, `SendTextParams`, `SendResult`, `MediaAttachment`, `InlineKeyboard`, `CallbackEvent`
- **`session.rs`** — `Session`, `SessionSource`, `ResetPolicy`; session key is `platform:chatId[:threadId]`
- **`event.rs`** — `GatewayEvent` with event type constants (`message.inbound`, `adapter.connected`, etc.)
- **`error.rs`** — `GatewayError` enum with error codes, HTTP status mapping, `BoxFuture` type alias
- **`config.rs`** — `GatewayConfig` matching YAML structure: `ServerConfig`, `ApiConfig`, `StorageConfig`, `AdapterConfig`

### Configuration Directory

User-level config stored at `~/.easybot/` (macOS/Linux) or `%APPDATA%\easybot\` (Windows). Resolution priority: CLI `--dir` > `EASYBOT_HOME` env var > platform default (`~/.easybot/` on macOS/Linux, `%APPDATA%\easybot\` on Windows).

```
~/.easybot/
├── gateway.yaml              # Base config (version-controlled)
├── gateway.local.yaml        # Local overrides (.gitignore)
├── .env                      # Secrets (chmod 600, loaded via dotenvy)
├── data/gateway.db           # SQLite database (auto-created)
├── logs/                     # Log files
├── plugins/                  # Third-party adapter plugins
├── certs/                    # TLS certificates (optional)
└── secrets/                  # Key storage (optional, chmod 600)
```

Config supports `${VAR_NAME}` for environment variable substitution and merges `gateway.local.yaml` on top of `gateway.yaml`.
Environment variable priority: `export` / Docker `environment:` > `.env` file (loaded via `dotenvy::from_path`, does not override existing vars).
Run `easybot --init` to create `gateway.yaml`, `gateway.local.yaml` (example override), and `.env` (template with all known variables).
Adapters auto-enable via credential env var detection — no need to manually set `enabled: true` in YAML config.
**IMPORTANT for `gateway.local.yaml`**: Adapter overrides MUST be under `adapters:` key, NOT at YAML top level (serde silently ignores unknown struct fields).

### Adapter Lifecycle

```
init(config) → connect() → send()/send_media()/... → disconnect()
   ↓              ↓
 Created →   Starting → Connecting → Connected → Reconnecting → Failed → Stopped
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
| `/messages/{message_id}` | PUT | Edit message |
| `/messages/{message_id}` | DELETE | Delete message |
| `/messages` | GET | Message history (supports `?platform=` filter) |
| `/sessions` | GET | List active sessions |
| `/sessions/{key}` | GET | Get session details |
| `/sessions/{key}` | DELETE | Delete session |
| `/chats/{platform}` | GET | List chats for platform |
| `/chats/{platform}/{chat_id}` | GET | Get chat info |
| `/config` | GET | Get current config |
| `/config` | PUT | Update config (hot-reload) |
| `/ws` | GET | WebSocket real-time event stream (HTTP upgrade 需 `Authorization: Bearer <key>` 头，连接后发送 `{"token":"..."}` 二次认证) |
| `/metrics` | GET | Prometheus metrics |
| `/swagger` | GET | Swagger UI (OpenAPI 文档浏览器) |
| `/openapi.json` | GET | OpenAPI 3.1 JSON schema |

### Implementation Roadmap

| Phase | Scope | Status |
|-------|-------|--------|
| **P1 MVP** | Core types, PlatformAdapter trait, Telegram adapter, REST API, config loading, cross-platform paths | ✅ Done |
| **P2 Bidirectional** | Event bus, WebSocket push, webhooks, inbound message handling, session persistence, message edit/delete, adapter lifecycle events | 100% ✅ |
| **P3 Multi-platform** | Telegram ✅, Discord ✅ (含 send_interactive + list_chats), **飞书/Lark** ✅, **QQ** ✅ (含 send_interactive + list_chats), **个人微信(wechat)** ✅ (iLink Bot API 已验证; edit/delete/send_interactive/list_chats 平台不支持) — 五平台全部完成 | 100% ✅ |
| **P4 Production** | API key auth (Argon2), rate limiting, hot-reload, graceful shutdown, PostgreSQL, Prometheus, Docker, TTL retention, health monitor + auto-reconnect, send_draft streaming, health uptime, QQ real-env verification, status cache fix | 95% ✅ |

> **P3 完成**: 所有可实现功能已交付，微信平台限制项 (edit/delete/send_interactive/list_chats) 已确认关闭。
> **P4 未完成项**: 权限模型 RBAC (`auth/permissions.rs`)、TLS 仅配置层未在应用层处理（均暂缓）。
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

### P3: Multi-Platform (100% complete)

All P3 features are delivered. WeChat platform limitations (edit/delete/send_interactive/list_chats) confirmed and documented.

### P4: Production (95% complete — 2 items deferred)

| Gap | File | Description |
|-----|------|-------------|
| Permission model (RBAC) | `crates/easybot-core/src/auth/` (new `permissions.rs`) | Role-based access control with permission-check middleware (暂缓) |
| TLS/HTTPS termination | `crates/easybot-api/src/server.rs` | TLS config exists but cert loading/serving not wired at application layer (暂缓) |
