# Changelog

All notable changes to EasyBot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Documentation overhaul: deleted 3 outdated historical docs (rust-implementation-plan.md,
  AUDIT_FIX_PLAN.md, api-capabilities-research.md). Merged api-capabilities-research.md
  into platform-capabilities.md. Simplified TEST_PLAN.md (removed verbose expected-result
  columns). Updated im-gateway-architecture.md with 14 categories of corrections to match
  actual implementation (API routes, CLI commands, plugin system, lifecycle states, etc.).
  Updated TODO.md and frontend-plan.md with current completion status.
  Build.rs automatically regenerates docs.html.

## [0.0.5] - 2026-06-27

### Fixed

- Admin dashboard adapter start/stop buttons now poll the adapter status
  endpoint until the state stabilises (Connected/Failed/Disconnected),
  showing immediate optimistic feedback ("启动中..." / "停止中...") instead
  of relying on a fixed 100 ms delay before re-rendering the full list.
- `GET /api/v1/config` now returns the actual runtime values for config fields
  that are overridden after YAML loading (admin password from env var, resolved
  storage path, defaulted connection string, unknown storage-type fallback).
  Oversight corrected by sinking runtime overrides into `ConfigManager`.
- `POST /api/v1/adapters/{platform}/start` now injects credentials from
  environment variables (same as `start_all()`), so adapters stopped via the
  admin dashboard can be restarted manually. Init failures also update the
  status cache to `Failed`, preventing the frontend from showing stale state.
  The admin panel's start/stop buttons now check the API response and show
  error alerts on failure.
- `easybot.sh install` no longer fails to find the binary on Raspberry Pi
  (musl-based systems where `file` reports Linux binary as "data").
- `gateway.local.yaml` adapter overrides are no longer silently ignored when
  placed under `adapters:` key (serde unknown-field deserialization fix).
- Default config directory now uses `~/.easybot` on macOS/Linux consistently,
  instead of falling back to the legacy `~/.config/easybot` path.

### Changed

- Pre-commit hook (`scripts/pre-commit`) now also runs `cargo clippy --all-targets -- -D warnings`, catching clippy issues before they reach the pre-push verification suite.

### Removed

- Release Drafter workflow (`release-drafter.yml`) and its config — unused, no downstream workflow consumes its draft releases. The v0.0.5 draft release on GitHub has been cleaned up.

## [0.0.4] - 2026-06-27

### Fixed

- `EASYBOT_ADMIN_PASSWORD` environment variable now correctly overrides the
  `admin_password` value from `gateway.yaml` at all config loading stages.
- Generated systemd service unit now sets the correct `User=` and `Group=`
  by detecting the current user and their primary group via `whoami` + `id -gn`.
- Stale GitHub release artifacts no longer accumulate; a cleanup step removes
  drafts from the same tag before publishing a new release.

### Changed

- Release workflow migrated to tag-driven trigger (`git tag v0.0.x && git push
  --tags`), replacing the previous workflow-dispatch + manual-version-input
  approach.
- `gateway.local.yaml` template expanded with all five adapter platform override
  examples and a clear comment that overrides must be under the `adapters:` key,
  not at the YAML root.

## [0.0.3] - 2026-06-27

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
