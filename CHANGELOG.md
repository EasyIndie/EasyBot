# Changelog

All notable changes to EasyBot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.9] - 2026-07-07

## [0.0.8] - 2026-07-07

### Fixed

- **长期运行资源耗尽修复** — 全面审计并修复 8 项资源耗尽可能：
  - SQLite WAL 文件无限增长：新增后台 WAL checkpoint 任务，按 TTL 清理间隔运行 `PRAGMA wal_checkpoint(TRUNCATE)`
  - Webhook 分发无并发控制：新增 `Semaphore` 上限 16 并发，防止事件洪水压垮运行时
  - SessionBridge 每消息 spawn 两个任务：改为内联执行，消除无限制任务增长
  - SessionManager DashMap 内存堆积：新增 `prune_expired()` 方法，按 TTL 周期清理过期会话的内存残留
  - QQ `chat_types` 缓存：4 处插入点加 10,000 条上限，超限时自动清空
  - Telegram `admin_cache` 缓存：插入点加 5,000 条上限
  - Discord `guild_owner_cache` 缓存：2 处插入点加 5,000 条上限
  - 飞书 `role_cache` 30 秒 TTL 实际生效：缓存读取时检查 `Instant::elapsed()`，过期自动移除

## [0.0.7] - 2026-07-07

### Fixed

- **QQ 适配器 Group 媒体消息回归修复** — QQ v2 群聊端点 (`/v2/groups/{id}/messages`)
  不支持 `msg_type: 1` (image embed) 和 `msg_type: 2` (markdown，需要模板权限)。
  `dd5c1cf` (直接路由优化) 让已知 Group chat 跳过三级回退，直接命中群聊端点并发送
  `msg_type: 2`，导致 `40034011 "无效 markdown content"` 或
  `40034127 "无markdown模板权限"`。修复：新增 `send_group_media_upload()` 方法，
  通过文件上传 + `msg_type: 7` (media) 发送群聊媒体消息。新增 2 个回归测试。

### Changed

- **Default features now include all 5 adapters** (`bin/Cargo.toml`): `default = ["adapter-telegram", "adapter-discord", "adapter-feishu", "adapter-qq", "adapter-wechat"]`. Previously only Telegram was enabled by default. `cargo run` / `cargo build` now compiles all platform adapters. To build a subset, use `cargo build --no-default-features --features "adapter-telegram,adapter-discord"`.
- Documentation updated (`README.md`, `CONTRIBUTING.md`) and feature matrix corrected (`scripts/verify.sh`, `.github/workflows/ci.yml`) to reflect the new default feature set.

## [0.0.6] - 2026-06-28

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
