# CLAUDE.md

Guidance for Claude Code when working on EasyBot.

## Build & Test

```bash
# All common tasks via Makefile
make           # show help
make run       # run with --debug
make run-fresh # fresh init + run in /tmp/easybot-dev
make watch     # auto-rebuild on save (needs cargo-watch)
make test      # cargo test --workspace
make verify    # full CI check
make lint      # fmt check + clippy

# Notable raw cargo commands
cargo build -p mock-adapter                      # before integration tests
cargo test -p easybot-core config::tests         # config/env tests only
cargo test -p easybot-core test_get_or_create -- --exact  # single test
```

## Architecture

EasyBot is an **IM Gateway** connecting Telegram / Discord / 飞书 / QQ / WeChat, exposing a unified REST + WebSocket API. Rust + tokio + axum.

```
External Clients
     ↕
API (easybot-api)         axum · REST · WebSocket · ApiError
     ↕
Core (easybot-core)       EventBus · SessionManager · AdapterManager · ApiKeyManager · ConfigLoader
     ↕
Adapters (easybot-adapter-*)  Telegram · Discord · 飞书 · QQ · WeChat
```

### 模板构建系统（别改错文件）

`build.rs` 从源文件生成 HTML 到 `templates/gen/`（gitignore）。改源文件，不要改产物。

| 产物 (templates/gen/) | 源文件 |
|---|---|
| `admin.html` | `templates/admin_layout.html` + `admin.js` + `admin.css` |
| `docs.html` | `templates/docs_layout.html` + `docs/*.md` + `vendor/` |
| `home.html` | `templates/home_layout.html` |

### Crate Layout

| Crate | Role |
|---|---|
| `bin/` | CLI args, wiring, signal handling |
| `crates/easybot-core` | Core: types, event bus, sessions, adapters, auth, config, storage (SQLite/PG) |
| `crates/easybot-api` | Axum server, REST, WebSocket, metrics, rate limiting, error responses |
| `crates/easybot-adapter-telegram` | Telegram Bot API |
| `crates/easybot-adapter-discord` | Discord Gateway |
| `crates/easybot-adapter-feishu` | 飞书 REST + WebSocket |
| `crates/easybot-adapter-qq` | QQBot Gateway |
| `crates/easybot-adapter-wechat` | 个人微信 iLink Bot API 长轮询 |
| `crates/easybot-plugin-sdk` | Re-exports core types for plugins |
| `tests/` | Integration, e2e, mock-adapter, fixtures |

### Core Types (`easybot-core/src/types/`)

- **`adapter.rs`** — `PlatformAdapter` trait: `init()`, `connect()`, `disconnect()`, `send()`, `send_media()`, `get_chat_info()`, capability declarations
- **`message.rs`** — `InboundMessage`, `OutboundMessage`, `SendTextParams`, `SendResult`, `MediaAttachment`, `InlineKeyboard`, `CallbackEvent`
- **`session.rs`** — `Session`, `SessionSource`, `ResetPolicy`; key = `platform:chatId[:threadId]`
- **`event.rs`** — `GatewayEvent` + constants (`message.inbound`, `adapter.connected`, ...)
- **`error.rs`** — `GatewayError` with error codes, HTTP mapping, `BoxFuture`
- **`config.rs`** — `GatewayConfig` mapping YAML: `ServerConfig`, `ApiConfig`, `StorageConfig`, `AdapterConfig`

### Config Directory

Priority: `--dir` > `EASYBOT_HOME` > `~/.easybot/` (macOS/Linux) / `%APPDATA%\easybot\` (Windows).

```
~/.easybot/
├── gateway.yaml            # Base config (VCS)
├── gateway.local.yaml      # Overrides (.gitignore) — adapters MUST go under `adapters:` key
├── .env                    # Secrets (chmod 600, loaded via dotenvy)
├── data/gateway.db         # SQLite (auto-created)
├── logs/ plugins/ certs/ secrets/
```

Config supports `${VAR_NAME}` substitution; `.local.yaml` merges on top. Env priority: export / Docker > `.env`. Run `easybot --init` to scaffold. Adapters auto-enable when their credential env vars are present — no `enabled: true` needed.

### Adapter Lifecycle

```
init(config) → connect() → send()/... → disconnect()
  Created → Starting → Connecting → Connected → Reconnecting → Failed → Stopped
```

`AdapterRegistry` holds factories keyed by platform. `AdapterManager::start_all()` auto-detects credentials and starts adapters. `AdapterConfig.enabled`: `None`=auto, `Some(true/false)`=force.

### API Routes (prefix: `/api/v1`)

| Path | Method | Description |
|---|---|---|
| `/health` | GET | Health (connected adapters, session count) |
| `/adapters` | GET | List all with status |
| `/adapters/{p}/start\|stop\|status` | POST/GET | Lifecycle |
| `/messages/send` | POST | Send (`target: "platform:chatId"`) |
| `/messages/batch-send` | POST | Multi-target send |
| `/messages/{id}` | PUT/DELETE | Edit/delete message |
| `/messages` | GET | History (`?platform=` filter) |
| `/sessions` | GET | List active |
| `/sessions/{key}` | GET/DELETE | Details / delete |
| `/chats/{p}[/{chat_id}]` | GET | List / info |
| `/config` | GET/PUT | Get / hot-reload |
| `/ws` | GET | WebSocket stream (Auth: Bearer header + `{"token":"..."}`) |
| `/metrics` | GET | Prometheus |
| `/swagger` | GET | OpenAPI browser |
| `/openapi.json` | GET | OpenAPI 3.1 schema |

### Roadmap

| Phase | Scope | Status |
|---|---|---|
| **P1 MVP** | Core types, PlatformAdapter trait, Telegram adapter, REST API, config, paths | ✅ |
| **P2 Bidirectional** | Event bus, WebSocket push, inbound handling, session persistence, edit/delete | ✅ |
| **P3 Multi-platform** | Telegram, Discord, 飞书, QQ, 微信 — 五平台全部完成 | ✅ |
| **P4 Production** | Argon2 auth, rate limit, hot-reload, PostgreSQL, Docker, Prometheus, TTL, auto-reconnect, streaming, uptime | 95% (暂缓: RBAC, TLS) |
| **P5 Plugin System** | SDK, dynamic loading, registry, docs | ✅ |

### 不可退让的设计约束

**必须同时支持 Docker 部署和独立运行。** 功能迭代不得引入仅 Docker/仅裸机可用的能力。测试默认在独立运行模式下执行。

### Key Patterns

| Pattern | Detail |
|---|---|
| Error handling | `GatewayError` → `ApiError` newtype `IntoResponse` |
| Adapter creation | Register factory in `AdapterRegistry`, manager handles lifecycle |
| Event bus | `EventBus::publish()` / `subscribe()` — tokio broadcast |
| Session key | `{platform}:{chatId}[:{threadId}]` |
| Target format | `{platform}:{chatId}` — parse via `parse_target()` |
| Env loading | `load_env()` before `load_config()` in `bin/main.rs`; `.env` at `{config_dir}/.env` |
| Config precedence | YAML → `.local.yaml` → `${VAR_NAME}` substitution; export/Docker > `.env` |
| Raw payload passthrough | 各适配器在解析字段前将平台原始 payload 序列化存入 `InboundMessage.metadata`，**仅调试用，不做任何二次处理/消费**。由 `api.raw_payload_enabled`（默认 `false`）控制 WebSocket 事件中是否透传该字段——关闭时 `ws.rs:172-178` 在广播前剥离 `metadata`；可通过配置或 `EASYBOT_RAW_PAYLOAD_ENABLED` 环境变量开启。 |

## Known Gaps

Full checklist: `docs/TODO.md`. P4 deferred items:

| Gap | File | Description |
|---|---|---|
| RBAC | `crates/easybot-core/src/auth/permissions.rs` | Role-based access control (暂缓) |
| TLS termination | `crates/easybot-api/src/server.rs` | Cert loading/serving not wired (暂缓) |
