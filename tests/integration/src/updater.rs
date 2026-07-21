//! Updater 集成测试
//!
//! 使用临时目录和模拟文件测试更新流程的核心组件。

use std::path::PathBuf;
use std::sync::OnceLock;

/// 获取测试用的临时目录
fn test_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir =
            std::env::temp_dir().join(format!("easybot_integration_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use easybot_core::storage::migration;
    use easybot_core::updater::types;

    // ── 版本比较 ──

    #[test]
    fn test_version_comparison_semver() {
        assert!(types::is_newer_than("0.1.0", "0.0.16"));
        assert!(types::is_newer_than("1.0.0", "0.9.99"));
        assert!(!types::is_newer_than("0.0.16", "0.1.0"));
        assert!(!types::is_newer_than("0.0.16", "0.0.16"));
    }

    #[test]
    fn test_version_comparison_edge_cases() {
        // 预发布版本不视为更新
        assert!(!types::is_newer_than("0.0.16-alpha", "0.0.16"));
        // 无效版本保守处理
        assert!(!types::is_newer_than("invalid", "0.0.16"));
        assert!(!types::is_newer_than("0.0.16", "invalid"));
    }

    #[test]
    fn test_version_from_tag() {
        assert_eq!(types::version_from_tag("v0.1.0"), Some("0.1.0".into()));
        assert_eq!(types::version_from_tag("0.1.0"), None); // 缺 v 前缀
        assert_eq!(types::version_from_tag("v1"), None); // 非完整 semver
    }

    #[test]
    fn test_version_from_tag_edge_cases() {
        assert_eq!(types::version_from_tag(""), None);
        assert_eq!(types::version_from_tag("v"), None);
        assert_eq!(types::version_from_tag("vabc"), None);
        assert_eq!(
            types::version_from_tag("v0.0.16-rc1"),
            Some("0.0.16-rc1".into())
        );
    }

    // ── 平台检测 ──

    #[test]
    fn test_current_target_triple() {
        let triple = types::current_target_triple().expect("Should detect platform");
        // 验证格式：<arch>-<vendor>-<os>
        let parts: Vec<&str> = triple.split('-').collect();
        assert!(
            parts.len() >= 3,
            "Triple should have at least 3 parts: {}",
            triple
        );
        // 验证是已知目标
        let valid =
            triple.contains("linux") || triple.contains("apple") || triple.contains("windows");
        assert!(valid, "Triple should match a known OS: {}", triple);
    }

    #[test]
    fn test_current_asset_name() {
        let name = types::current_asset_name().expect("Should generate asset name");
        assert!(
            name.starts_with("easybot-"),
            "Asset should start with 'easybot-'"
        );
        // 验证包含平台标识
        assert!(
            name.len() > "easybot-".len(),
            "Asset should contain platform info"
        );
    }

    // ── 语义版本解析 ──

    #[test]
    fn test_semver_parse_valid() {
        assert!(semver::Version::parse("0.0.16").is_ok());
        assert!(semver::Version::parse("1.0.0").is_ok());
        assert!(semver::Version::parse("0.1.0-alpha").is_ok());
        assert!(semver::Version::parse("255.255.65535").is_ok());
    }

    #[test]
    fn test_semver_parse_invalid() {
        assert!(semver::Version::parse("0.0").is_err());
        assert!(semver::Version::parse("abc").is_err());
        assert!(semver::Version::parse("").is_err());
        assert!(semver::Version::parse("v0.0.1").is_err()); // v 前缀非法
    }

    // ── 迁移测试（使用临时 SQLite 数据库）──

    /// 创建临时 SQLite 数据库路径
    fn temp_db_path() -> PathBuf {
        test_dir().join(format!("test_migration_{}.db", rand_id()))
    }

    fn rand_id() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }

    #[tokio::test]
    async fn test_migration_on_temp_db() {
        let db_path = temp_db_path();

        // 创建连接池
        let pool = easybot_core::storage::sqlite::create_pool(&db_path)
            .await
            .expect("Should create SQLite pool");

        // 运行迁移
        migration::run_migrations(&pool)
            .await
            .expect("Migration should succeed");

        // 验证版本
        let version = migration::get_current_version(&pool)
            .await
            .expect("Should get version");
        assert_eq!(
            version,
            migration::SCHEMA_VERSION,
            "DB schema version should match binary SCHEMA_VERSION"
        );

        // 验证表存在
        let table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sessions', 'messages', 'api_keys', '_schema_version')"
        )
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
        assert_eq!(table_count, 4, "All 4 tables should exist");

        // 验证可以插入和查询数据
        sqlx::query(
            "INSERT INTO _schema_version (version, applied_at, description) VALUES (999, 1234567890, 'test entry')"
        )
            .execute(&pool)
            .await
            .expect("Should insert test entry");

        // 清理
        let _ = std::fs::remove_file(&db_path);
        drop(pool);
    }

    #[tokio::test]
    async fn test_migration_rollback_on_temp_db() {
        let db_path = temp_db_path();
        let pool = easybot_core::storage::sqlite::create_pool(&db_path)
            .await
            .expect("Should create pool");

        // 前向迁移
        migration::run_migrations(&pool).await.expect("Migration");
        assert_eq!(
            migration::get_current_version(&pool).await.unwrap(),
            migration::SCHEMA_VERSION
        );

        // 回滚到 0
        migration::rollback_to(&pool, 0).await.expect("Rollback");
        assert_eq!(migration::get_current_version(&pool).await.unwrap(), 0);

        // 验证表被删除
        let has_sessions: bool = sqlx::query_scalar(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='sessions'",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(false);
        assert!(
            !has_sessions,
            "sessions table should be gone after rollback"
        );

        // 再次前向迁移
        migration::run_migrations(&pool)
            .await
            .expect("Re-migration");
        assert_eq!(
            migration::get_current_version(&pool).await.unwrap(),
            migration::SCHEMA_VERSION
        );

        let _ = std::fs::remove_file(&db_path);
        drop(pool);
    }

    // ── 备份清单文件读写测试 ──

    #[tokio::test]
    async fn test_manifest_file_read_write() {
        let dir = test_dir().join(format!("manifest_test_{}", rand_id()));
        std::fs::create_dir_all(&dir).expect("Should create temp dir");

        // 直接测试备份清单文件的序列化/反序列化
        let manifest = serde_json::json!({
            "timestamp": 1234567890,
            "from_version": "0.0.16",
            "to_version": "0.1.0",
            "from_schema_version": 1,
            "to_schema_version": 2,
            "binary_backup": null,
            "db_backup": null,
            "config_backup": null,
            "migrations_applied": [2]
        });

        let manifest_path = dir.join(".update_manifest.json");
        std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .expect("Should write manifest");

        // 验证可读取
        let content = std::fs::read_to_string(&manifest_path).expect("Should read manifest");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("Should parse");
        assert_eq!(parsed["from_version"], "0.0.16");
        assert_eq!(parsed["to_version"], "0.1.0");
        assert_eq!(parsed["migrations_applied"][0], 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── 错误类型测试 ──

    #[test]
    fn test_update_error_display() {
        let err = types::UpdateError::UnsupportedPlatform;
        assert_eq!(format!("{}", err), "Unsupported platform for auto-update");

        let err = types::UpdateError::AlreadyUpToDate("0.0.16".into());
        assert!(format!("{}", err).contains("0.0.16"));

        let err = types::UpdateError::ChecksumMismatch {
            expected: "abc123".into(),
            actual: "def456".into(),
        };
        assert!(format!("{}", err).contains("abc123"));
        assert!(format!("{}", err).contains("def456"));

        let err = types::UpdateError::RateLimited;
        assert!(format!("{}", err).contains("GITHUB_TOKEN"));
    }
}
