# 自动更新功能 — 实施任务列表

> **归档位置**: `docs/other/upgrade-strategy.md`（设计文档）
>
> 任务来源 design doc，按实施阶段拆分。每个任务为单个 PR 可处理的粒度（适合 Flash 模型）。

---

## Phase 0: 前置依赖

这些任务需要在升级功能开始前完成（优化现有代码结构）：

| # | 任务 | 文件 | 说明 | 依赖 |
|---|------|------|------|------|
| 0.1 | 提取 `db_path` 为 `EasyBotPaths` 公共字段 | `core/src/config/home.rs` | 后续迁移模块需引用 DB 路径 | — |
| 0.2 | 导出 `run_migrations()` 到独立函数 | `core/src/storage/sqlite.rs` | 当前嵌入在 main.rs 逻辑中 | — |

## Phase 1: 版本化迁移系统

数据库 schema 从"幂等全量建表"改造为"版本化增量迁移"。

| # | 任务 | 文件 | 说明 | 依赖 |
|---|------|------|------|------|
| **1.1** | 新建 `migration.rs` 定义迁移引擎核心 | `core/src/storage/migration.rs` | `Migration` struct, `SCHEMA_VERSION` 常量, `MIGRATIONS` 数组 | — |
| **1.2** | 实现 `_schema_version` 表创建与查询 | `core/src/storage/migration.rs` | `create_version_table()`, `get_current_version()`, `record_migration()` | 1.1 |
| **1.3** | 将现有 SQLite schema 提取为 v1 迁移脚本 | `core/src/storage/migration.rs` | 把 `sqlite.rs` 中的 `SCHEMA_SQL` 拆入 `MIGRATIONS[0]` | 1.1 |
| **1.4** | 实现前向迁移引擎 | `core/src/storage/migration.rs` | `run_migrations()` 遍历未执行迁移，逐版执行 + 记录 | 1.2, 1.3 |
| **1.5** | 实现反向迁移引擎（回滚） | `core/src/storage/migration.rs` | `rollback_to(version)` 逆序遍历，执行 `rollback_sql` | 1.4 |
| **1.6** | SQLite 迁移集成（替换旧 `run_migrations`） | `core/src/storage/sqlite.rs` | 将 `sqlite::run_migrations()` 委托给 `migration::run_migrations()` | 1.4 |
| **1.7** | PostgreSQL 迁移集成 + `pg_advisory_lock` 互斥 | `core/src/storage/postgres.rs` | 同上 + 多实例竞争保护 | 1.4 |
| **1.8** | 启动时 schema 版本校验 | `bin/src/main.rs` | 存储初始化后检查 `db_version == SCHEMA_VERSION`，不匹配则拒绝启动 | 1.6, 1.7 |
| **1.9** | 迁移系统单元测试 | `core/src/storage/migration.rs` | `test_migration_roundtrip`（前向→回滚→前向）、`test_empty_db`、`test_upgrade_from_v0` | 1.5 |

## Phase 2: 核心 updater 模块

| # | 任务 | 文件 | 说明 | 依赖 |
|---|------|------|------|------|
| **2.1** | 定义 updater 数据结构与错误类型 | `core/src/updater/types.rs` | `UpdateInfo`, `UpdatePlan`, `UpdateResult`, `UpdateError` 枚举（含所有失败场景） | — |
| **2.2** | 实现目标平台检测 | `core/src/updater/types.rs` | `current_target_triple()` 映射 `ARCH + OS` → GitHub asset 名 | 2.1 |
| **2.3** | 实现语义版本比较 | `core/src/updater/types.rs` | `is_newer_than(a, b)`, `cmp_versions()` | 2.1 |
| **2.4** | 实现预检：磁盘空间检查 | `core/src/updater/precheck.rs` | `check_disk_space()` — 获取 `available_space()` vs 3× 当前二进制大小 | — |
| **2.5** | 实现预检：权限检查 | `core/src/updater/precheck.rs` | `check_permissions()` — 测试二进制目录可写 | — |
| **2.6** | 实现预检：Docker/Dev 模式检测 | `core/src/updater/precheck.rs` | `detect_environment()` — Docker `/dockerenv`、dev 路径识别 | — |
| **2.7** | 实现预检：插件 ABI 兼容性 | `core/src/updater/precheck.rs` | `check_plugin_compatibility()` — 扫描已安装插件并比对 ABI 版本 | — |
| **2.8** | 实现 GitHub API 客户端 | `core/src/updater/github.rs` | `GitHubClient`：`latest_release()`, `version_manifest()`, `checksums()`, 缓存 + 速率限制处理 | 2.1 |
| **2.9** | 实现二进制下载 + SHA256 校验 | `core/src/updater/download.rs` | `download_verify()` — 流式下载到临时路径，用 `sha2` 计算哈希并与 `checksums.txt` 比对 | 2.8 |
| **2.10** | 实现备份管理 | `core/src/updater/compact.rs` | `BackupManager`：`create_backup()`（二进制 + SQLite + config）、`UpdateManifest` 读写、`restore_all()` | — |
| **2.11** | 实现服务单元路径更新 | `core/src/updater/compact.rs` | `update_service_bin_path()` — 检测 systemd/launchd 并更新 `ExecStart`/`ProgramArguments` | — |
| **2.12** | 实现二进制替换 + 回滚 | `core/src/updater/replace.rs` | `replace_binary()` — 备份→暂存→原子 rename→权限设置；`rollback_binary()` — 从 backup 恢复 | 2.10 |
| **2.13** | 实现完整更新流程编排 | `core/src/updater/mod.rs` | `Updater` struct：`check_update()` → `UpdatePlan`，`perform_update()` → 预检→备份→下载→替换→迁移→验证→清理 | 2.1-2.12 |
| **2.14** | 注册 updater 模块 | `core/src/lib.rs` | 添加 `pub mod updater;` | 2.13 |

## Phase 3: CLI 集成

| # | 任务 | 文件 | 说明 | 依赖 |
|---|------|------|------|------|
| **3.1** | CLI 增加子命令枚举 | `bin/src/main.rs` | `Commands` enum: `CheckUpdate`, `Update`, `Rollback` | 2.13 |
| **3.2** | 实现 `check-update` 处理函数 | `bin/src/main.rs` | `handle_check_update()` — 调用 `Updater::check_update()`，打印格式化的版本和迁移信息 | 3.1 |
| **3.3** | 实现 `update` 处理函数 | `bin/src/main.rs` | `handle_update()` — 预检 → 确认 → 执行完整更新流程 + 重启指引 | 3.1 |
| **3.4** | 实现 `rollback` 处理函数 | `bin/src/main.rs` | `handle_rollback()` — 读取 manifest → 恢复二进制 → 回滚 DB → 恢复 config | 3.1 |
| **3.5** | `main()` 入口分派逻辑 | `bin/src/main.rs` | 匹配子命令 → 分别调用 handle_* → 网关逻辑仅在无子命令时启动 | 3.2-3.4 |

## Phase 4: GitHub Release Workflow

| # | 任务 | 文件 | 说明 | 依赖 |
|---|------|------|------|------|
| **4.1** | 增加 `easybot-version.json` 生成步骤 | `.github/workflows/release.yml` | `prepare-release` job 末尾用 `jq` 生成版本元数据 JSON | — |
| **4.2** | 将 `easybot-version.json` 上传为 release asset | `.github/workflows/release.yml` | `create-release` 步骤的 `files:` 中包含该文件 | 4.1 |

## Phase 5: API 端点

| # | 任务 | 文件 | 说明 | 依赖 |
|---|------|------|------|------|
| **5.1** | 创建 `update-check` API 路由 | `api/src/routes/update.rs` | `GET /api/v1/system/update-check`，返回当前版本 + 最新版本 + schema 版本 + 更新状态 | 2.13 |
| **5.2** | 注册路由、权限和 OpenAPI | `api/src/server.rs`, `api/src/openapi.rs` | 添加到 protected_routes + 权限中间件 + OpenAPI 路径列表 | 5.1 |
| **5.3** | health 端点增加 `schema_version` | `api/src/routes/health.rs` | `HealthResponse` 增加 `schema_version` 字段 | — |

## Phase 6: 测试

| # | 任务 | 文件 | 说明 | 依赖 |
|---|------|------|------|------|
| **6.1** | 迁移系统测试 | `core/src/storage/migration.rs` | `test_migration_forward`, `test_migration_backward`, `test_rollback_to`, `test_idempotent` | 1.5 |
| **6.2** | updater 单元测试 | `core/src/updater/types.rs` | `test_version_comparison`, `test_target_triple` | 2.3, 2.2 |
| **6.3** | 下载 + SHA256 测试 | `core/src/updater/download.rs` | `test_sha256_match`, `test_sha256_mismatch`, `test_parse_checksums` | 2.9 |
| **6.4** | 备份/恢复测试 | `core/src/updater/replace.rs` | `test_backup_restore_cycle`（临时目录中模拟） | 2.12 |
| **6.5** | precheck 测试 | `core/src/updater/precheck.rs` | `test_disk_space_check`, `test_env_detection` | 2.4-2.7 |
| **6.6** | GitHub API mock 测试 | `core/src/updater/github.rs` | 使用 `wiremock` 模拟 GitHub API 响应 | 2.8 |
| **6.7** | 升级 + 回滚集成测试 | `tests/integration/` | 创建模拟 release + 旧版 DB → `update` → 验证 schema → `rollback` → 验证 schema 回退 | 3.1-3.5 |
| **6.8** | API 端点测试 | `api/tests/` | 验证 `update-check` 返回结构、健康端点 schema_version | 5.1 |

## Phase 7: 文档同步

| # | 任务 | 文件 | 说明 | 依赖 |
|---|------|------|------|------|
| **7.1** | 更新 `upgrade-strategy.md` | `docs/other/upgrade-strategy.md` | 根据实现实际情况修正设计文档 | 全部 |
| **7.2** | 更新 `CLAUDE.md` 项目指引 | `CLAUDE.md` | 添加 migration/updater 模块说明、任务拆分参考 | — |
| **7.3** | 更新 `user-guide.md` | `docs/01 user-guide.md` | 添加升级命令章节（check-update, update, rollback） | 3.5 |

---

## 实施顺序

```
Phase 1 (迁移引擎)
  1.1 → 1.2 → 1.3 → 1.4 → 1.5 → 1.6 → 1.7 → 1.8 → 1.9
  │
  └────────── 可以并行测试 ──────────┘

Phase 2 (updater 模块)
  2.1 → 2.2/2.3 ──→ 2.4/2.5/2.6/2.7 ──→ 2.8 → 2.9
                                                  │
                              2.10 → 2.11 → 2.12 ─┤
                                                  ▼
                                              2.13 → 2.14
  │
  └── 2.4-2.7（预检）可并行开发 ──┘

Phase 3 (CLI)  ← 需要 2.13
  └── 3.1 → 3.2 → 3.3 → 3.4 → 3.5

Phase 4 (Workflow)  ← 可独立于 Phase 2/3 开发
  └── 4.1 → 4.2

Phase 5 (API)  ← 需要 2.13
  └── 5.1 → 5.2 → 5.3

Phase 6 (Test)  ← 依赖各 Phase 完成
  └── 可并行开发：6.1/6.2/6.3/6.4/6.5/6.6/6.7/6.8

Phase 7 (文档)  ← 最后
  └── 7.1 → 7.2 → 7.3
```

## 文件映射（任务→文件）

```
core/src/storage/migration.rs        → 1.1, 1.2, 1.3, 1.4, 1.5, 1.9, 6.1
core/src/storage/sqlite.rs           → 1.6
core/src/storage/postgres.rs         → 1.7
core/src/storage/mod.rs              → 注册 migration 模块
core/src/updater/mod.rs              → 2.13
core/src/updater/types.rs            → 2.1, 2.2, 2.3, 6.2
core/src/updater/precheck.rs         → 2.4, 2.5, 2.6, 2.7, 6.5
core/src/updater/github.rs           → 2.8, 6.6
core/src/updater/download.rs         → 2.9, 6.3
core/src/updater/compact.rs          → 2.10, 2.11
core/src/updater/replace.rs          → 2.12, 6.4
core/src/lib.rs                      → 2.14
bin/src/main.rs                      → 1.8, 3.1, 3.2, 3.3, 3.4, 3.5
api/src/routes/update.rs             → 5.1
api/src/routes/health.rs             → 5.3
api/src/server.rs                    → 5.2
api/src/openapi.rs                   → 5.2
.github/workflows/release.yml        → 4.1, 4.2
tests/integration/src/               → 6.7
api/tests/                           → 6.8
docs/other/upgrade-strategy.md       → 7.1
CLAUDE.md                            → 7.2
docs/01 user-guide.md                → 7.3
```
