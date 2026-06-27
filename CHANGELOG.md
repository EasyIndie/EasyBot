# Changelog

All notable changes to EasyBot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Linux builds switched to musl** for fully static binaries. Release artifacts for
  `x86_64` and `aarch64` Linux now use `*-unknown-linux-musl` targets via
  `cargo-zigbuild`. Solves `GLIBC_X.XX not found` errors on older Linux systems
  (e.g. Raspberry Pi) — binary runs on any Linux without glibc dependency.
- SQLite is now compiled from source (`sqlite-bundled`) for all targets, removing
  the runtime dependency on system `libsqlite3`.
- macOS builds now set `MACOSX_DEPLOYMENT_TARGET=10.15` (x86_64) and
  `MACOSX_DEPLOYMENT_TARGET=11.0` (aarch64) for better cross-version compatibility.
- CI now includes a `musl-check` job that verifies musl compilation and static
  linking on every push/PR.
- Docker release image now packages musl-static binaries (no functional change to
  the container runtime).

### Added

- Optional macOS code signing + notarization support in release workflow. When
  Apple Developer ID credentials are configured as GitHub Secrets, macOS binaries
  are automatically signed and notarized for Gatekeeper compatibility.
- `.cargo/config.toml` with musl build documentation for local development.

### Fixed

- Release workflow no longer creates Git tags on version bump; tags are now
  created only after successful binary builds, preventing orphaned version tags
  when a release fails mid-way.
- macOS CI failure caused by `pip3` PEP 668 externally-managed-environment error
  when installing `cargo-zigbuild` on macOS runners (only install for musl targets).
- Musl builds now use `--bin easybot` to avoid workspace cdylib (`mock-adapter`)
  incompatibility with `*-linux-musl` targets.

## [0.0.2] - 2026-06-26

### Added

- Web UI management pages: home page, API documentation browser, and admin dashboard
  with real-time system monitoring (CPU, memory, process count).
- Admin password-based authentication for the admin dashboard.
- Cross-platform service management: `easybot service install/uninstall/status/start/stop`
  commands with systemd (Linux), launchd (macOS), and auto-run script (Windows).
- `CARGO_NET_RETRY`, `CARGO_HTTP_TIMEOUT`, `CARGO_HTTP_MULTIPLEXING` env vars in Dockerfile
  for robust crate downloads during Docker builds (fixes HTTP/2 connection reset errors).

### Changed

- Enhanced health endpoint to include admin auth status and uptime info.
- Improved `build.rs` to automatically regenerate Swagger/OpenAPI docs.

### Fixed

- Docker build failure due to transient crates.io HTTP/2 connection resets
  ([run #122](https://github.com/EasyIndie/EasyBot/actions/runs/28235374371)).
- Health test snapshot updated for new version format.

## [0.0.1] - 2026-06-26

### Added

- Five platform IM adapters: Telegram, Discord, Feishu (飞书), QQ, WeChat (微信).
- REST API at `/api/v1/` with endpoints for health, adapters, messages, sessions,
  chats, config, WebSocket, Prometheus metrics, and Swagger UI.
- Event bus with WebSocket push and webhook delivery for real-time event streaming.
- API key authentication (Argon2 hashing), rate limiting, and config hot-reload.
- Plugin system with SDK, dynamic library loading, and plugin registry.
- Configuration: YAML + local overrides + env var substitution (`${VAR_NAME}`)
  + `.env` file loading.
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
