# Changelog

All notable changes to EasyBot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.2] - 2026-06-26

### Added

- Web UI management pages: home, API documentation, admin dashboard with system monitoring.
- Admin password-based authentication for the admin dashboard.
- Cross-platform service management: `easybot service install/uninstall/status/start/stop` commands
  with systemd (Linux), launchd (macOS), and auto-run script (Windows).
- `CARGO_NET_RETRY` / `CARGO_HTTP_TIMEOUT` / `CARGO_HTTP_MULTIPLEXING` env vars in Dockerfile
  for robust crate downloads during Docker builds (fixes HTTP/2 connection reset errors).

### Changed

- Enhanced health endpoint to include admin auth status and uptime.
- Improved build.rs to automatically regenerate Swagger/OpenAPI docs.

### Fixed

- Docker build failure due to transient crates.io HTTP/2 connection resets
  (run [#122](https://github.com/EasyIndie/EasyBot/actions/runs/28235374371)).
- Preventitive: Telegram health test snapshot updated for new version format.

## [0.0.1] - 2026-06-26

### Added

- Five platform IM adapters: Telegram, Discord, Feishu (飞书), QQ, WeChat (微信).
- REST API at `/api/v1/` with health, adapters, messages, sessions, chats, config, WebSocket, metrics, Swagger UI endpoints.
- Event bus with WebSocket push and webhook delivery for real-time event streaming.
- API key authentication (Argon2), rate limiting, hot-reload.
- Plugin system with SDK, dynamic library loading, and plugin registry.
- Configuration: YAML + local overrides + env var substitution + `.env` file loading.
- SQLite and PostgreSQL storage with session persistence and TTL retention.
- Prometheus metrics endpoint.
- Docker support with multi-arch images.

### Platform Capabilities

| Feature | Telegram | Discord | Feishu | QQ | WeChat |
|---------|----------|---------|--------|-----|--------|
| Send text | ✅ | ✅ | ✅ | ✅ | ✅ |
| Send media | ✅ | ✅ | ✅ | ✅ | ✅ |
| Send interactive | ✅ | ✅ | ✅ | ✅ | ❌ |
| Edit message | ✅ | ✅ | ✅ | ✅ | ❌ |
| Delete message | ✅ | ✅ | ✅ | ✅ | ❌ |
| List chats | ❌ | ✅ | ❌ | ✅ | ❌ |
| Inbound events | ✅ | ✅ | ✅ | ✅ | ✅ |
| Group/channel | ✅ | ✅ | ✅ | ✅ | ❌ |
