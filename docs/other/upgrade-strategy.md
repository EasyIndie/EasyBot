# 版本升级与自动更新方案

> **设计文档** — 描述 Rust 二进制应用的自动更新架构设计，涵盖二进制替换、数据库迁移、配置迁移、回滚等完整升级流程。
>
> 本文档可作为其他类似项目的参考。

---

## 1. 背景与目标

### 1.1 问题

随着版本迭代推进，用户在从旧版本升级到新版本时，需要完成一系列繁琐的手动操作。一个完整的升级远不止替换二进制文件——它还涉及数据库 schema 迁移、配置格式适配、插件兼容性验证、服务管理器更新等多个层面。

### 1.2 目标

- 提供 `easybot update` 一键升级命令
- 支持安全回滚（`easybot rollback`）
- 覆盖所有部署场景（裸机二进制、systemd/launchd 服务、Docker 容器）
- 更新前可预览影响范围（`easybot check-update`）
- 更新过程可审计、可中断、可恢复

### 1.3 非目标

- 不实现零宕机滚动升级（当前架构为单进程 daemon）
- 不自动重启服务（由用户/服务管理器决定何时重启）
- 不提供插件自动重新编译（插件由第三方维护）

---

## 2. 升级涉及的所有层面

版本升级的核心挑战在于：**替换二进制只是起点**。一个安全的升级系统必须审计以下 8 个层面：

```
二进制 (binary)
  └─ 可执行文件替换（OS 差异、原子性、权限）

数据库 Schema (schema)
  ├─ 表结构增减（CREATE TABLE / ALTER TABLE）
  ├─ 索引变更
  └─ 数据约束变化

数据库数据 (data)
  ├─ 数据迁移（列拆分、类型转换、默认值填充）
  └─ 数据完整性（外键、唯一约束）

配置 (config)
  ├─ 配置字段新增/删除/重命名
  └─ 配置结构重组（嵌套层级变化）

插件兼容性 (plugin)
  ├─ Plugin ABI 版本
  └─ Plugin SDK 接口变更

服务管理器 (service manager)
  ├─ systemd unit 中的 ExecStart 硬编码路径
  ├─ launchd plist 中的 ProgramArguments
  └─ Windows 服务的 binPath

API/观测面 (observability)
  ├─ API 响应格式变化（字段新增/删除/重命名）
  ├─ Prometheus metric 标签变化
  └─ 日志格式变化

升级自身安全 (safety)
  ├─ 磁盘空间不足
  ├─ 权限不足（二进制被 root 拥有）
  ├─ 网络不可达（离线环境）
  ├─ 多实例竞争迁移
  ├─ 降级保护
  └─ 升级跳版本过多
```

---

## 3. 总体架构

### 3.1 核心流程

```
用户: easybot check-update
  │
  ▼
┌─────────────┐    ┌──────────────────┐    ┌─────────────┐
│ Pre-Check   │───▶│ GitHub API 检测  │───▶│ UpdatePlan  │
│ 磁盘/权限/网 │    │ 版本/迁移清单    │    │ 完整预览    │
│ 络/插件/Docker│   │ checksums.txt    │    │             │
└─────────────┘    └──────────────────┘    └─────────────┘
                                                │
                                                ▼ 用户确认
                                          ┌────────────────┐
                                          │ Backup Phase   │
                                          │ 二进制 + DB +   │
                                          │ config + manifest│
                                          └────────────────┘
                                                │
                                                ▼
                                          ┌────────────────┐
                                          │ Apply Phase    │
                                          │ 下载 → SHA256→ │
                                          │ 替换 → 迁移 →  │
                                          │ 服务更新       │
                                          └────────────────┘
                                                │
                                                ▼
                                          ┌────────────────┐
                                          │ Verify Phase   │
                                          │ 验证新二进制 → │
                                          │ 成功: 清理备份  │
                                          │ 失败: 自动回滚 │
                                          └────────────────┘
                                                │
                                                ▼
                                          提示用户重启服务
```

### 3.2 状态流转

```
                    ┌──────────┐
                    │  就绪    │
                    └────┬─────┘
                         │ easybot update
                         ▼
                    ┌──────────┐
                    │  预检中  │ ← 磁盘/权限/网络/插件检查
                    └────┬─────┘
                         │ 通过
                         ▼
                    ┌──────────┐
                    │  计划中  │ ← GitHub API + 生成 UpdatePlan
                    └────┬─────┘
                         │ 用户确认
                         ▼
                    ┌──────────┐
                    │  备份中   │ ← 二进制 + DB + config
                    └────┬─────┘
                         │ 成功
                         ▼
                    ┌──────────┐
                    │  应用中   │ ← 下载 → SHA256 → 替换 → 迁移
                    └────┬─────┘
                    ┌────┴────┐
                    │         │
                    ▼         ▼
              ┌────────┐ ┌──────────┐
              │ 验证中  │ │ 自动回滚 │ ← 任何失败
              └────┬───┘ └────┬─────┘
                   │ 成功     │
                   ▼          ▼
              ┌────────┐ ┌──────────┐
              │ 已完成  │ │ 回滚完成 │
              └────────┘ └──────────┘
```

### 3.3 回滚流程

```
easybot rollback
  │
  ▼
读取 .update_manifest.json（备份清单）
  │
  ▼
┌──────────┐    ┌──────────┐    ┌──────────┐
│ 停止服务 │───▶│ 恢复二进制│───▶│ 回滚 DB  │
└──────────┘    └──────────┘    └──────────┘
                                      │
                                      ▼
                               ┌──────────────┐
                               │ 恢复 config  │
                               └──────┬───────┘
                                      ▼
                               ┌──────────────┐
                               │ 启动服务     │
                               │ 清除 manifest│
                               └──────────────┘
```

---

## 4. 数据模型

### 4.1 版本声明（编译时嵌入）

每个二进制版本在编译时声明其所需的 schema 版本和插件 ABI 版本：

```rust
// crates/easybot-core/src/storage/migration.rs
/// 二进制所期望的数据库 schema 版本
pub const SCHEMA_VERSION: i64 = 1;

/// 已注册的所有迁移脚本
pub static MIGRATIONS: &[Migration] = &[
    Migration { version: 1, description: "...", sql: V1_SQL, rollback_sql: Some(V1_RB) },
];
```

### 4.2 数据库 Schema 版本表

```sql
CREATE TABLE IF NOT EXISTS _schema_version (
    version     INTEGER NOT NULL,
    applied_at  INTEGER NOT NULL,
    description TEXT NOT NULL
);
```

### 4.3 版本发布清单（Release asset）

```json
{
  "version": "0.1.0",
  "tag": "v0.1.0",
  "release_date": "2026-07-21",
  "schema_version": 2,
  "requires_db_migration": true,
  "migrations": [
    { "version": 2, "description": "Add webhook_url column to sessions" }
  ],
  "requires_config_migration": false,
  "config_changes": [],
  "breaking_changes": [
    "The /api/v1/messages endpoint now requires 'platform' parameter"
  ],
  "plugin_abi_version": 1,
  "min_upgradable_from": "0.0.10"
}
```

### 4.4 更新备份清单

```json
{
  "timestamp": 1721571200000,
  "from_version": "0.0.16",
  "to_version": "0.1.0",
  "from_schema_version": 1,
  "to_schema_version": 2,
  "binary_backup": "/usr/local/bin/easybot.bak.0.0.16",
  "db_backup": "/home/easybot/.easybot/data/gateway.db.bak.0.0.16",
  "config_backup": "/home/easybot/.easybot/gateway.yaml.bak.0.0.16",
  "migrations_applied": [2]
}
```

### 4.5 更新计划（用户预览）

```rust
pub struct UpdatePlan {
    pub current_version: String,           // v0.0.16
    pub target_version: String,            // v0.1.0
    pub target_schema_version: i64,        // 2
    pub current_schema_version: i64,       // 1
    pub requires_db_migration: bool,       // true
    pub db_migrations: Vec<MigrationInfo>, // [{v2, "Add webhook_url"}]
    pub requires_config_migration: bool,
    pub config_changes: Vec<String>,
    pub breaking_changes: Vec<String>,
    pub plugin_incompatible: Vec<String>,
    pub binary_size: u64,
    pub checksum: String,
    pub requires_service_update: bool,     // systemd ExecStart 需更新
}
```

---

## 5. 关键设计决策

### 5.1 使用 GitHub Releases API 而非独立更新服务器

| 方案 | 优点 | 缺点 |
|------|------|------|
| ✅ GitHub Releases API | 零运维、与 CI 合一、已有 assets | 速率限制（60/h 未认证） |
| ❌ 独立更新服务器 | 无速率限制、可私有化部署 | 额外运维成本 |
| ❌ S3/对象存储 | 下载快 | 需要签名、额外成本 |

**结论**：GitHub Releases API + 可选 `GITHUB_TOKEN` 提升速率限制。

### 5.2 自定义实现 vs `self_update` crate

| 方案 | 优点 | 缺点 |
|------|------|------|
| ✅ 自定义（reqwest + sha2） | 轻量、全可控、无额外传递依赖 | 需要编写更多代码 |
| ❌ `self_update` crate | 开箱即用、成熟 | 依赖重（zip/tar/gz 等）、不符合 EasyBot 发布模式 |

**结论**：自定义实现。EasyBot 发布的是纯二进制（非压缩包），`self_update` 的 archive 处理能力是冗余的。已有 `reqwest` + `sha2` 依赖。

### 5.3 版本化增量迁移 vs 全量幂等脚本

| 方案 | 优点 | 缺点 |
|------|------|------|
| ✅ 版本化增量迁移 | 可追溯、可控、可回滚、可跳过已执行 | 需要 schema 版本表 |
| ❌ 全量幂等 `CREATE TABLE IF NOT EXISTS` | 简单 | 不支持 `ALTER TABLE`、无回滚 |

**结论**：将现有全量迁移改造为版本化增量迁移。首次迁移系统记录现有 schema 为 v1。

### 5.4 嵌入迁移脚本 vs 外部 SQL 文件

| 方案 | 优点 | 缺点 |
|------|------|------|
| ✅ 嵌入二进制（`const &str`） | 版本与二进制锁定、无文件丢失 | 大 SQL 增加二进制体积 |
| ❌ 外部 SQL 文件（SQLite 迁移目录） | 迁移文件可独立管理 | 路径问题、版本与二进制可能不一致 |

**结论**：嵌入二进制。迁移 SQL 通常很小（KB 级别），版本锁定更安全。

---

## 6. 安全性设计

### 6.1 传输安全

- 所有下载通过 HTTPS（`reqwest` 默认使用 rustls）
- GitHub API 强制 HTTPS

### 6.2 完整性校验

```
GitHub Release
  ├── easybot-x86_64-unknown-linux-musl  (二进制)
  ├── checksums.txt                       (SHA256 校验和)
  └── easybot-version.json                (版本元数据)

校验流程:
  github.rs → 获取 checksums.txt
  download.rs → 流式下载二进制
  verify.rs → SHA256(二进制) == checksums.txt[文件名]
  replace.rs → 仅在校验通过后才替换
```

### 6.3 原子替换与回滚

```rust
fn replace_binary(new_bin: &Path, current: &Path) -> Result<Backup, Error> {
    // 1. 备份
    std::fs::copy(current, backup)?;
    // 2. 暂存（同文件系统，确保 rename 原子性）
    std::fs::rename(new_bin, temp)?;
    // 3. 原子替换
    std::fs::rename(temp, current)?;
    // 4. 如失败 → 从 backup 恢复
    // 5. 如成功 → 删除 backup
}
```

### 6.4 降级保护

- 仅允许 `semver::Version::newer_than`（新版本 > 当前版本）
- 启动时：如果 `DB.schema_version > binary.SCHEMA_VERSION` → 拒绝启动
- 如需要降级：`easybot rollback [version]`

### 6.5 互斥迁移

| 数据库 | 机制 |
|--------|------|
| SQLite | `BEGIN IMMEDIATE` 事务 |
| PostgreSQL | `pg_advisory_lock(1145258561)` |

---

## 7. 错误处理策略

### 7.1 预检失败

所有预检失败在 Phase 1 中提前报出，不产生任何副作用：

| 失败原因 | 提示信息 |
|---------|---------|
| 磁盘空间不足 | "需要 xxx MB 可用空间，当前仅 xxx MB" |
| 权限不足 | "无权写入 {path}，请使用 sudo 或以 root 执行" |
| 运行在 Docker 中 | "Docker 用户请使用 docker compose pull && up -d" |
| 运行在开发模式 | "开发模式不支持自动更新" |
| GitHub API 不可达 | "离线模式：无法检查更新。使用 --offline <path> 从本地文件更新" |
| 插件不兼容 | "以下插件与新版本不兼容: [列表]。使用 --skip-plugin-check 强制更新" |

### 7.2 备份失败

备份失败的恢复策略：

```rust
// 备份分阶段进行，早期失败不产生副作用
fn create_backups() -> Result<UpdateManifest, Error> {
    // 每个备份独立进行，已成功的备份在失败时回退
    let bin_backup = backup_binary().ok();
    let db_backup = backup_database().ok();
    let cfg_backup = backup_config()?;  // config 是最重要的，失败则中止
    
    if bin_backup.is_none() || db_backup.is_none() {
        // 警告但不中止：某些备份失败（如 SQLite 不可用），但仍可继续
    }
    Ok(UpdateManifest {
        binary_backup: bin_backup,
        db_backup,
        config_backup: cfg_backup,
    })
}
```

### 7.3 迁移失败

数据库迁移失败 → 自动回滚：

```
migration::run_migrations()
  │
  ┌─── v2: ALTER TABLE sessions ADD COLUMN webhook_url TEXT
  │     ├── 成功 → 提交，写入 _schema_version
  │     └── 失败 → 回滚事务，上一步的 migration 状态恢复
  │
  └─── 任何 migration 失败 → 整体失败提示
            │
            自动回滚: replace.rs 恢复旧二进制
                      完整恢复到备份状态
```

### 7.4 验证失败

新二进制替换后验证失败 → 全量回滚：

```rust
match verify_new_binary(new_bin).await {
    Ok(_) => {
        // 清理备份
        cleanup_backup(manifest);
        println!("✓ 升级完成，请重启服务");
    }
    Err(e) => {
        // 自动回滚
        rollback::restore_binary(&manifest).await?;
        rollback::restore_database(&manifest).await?;
        // 无需恢复 config（未修改）
        println!("✗ 新二进制验证失败（{}），已自动回滚", e);
    }
}
```

---

## 8. EasyBot 实现计划

### 8.1 实施阶段

```
Phase 1: 重构迁移系统 (2-3 天)
  ├── 创建 _schema_version 表
  ├── 将现有 SCHEMA_SQL 拆分为 v1 迁移
  ├── 运行迁移时版本追踪
  ├── 启动时 schema 版本锁定
  └── PostgreSQL pg_advisory_lock 互斥

Phase 2: 核心 updater 模块 (2-3 天)
  ├── types.rs 数据结构
  ├── github.rs API 客户端
  ├── download.rs 下载 + SHA256
  ├── replace.rs 二进制替换（含 backup/rollback）
  ├── precheck.rs 预检
  └── compact.rs 备份清单 + 服务更新

Phase 3: CLI 集成 (1 天)
  ├── check-update / update / rollback 子命令
  ├── 完整更新流程编排
  ├── 启动时 DB schema 校验
  └── 用户交互提示

Phase 4: Release 工作流 (0.5 天)
  ├── easybot-version.json 生成
  └── workflow 中上传

Phase 5: API + 文档 (0.5 天)
  ├── GET /api/v1/system/update-check 端点
  └── 更新用户文档

Phase 6: 测试 (1-2 天)
  ├── 单元测试
  ├── 迁移前/后测试
  ├── 升级 + 回滚集成测试
  └── 边界测试（离线、权限不足等）
```

### 8.2 涉及文件

| 文件 | 操作 | 说明 |
|------|------|------|
| `crates/easybot-core/src/storage/migration.rs` | **新建** | 版本化迁移引擎 |
| `crates/easybot-core/src/storage/mod.rs` | 修改 | 注册 migration 模块 |
| `crates/easybot-core/src/storage/sqlite.rs` | 修改 | `run_migrations` 委托给 migration 引擎 |
| `crates/easybot-core/src/storage/postgres.rs` | 修改 | 同上 + `pg_advisory_lock` |
| `crates/easybot-core/src/updater/mod.rs` | **新建** | 更新入口 + 流程编排 |
| `crates/easybot-core/src/updater/types.rs` | **新建** | 数据模型 |
| `crates/easybot-core/src/updater/precheck.rs` | **新建** | 预检 |
| `crates/easybot-core/src/updater/github.rs` | **新建** | GitHub API |
| `crates/easybot-core/src/updater/download.rs` | **新建** | 下载 + SHA256 |
| `crates/easybot-core/src/updater/replace.rs` | **新建** | 二进制替换 |
| `crates/easybot-core/src/updater/compact.rs` | **新建** | 备份 + 服务更新 |
| `crates/easybot-core/src/lib.rs` | 修改 | 注册 updater |
| `bin/src/main.rs` | 修改 | CLI 子命令 + schema 校验 |
| `crates/easybot-api/src/routes/update.rs` | **新建** | API 端点 |
| `crates/easybot-api/src/server.rs` | 修改 | 注册路由 |
| `crates/easybot-api/src/openapi.rs` | 修改 | OpenAPI 注册 |
| `.github/workflows/release.yml` | 修改 | 生成 version.json |
| `Cargo.toml` (workspace) | 修改 | 添加 `semver` 依赖 |

### 8.3 分支策略

```
main
  │
  └── feature/auto-update       ← 在此分支开发
        │
        ├── Phase 1 (migration)
        ├── Phase 2 (updater)
        ├── Phase 3 (CLI)
        ├── Phase 4 (workflow)
        └── Phase 5 (API + docs)
              │
              └── merge → main (@ v0.1.0)
```

---

## 9. 扩展性考虑

### 9.1 被其他项目参考

本文档的设计模式不限于 Rust 或 EasyBot。以下框架可直接复用：

| 设计要素 | 通用性 |
|---------|--------|
| 8 层面审计清单 | 任意语言/项目的升级风险评估 |
| Schema 版本表 + 增量迁移 | 任意需要数据库迁移的项目 |
| 迁移互斥锁 | 多实例部署的任何项目 |
| 备份清单 Manifest | 任何需要安全回滚的项目 |
| 预检机制 | 任何变更操作的边界检查 |
| 原子替换 + 3 步回滚 | 任何二进制/文件替换操作 |
| `min_upgradable_from` 跳版本保护 | 任何无法无限向后兼容的项目 |

### 9.2 未来增强方向

- **Zero-downtime rolling update**：当 EasyBot 支持多实例部署时，可通过 LB 健康检查实现
- **Watchtower 集成**：Docker 用户的自动更新 sidecar（已有 `containrrr/watchtower`）
- **Webhook 通知**：更新完成后通过 Webhook 通知管理员
- **自动回滚监控**：升级后监控一段时间内是否有异常，自动触发回滚
- **增量二进制下载**：使用 `bsdiff` 实现增量更新（大幅减少下载量）

---

## 10. 测试策略

### 10.1 单元测试

```rust
// 版本比较
#[test]
fn test_version_comparison() {
    assert!(is_newer_than("0.1.0", "0.0.16"));
    assert!(!is_newer_than("0.0.16", "0.1.0"));
    assert!(!is_newer_than("0.0.16", "0.0.16"));
}

// 校验和解析
#[test]
fn test_parse_checksums() {
    let c = parse_checksums("abc  easybot-x86_64-linux\n");
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].0, "abc");
}

// 平台检测
#[test]
fn test_target_triple() {
    let t = current_target_triple();
    assert!(t.contains("linux") || t.contains("apple") || t.contains("windows"));
}

// 迁移前/后
#[test]
fn test_migration_roundtrip() {
    // 创建空 DB → version 0
    // 运行迁移 → version 1
    // 回滚到 version 0 → 表不存
    // 再次迁移 → version 1
}

// 备份/恢复
#[test]
fn test_backup_restore_cycle() {
    // 创建临时二进制
    // 备份它
    // 写入新内容
    // 从备份恢复
    // 验证内容与原始一致
}
```

### 10.2 集成测试

```rust
// 模拟 GitHub API
#[tokio::test]
async fn test_github_api_latest() {
    let mock = wiremock::MockServer::start().await;
    // 模拟 /repos/EasyIndie/EasyBot/releases/latest
    // 验证解析正确
}

// 升级 + 回滚完整模拟
#[tokio::test]
async fn test_full_update_rollback() {
    // 1. 创建旧版 binary + 旧版 SQLite DB
    // 2. 创建模拟的 "新版本" release
    // 3. 执行 update
    // 4. 验证 DB schema 升级
    // 5. 执行 rollback
    // 6. 验证 DB schema 回退到旧版本
    // 7. 验证旧 binary 能正常读写 DB
}
```

### 10.3 手动验证清单

```bash
# 1. 编译
cargo build --release

# 2. 检查更新
./target/release/easybot check-update

# 3. 离线测试
# 阻断网络
./target/release/easybot check-update
# 预期: "离线模式，无法检查更新"

# 4. 权限测试
sudo chown root:root /usr/local/bin/easybot
./target/release/easybot update
# 预期: "权限不足，请使用 sudo 执行"

# 5. Docker 检测测试
touch /.dockerenv
./target/release/easybot update
# 预期: "Docker 用户请使用 docker compose pull"
rm /.dockerenv
```

---

## 11. 附录

### 11.1 术语表

| 术语 | 说明 |
|------|------|
| Schema | 数据库表结构定义 |
| Migration | 数据库 schema 版本间的增量变更（含前向和回滚） |
| Update | 完整的版本升级过程（二进制 + 数据库 + 配置） |
| Rollback | 恢复到前一个版本（二进制 + 数据库 + 配置） |
| Pre-check | 更新前的预检阶段（磁盘/权限/网络等） |
| Backup Manifest | 备份清单文件，记录所有备份的位置，用于回滚 |
| ABI | Application Binary Interface，插件的二进制接口版本 |
| `semver` | 语义化版本号 (`major.minor.patch`) |
| `min_upgradable_from` | 发布清单中的字段，标识从哪个版本开始可直接升级到本版本 |

### 11.2 参考

- [semver.org](https://semver.org/) — 语义化版本标准
- [GitHub Releases API](https://docs.github.com/en/rest/releases/releases) — 版本发布接口
- [SQLite WAL mode](https://www.sqlite.org/wal.html) — 并发读写支持
- [PostgreSQL Advisory Lock](https://www.postgresql.org/docs/current/explicit-locking.html#ADVISORY-LOCKS) — 应用级互斥锁
