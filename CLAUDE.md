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
Core (easybot-core)       EventBus · SessionManager · AdapterManager · ApiKeyManager · ConfigLoader · PluginManager
     ↕
Adapters (easybot-adapter-*)  Telegram · Discord · 飞书 · QQ · WeChat
     ↕
Plugins (easybot-plugin-sdk)  cdylib 适配器插件（市场分发 · ed25519 签名 · PluginTestHost）
```

### 模板构建系统（别改错文件）

`build.rs` 从源文件生成 HTML 到 `templates/gen/`（gitignore）。改源文件，不要改产物。

| 产物 (templates/gen/) | 源文件 |
|---|---|
| `admin.html` | `templates/admin_layout.html` + `admin.js` + `admin.css` |
| `docs.html` | `templates/docs_layout.html` + `docs/*.md` + `vendor/` |
| `home.html` | `templates/home_layout.html` |

### 版本化迁移系统

`crates/easybot-core/src/storage/migration.rs` — 数据库 schema 从"幂等全量建表"升级为"版本化增量迁移"。

| 概念 | 说明 |
|---|---|
| `SCHEMA_VERSION` | 当前二进制期望的 schema 版本（编译时常量） |
| `MIGRATIONS` | 所有已注册的迁移数组，按版本递增 |
| `_schema_version` 表 | 记录已执行的迁移历史 |
| `run_migrations()` | 前向迁移引擎（SQLite），迭代未执行的迁移 |
| `run_migrations_pg()` | 同上 + `pg_advisory_lock` 互斥（PostgreSQL 多实例） |
| `rollback_to()` | 反向迁移引擎，执行 rollback_sql |

**新增 schema 迁移时**：向 `MIGRATIONS` 追加 `Migration` 条目，提供 `sql_sqlite` + `sql_postgres` 双后端 SQL，及对应的 `rollback_sqlite/rollback_postgres`。**禁止修改或删除已发行的迁移条目**。

### 自动更新系统

`crates/easybot-core/src/updater/` — 完整更新生命周期管理：

| 模块 | 职责 |
|---|---|
| `types.rs` | `UpdateInfo`, `UpdatePlan`, `UpdateError`, 平台检测, 版本比较 |
| `precheck.rs` | 磁盘空间/权限/Docker/dev 模式/插件兼容性检测 |
| `github.rs` | GitHub Releases API 客户端（缓存 + 速率限制） |
| `download.rs` | 流式下载 + SHA256 校验 |
| `compact.rs` | 备份管理（二进制+DB+config） + 服务单元路径更新 |
| `replace.rs` | 原子二进制替换 + 回滚 |
| `mod.rs` | `Updater` struct 编排完整流程 |

**升级流程**：`check-update`（GitHub API → 版本对比 → 迁移清单）→ 预检 → 备份 → 下载+SHA256 → 原子替换 → DB 迁移 → 验证 → 清理。失败时自动回滚。

**启动时版本锁定**：`main.rs` 中 storage 初始化后校验 `DB.schema_version == SCHEMA_VERSION`，不匹配则拒绝启动并提示运行 `easybot update`。

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
| `crates/easybot-adapter-wechat` | 个人微信 iLink Bot API 长轮询（v2，channel_version=2.2.0） |
| `crates/easybot-plugin-sdk` | Re-exports core types for plugins + `testing` feature (PluginTestHost) |
| `crates/easybot-plugin-sign` | 发布者 ed25519 密钥对/签名工具（gen-keypair / sign，仅发布者 CI 用，主程序不持私钥） |
| `plugins/` | 官方入门样例已独立为 [`EasyIndie/easybot-hello-adapter`](https://github.com/EasyIndie/easybot-hello-adapter)（教学样例仓库，含插件开发指南，与主仓仅文档/链接互引）；本仓库仅保留 `example-slack-plugin` 高级参考（非 workspace 成员） |
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
├── logs/ certs/ secrets/
├── plugins/                # 插件（含 .trust 发布者信任状态、.marketplace 临时下载）
```

Plugins 目录：`{config_dir}/plugins/`，`plugins/<name>/` 每插件一目录（plugin.yaml + 库文件 + plugin.sig.json）；`plugins/.trust` 是用户级发布者信任状态；`plugins/.marketplace/` 是安装临时下载目录（校验通过才原子落位）。

Config supports `${VAR_NAME}` substitution; `.local.yaml` merges on top. Env priority: export / Docker > `.env`. Run `easybot --init` to scaffold. Adapters auto-enable when their credential env vars are present — no `enabled: true` needed.

### Adapter Lifecycle

```
init(config) → connect() → send()/... → disconnect()
  Created → Starting → Connecting → Connected → Reconnecting → Failed → Stopped
```

`AdapterRegistry` holds factories keyed by platform. `AdapterManager::start_all()` auto-detects credentials and starts adapters. `AdapterConfig.enabled`: `None`=auto, `Some(true/false)`=force.

### 插件系统（市场 + DX）

`crates/easybot-core/src/plugin/` — 插件生命周期全链路：

| 模块 | 职责 |
|---|---|
| `manifest.rs` | `PluginManifest`（name/sdk_version/library/enabled）+ `library_path()` 安全校验 |
| `loader.rs` | `PluginLoader`（libloading 进程内 dlopen）+ `PluginLoadPolicy`（lenient=dev / strict=prod）+ 启动验签 |
| `registry/` | `PluginRegistry` trait（抽象）+ `GitHubRegistry`（catalog.json + Releases 的 `easybot-plugin.json`） |
| `signing/` | ed25519 验签（`verify_artifact`）+ `TrustStore`（`{plugins_dir}/.trust` 用户信任状态） |
| `manager.rs` | `PluginManager` 编排：install/update/uninstall/enable/disable/list/search/info/trust |
| `install.rs` | 安装流水线：triple 匹配 → ABI 预检 → `requires.easybot` semver → 信任确认 → 下载 → sha256+验签 → 原子落位 |
| `error.rs` | `PluginManagerError` |

**核心语义**：
- **签名 ≠ 安全**：ed25519 只证「作者 + 完整性」，不证代码无害。插件无沙箱，以宿主权限运行。
- **信任按发布者**：`--yes` 不自动加入 `.trust`；显式 `plugin trust <publisher> --public-key <k>` 才加入。
- **更新默认 pin**：`plugin update` 默认当前版本，`--latest`/`--channel` 才跨版本。
- **启停语义**：`disable` 写 `enabled:false` + 立即 stop+unregister；`enable` 下次启动生效（v1 不做热 dlopen）。
- **孤儿数据**：卸载/禁用后的会话/消息由现有 TTL 保留策略兜底，不级联删除。
- **生产门禁**：`--production`/`EASYBOT_ENV=production` 启动扫描签名，未签名且未 `allowUntrusted` 则拒绝。

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
| `/callbacks/answer` | POST | Answer inline-keyboard callback |
| `/sessions` | GET | List active |
| `/sessions/{key}` | GET/PUT/DELETE | Details / rename (custom_name) / delete |
| `/sessions/{key}/export` | GET | Export session data |
| `/chats/{p}[/{chat_id}]` | GET | List / info |
| `/config` | GET/PUT | Get / PUT returns 409 (change requires reviewed restart) |
| `/ws` | GET | WebSocket stream (Auth: JSON frame `{"token":"..."}`) |
| `/metrics` | GET | Prometheus |
| `/swagger` | GET | OpenAPI browser |
| `/openapi.json` | GET | OpenAPI 3.1 schema |
| `/system` | GET | System info (CPU, memory) |
| `/logs` | GET | Log stream (ring buffer) |
| `/api-keys/types` | GET | List API key types |
| `/api-keys/{id}` | DELETE | Revoke API key |
| `/api-keys/{id}/purge` | DELETE | Purge API key entirely |
| `/plugins` | GET | Installed plugins（含加载失败清单与原因） |
| `/plugins/catalog` | GET | Market catalog（`?query=`，5min 缓存） |
| `/plugins/install` | POST | Install（body: `publisher/name` + `channel`；首次未信任发布者返回 `needsTrustConfirmation`） |
| `/plugins/{name}` | DELETE | Uninstall |
| `/plugins/{name}/enable\|disable` | POST | Toggle |

### Roadmap

| Phase | Scope | Status |
|---|---|---|
| **P1 MVP** | Core types, PlatformAdapter trait, Telegram adapter, REST API, config, paths | ✅ |
| **P2 Bidirectional** | Event bus, WebSocket push, inbound handling, session persistence, edit/delete | ✅ |
| **P3 Multi-platform** | Telegram, Discord, 飞书, QQ, 微信 — 五平台全部完成 | ✅ |
| **P4 Production** | Argon2 auth, rate limit, PostgreSQL, Docker, Prometheus, TTL, auto-reconnect, streaming, uptime | 95% (暂缓: RBAC, TLS；配置变更需审阅后重启，PUT /config 返回 409) |
| **P5 Plugin System** | SDK, dynamic loading, registry, docs | ✅ |
| **P6 Plugin Market** | GitHub Releases 分发、ed25519 签名、多注册表 Taps、信任语义、脚手架/DX、plugin-publish.yml | ✅ |

### 不可退让的设计约束

**必须同时支持 Docker 部署和独立运行。** 功能迭代不得引入仅 Docker/仅裸机可用的能力。测试默认在独立运行模式下执行。

**`assets/` 目录中的图片即使未被代码引用也不算死文件。** 它们是品牌素材（Banner、Logo、测试用图等），可能用于文档、README、Release 发布等场景，不被代码直接引用是正常的。

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
| QQ chat type routing | QQ 适配器通过 `chat_types: Arc<Mutex<HashMap<String, ChatType>>>` 记录已知会话类型（由入站 Gateway 事件自动填充）。`try_send()` 优先查表直接路由到正确端点（`/channels/`, `/v2/groups/`, `/v2/users/`），仅在 chat type 未知时回退到三级级联。**改 media 发送时必须按 chat type 区分 msg_type，不可统一使用同一值。** |
| QQ media msg_type | **Channel**: `msg_type:2` (rich media, image+text)。**Dm (C2C)**: `msg_type:1` (image embed, v2 API 不支持 msg_type:2)。**Group**: 必须走文件上传 + `msg_type:7` (media)，v2 群聊端点的 `msg_type:1/2` 均不可用——`content` 字段被 QQ API 统一按 markdown 解析，msg_type:2 触发 `40034011`/`40034127`。上传使用 JSON body + base64 `file_data`（非 multipart）。详见 `send_media()` / `send_group_media_upload()`。 |
| QQ file upload | `/v2/{users\|groups}/{id}/files` — JSON body: `{"file_type": 1|2|3, "srv_send_msg": bool, "file_data": "<base64>"}`。`srv_send_msg=true` 上传后自动发送（不含文本）；`srv_send_msg=false` 仅获取 `file_info`，再通过 `msg_type:7` + `media.file_info` 手动发送。 |
| Health monitor | 分级响应：`retry_transport()`（纯传输重启，不鉴权）→ `reconnect_adapter()`（完整 stop+start，含鉴权）→ 30min 慢重试。错误分类 `classify_error()` **结构化优先**：`AuthFailed/Unauthorized/Forbidden` → 永久（立即 Failed），`Transient/RequestTimeout/RateLimited` → 瞬态（退避重试）；启发式兜底且**网络信号优先于永久关键词**（避免 `"X auth failed: <网络错误>"` 把瞬态网络失败误判为永久）。网络层错误用 `GatewayError::Transient` 标记，connect 失败用 `ConnectResult::failed(_, kind)` 透传分类（`reconnect_adapter` 按 kind 返回 typed error）。详见 `core/src/adapter/manager.rs`。 |
| `retry_transport()` trait | `PlatformAdapter` 新增方法，默认 `Ok(false)` 回退到完整重连。`connect()` 含网络鉴权的适配器（Telegram/Discord/飞书）需覆盖为 `Ok(true)`，取消旧后台任务后直接重启，**跳过鉴权**。内置重试循环的适配器（QQ/微信）无需覆盖。**新适配器约定**：reqwest `send()` 网络层失败必须用 `GatewayError::Transient` 包裹；`connect()` 失败用 `ConnectResult::failed(error, kind)` 标记（网络→`Transient`，凭据拒绝→`Permanent`，未知→`None`）。详见 `core/src/types/adapter.rs`。 |
| Heartbeat 语义 | 心跳表示"后台任务存活且正在重试"，不要求消息一定成功。各适配器在错误重试路径中调用 `heartbeat.beat()`（如 `polling_loop` 错误分支、`gateway_loop` 重连循环）。**禁止**使用独立定时器无条件 beat（飞书已修复）。心跳过期 120s → `HealthStatus::Degraded` → 健康监测器介入。 |
| WeChat iLink API | v2 协议（与官方 openclaw-weixin SDK 一致），`channel_version: "2.2.0"` 常量 `CHANNEL_VERSION`。`message_id` 字段兼容整数和字符串（`deserialize_flexible_id`）。`sendmessage` 请求需 `message_type: 2, message_state: 2`。`getupdates` 需 `base_info.channel_version`。 |
| Plugin 签名 ≠ 安全 | ed25519 签名只证作者+完整性，**不证代码安全**（VS Code 徽章被滥用教训）。插件无沙箱以宿主权限运行，生产隔离用容器化兜底（`docs/18 plugin-security.md`）。`docs/18 plugin-security.md` 信任模型含威胁模型表。 |
| Plugin 信任语义 | 信任按**发布者**粒度（VS Code 1.97）：`plugin install --yes` **不自动**写入 `.trust`；显式 `plugin trust <publisher> --public-key <k>` 才加入。状态存 `{plugins_dir}/.trust`。密钥泄露=官方从 `trustedPublishers` 移除公钥→新版拒绝该发布者，不自动卸载已装插件。 |
| Plugin 签名对象 | 签名的对象 = **产物字节本身**（`.so/.dylib/.dll`），元数据被 sha256 间接锚定。验签两时点：install 下载后 + load 启动时（防安装后被替换）。`easybot-plugin-sign` 是独立工具（主程序不持私钥），私钥只存发布者 GitHub Actions secret `PUBLISHER_PRIVATE_KEY`。 |
| Plugin 安装流水线 | `current_target_triple()` 匹配 artifacts → ABI 预检（sdk_version）→ `requires.easybot` semver range → 信任确认 → 下载临时目录 → sha256 + `verify_artifact` 双通过 → **原子 rename** 落位 + 合成 plugin.yaml。name 白名单 `[A-Za-z0-9_-]` 拒绝 `..`/路径穿越。`install --file` 走同流水线跳过下载（离线部署）。`PluginManager` 内部 Mutex 串行化 install/uninstall/enable/disable。 |
| Plugin 多注册表 | `plugins.registries` 列表（Taps 模型：官方 + 社区源），catalog 合并去重，插件名支持 `publisher/name` 限定。市场不可达明确报错不崩；`plugin update --refresh` 强制清缓存。 |
| Plugin 更新语义 | `plugin update` 显式触发（非自动），默认 **pin 当前版本**，`--latest`/`--channel beta` 才跨版本（防更新攻击）。`requires.easybot` 安装前校验兼容范围；主程序更新时 `check_plugin_compatibility`（`updater/precheck.rs`）阻止不兼容插件。 |
| Plugin 跨平台分发 | 插件按 6 target triple 分别编译（Linux 两项必须 **musl**，宿主 musl-static glibc `.so` 无法 dlopen；macOS `MACOSX_DEPLOYMENT_TARGET` ≤ 宿主；Windows 可选 `crt-static`）。`plugin-publish.yml` 是**自包含**模板（只引用公开 action + 40 位 SHA 固定），copy 到插件仓库即用：6-target matrix + gitleaks 扫描 + sign + `easybot-plugin.json` + Release。 |
| Plugin 脚手架 | `easybot plugin new <name>` 生成独立可构建工程（`bin/src/plugin_scaffold.rs` + `plugin_scaffold_template.rs`），SDK git tag 依赖由编译时版本常量生成（`v{CARGO_PKG_VERSION}`），离线可用。模板含 `[patch]` 本地联调注释。脚手架产出形状由独立样例仓库 [`EasyIndie/easybot-hello-adapter`](https://github.com/EasyIndie/easybot-hello-adapter) 持续编译验证（教学样例，含插件开发指南）。 |
| Plugin 测试宿主 | SDK `testing` feature 提供 `PluginTestHost`（`crates/easybot-plugin-sdk/src/testing.rs`）：内存宿主模拟 attach/init/connect/send/事件流，离线跑通。传输可注入方法论：HTTP client 经构造器或 `init(config)` 注入，协议交互用 wiremock 替换。测试金字塔：单元 → PluginTestHost → wiremock → e2e。 |
| Plugin 命名规则 | 官方插件统一 **`easybot-xxx`** 前缀，且同一个名字贯穿仓库名 / `Cargo.toml [package].name` / cdylib 产物（`libeasybot_xxx.{so,dylib,dll}`，Rust 下划线连 crate 名）/ `plugin.yaml` name / `platform_name()` / 市场安装名。命名一经发布即对外稳定（`platform_name()` 参与会话 key 与路由）。社区插件不强前缀，用 `publisher/name` 限定。详见 `docs/16 plugin-guide.md`「命名规则」。官方入门样例 = 独立仓库 [`EasyIndie/easybot-hello-adapter`](https://github.com/EasyIndie/easybot-hello-adapter)（教学用，含插件开发指南，与主仓仅文档/链接互引）。 |
| Plugin FFI 分配器契约 | 插件在进程内 dlopen，与宿主通过 FFI 收发 String/Vec/Value 等**带堆所有权**的值（宿主构造→插件 Drop，或反向），两侧**必须共用同一全局分配器**。因此宿主**禁用自定义 `#[global_allocator]`**（`bin/Cargo.toml` 已移除 mimalloc 并注释警示）：宿主 mimalloc + 插件系统 malloc → 交叉 free → SIGABRT；插件各自静态链接 mimalloc → 进程内两套堆 → 析构死锁。插件侧同样**不要**声明 `#[global_allocator]`，保持默认。详见 `docs/17 plugin-methodology.md`「FFI 分配器契约」与独立样例仓库的[插件开发指南](https://github.com/EasyIndie/easybot-hello-adapter/blob/main/docs/plugin-development-guide.md)。 |

## 发布流程

收到"发布新版本"请求时直接按此固定流程执行（v0.0.38 验证；治理原则见 `docs/07 commercial-release.md`）：

1. **文档对齐**：CHANGELOG.md 在 `[Unreleased]` 下新增 `[0.0.X]` 条目（Keep a Changelog，中文）；README/CLAUDE/docs 与代码实现对齐。
2. **版本同步**：`Cargo.toml`（version）+ `Cargo.lock`（`cargo update --workspace`）+ `compose.quickstart.yml`（EASYBOT_IMAGE 注释）+ `crates/easybot-api/src/routes/update.rs`（`#[schema(example)]`）+ `crates/easybot-api/tests/routes.rs`（current_version）+ 两个快照（health + openapi 的 version/example）+ `deploy-kit/deploy.sh`（注释→**下一**版本）+ `docs/01 user-guide.md`（版本引用）+ `docs/other/windows-deployment.md`（版本要求）。
3. **前置构建**：`cargo build -p mock-adapter` + `cargo build --features "default,plugin-system"`（CLI 测试硬编码找 `target/debug/easybot`，且插件集成测试要求该二进制带 `plugin-system` —— 裸 `cargo build` 会把二进制重建成无插件版，导致 `cli::test_production_*` / `cli::test_plugin_cli_*` 失败）。`cargo clean` 后必做。
4. **预检**：`bash scripts/release-preflight.sh`。**禁止 `| tail`**（管道吞掉退出码）——用 `> /tmp/log 2>&1; echo EXIT_CODE=$?`。含 Actions 40 位 SHA 固定门禁。
5. **测试**：`cargo test --workspace --features "default,plugin-system" --locked` + `cargo fmt --all --check`。注意 APFS 磁盘空间：全量编译可打满共享容器，满盘表现为空输出 + exit 1，不是测试失败。
6. **提交**（3 个）：`docs: align documentation with current implementation` / `ci: pin <action> action to commit SHA` / `release: bump v0.0.X → v0.0.Y`。
7. **推送**：`git push origin main` + `git push origin v0.0.Y` 触发 `release.yml`（要求 Cargo.toml 版本==标签、CHANGELOG 有条目、标签时工作树干净）；推送后必须 `gh api` 确认 CI 通过，失败立即修复。

## Known Gaps

Full checklist: `docs/other/todo-list.md`. P4 deferred items:

| Gap | File | Description |
|---|---|---|
| RBAC | `crates/easybot-core/src/auth/permissions.rs` | Role-based access control (暂缓) |
| TLS termination | `crates/easybot-api/src/server.rs` | Cert loading/serving not wired (暂缓) |

## 资源管理指南 (Resource Management)

长期运行稳定性依赖以下机制协同工作，修改相关代码时请保持这些模式的完整性：

### 已实现的资源保护

| 保护层 | 机制 | 文件 |
|---|---|---|
| SQLite WAL checkpoint | 后台任务按 TTL 间隔循环 `PRAGMA wal_checkpoint(TRUNCATE)` | `bin/src/main.rs` |
| SQLite TTL 数据保留 | `RetentionWorker` 每小时删除过期消息/会话 (默认: 消息90天, 会话365天) | `core/src/storage/retention.rs` |
| Session 内存清理 | `SessionManager::prune_expired()` 按 TTL 周期同步清理 DashMap | `core/src/session/manager.rs` |
| 事件总线 | tokio `broadcast` 有界通道 (cap 256)，慢消费者 Lagged 丢事件 | `core/src/bus/event_bus.rs` |
| WebSocket 上限 | Semaphore 最大 `max_clients` (默认1000)，发送超时 100ms，连续丢 50 事件断开 | `api/src/routes/ws.rs` |
| Webhook 并发 | Semaphore 上限 16 并发分发 | `core/src/webhook/mod.rs` |
| 速率限制 | IP 滑动窗口，Max 100,000 桶，5 分钟 LRU 清理 | `api/src/middleware/rate_limit.rs` |
| 环形日志 | `LogCollector` 固定 5000 条环形缓冲 | `api/src/log_collector.rs` |
| Prometheus 指标 | 固定标签集，路径归一化防标签爆炸 | `api/src/metrics.rs` |
| 适配器重连 | 指数退避 5s→300s，最多 20 次封顶 | `core/src/adapter/manager.rs` |
| 适配器缓存上限 | QQ 10K / Telegram 5K / Discord 5K / 飞书 30s TTL | 各 adapter crate |

### 新增功能时需注意

- **所有适配器缓存必须有大小上限或 TTL 淘汰** — 参考 `CHAT_TYPE_CACHE_LIMIT` / `ADMIN_CACHE_LIMIT` / `GUILD_CACHE_LIMIT` 模式
- **所有 tokio::spawn 循环必须考虑并发上限** — 使用 `Semaphore` 或 `JoinSet`
- **SQLite 清理后需配合 WAL checkpoint** — 避免 WAL 文件无限增长
- **内存 Session 清理必须匹配数据库 TTL** — `SessionManager::prune_expired()` 调用周期需与 `RetentionWorker` 一致
