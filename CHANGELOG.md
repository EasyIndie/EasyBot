# Changelog

All notable changes to EasyBot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.16] - 2026-07-20

### Fixed

- **管理后台适配器状态变化未能实时更新** — 健康监测器 Tier 1（transport-only retry）路径存在四个
  断点导致状态变更无法通过 WebSocket 事件送达前端：(1) 进入 retry 前不发布 `adapter.reconnecting`；
  (2) 重试成功后状态缓存设为 `Reconnecting` 而非 `Connected`，且不发布 `adapter.reconnected`；
  (3) 永久失败后不发布 `adapter.reconnect_failed`；(4) 前端 `handleGatewayEvent` 忽略事件数据中的
  `health` 字段，`updateAdapterCard` 也不更新健康副标题 DOM。修复：`run_health_check()` 三处补发
  事件、状态缓存正确回连、前端传递 health 并更新副标题。
  `cache-save-if` 输入描述的例子里包含 `${{ github.ref }}` 表达式，GitHub Actions 在解析
  复合 action 元数据时对所有 `${{ }}` 语法求值，但 `github` 上下文在此层面不可用。
  修复：description 中使用纯文本示例，去除表达式符号。
- **CI: feature-check 矩阵缺少 protoc** — `.github/workflows/ci.yml` feature-check 矩阵前
  两组使用 `install-protoc: none`，但 `cargo check --workspace` 编译所有 workspace 成员
  （含 `easybot-adapter-feishu`，依赖需 protoc 的 `larksuite-oapi-sdk-rs`）。修复：所有
  矩阵条目统一使用 `install-protoc: auto`。
- **Security Audit: `spin v0.9.8` yanked 告警阻塞** — `cargo audit --deny warnings` 检测到
  传递依赖 `spin v0.9.8`（通过 `flume` → `sqlx`）已从 crates.io 撤回，且无 RUSTSEC 咨询
  ID 可忽略。修复：在 audit 工作流中添加 `--no-yanked` 跳过 yanked 检查（yanked 是发布管理
  问题而非安全漏洞）。
- **飞书适配器独立心跳定时器** — `FeishuAdapter` 使用独立 `tokio::spawn` 每 30s 无条件调用
  `heartbeat.beat()`，与 WebSocket 连接状态完全无关。导致 WiFi 断连期间健康监测器**从不触发
  重连**，管理后台始终显示 "Connected"。修复：删除独立定时器，改为**事件驱动心跳**（收到
  `message_receive_v1` / `chat.updated_v1` 事件时 beat，WS 任务启动时初始 beat）。
- **WiFi 断连后 Telegram/Discord/QQ 恢复过慢** — `reconnect_adapter()` 对瞬态网络故障做完整
  stop + start（含鉴权 API 调用），WiFi 断连期间鉴权必然失败，导致 20 次后进入 30 分钟慢重试。
  修复：新增 `retry_transport()` trait 方法（仅重启后台传输，不鉴权）；健康监测器采用分级响应。
- **微信 `message_id` 解析失败** — iLink API v2 返回整数格式 `message_id`，但代码定义为
  `Option<String>`，serde 解析失败。修复：新增 `deserialize_flexible_id` 自定义反序列化器，
  兼容整数和字符串两种格式。

### Added

- **`PlatformAdapter::retry_transport()` trait 方法** — 纯传输重启（取消旧后台任务后直接启动新
  任务，跳过鉴权）。Telegram/Discord/飞书已实现。有内置重试循环的适配器（QQ/微信）使用默认
  `Ok(false)` 回退到完整重连。
- **健康监测器分级响应** — `run_health_check()` 采用两级重试：传输重试（5 次/30s，不鉴权）→
  完整重连（指数退避 5s→300s，含鉴权）→ 慢重试（20 次后 30min 间隔）。新增 `classify_error()`
  区分永久错误（401/403 鉴权失败，立即 Failed）和瞬态错误（网络超时、DNS 失败）。
- **管理 API `health` 字段** — `/api/v1/adapters` 和 `/api/v1/adapters/{platform}/status` 响应
  新增 `health` 字段（`"Healthy"` / `"Degraded"` / `"Down"`），区分"适配器在运行"和"消息流
  是否正常"。
- **管理后台健康状态展示** — 适配器卡片根据 `health` 字段显示不同徽章颜色：绿色 Healthy、
  黄色 Degraded、红色 Failed。传输异常时显示 "传输异常" 副标题。
- **心跳语义统一** — 所有适配器在错误重试路径中调用 `heartbeat.beat()`（Telegram polling loop、
  QQ gateway loop、微信 longpoll loop），告诉健康监测器"后台任务存活且正在重试"。心跳过期
  阈值统一为 120s（`DEFAULT_LIVENESS_THRESHOLD_MS`）。

### Changed

- **微信 iLink API 升级到 v2** — 统一 `channel_version: "2.2.0"`（与官方 openclaw-weixin SDK
  一致）。`getupdates` 请求新增 `base_info.channel_version` 字段。引入 `CHANNEL_VERSION` 常量。
- **微信长轮询容错增强** — 连续失败退出上限从 10 次提升到 30 次（约 15-20 分钟）。错误路径新增
  `heartbeat.beat()`。
- **飞书 WebSocket 任务简化** — 移除独立心跳定时器，心跳改为事件驱动。

### Cleanup

- **移除 `PendingConnection` 死代码** — struct 的两个字段（`platform`、`display_name`）从未被读取，
  HashMap key 已存储平台名。替换为 `HashSet<String>`，语义更清晰。
- **CLAUDE.md 新增约束** — `assets/` 目录中的图片即使未被代码引用也不算死文件（品牌素材）。

### Docs

- **CLAUDE.md 更新** — 新增 Health monitor、`retry_transport()`、Heartbeat 语义、WeChat iLink API
  四个 Key Pattern 条目。适配器表格标注 WeChat v2。新增 `assets/` 品牌素材保护规则。
- **平台能力文档更新** — `03 platform-capabilities.md` 断线自动重连章节重写，反映分级响应机制
  和事件驱动心跳。
- **用户指南更新** — WeChat 章节 iLink Bot 链接更新为官方 openclaw-weixin SDK。
- **架构文档更新** — 健康监控描述从简单"指数退避"更新为分级响应机制。

## [0.0.15] - 2026-07-10

### Added

- **API 路由层集成测试** — 新增 `tests/api_integration.rs`（24 个真实 HTTP 测试覆盖
  health、auth、适配器管理、消息收发/编辑/删除、batch-send、配置、系统信息、日志、
  API Key 管理端点）
- **微信凭据过期自动重启** — `poll_messages()` 检测 HTTP 401 凭据过期信号 →
  `PollOutcome::TokenExpired` → 清除磁盘凭据文件 → 健康监测自动重启 → `init()` 无法
  恢复凭据 → `connect()` 触发新 QR 扫码登录
- **飞书事件签名验证** — 支持从 `config.extra.verification_token`/`encrypt_key` 或
  `FEISHU_VERIFICATION_TOKEN`/`FEISHU_ENCRYPT_KEY` 环境变量读取验证配置；配置时启用
  真实 HMAC 验证，未配置时保持 WebSocket OAuth 鉴权兼容
- **Discord guild name cache** — 从 `GuildCreate`/`GuildUpdate` 事件缓存 guild 名称，
  填充入站消息 `chat_name` 字段（DM 使用 `author.name`）
- **Discord API 429 Retry-After 自动重试** — `api_call` 方法最多重试一次 429，读取
  真实 `Retry-After` 头等待
- **Telegram media convert 测试** — 新增 8 个测试覆盖 photo/document/video/audio/
  voice/sticker/animation 及 caption fallback
- **AdapterManager health check 状态缓存** — 不可达适配器立即更新为 `Failed` 状态，
  不再显示陈旧的 `Connected`
- **WeChat HTTP 响应超时保护** — 为 `getuploadurl` 响应读取、CDN 错误响应读取和
  `download_media` 添加显式 `tokio::time::timeout` 保护

### Fixed

- **WeChat send API `ret=-2` 处理** — 补充为与 `ret=-14` 相同的 context_token 过期
  重试逻辑（剥离 token 重试一次），避免误判为真限流
- **Telegram 长轮询串行处理消息** — 改为 `Semaphore(5)` + `tokio::spawn` 并行处理
  入站消息，chat member update 保持串行（共享缓存）
- **QQ `send_media` 媒体解析逻辑去重** — 提取 `resolve_upload_media` 共享异步关联函数，
  消除 `send_c2c_media_upload_only`/`send_group_media_upload` 中约 40 行重复代码
- **飞书能力声明切换为 `capabilities!` 宏** — 消除 11 个冗余 `Capability` 结构体字面量
- **`SessionManager::update_source_fields` 写库优化** — 仅在至少一个富化字段变更时才
  持久化到 DB，避免高频场景下每次富化都写数据库
- **`messages_in` 计数器修复** — Telegram/Discord/Feishu 三个适配器的后台任务中正确
  递增 `messages_in`（`Arc<AtomicU64>` 传递）
- **WeChat 同步 I/O 修复** — `save_context_tokens` 和 `save_sync_buf` 改为
  `spawn_blocking` 异步写入
- **全量 Clippy warnings 清除** — 修复 collapsible_if、identical if blocks、unused
  imports、unused Result、too_many_arguments 等 lint 警告，`verify.sh` 8 步全通过

### Performance

- **EventBus 订阅零轮询化** — `subscribe_many` 从 100ms 固定间隔轮询改为
  `BroadcastStream` + `SelectAll`，零空闲 CPU 占用
- **飞书 `api_*` 方法合并** — 5 个独立 HTTP 请求方法合并为统一 `send_api_request`
- **Feishu token 管理合并** — 适配器实例和 WebSocket 后台任务的两套 token 缓存合并为
  共享 `FeishuTokenStore`

### Docs

- **各适配器验证文档全量标记更新** — Telegram/Discord/Feishu/QQ/WeChat 五份文档的测试
  状态和已验证功能清单更新至最新
- **性能评审报告已修复列表更新** — 记录 25+ 项已修复问题的验证日期
- **e2e-real.sh stdout 过滤** — `tail -f` 改为 `grep` 过滤，只显示 QR 码/API Key/
  连接状态/错误等关键行，减少终端刷屏

## [0.0.14] - 2026-07-09

### Fixed

- **Release workflow 改用 composite action 替代 git patch 做版本升级** — 原流程中 prepare-release
  用 `git diff HEAD` 生成 patch、下游 job 再 `git apply`，当 tag 指向的 commit 与 patch 上下文
  不一致时全部文件报 "patch does not apply"。改为每个下游 job 通过 `needs.prepare-release.outputs.version`
  获取版本号，使用统一 composite action（`.github/actions/apply-version-bump/`）运行 `sed` 更新
  Cargo.toml 和 insta snapshot。prepare-release 步骤已幂等化，重跑失败的 release 安全。
- **v0.0.13 发布失败 (#32)** — 同上原因，patch 无法应用到已包含版本升级的 tag。

## [0.0.13] - 2026-07-08

### Fixed

- **QQ 适配器 `fetch_gateway_url` 添加超时和错误日志** — QQ Gateway WebSocket 连接入口
  `fetch_gateway_url()` 未设 HTTP 超时，网络故障时会导致请求永久挂起。添加 15s 超时和连接
  失败时的详尽错误日志，便于排查 QSign 鉴权问题。

## [0.0.12] - 2026-07-08

### Performance

- **全链路性能优化 (P0–P3)** — 分阶段覆盖构建、数据流、存储、锁竞争和网络层：
  - **P0 构建优化** — `[profile.release]` 启用 `LTO=fat`、`codegen-units=1`、`strip`、`panic=abort`；
    引入 mimalloc 全局分配器降低内存分配开销；tokio features 从 `"full"` 缩小到最小必需集。
  - **P1 数据流重写** — EventBus 订阅从 `subscribe_many` + 100ms 轮询改为 `BroadcastStream` + `SelectAll`
    （零空闲 CPU、零延迟）；WebSocket 帧序列化从 `serde_json::json!()` 宏（中间
    `Value` 树）改为直接 `Serializer`；`InboundMessage.metadata` 从
    `Option<serde_json::Value>` 改为预序列化的 `Option<String>`。
  - **P1 存储修复** — PostgreSQL `store_messages` 从 N 次单条 INSERT 改为事务批量写入；
    SQLite/PostgreSQL 单行查询改用 `fetch_optional()` 避免 `Vec` 分配。
  - **P2 速率限制器重构** — 4 个独立限流器合并为 1 个共享桶池（1 个清理任务）；滑动窗口容量随配置动态调整；
    LRU 淘汰改为采样 20 条（原遍历 100K）。
  - **P2 指数退避+抖动** — 新增 `util::backoff_with_jitter()`（1s→2s→…→30s 上限，±25% 抖动）；
    Telegram/QQ/WeChat 三个适配器的固定 sleep 全部替换。
  - **P2 HTTP 客户端复用** — Telegram 轮询复用适配器 `reqwest::Client` 连接池；
    WeChat CDN 上传缓存为 `OnceLock`；Discord 429 读取真实 `Retry-After` 头。
  - **P3 锁竞争降低** — `AdapterManager` 读锁用于 `status_summary()`、缩短写锁区间、延迟加载配置；
    QQ `chat_types` 从 `std::sync::Mutex` 改为 `parking_lot::Mutex`（无中毒、更快的 async 锁）。
- **`InboundMessage.platform` 使用 `Cow<'static, str>`** — 适配器直接返回 `Cow::Borrowed("telegram"/...)`，
  零分配，`Clone` 仅指针+长度复制，`serde`/`sqlx` 兼容（涉及 10 个文件）。
- **SQLite 连接池分离** — 会话和消息使用独立连接池，降低读写竞争。
- **QQ Gateway RESUME 支持** — Gateway WebSocket 断线后通过 RESUME opcode 快速恢复会话，
  避免完整重连握手（`gateway.rs` +70/-20 行）。
- **WebSocket 事件广播去重** — 序列化一次，共享给所有 WS 客户端，消除 N 次重复序列化开销。
- **媒体大小限制** — Telegram/Discord/飞书/QQ 四适配器添加媒体文件大小上限校验。
- **健康检查退避** — AdapterManager 健康检查在非健康适配器上采用退避策略，减少空闲检查频率。
- **Retention 优化** — RetentionWorker 调整清理周期和批处理逻辑。

### Build

- **Release profile 去除调试符号** — `debug = false`，缩减二进制体积。

### Docs

- **README logo 更新** — 替换为新项目图标。

## [0.0.11] - 2026-07-08

### Fixed

- **QQ 适配器 rustls CryptoProvider 未初始化 panic** — QQ Gateway WebSocket 连接因 `rustls`
  `CryptoProvider` 未调用 `install_default()` 导致 TLS 握手时 panic。现已在 QQ 适配器
  `connect()` 中初始化 `aws-lc-rs` provider。
- **`QrCodeResponse` dead_code 警告** — 移除 WeChat 模块中未使用的 `errmsg` 字段。
- **文档与 CI 中已废弃 full feature 引用清除** — 将 `Makefile`、`verify.sh`、`.github/workflows/ci.yml`、
  `README.md`、`CONTRIBUTING.md` 中所有 `--features "full,..."` 替换为 `--features "default,..."`，
  因 `full` feature 已于 v0.0.7 移除（default 已包含全部 5 个适配器）。

### Changed

- **WeChat 适配器从 default features 中移除** — 不再默认启用（`d6d16e9`）。需要构建时
  通过 `--features adapter-wechat` 或 `--no-default-features --features adapter-wechat`
  显式启用，以保持默认构建一致性（WeChat 适配器依赖 iLink Bot API 运行环境）。
- **移除已废弃的 `WECHAT_BOT_TOKEN` 环境变量** — WeChat 适配器仅通过扫码登录，
  `WECHAT_BOT_TOKEN` 不再支持（`ebd4a26`）。添加 `qrcode` 依赖用于 QR 码生成。
- **文档全量更新** — 逐段检验并更新所有文档与代码实现保持一致（`6b90a62`）。
- **CI 改进** — `setup-rust` action 默认安装 protoc；清理 CI 工作流配置（`117c78b`）；
  Dockerfile 安装 protobuf-compiler 以支持 `larksuite-oapi-sdk-rs` 构建。

### Cleanup

- **`gateway.local.yaml` 模板修正** — 更新适配器配置示例以匹配当前代码。
- **无用代码、依赖和文件清理** — 移除 dead code、未使用依赖和遗留文件。

## [0.0.10] - 2026-07-07

### Fixed

- **WebSocket 频繁断开重连问题** — 修复 WebSocket 连接因多种原因异常断开导致的反复重连循环。
- **WebSocket 升级成功(101) 被管理后台误计为 err** — HTTP 101 Switching Protocols 是正常
  WebSocket 升级响应，不再计入错误计数。
- **dev API key 跨重启复用** — `dev_api_key` 改为懒创建并持久化存储，避免每次启动生成新 key。
- **`send_message` handler 添加 15s 超时** — 防止慢平台阻塞 API 请求处理。
- **Telegram 和 Discord 适配器 HTTP 客户端添加 15s 请求超时** — 防止外部 API 超时导致
  内部请求堆积。
- **CSP script-src 添加 `static.cloudflareinsights.com`** — 允许 Cloudflare Insights 脚本加载。

## [0.0.9] - 2026-07-07

### Fixed

- **管理后台 log tab 页面** — 修正日志标签页渲染错误。
- **管理后台按钮文字折行** — `white-space: nowrap` 防止窄窗口下文字换行。

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
