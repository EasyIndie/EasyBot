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
use sqlx::{PgPool, Sqlite, SqlitePool, Transaction};

/// 当前二进制所期望的数据库 schema 版本。
///
/// 每次新增/修改表结构时 +1，并追加 `MIGRATIONS` 条目。
pub const SCHEMA_VERSION: i64 = 2;

/// 版本追踪表（两种后端通用）
const VERSION_TABLE_SQL: &str = "
CREATE TABLE IF NOT EXISTS _schema_version (
    version     INTEGER NOT NULL,
    applied_at  BIGINT NOT NULL,
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
    Migration {
        version: 2,
        description: "Commercial readiness: delivery, audit, usage, billing and idempotency ledgers",
        sql_sqlite: V2_SQLITE,
        sql_postgres: V2_POSTGRES,
        rollback_sqlite: Some(V2_ROLLBACK_SQLITE),
        rollback_postgres: Some(V2_ROLLBACK_POSTGRES),
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

CREATE TABLE IF NOT EXISTS api_keys (
    id            VARCHAR(255) PRIMARY KEY,
    name          TEXT NOT NULL,
    prefix        VARCHAR(64) NOT NULL,
    created_at    BIGINT NOT NULL,
    expires_at    BIGINT,
    last_used_at  BIGINT,
    revoked       BOOLEAN NOT NULL DEFAULT FALSE,
    permissions   JSONB NOT NULL DEFAULT '[]',
    event_filters JSONB NOT NULL DEFAULT '[]',
    hash          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_keys_created ON api_keys(created_at DESC);
";

const V1_ROLLBACK_POSTGRES: &str = "
DROP TABLE IF EXISTS api_keys;
DROP TABLE IF EXISTS messages;
DROP TABLE IF EXISTS sessions;
";

const V1_ROLLBACK_SQLITE: &str = "
DROP TABLE IF EXISTS api_keys;
DROP TABLE IF EXISTS messages;
DROP TABLE IF EXISTS sessions;
";

const V2_SQLITE: &str = "
UPDATE api_keys SET subject_id = id WHERE subject_id IS NULL OR subject_id = '';

CREATE TABLE IF NOT EXISTS api_key_rotation_transitions (
    source_id      TEXT PRIMARY KEY REFERENCES api_keys(id),
    replacement_id TEXT NOT NULL UNIQUE REFERENCES api_keys(id),
    state          TEXT NOT NULL CHECK (state IN ('created','prepared')),
    created_at     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS outbound_deliveries (
    id              TEXT PRIMARY KEY,
    actor_id        TEXT NOT NULL,
    idempotency_key TEXT,
    platform        TEXT NOT NULL,
    chat_id         TEXT NOT NULL,
    request_json    TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('pending','succeeded','failed')),
    result_json     TEXT,
    created_at      INTEGER NOT NULL,
    completed_at    INTEGER,
    event_published INTEGER NOT NULL DEFAULT 0,
    reconciliation_evidence TEXT,
    reconciled_by   TEXT
);

CREATE INDEX IF NOT EXISTS idx_outbound_deliveries_state
    ON outbound_deliveries(state, created_at);

CREATE TABLE IF NOT EXISTS api_quota_events (
    id          TEXT PRIMARY KEY,
    subject_id  TEXT NOT NULL,
    occurred_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_quota_events_subject_time
    ON api_quota_events(subject_id, occurred_at);
CREATE INDEX IF NOT EXISTS idx_api_quota_events_time
    ON api_quota_events(occurred_at);

CREATE TABLE IF NOT EXISTS audit_events (
    id            TEXT PRIMARY KEY,
    timestamp     INTEGER NOT NULL,
    actor_id      TEXT NOT NULL,
    action        TEXT NOT NULL,
    resource      TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    previous_hash TEXT NOT NULL,
    event_hash    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_events_timestamp ON audit_events(timestamp DESC);

CREATE TABLE IF NOT EXISTS audit_chain_state (
    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
    head_hash   TEXT NOT NULL,
    event_count INTEGER NOT NULL CHECK (event_count >= 0)
);

INSERT OR IGNORE INTO audit_chain_state(singleton, head_hash, event_count)
SELECT 1,
       COALESCE((SELECT event_hash FROM audit_events ORDER BY timestamp DESC, rowid DESC LIMIT 1), 'GENESIS'),
       COUNT(*)
FROM audit_events;

CREATE TABLE IF NOT EXISTS api_usage_hourly (
    key_id        TEXT NOT NULL,
    subject_id    TEXT NOT NULL,
    bucket_start  INTEGER NOT NULL,
    status_class  INTEGER NOT NULL,
    request_count INTEGER NOT NULL CHECK (request_count >= 0),
    PRIMARY KEY (key_id, bucket_start, status_class)
);

CREATE INDEX IF NOT EXISTS idx_api_usage_hourly_bucket
    ON api_usage_hourly(bucket_start, key_id);

CREATE TABLE IF NOT EXISTS api_usage_integrity (
    key_id        TEXT NOT NULL,
    subject_id    TEXT NOT NULL,
    bucket_start  INTEGER NOT NULL,
    status_class  INTEGER NOT NULL,
    request_count INTEGER NOT NULL CHECK (request_count >= 0),
    PRIMARY KEY (key_id, bucket_start, status_class)
);

INSERT OR IGNORE INTO api_usage_integrity
    (key_id, subject_id, bucket_start, status_class, request_count)
SELECT key_id, subject_id, bucket_start, status_class, request_count
FROM api_usage_hourly;

CREATE TABLE IF NOT EXISTS api_usage_ledger_state (
    singleton       INTEGER PRIMARY KEY CHECK (singleton = 1),
    total_requests  INTEGER NOT NULL CHECK (total_requests >= 0)
);

INSERT OR IGNORE INTO api_usage_ledger_state(singleton, total_requests)
SELECT 1, COALESCE(SUM(request_count), 0) FROM api_usage_hourly;

CREATE TABLE IF NOT EXISTS billing_events (
    provider       TEXT NOT NULL,
    event_id       TEXT NOT NULL,
    event_type     TEXT NOT NULL,
    object_id      TEXT NOT NULL,
    customer_ref   TEXT NOT NULL,
    amount_minor   INTEGER NOT NULL CHECK (amount_minor >= 0),
    currency       TEXT NOT NULL,
    occurred_at    INTEGER NOT NULL,
    received_at    INTEGER NOT NULL,
    event_hash     TEXT NOT NULL,
    PRIMARY KEY (provider, event_id)
);

CREATE INDEX IF NOT EXISTS idx_billing_events_occurred
    ON billing_events(occurred_at DESC, provider, event_id);
CREATE INDEX IF NOT EXISTS idx_billing_events_customer
    ON billing_events(customer_ref, occurred_at DESC);

CREATE TABLE IF NOT EXISTS billing_ledger_state (
    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
    event_count INTEGER NOT NULL CHECK (event_count >= 0)
);

INSERT OR IGNORE INTO billing_ledger_state(singleton, event_count)
SELECT 1, COUNT(*) FROM billing_events;

CREATE TABLE IF NOT EXISTS api_idempotency (
    key_id          TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash    TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('pending','completed')),
    http_status     INTEGER,
    response_json   TEXT,
    created_at      INTEGER NOT NULL,
    expires_at      INTEGER NOT NULL,
    PRIMARY KEY (key_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_api_idempotency_expiry ON api_idempotency(expires_at);
";

const V2_POSTGRES: &str = "
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS subject_id VARCHAR(255);
ALTER TABLE api_keys ADD COLUMN IF NOT EXISTS requests_per_minute INTEGER;
UPDATE api_keys SET subject_id = id WHERE subject_id IS NULL OR subject_id = '';

CREATE TABLE IF NOT EXISTS api_key_rotation_transitions (
    source_id      VARCHAR(255) PRIMARY KEY REFERENCES api_keys(id),
    replacement_id VARCHAR(255) NOT NULL UNIQUE REFERENCES api_keys(id),
    state          VARCHAR(16) NOT NULL CHECK (state IN ('created','prepared')),
    created_at     BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS outbound_deliveries (
    id              VARCHAR(255) PRIMARY KEY,
    actor_id        VARCHAR(255) NOT NULL,
    idempotency_key VARCHAR(128),
    platform        VARCHAR(64) NOT NULL,
    chat_id         VARCHAR(255) NOT NULL,
    request_json    JSONB NOT NULL,
    state           VARCHAR(16) NOT NULL CHECK (state IN ('pending','succeeded','failed')),
    result_json     JSONB,
    created_at      BIGINT NOT NULL,
    completed_at    BIGINT,
    event_published BOOLEAN NOT NULL DEFAULT FALSE,
    reconciliation_evidence TEXT,
    reconciled_by   VARCHAR(255)
);

CREATE INDEX IF NOT EXISTS idx_outbound_deliveries_state
    ON outbound_deliveries(state, created_at);

CREATE TABLE IF NOT EXISTS api_quota_events (
    id          VARCHAR(255) PRIMARY KEY,
    subject_id  VARCHAR(255) NOT NULL,
    occurred_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_quota_events_subject_time
    ON api_quota_events(subject_id, occurred_at);
CREATE INDEX IF NOT EXISTS idx_api_quota_events_time
    ON api_quota_events(occurred_at);

CREATE TABLE IF NOT EXISTS audit_events (
    id            VARCHAR(255) PRIMARY KEY,
    timestamp     BIGINT NOT NULL,
    actor_id      VARCHAR(255) NOT NULL,
    action        TEXT NOT NULL,
    resource      TEXT NOT NULL,
    metadata_json JSONB NOT NULL DEFAULT '{}',
    previous_hash TEXT NOT NULL,
    event_hash    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_events_timestamp ON audit_events(timestamp DESC);

CREATE TABLE IF NOT EXISTS audit_chain_state (
    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
    head_hash   TEXT NOT NULL,
    event_count BIGINT NOT NULL CHECK (event_count >= 0)
);

INSERT INTO audit_chain_state(singleton, head_hash, event_count)
SELECT 1,
       COALESCE((SELECT event_hash FROM audit_events ORDER BY timestamp DESC LIMIT 1), 'GENESIS'),
       COUNT(*)
FROM audit_events
ON CONFLICT (singleton) DO NOTHING;

CREATE TABLE IF NOT EXISTS api_usage_hourly (
    key_id        VARCHAR(255) NOT NULL,
    subject_id    VARCHAR(255) NOT NULL,
    bucket_start  BIGINT NOT NULL,
    status_class  INTEGER NOT NULL,
    request_count BIGINT NOT NULL CHECK (request_count >= 0),
    PRIMARY KEY (key_id, bucket_start, status_class)
);

CREATE INDEX IF NOT EXISTS idx_api_usage_hourly_bucket
    ON api_usage_hourly(bucket_start, key_id);

CREATE TABLE IF NOT EXISTS api_usage_integrity (
    key_id        VARCHAR(255) NOT NULL,
    subject_id    VARCHAR(255) NOT NULL,
    bucket_start  BIGINT NOT NULL,
    status_class  INTEGER NOT NULL,
    request_count BIGINT NOT NULL CHECK (request_count >= 0),
    PRIMARY KEY (key_id, bucket_start, status_class)
);

INSERT INTO api_usage_integrity
    (key_id, subject_id, bucket_start, status_class, request_count)
SELECT key_id, subject_id, bucket_start, status_class, request_count
FROM api_usage_hourly
ON CONFLICT DO NOTHING;

CREATE TABLE IF NOT EXISTS api_usage_ledger_state (
    singleton       INTEGER PRIMARY KEY CHECK (singleton = 1),
    total_requests  BIGINT NOT NULL CHECK (total_requests >= 0)
);

INSERT INTO api_usage_ledger_state(singleton, total_requests)
SELECT 1, COALESCE(SUM(request_count), 0) FROM api_usage_hourly
ON CONFLICT (singleton) DO NOTHING;

CREATE TABLE IF NOT EXISTS billing_events (
    provider       VARCHAR(64) NOT NULL,
    event_id       VARCHAR(255) NOT NULL,
    event_type     TEXT NOT NULL,
    object_id      TEXT NOT NULL,
    customer_ref   TEXT NOT NULL,
    amount_minor   BIGINT NOT NULL CHECK (amount_minor >= 0),
    currency       VARCHAR(16) NOT NULL,
    occurred_at    BIGINT NOT NULL,
    received_at    BIGINT NOT NULL,
    event_hash     TEXT NOT NULL,
    PRIMARY KEY (provider, event_id)
);

CREATE INDEX IF NOT EXISTS idx_billing_events_occurred
    ON billing_events(occurred_at DESC, provider, event_id);
CREATE INDEX IF NOT EXISTS idx_billing_events_customer
    ON billing_events(customer_ref, occurred_at DESC);

CREATE TABLE IF NOT EXISTS billing_ledger_state (
    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
    event_count BIGINT NOT NULL CHECK (event_count >= 0)
);

INSERT INTO billing_ledger_state(singleton, event_count)
SELECT 1, COUNT(*) FROM billing_events
ON CONFLICT (singleton) DO NOTHING;

CREATE TABLE IF NOT EXISTS api_idempotency (
    key_id          VARCHAR(255) NOT NULL,
    idempotency_key VARCHAR(255) NOT NULL,
    request_hash    TEXT NOT NULL,
    state           VARCHAR(16) NOT NULL CHECK (state IN ('pending','completed')),
    http_status     INTEGER,
    response_json   JSONB,
    created_at      BIGINT NOT NULL,
    expires_at      BIGINT NOT NULL,
    PRIMARY KEY (key_id, idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_api_idempotency_expiry ON api_idempotency(expires_at);
";

const V2_ROLLBACK_SQLITE: &str = "
DROP TABLE IF EXISTS api_idempotency;
DROP TABLE IF EXISTS billing_ledger_state;
DROP TABLE IF EXISTS billing_events;
DROP TABLE IF EXISTS api_usage_ledger_state;
DROP TABLE IF EXISTS api_usage_integrity;
DROP TABLE IF EXISTS api_usage_hourly;
DROP TABLE IF EXISTS audit_chain_state;
DROP TABLE IF EXISTS audit_events;
DROP TABLE IF EXISTS api_quota_events;
DROP TABLE IF EXISTS outbound_deliveries;
DROP TABLE IF EXISTS api_key_rotation_transitions;
";

const V2_ROLLBACK_POSTGRES: &str = "
DROP TABLE IF EXISTS api_idempotency;
DROP TABLE IF EXISTS billing_ledger_state;
DROP TABLE IF EXISTS billing_events;
DROP TABLE IF EXISTS api_usage_ledger_state;
DROP TABLE IF EXISTS api_usage_integrity;
DROP TABLE IF EXISTS api_usage_hourly;
DROP TABLE IF EXISTS audit_chain_state;
DROP TABLE IF EXISTS audit_events;
DROP TABLE IF EXISTS api_quota_events;
DROP TABLE IF EXISTS outbound_deliveries;
DROP TABLE IF EXISTS api_key_rotation_transitions;
ALTER TABLE api_keys DROP COLUMN IF EXISTS requests_per_minute;
ALTER TABLE api_keys DROP COLUMN IF EXISTS subject_id;
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
async fn record_migration_sqlite_tx(
    tx: &mut Transaction<'_, Sqlite>,
    version: i64,
    description: &str,
) -> Result<(), StoreError> {
    sqlx::query("INSERT INTO _schema_version (version, applied_at, description) VALUES (?, ?, ?)")
        .bind(version)
        .bind(Utc::now().timestamp_millis())
        .bind(description)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 运行所有未执行的 SQLite 前向迁移
///
/// 幂等：已执行的迁移不会重复执行。每步在独立事务中执行。
pub async fn run_migrations(pool: &SqlitePool) -> Result<(), StoreError> {
    // 1. 确保版本追踪表存在
    sqlx::query(VERSION_TABLE_SQL).execute(pool).await?;

    // 2. 未版本化的旧数据库不在上线前兼容范围内。
    let current = get_current_version(pool).await?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::Database(format!(
            "SQLite schema version {current} is newer than supported version {SCHEMA_VERSION}"
        )));
    }
    if current == 0 && has_existing_tables(pool).await? {
        return Err(StoreError::Database(
            "Unversioned existing SQLite schema is not supported; reset the database before starting"
                .into(),
        ));
    }

    // 3. 逐版执行未应用的迁移
    for m in MIGRATIONS {
        if m.version > current {
            tracing::info!("Running SQLite migration v{}: {}", m.version, m.description);
            let mut tx = pool.begin().await?;
            let outcome = async {
                if m.version == 2 {
                    apply_sqlite_v2(&mut tx).await?;
                } else {
                    execute_sqlite_batch(&mut tx, m.sql_sqlite).await?;
                }
                record_migration_sqlite_tx(&mut tx, m.version, m.description).await
            }
            .await;
            match outcome {
                Ok(()) => tx.commit().await?,
                Err(error) => {
                    let _ = tx.rollback().await;
                    return Err(error);
                }
            }
            tracing::info!("SQLite migration v{} applied", m.version);
        }
    }

    Ok(())
}

async fn apply_sqlite_v2(tx: &mut Transaction<'_, Sqlite>) -> Result<(), StoreError> {
    add_sqlite_column_if_missing(
        tx,
        "SELECT name FROM pragma_table_info('api_keys')",
        "subject_id",
        "ALTER TABLE api_keys ADD COLUMN subject_id TEXT",
    )
    .await?;
    add_sqlite_column_if_missing(
        tx,
        "SELECT name FROM pragma_table_info('api_keys')",
        "requests_per_minute",
        "ALTER TABLE api_keys ADD COLUMN requests_per_minute INTEGER",
    )
    .await?;
    sqlx::query("UPDATE api_keys SET subject_id = id WHERE subject_id IS NULL OR subject_id = ''")
        .execute(&mut **tx)
        .await?;
    if sqlite_table_exists(tx, "api_usage_hourly").await? {
        add_sqlite_column_if_missing(
            tx,
            "SELECT name FROM pragma_table_info('api_usage_hourly')",
            "subject_id",
            "ALTER TABLE api_usage_hourly ADD COLUMN subject_id TEXT",
        )
        .await?;
        sqlx::query(
            "UPDATE api_usage_hourly SET subject_id = key_id WHERE subject_id IS NULL OR subject_id = ''",
        )
        .execute(&mut **tx)
        .await?;
    }
    execute_sqlite_batch(tx, V2_SQLITE).await?;
    Ok(())
}

async fn execute_sqlite_batch(
    tx: &mut Transaction<'_, Sqlite>,
    sql: &'static str,
) -> Result<(), StoreError> {
    for statement in sql.split(';').map(str::trim).filter(|sql| !sql.is_empty()) {
        // Migration SQL is compiled into this binary and never includes user input.
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn add_sqlite_column_if_missing(
    tx: &mut Transaction<'_, Sqlite>,
    table_info_sql: &'static str,
    column: &str,
    alter_sql: &'static str,
) -> Result<(), StoreError> {
    let columns = sqlx::query_scalar::<_, String>(table_info_sql)
        .fetch_all(&mut **tx)
        .await?;
    if !columns.iter().any(|existing| existing == column) {
        sqlx::query(alter_sql).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn sqlite_table_exists(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
) -> Result<bool, StoreError> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1")
            .bind(table)
            .fetch_one(&mut **tx)
            .await?;
    Ok(count == 1)
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
                let mut tx = pool.begin().await?;
                let outcome = async {
                    execute_sqlite_batch(&mut tx, rollback).await?;
                    sqlx::query("DELETE FROM _schema_version WHERE version = ?")
                        .bind(m.version)
                        .execute(&mut *tx)
                        .await?;
                    Ok::<(), StoreError>(())
                }
                .await;
                match outcome {
                    Ok(()) => tx.commit().await?,
                    Err(error) => {
                        let _ = tx.rollback().await;
                        return Err(error);
                    }
                }
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

/// 检查是否存在未版本化的数据表。
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
    // v0.0.26 mistakenly created this column as PostgreSQL INTEGER while
    // storing millisecond Unix timestamps. Widen it before recording any
    // migration so existing databases recover without manual SQL.
    sqlx::query("ALTER TABLE _schema_version ALTER COLUMN applied_at TYPE BIGINT")
        .execute(pool)
        .await?;

    // 2. 未版本化的旧数据库不在上线前兼容范围内。
    let current = get_current_version_pg(pool).await?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::Database(format!(
            "PostgreSQL schema version {current} is newer than supported version {SCHEMA_VERSION}"
        )));
    }
    if current == 0 && has_existing_tables_pg(pool).await? {
        return Err(StoreError::Database(
            "Unversioned existing PostgreSQL schema is not supported; reset the database before starting"
                .into(),
        ));
    }

    // 3. 逐版执行
    for m in MIGRATIONS {
        if m.version > current {
            tracing::info!(
                "Running PostgreSQL migration v{}: {}",
                m.version,
                m.description
            );
            sqlx::raw_sql(m.sql_postgres).execute(pool).await?;
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

/// 检查 PostgreSQL 是否存在未版本化的数据表。
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

        // 空库 → 运行迁移 → 版本应为当前 schema 版本
        run_migrations(&pool).await.unwrap();
        let version = get_current_version(&pool).await.unwrap();
        assert_eq!(
            version, SCHEMA_VERSION,
            "After migration, schema version should be current"
        );

        // 验证表存在
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sessions', 'messages', 'api_keys', 'outbound_deliveries', 'audit_events', 'api_usage_hourly', 'billing_events', 'api_idempotency', '_schema_version')")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
        assert_eq!(count, 9, "All commercial schema tables should exist");
    }

    #[test]
    fn version_table_timestamp_is_64_bit() {
        assert!(VERSION_TABLE_SQL.contains("applied_at  BIGINT NOT NULL"));
    }

    #[tokio::test]
    async fn test_migration_idempotent() {
        let pool = create_test_pool().await;

        // 两次运行迁移
        run_migrations(&pool).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let version = get_current_version(&pool).await.unwrap();
        assert_eq!(
            version, SCHEMA_VERSION,
            "Idempotent: version should stay current"
        );
    }

    #[tokio::test]
    async fn test_rollback_and_reapply() {
        let pool = create_test_pool().await;

        // 前向迁移
        run_migrations(&pool).await.unwrap();
        assert_eq!(get_current_version(&pool).await.unwrap(), SCHEMA_VERSION);

        // 验证 sessions 表存在
        let has_sessions: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='sessions'",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(false);
        assert!(has_sessions, "sessions table should exist after migration");

        rollback_to(&pool, 0).await.unwrap();
        assert_eq!(get_current_version(&pool).await.unwrap(), 0);

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

        run_migrations(&pool).await.unwrap();
        assert_eq!(get_current_version(&pool).await.unwrap(), SCHEMA_VERSION);
    }

    #[tokio::test]
    async fn test_reject_unversioned_existing_schema() {
        let pool = create_test_pool().await;

        // 未版本化的旧 schema 不再作为上线前兼容目标。
        sqlx::query(V1_SQLITE).execute(&pool).await.unwrap();

        let error = run_migrations(&pool).await.unwrap_err().to_string();
        assert!(error.contains("Unversioned existing SQLite schema"));
        assert_eq!(get_current_version(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_rollback_to_non_existent_version() {
        let pool = create_test_pool().await;
        run_migrations(&pool).await.unwrap();

        // 回滚到相同版本应报错
        let result = rollback_to(&pool, SCHEMA_VERSION).await;
        assert!(result.is_err(), "Rollback to same version should fail");
    }
}
