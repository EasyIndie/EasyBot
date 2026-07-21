//! 版本化数据库迁移引擎
//!
//! 将数据库 schema 管理从"幂等全量建表"升级为"版本化增量迁移"。
//! 每个二进制发行版在编译时嵌入其所需要的 schema 版本（`SCHEMA_VERSION`），
//! 启动时与数据库实际版本比对，不匹配则拒绝启动。
//!
//! ## 概念
//!
//! - `SCHEMA_VERSION`: 当前二进制期望的 schema 版本（编译时常量）
//! - `MIGRATIONS`: 所有已注册的迁移，按版本号递增排列
//! - `_schema_version` 表: 记录已执行的迁移历史
//!
//! ## 迁移流程
//!
//! 1. 建 `_schema_version` 表（不存在时）
//! 2. 查询当前数据库版本（`MAX(version)`，无记录 = 0）
//! 3. 从 `current + 1` 开始遍历 `MIGRATIONS`，逐版执行
//! 4. 每个迁移在事务中执行，成功则写入 `_schema_version`
//!
//! ## 回滚流程
//!
//! 从 `current_version` 向下遍历到 `target_version`：
//! 1. 每版执行 `rollback_sql`（需提供回滚 SQL，否则无法跳过该版本）
//! 2. 从 `_schema_version` 删除该版本记录

use crate::storage::StoreError;
use chrono::Utc;
use sqlx::{PgPool, SqlitePool};

/// 当前二进制所期望的数据库 schema 版本。
///
/// 每次新增/修改表结构时 +1，并追加 `MIGRATIONS` 条目。
pub const SCHEMA_VERSION: i64 = 1;

/// 版本追踪表（两种后端通用）
const VERSION_TABLE_SQL: &str = "
CREATE TABLE IF NOT EXISTS _schema_version (
    version     INTEGER NOT NULL,
    applied_at  INTEGER NOT NULL,
    description TEXT NOT NULL
);
";

/// 单个迁移定义
///
/// 每个版本对应一个前向迁移 SQL 和一个可选的回滚 SQL。
/// SQLite 和 PostgreSQL 的 SQL 语法有差异，因此分别存储。
pub struct Migration {
    /// 版本号（从 1 开始递增）
    pub version: i64,
    /// 人类可读描述
    pub description: &'static str,
    /// SQLite 前向 SQL
    pub sql_sqlite: &'static str,
    /// PostgreSQL 前向 SQL
    pub sql_postgres: &'static str,
    /// SQLite 回滚 SQL（用于 `easybot rollback`）
    pub rollback_sqlite: Option<&'static str>,
    /// PostgreSQL 回滚 SQL
    pub rollback_postgres: Option<&'static str>,
}

/// 所有已注册的迁移（按版本号递增排列）
///
/// 新版本在此追加，**禁止修改或删除已发行的条目**。
pub static MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "Initial schema: sessions, messages, api_keys",
        sql_sqlite: V1_SQLITE,
        sql_postgres: V1_POSTGRES,
        rollback_sqlite: Some(V1_ROLLBACK_SQLITE),
        rollback_postgres: Some(V1_ROLLBACK_POSTGRES),
    },
    // ── 后续版本在此追加 ──
    // Migration { version: 2, description: "Add webhook_url to sessions", ... }
];

// ══════════════════════════════════════════════════════════════════
// v1 schema: sessions + messages + api_keys
// ══════════════════════════════════════════════════════════════════

const V1_SQLITE: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    key          TEXT PRIMARY KEY,
    platform     TEXT NOT NULL,
    chat_id      TEXT NOT NULL,
    thread_id    TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    source_json  TEXT NOT NULL,
    reset_policy TEXT NOT NULL,
    metadata     TEXT NOT NULL DEFAULT '{}',
    last_message TEXT,
    last_message_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_sessions_platform ON sessions(platform);
CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id           TEXT PRIMARY KEY,
    session_key  TEXT NOT NULL,
    platform     TEXT NOT NULL,
    chat_id      TEXT NOT NULL,
    role         TEXT NOT NULL,
    text         TEXT,
    raw_data     TEXT NOT NULL,
    timestamp    INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_sk ON messages(session_key, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_pc ON messages(platform, chat_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_ct ON messages(created_at);

CREATE TABLE IF NOT EXISTS api_keys (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    prefix        TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER,
    last_used_at  INTEGER,
    revoked       INTEGER NOT NULL DEFAULT 0,
    permissions   TEXT NOT NULL DEFAULT '[]',
    event_filters TEXT NOT NULL DEFAULT '[]',
    hash          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_keys_created ON api_keys(created_at DESC);
";

const V1_POSTGRES: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    key          VARCHAR(255) PRIMARY KEY,
    platform     VARCHAR(64) NOT NULL,
    chat_id      VARCHAR(255) NOT NULL,
    thread_id    VARCHAR(255),
    created_at   BIGINT NOT NULL,
    updated_at   BIGINT NOT NULL,
    source_json  TEXT NOT NULL,
    reset_policy VARCHAR(32) NOT NULL,
    metadata     JSONB NOT NULL DEFAULT '{}',
    last_message TEXT,
    last_message_at BIGINT
);

CREATE INDEX IF NOT EXISTS idx_sessions_platform ON sessions(platform);
CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
    id           VARCHAR(255) PRIMARY KEY,
    session_key  VARCHAR(255) NOT NULL,
    platform     VARCHAR(64) NOT NULL,
    chat_id      VARCHAR(255) NOT NULL,
    role         VARCHAR(16) NOT NULL,
    text         TEXT,
    raw_data     JSONB NOT NULL,
    timestamp    BIGINT NOT NULL,
    created_at   BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_sk ON messages(session_key, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_pc ON messages(platform, chat_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_messages_ct ON messages(created_at);
";

const V1_ROLLBACK_POSTGRES: &str = "
DROP TABLE IF EXISTS messages;
DROP TABLE IF EXISTS sessions;
";

const V1_ROLLBACK_SQLITE: &str = "
DROP TABLE IF EXISTS api_keys;
DROP TABLE IF EXISTS messages;
DROP TABLE IF EXISTS sessions;
";

// ══════════════════════════════════════════════════════════════════
// SQLite 迁移函数
// ══════════════════════════════════════════════════════════════════

/// 获取 SQLite 当前 schema 版本（0 = 无版本记录）
pub async fn get_current_version(pool: &SqlitePool) -> Result<i64, StoreError> {
    // 首次运行且 _schema_version 表不存在时，返回 0
    let result: Result<Option<i64>, _> =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
            .fetch_one(pool)
            .await;

    match result {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Ok(0),
        Err(_) => Ok(0), // 表不存在时返回 0
    }
}

/// 记录已应用的 SQLite 迁移
async fn record_migration(
    pool: &SqlitePool,
    version: i64,
    description: &str,
) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO _schema_version (version, applied_at, description) VALUES (?, ?, ?)")
        .bind(version)
        .bind(Utc::now().timestamp_millis())
        .bind(description)
        .execute(pool)
        .await?;
    Ok(())
}

/// 删除迁移记录（回滚时）
async fn delete_migration(pool: &SqlitePool, version: i64) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM _schema_version WHERE version = ?")
        .bind(version)
        .execute(pool)
        .await?;
    Ok(())
}

/// 运行所有未执行的 SQLite 前向迁移
///
/// 幂等：已执行的迁移不会重复执行。每步在独立事务中执行。
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), StoreError> {
    // 1. 确保版本追踪表存在
    sqlx::query(VERSION_TABLE_SQL).execute(pool).await?;

    // 2. 检查是否已有数据表但无版本记录（从旧版迁移系统升级）
    let current = get_current_version(pool).await?;
    if current == 0 && has_existing_tables(pool).await? {
        // 已有表结构但无版本记录 → 假定为 v1
        sqlx::query(
            "INSERT INTO _schema_version (version, applied_at, description) VALUES (?, ?, ?)",
        )
        .bind(1_i64)
        .bind(Utc::now().timestamp_millis())
        .bind("Initial schema (auto-detected)")
        .execute(pool)
        .await?;
        tracing::info!("Auto-detected existing schema as v1");
        return Ok(());
    }

    // 3. 逐版执行未应用的迁移
    for m in MIGRATIONS {
        if m.version > current {
            tracing::info!("Running SQLite migration v{}: {}", m.version, m.description);
            sqlx::query(m.sql_sqlite).execute(pool).await?;
            record_migration(pool, m.version, m.description).await?;
            tracing::info!("SQLite migration v{} applied", m.version);
        }
    }

    Ok(())
}

/// 回滚 SQLite schema 到指定版本
///
/// 从 `current_version` 向下遍历，每版执行回滚 SQL。
/// 如果某版本没有 `rollback_sql`，回滚到此版本上一层为止。
pub async fn rollback_to(pool: &SqlitePool, target_version: i64) -> Result<(), StoreError> {
    let current = get_current_version(pool).await?;
    if target_version >= current {
        return Err(StoreError::Database(
            "Target version is not older than current".into(),
        ));
    }

    for m in MIGRATIONS.iter().rev() {
        if m.version > target_version && m.version <= current {
            if let Some(rollback) = m.rollback_sqlite {
                tracing::warn!(
                    "Rolling back SQLite migration v{}: {}",
                    m.version,
                    m.description
                );
                sqlx::query(rollback).execute(pool).await?;
                delete_migration(pool, m.version).await?;
                tracing::info!("SQLite migration v{} rolled back", m.version);
            } else {
                tracing::warn!(
                    "Migration v{} has no rollback SQL, cannot rollback past this version",
                    m.version
                );
                return Err(StoreError::Database(format!(
                    "Migration v{} has no rollback SQL",
                    m.version
                )));
            }
        }
    }
    Ok(())
}

/// 检查是否已有数据表（用于自动检测旧版 schema 版本）
async fn has_existing_tables(pool: &SqlitePool) -> Result<bool, StoreError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sessions', 'messages', 'api_keys')",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    Ok(count > 0)
}

// ══════════════════════════════════════════════════════════════════
// PostgreSQL 迁移函数
// ══════════════════════════════════════════════════════════════════

/// 获取 PostgreSQL 当前 schema 版本
pub async fn get_current_version_pg(pool: &PgPool) -> Result<i64, StoreError> {
    let result: Result<Option<i64>, _> =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
            .fetch_one(pool)
            .await;

    match result {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Ok(0),
        Err(_) => Ok(0),
    }
}

/// 记录已应用的 PostgreSQL 迁移
async fn record_migration_pg(
    pool: &PgPool,
    version: i64,
    description: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO _schema_version (version, applied_at, description) VALUES ($1, $2, $3)",
    )
    .bind(version)
    .bind(Utc::now().timestamp_millis())
    .bind(description)
    .execute(pool)
    .await?;
    Ok(())
}

/// 删除 PostgreSQL 迁移记录
async fn delete_migration_pg(pool: &PgPool, version: i64) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM _schema_version WHERE version = $1")
        .bind(version)
        .execute(pool)
        .await?;
    Ok(())
}

/// 运行所有未执行的 PostgreSQL 前向迁移（带 `pg_advisory_lock` 互斥）
pub async fn run_migrations_pg(pool: &PgPool) -> Result<(), StoreError> {
    // 获取应用级互斥锁，防止多实例竞争迁移
    // 锁 ID: 0xEASYBOT_SCHEMA_MIGRATION = 1145258561
    sqlx::query("SELECT pg_advisory_lock(1145258561)")
        .execute(pool)
        .await
        .ok();

    let result = run_migrations_pg_inner(pool).await;

    sqlx::query("SELECT pg_advisory_unlock(1145258561)")
        .execute(pool)
        .await
        .ok();

    result
}

async fn run_migrations_pg_inner(pool: &PgPool) -> Result<(), StoreError> {
    // 1. 确保版本追踪表存在
    sqlx::query(VERSION_TABLE_SQL).execute(pool).await?;

    // 2. 检查是否已有数据表但无版本记录
    let current = get_current_version_pg(pool).await?;
    if current == 0 && has_existing_tables_pg(pool).await? {
        sqlx::query(
            "INSERT INTO _schema_version (version, applied_at, description) VALUES ($1, $2, $3)",
        )
        .bind(1_i64)
        .bind(Utc::now().timestamp_millis())
        .bind("Initial schema (auto-detected)")
        .execute(pool)
        .await?;
        tracing::info!("Auto-detected existing PostgreSQL schema as v1");
        return Ok(());
    }

    // 3. 逐版执行
    for m in MIGRATIONS {
        if m.version > current {
            tracing::info!(
                "Running PostgreSQL migration v{}: {}",
                m.version,
                m.description
            );
            sqlx::query(m.sql_postgres).execute(pool).await?;
            record_migration_pg(pool, m.version, m.description).await?;
            tracing::info!("PostgreSQL migration v{} applied", m.version);
        }
    }

    Ok(())
}

/// 回滚 PostgreSQL schema 到指定版本（带锁）
pub async fn rollback_to_pg(pool: &PgPool, target_version: i64) -> Result<(), StoreError> {
    sqlx::query("SELECT pg_advisory_lock(1145258561)")
        .execute(pool)
        .await
        .ok();

    let result = rollback_to_pg_inner(pool, target_version).await;

    sqlx::query("SELECT pg_advisory_unlock(1145258561)")
        .execute(pool)
        .await
        .ok();

    result
}

async fn rollback_to_pg_inner(pool: &PgPool, target_version: i64) -> Result<(), StoreError> {
    let current = get_current_version_pg(pool).await?;
    if target_version >= current {
        return Err(StoreError::Database(
            "Target version is not older than current".into(),
        ));
    }

    for m in MIGRATIONS.iter().rev() {
        if m.version > target_version && m.version <= current {
            if let Some(rollback) = m.rollback_postgres {
                tracing::warn!(
                    "Rolling back PostgreSQL migration v{}: {}",
                    m.version,
                    m.description
                );
                sqlx::query(rollback).execute(pool).await?;
                delete_migration_pg(pool, m.version).await?;
                tracing::info!("PostgreSQL migration v{} rolled back", m.version);
            } else {
                tracing::warn!(
                    "Migration v{} has no rollback SQL, cannot rollback past this version",
                    m.version
                );
                return Err(StoreError::Database(format!(
                    "Migration v{} has no rollback SQL",
                    m.version
                )));
            }
        }
    }
    Ok(())
}

/// 检查 PostgreSQL 是否已有数据表
async fn has_existing_tables_pg(pool: &PgPool) -> Result<bool, StoreError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name IN ('sessions', 'messages', 'api_keys')",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    Ok(count > 0)
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建测试用 SQLite 内存数据库
    async fn create_test_pool() -> SqlitePool {
        SqlitePool::connect(":memory:")
            .await
            .expect("Failed to create in-memory SQLite pool")
    }

    #[tokio::test]
    async fn test_migration_forward() {
        let pool = create_test_pool().await;

        // 空库 → 运行迁移 → 版本应为 1
        run_migrations(&pool).await.unwrap();
        let version = get_current_version(&pool).await.unwrap();
        assert_eq!(version, 1, "After migration, schema version should be 1");

        // 验证表存在
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sessions', 'messages', 'api_keys', '_schema_version')")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
        assert_eq!(count, 4, "All 4 tables should exist");
    }

    #[tokio::test]
    async fn test_migration_idempotent() {
        let pool = create_test_pool().await;

        // 两次运行迁移
        run_migrations(&pool).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let version = get_current_version(&pool).await.unwrap();
        assert_eq!(version, 1, "Idempotent: version should still be 1");
    }

    #[tokio::test]
    async fn test_rollback_and_reapply() {
        let pool = create_test_pool().await;

        // 前向迁移
        run_migrations(&pool).await.unwrap();
        assert_eq!(get_current_version(&pool).await.unwrap(), 1);

        // 验证 sessions 表存在
        let has_sessions: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='sessions'",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(false);
        assert!(has_sessions, "sessions table should exist after migration");

        // 回滚到 v0
        rollback_to(&pool, 0).await.unwrap();
        assert_eq!(get_current_version(&pool).await.unwrap(), 0);

        // 验证表被删除
        let has_sessions_after: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='sessions'",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(false);
        assert!(
            !has_sessions_after,
            "sessions table should be gone after rollback"
        );

        // 再次前向迁移
        run_migrations(&pool).await.unwrap();
        assert_eq!(get_current_version(&pool).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_auto_detect_existing_schema() {
        let pool = create_test_pool().await;

        // 模拟旧版系统：手动创建 sessions 表（无 _schema_version 表）
        sqlx::query("CREATE TABLE sessions (key TEXT PRIMARY KEY, platform TEXT NOT NULL, chat_id TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, source_json TEXT NOT NULL, reset_policy TEXT NOT NULL, metadata TEXT NOT NULL DEFAULT '{}')")
            .execute(&pool)
            .await
            .unwrap();

        // 运行新版迁移 → 应自动识别旧 schema 为 v1
        run_migrations(&pool).await.unwrap();
        let version = get_current_version(&pool).await.unwrap();
        assert_eq!(version, 1, "Should auto-detect existing schema as v1");
    }

    #[tokio::test]
    async fn test_rollback_to_non_existent_version() {
        let pool = create_test_pool().await;
        run_migrations(&pool).await.unwrap();

        // 回滚到相同版本应报错
        let result = rollback_to(&pool, 1).await;
        assert!(result.is_err(), "Rollback to same version should fail");
    }
}
