# Changelog

All notable changes to EasyBot will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.33] - 2026-08-12

### Added

- **用户自定义会话显示名（`custom_name`）** — 新增 `PUT /api/v1/sessions/{key}`
  重命名接口，操作者可为平台无法可靠命名的会话设置自己的显示名。核心层
  `Session.custom_name` 新增列（SQLite/PostgreSQL **v3 迁移**，含回滚 SQL）、
  `SessionMutation.custom_name` 三态语义（不变更/清除/设置）、经 `SessionStore`
  upsert 持久化；API 层需 `SESSIONS_MANAGE` 权限，并记录审计事件
  `privacy.session.renamed`；空/trim 后为空的名字清除自定义名并回退自动推导链
  `custom_name → chat_name → user_name → label(id)`，仅非空自定义名覆盖显示。
  管理后台会话表新增重命名按钮；发送页目标选择器与 `/chats` 端点优先展示自定义名。
- **QQ / 飞书会话名自动增强** — QQ 解析群成员昵称并修正 `enrich_source` 字段接线
  （成员详情有界缓存避免重复查询）；飞书私聊会话以对端用户名命名（`user_name`
  缓存 + TTL 淘汰）。两者减少会话显示对 `chat_type_label(id)` 回退的依赖。
- **回调应答 API** — 新增 `POST /api/v1/callbacks/answer`（内联键盘回调应答），
  基于新的 `PlatformAdapter::answer_callback_query` trait 与 `AnswerCallbackParams`，
  按 `MESSAGES_SEND` 目标权限校验，OpenAPI 契约同步更新。

### Changed

- **平台令牌刷新收敛到核心 `CachedTokenStore`** — 将各适配器重复的"缓存 + 单飞刷新
  + 拒收重试"机制抽取为 `easybot-core` 的 `CachedTokenStore` 与
  `send_with_token_retry`（token 拒收时恰好一次刷新、至多两次请求调用），QQ 与飞书
  统一接入，未来适配器无需重复实现。QQ：三个重复的 token 失效判定合并为
  `is_qq_token_invalid_response`（401 / 11244 / 11242），网关拉取先读 body，使
  HTTP 200 + code 11244 能正常终止而非无限重试；`qq_fetch_access_token` 将 100001
  归类为瞬态（原为永久停用）、100016/100007/10004 归类为凭据错误；默认 API 域由
  `api.sgroup.qq.com` 改为官方 `api.bot.qq.com`。飞书：替换原 `FeishuTokenStore` 为
  `CachedTokenStore` 包装，修复了缺失的 token 拒收→刷新→重试潜在缺陷，失效判定码
  99991663/99991665/20013/20005（排除 99991661 缺头与 99991664 app-access-token），
  HTTP 401 以 `Unauthorized` 呈现，`upload_media` 补重试。
- **五平台协议审查修复（54 项）** — Telegram/Discord/飞书/QQ/微信协议审查统一修复。
  核心：`cursor_state` 游标持久化（`AdapterManager` 保存/恢复）、心跳连续失败计数与
  失败循环阈值判定。适配器：Telegram 401 永久鉴权失败、429 `RateLimited`、
  callback_query 入站发布、`last_offset` 持久化；Discord 心跳 connected 门控、
  分片流自愈重连；微信空响应按成功处理、stop 时 flush 未持久化的 `context_token`。
  API：`POST /callbacks/answer` 路由 + 权限映射。

### Fixed

- **Telegram getUpdates 409 升级为可见 Degraded** — 持续 409 Conflict（另一实例轮询
  同一 token，或已设置 webhook）此前被归类为通用瞬态错误无限重试，心跳仍保持
  Healthy 而实际收不到消息。现在 `poll_once` 将 409 映射为 `GatewayError::Conflict`，
  连续冲突计数：第 1 次维持 WARN（正常重启重叠，可自愈），第 2 次起升级为 ERROR 并
  给出可操作提示；冲突路径保留存活 beat 但跳过 `beat_success()`，使消息流心跳老化、
  `health_status()` 上报 Degraded（`/adapters`、`/health`、管理后台可见）。Telegram
  通过 `heartbeat_success_age_ms()` 追踪实际成功轮询，>120s 轮询停滞同样上报
  Degraded（非 Down——外部实例冲突无法靠重连解决，健康监测器仅对 Down 介入）。
- **Telegram 429 无 retry_after 映射修正** — 429 无 `retry_after` 时映射
  `RateLimited(0)` 而非 `Internal(500)`。

## [0.0.32] - 2026-08-08

### Fixed

- **Windows 服务部署全面修复** — `manage-service.ps1` 从 `sc.exe` 体系重写为
  **NSSM** 体系：修复 `Start-Service` 1053 启动超时（EasyBot 非原生服务程序，需
  NSSM 包装）、NSSM `Application/AppParameters` 被引号包裹导致 1066 退出码、`$Home`
  与 PowerShell 只读自动变量冲突导致脚本报错、以及服务用 `--config` 注册导致
  `.env` / `gateway.local.yaml` / 数据 / 日志从错误目录（`%APPDATA%\easybot\`）解析
  的根因问题。新脚本以 `--dir <home>` 启动，自动检测 NSSM 并给出安装指引，崩溃自动
  重启、stdout/stderr 重定向到 `logs/easybot.out.log` / `.err.log`。
- **Windows 优雅关闭增强** — `shutdown_signal()` 的非 unix 分支新增
  `ctrl_break` / `ctrl_close` / `ctrl_shutdown` 监听，NSSM 以非 Ctrl+C 方式停止服务
  时也能触发同一套优雅关闭流程。
- **`--init` 模板默认管理密码改为空** — `gateway.yaml` 模板 `adminPassword` 默认值由
  `"easybot"` 改为 `""`（管理后台默认禁用登录），与 `default_admin_password()` 返回空、
  未配置即拒绝登录的既有安全设计一致；`EASYBOT_ADMIN_PASSWORD` 占位值改为
  `replace-with-a-long-random-password` 并补充生产环境强度提示。
- **误导性告警文案修正** — 配置文件密钥缺失的告警提示 `server.admin_password` 改为
  实际 YAML 键名 `server.adminPassword`（camelCase），避免用户按 snake_case 配置无效。
- **Windows 部署文档** — 新增 `docs/other/windows-deployment.md` 完整部署指南（下载校验
  → `--init --dir` → `.env` / `gateway.local.yaml` → NSSM 安装 → 验证 → 排查对照表），
  `docs/01 user-guide.md` Windows（Service）段落同步更新。

## [0.0.31] - 2026-08-04

### Fixed

- **微信适配器凭据/数据路径（容器部署）** — `crypto.rs` 原用 `dirs::home_dir()` 解析凭据与数据
  路径；容器内 easybot 用户无 home 目录（`HOME=/home/easybot` 不存在），扫码登录成功后
  `保存凭据失败: No such file or directory (os error 2)`，容器重启需重新扫码。修复：凭据/数据
  路径改用全局一致的配置根目录解析（`EASYBOT_HOME` > `~/.easybot`，复用 `resolve_home`），
  容器内落到 `/var/lib/easybot` 挂载卷。
- **deploy.sh 忽略 .env 中的管理密码** — `EASYBOT_ADMIN_PASSWORD` 只在解析变量时读 shell 环境，
  `.env` 在其后加载且仅透传平台令牌，导致 .env 配置的密码被忽略、容器使用默认
  `strong-admin-password`，用户无法用自设密码登录管理后台。修复：`.env` 提前加载
  （`$KIT_ROOT/.env`），管理密码与平台令牌均生效。
- **quickstart compose 目录导致 .env 不加载** — compose 默认 project-directory 取 compose 文件
  所在目录，工具包把 `compose.quickstart.yml` 放 `deploy/` 子目录导致根目录 `.env`（端口/令牌/
  密码）不生效。修复：quickstart compose 移至工具包根目录（与仓库布局一致）。
- **工具包版本锁定 + 端口自定义 + bash 3.2 兼容** — `deploy.sh` 与 kit 内 compose 默认拉取与
  工具包同版本的镜像 tag（读 `VERSION`），不再跟随 `latest` 漂移；`EASYBOT_IMAGE` /
  `EASYBOT_PORT` / `EASYBOT_BIND_ADDRESS` 可覆盖；`$NAME` 紧跟中文全角括号时 macOS bash 3.2
  `unbound variable` 缺陷以 `${NAME}` 花括号规避。

## [0.0.30] - 2026-08-04

### Added

- **部署工具包 Release 资产** — 新增 `easybot-deploy-kit-<version>.tar.gz` 随 Release 发布，
  下载解压即可部署，无需克隆源码。内含部署总说明（`deploy-kit/README.md`）、一键部署脚本
  （`deploy.sh`）、Docker 快速开始 / 生产加固 Compose、配置模板（`gateway.yaml` / `.env.example`）
  与生产运维脚本（`production-up/backup/restore/retention`、`verify-container-image`、
  `easybot-backup`）。组装脚本 `scripts/build-deploy-kit.sh` 在 release 工作流 `create-release`
  job 中生成并上传。
- **生产脚本独立部署** — `production-up.sh` / `production-backup.sh` / `production-restore.sh`
  的根路径解析从 `git rev-parse` 改为脚本目录相对解析（`BASH_SOURCE`），使部署工具包在无 git
  检出时也能独立运行，同时保持仓库内行为不变。

## [0.0.29] - 2026-08-04

### Fixed

- **适配器重连失败结构化分类（瞬态 vs 永久）** — QQ/Discord 的 `connect()` 把网络层鉴权失败统一
  套上 `"X auth failed:"` 前缀，`classify_error()` 的永久关键词先于瞬态关键词检查，导致瞬态网络
  故障（超时/拒连/DNS/限流）被误判为永久凭据错误 → 适配器被永久禁用。修复：新增
  `GatewayError::Transient` 结构化变体，网络层 reqwest `send()` 失败在各适配器源头打上该标记；
  `classify_error()` 改为结构化优先（`AuthFailed/Unauthorized/Forbidden` → 永久，
  `Transient/RequestTimeout/RateLimited` → 瞬态），启发式兜底且网络信号优先于永久关键词；
  connect 失败分类通过 `ConnectResult.error_kind` 穿透重连路径，`reconnect_adapter()` 按 kind
  返回 typed error。

### Added

- **适配器重连状态透出 API 与管理后台** — 重连分类修复后，适配器 `Failed` 状态语义改变：瞬态
  失败会自动退避重试而非永久禁用。新增 `ReconnectInfo { permanent_failure, retry_attempt,
  next_retry_in_ms }`，由健康监测器同步到 manager 侧 map，`get_reconnect_info()` 暴露；
  `/api/v1/adapters` 列表返回 `last_error` + `permanent_failure` + `retry_attempt` +
  `next_retry_in_ms`。管理后台 Failed 徽章区分永久（红色，停用）与瞬态自动重试（黄色），卡片
  显示重试次数 + 下次重试实时倒计时 + 最近错误，适配器 WS 事件刷新列表以获取富化字段。
- **batch-send 按目标授权** — `POST /api/v1/messages/batch-send` 逐目标鉴权：无权限的目标记录为
  失败但不阻塞有权限目标；仅当所有目标均无权限时返回 403。同步更新 OpenAPI 403 描述与测试。
- **GHCR 镜像描述 + 发布镜像快速部署** — 设置 index 级 OCI `org.opencontainers.image.description`
  注解，GHCR 包页展示使用简介；`gateway.container.yaml`（`server.host: 0.0.0.0`）内置进
  Dockerfile，`docker run -p` 开箱即可从宿主访问；新增 `compose.quickstart.yml` 一行命令从发布
  镜像部署；README/docs Docker 快速上手改为引导拉取 GHCR 镜像，记录
  `EASYBOT_ALLOW_PLAINTEXT` 并使用真实 liveness 端点（`/api/v1/live`）。

### Docs

- **G1 本地验证证据入档** — `verify.sh --locked` 全量验收 12/12 通过日志
  （`commercial/evidence/verify-sh-pass.log`）；G1 本地演练报告（配对备份/恢复、回滚边界、
  容量测试 500@c10 p95 397ms / 1000@c20 p95 977ms 零错误、告警检测）与剩余推进项记录
  （`commercial/evidence/g1-remaining-work.md`）。

## [0.0.28] - 2026-08-02

### Changed

- **First online baseline** — Freeze `v0.0.28` as the first version eligible to
  become the online production baseline after G1 acceptance. There is no
  compatibility commitment to pre-production data from earlier candidates;
  versioned schema bootstrap, migrations, backups, and safety rollback remain
  required.
- **Verified local capacity boundary** — On the G0 Docker baseline (2 CPUs,
  512 MiB application limit), 500 requests at concurrency 10 completed with
  zero errors and p95 480.52 ms. Concurrency 20 completed without errors but
  p95 was 1446 ms, so a one-second p95 is not promised at that concurrency.

### Fixed

- **Read-only Compose runtime** — Provide bounded writable tmpfs mounts for
  EasyBot and Caddy, omit unset plaintext secrets instead of injecting empty
  values alongside `*_FILE`, and make production Compose overrides merge
  cleanly with the base resource and security policies.
- **Portable PostgreSQL image pin** — Pin PostgreSQL 16.14 Alpine to its
  multi-architecture OCI index for both amd64 and arm64 hosts.

## [0.0.27] - 2026-08-01

### Changed

- **Pre-launch compatibility boundary** — Remove unversioned legacy database detection,
  old message/config aliases, legacy Release metadata fallback, and implicit plugin ABI defaults.
  Versioned databases continue to use automatic migrations and rollback safeguards.
- **Automatic upgrade baseline** — Future automatic upgrades require EasyBot `0.0.26` or newer
  and a release `easybot-version.json` manifest.

### Fixed

- **Release metadata generation** — Derive schema and plugin ABI versions from source and emit
  valid expanded JSON for the updater manifest.

## [0.0.26] - 2026-08-01

### Fixed

- **Environment test isolation** — 串行化读取部署密钥文件的配置测试，避免并发测试互相污染进程环境变量。

## [0.0.25] - 2026-08-01

### Fixed

- **Production proxy reachability** — 生产 Compose 挂载容器内网监听 override，让 Caddy
  可以访问 EasyBot 的 `/api/v1/live`，修复应用仅监听 `127.0.0.1` 导致反向代理 503 的问题。

## [0.0.24] - 2026-08-01

### Fixed

- **Release asset upload** — 排除 release body 与版本元数据 artifact 的目录副本，避免
  `easybot-version.json` 被重复上传导致 GitHub Release 创建阶段失败。

## [0.0.23] - 2026-07-31

### Fixed

- **Production container smoke** — 生产 Compose 改用 release Alpine 运行时内置的 `wget`
  执行 `/api/v1/live` 健康检查，并统一使用 `127.0.0.1` 避免 `localhost` 解析差异导致误判。
- **Read-only production runtime** — 为生产 Compose 的只读根文件系统补齐
  `/var/lib/easybot/logs`、`plugins`、`certs` 和 `secrets` tmpfs，修复真实部署启动时
  初始化运行时目录触发 `Read-only file system` 的问题。

## [0.0.22] - 2026-07-31

### Fixed

- **Release container runtime** — `Dockerfile.release` 切换到 pinned Alpine 运行时镜像，
  保留固定 UID 与 `/api/v1/live` 容器健康检查，同时移除 Debian slim 运行时包面，修复
  `v0.0.21` tag release workflow 在 Docker Trivy 高危/严重漏洞扫描阶段失败的问题。

## [0.0.21] - 2026-07-30

### Fixed

- **Release signing fallback** — macOS/Windows 二进制构建在缺少签名/公证 secrets 时不再中断
  release workflow，而是发布 unsigned 产物并输出 warning，修复 `v0.0.20` tag release
  workflow 在桌面平台签名凭据检查阶段失败的问题。

## [0.0.20] - 2026-07-30

### Fixed

- **Release preflight portability** — 统一生产脚本中的 `stat` 调用顺序为 GNU/Linux 优先、
  BSD/macOS 兜底，修复 `v0.0.19` tag release workflow 在 backup retention
  preflight 阶段因 Linux `stat -f` 输出进入算术表达式而中断的问题。

## [0.0.19] - 2026-07-30

### Fixed

- **Release preflight portability** — 修复 `commercial-launch-gate.sh` 在 Linux CI runner
  上读取 evidence 文件权限时误用 BSD `stat -f` 语义的问题，修复 `v0.0.18` tag release
  workflow 在 Commercial release preflight 阶段中断的问题。

## [0.0.18] - 2026-07-30

### Fixed

- **Release supply-chain policy** — 更新 `deny.toml` 的 `unmaintained` 配置以兼容
  `cargo-deny 0.20.x`，修复 `v0.0.17` tag release workflow 在 Supply-chain policy 阶段
  因配置反序列化失败而中断的问题。

## [0.0.17] - 2026-07-30

### Added

- **G0 商用候选范围** — 新增 `docs/14 commercial-g0-scope.md`，冻结首个封闭付费试点的
  Managed Pilot SKU、单客户独立实例边界、平台承诺前置条件和暂不支持清单，避免商用化目标继续发散。

### Fixed

- **SQLite 商用迁移可靠性** — SQLite v2 migration 改为事务化执行，避免部分语句成功后留下半迁移
  schema；自动识别旧 v1 schema 后继续应用待执行迁移；程序遇到高于当前代码支持的 schema version
  时拒绝启动。
- **SQLite v2 rollback/reapply** — 补齐 v2 rollback，覆盖商用账本、审计、幂等、delivery journal
  等新增表，并验证 rollback 后可重新应用迁移。
- **auth schema version 口径不一致** — 生产 backup/restore、健康检查和 API Key 管理统一读取
  `_schema_version`，不再混用旧的 `PRAGMA user_version`。
- **usage export 分页断言** — 修正 `limit=1` 时 `page_requests` 的测试期望，使其等于当前页实际返回
  记录的 request_count，同时保留 total_requests 和 key rotation 后 subject 归并验证。

### Security

- **GitHub Actions supply-chain hardening** — 发布、CI、labeler、platform matrix 和复合 Rust setup
  action 中的第三方 action 统一 pin 到 commit SHA，并补齐最小 `permissions`。

### Docs

- **商用化路线收窄** — 更新 commercial readiness、roadmap、用户指南和架构文档，将当前工程推进目标
  从“大商用平台”收窄为 `v0.0.17` G0 候选版本。

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
