# Changelog

All notable changes to EasyBot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **P5 Plugin System**: Plugin SDK, dynamic library loading, plugin registry, loader tests, developer docs.
- Pre-push hook: automatically runs verification script before push to catch CI failures early.
- Makefile with `make setup` for automatic hooksPath configuration for new clones.
- `send_draft` streaming: Telegram (sendMessage/editMessageText) + Discord (POST/PATCH).
- General health monitor + auto-reconnect: AdapterManager background health polling with exponential backoff (5s→10s→30s→60s→120s, capped at 300s).
- Health endpoint uptime: `started_at` field in AppState → uptime in seconds.

### Changed
- Adapter startup is now non-blocking: `connect()` runs in background, status is immediately visible as "Connecting".
- `verify.sh` auto-detects WSL cargo, fixing acceptance test failures under git-bash.

### Fixed
- **P0 Security Fixes**: Dev API key log redaction, WebSocket connection limit enforcement, storage path traversal prevention, plugin library path traversal prevention, PostgreSQL connection string redaction.
- **P1 Reliability Fixes**: Rate limiter IP map memory leak (periodic cleanup), SessionManager get_or_create race condition, production TLS enforcement check, config hot-reload input validation, batch-send max targets limit, Docker non-root user.
- **P1 Test Fixes**: E2E mock assertions changed from `expect(0..)` to `expect(1)`, Feishu auth failure test assertions fixed.
- WeChat adapter: 2 `assert!(matches!(...))` panic sites replaced with proper error handling.
- AdapterManager status cache fix: `list_statuses()`/`get_status()` now queries adapter status in real-time.
- QQ adapter: msg_type 7 C2C media message compatibility fix.
- Discord adapter: media attachment references fix, QQ media 11255 HTTP 500 compatibility.
- Feishu adapter: `edit_message` now uses correct PUT method, `delete_message` implemented.
- **Round 2 Audit Fixes (19/20)**: 20 findings (N1-N20) from comprehensive security/quality audit — permission middleware, unwrap removal, SQL injection hardening, CORS conditional, body size limits, WebSocket frame size limits, QQ Mutex migration, storage error logging, webhook error handling, client OnceLock patterns, new_with_event_bus removal, capabilities macro, register_adapter macro, SECURITY.md plugin sandbox docs, QQ/WeChat adapter file splitting, and more.
- **E2E script robustness**: e2e-real.sh exit handling, API key extraction from redacted logs, WSL cargo auto-detection, git root detection via rev-parse.
- EventBus subscribe_many race condition fix (try_recv polling fallback).
- compile fixes: unsafe_code lint exemption, clippy warnings, middleware ordering, register_adapter! macro event_bus move, workspace tokio-tungstenite default-features.

### Platform Limitations (by design)
- **WeChat (iLink Bot API)**: No edit/delete/send_interactive/list_chats support (API has only 7 endpoints, one-on-one chat only).
- **Feishu**: No ChatList, Streaming, or TypingIndicator (platform API limitation).
- **QQ**: No Audio/Video/Document media, no Streaming (platform API limitation).
- **Telegram**: No ChatList, no Thread support (platform API limitation).

## [0.1.0] - 2026-06-21

### Added
- **P1 MVP**: Core types, PlatformAdapter trait, Telegram adapter, REST API, config loading, cross-platform paths.
- **P2 Bidirectional**: Event bus, WebSocket push, webhooks, inbound message handling, session persistence, message edit/delete, adapter lifecycle events.
- **P3 Multi-platform**: Five platform adapters — Telegram, Discord, Feishu (飞书), QQ, WeChat (微信).
- **P4 Production**: API key auth (Argon2), rate limiting, hot-reload, graceful shutdown, PostgreSQL, Prometheus metrics, Docker, TTL retention.
- REST API at `/api/v1/` with 18 endpoints (health, adapters, messages, sessions, chats, config, WebSocket, metrics, Swagger UI).
- Configuration: YAML + `.local.yaml` merge + `${VAR_NAME}` env substitution + `.env` file loading.
- Adapter auto-enable via credential environment variable detection.
