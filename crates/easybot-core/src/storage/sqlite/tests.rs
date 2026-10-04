use super::*;
use crate::types::message::{ChatType, MessageSender, MessageType};
use crate::types::session::{ResetPolicy, SessionSource};

fn make_test_session(key: &str, platform: &str, chat_id: &str) -> Session {
    Session {
        key: key.to_string(),
        platform: platform.to_string(),
        chat_id: chat_id.to_string(),
        thread_id: None,
        created_at: 1000,
        updated_at: 1000,
        source: SessionSource {
            platform: platform.to_string(),
            chat_id: chat_id.to_string(),
            chat_name: None,
            chat_type: ChatType::Dm,
            user_id: None,
            user_name: None,
            is_bot: false,
            user_username: None,
            user_role: None,
        },
        reset_policy: ResetPolicy::Never,
        metadata: serde_json::json!({}),
        last_message: None,
        last_message_at: None,
        custom_name: None,
    }
}

async fn create_test_pool() -> SqlitePool {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}

// ── SessionStore 测试 ──

#[tokio::test]
async fn test_session_upsert_and_get() {
    let pool = create_test_pool().await;
    let store = SqliteSessionStore::new(pool);

    let session = make_test_session("tg:1", "telegram", "1");
    store.upsert_session(&session).await.unwrap();

    let loaded = store.get_session("tg:1").await.unwrap().unwrap();
    assert_eq!(loaded.key, "tg:1");
    assert_eq!(loaded.platform, "telegram");
    assert_eq!(loaded.chat_id, "1");
    assert_eq!(loaded.custom_name, None, "custom_name defaults to None");
}

#[tokio::test]
async fn test_session_custom_name_roundtrip() {
    let pool = create_test_pool().await;
    let store = SqliteSessionStore::new(pool);

    // 设置自定义名 → 读取
    let mut session = make_test_session("tg:1", "telegram", "1");
    session.custom_name = Some("公司客服群".to_string());
    store.upsert_session(&session).await.unwrap();
    let loaded = store.get_session("tg:1").await.unwrap().unwrap();
    assert_eq!(loaded.custom_name.as_deref(), Some("公司客服群"));

    // 清空自定义名 → 读取为 None
    session.custom_name = None;
    store.upsert_session(&session).await.unwrap();
    let loaded = store.get_session("tg:1").await.unwrap().unwrap();
    assert_eq!(
        loaded.custom_name, None,
        "cleared custom_name should round-trip to None"
    );
}

#[tokio::test]
async fn test_session_get_nonexistent() {
    let pool = create_test_pool().await;
    let store = SqliteSessionStore::new(pool);

    let result = store.get_session("nonexistent").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_session_delete() {
    let pool = create_test_pool().await;
    let store = SqliteSessionStore::new(pool);

    store
        .upsert_session(&make_test_session("tg:1", "telegram", "1"))
        .await
        .unwrap();
    assert!(store.delete_session("tg:1").await.unwrap());
    assert!(!store.delete_session("nonexistent").await.unwrap());
}

#[tokio::test]
async fn test_session_load_all() {
    let pool = create_test_pool().await;
    let store = SqliteSessionStore::new(pool);

    store
        .upsert_session(&make_test_session("a:1", "telegram", "1"))
        .await
        .unwrap();
    store
        .upsert_session(&make_test_session("b:2", "discord", "2"))
        .await
        .unwrap();

    let all = store.load_all_sessions().await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_session_list_filter() {
    let pool = create_test_pool().await;
    let store = SqliteSessionStore::new(pool);

    store
        .upsert_session(&make_test_session("tg:1", "telegram", "1"))
        .await
        .unwrap();
    store
        .upsert_session(&make_test_session("dc:2", "discord", "2"))
        .await
        .unwrap();

    let filter = SessionFilter {
        platform: Some("telegram".to_string()),
        active_within_minutes: None,
        limit: None,
        offset: None,
    };
    let list = store.list_sessions(&filter).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].platform, "telegram");
}

#[tokio::test]
async fn test_session_upsert_preserves_created_at() {
    let pool = create_test_pool().await;
    let store = SqliteSessionStore::new(pool);

    let mut session = make_test_session("tg:1", "telegram", "1");
    session.created_at = 100;
    session.updated_at = 100;
    store.upsert_session(&session).await.unwrap();

    // 第二次 upsert 只更新 updated_at
    let mut updated = session.clone();
    updated.updated_at = 200;
    store.upsert_session(&updated).await.unwrap();

    let loaded = store.get_session("tg:1").await.unwrap().unwrap();
    assert_eq!(loaded.created_at, 100, "created_at should not change");
    assert_eq!(loaded.updated_at, 200, "updated_at should be updated");
}

// ── MessageStore 测试 ──

fn make_test_inbound() -> InboundMessage {
    InboundMessage {
        id: "msg1".to_string(),
        platform: "telegram".to_string().into(),
        msg_type: MessageType::Text,
        text: Some("Hello".to_string()),
        sender: MessageSender {
            id: "user1".to_string(),
            name: Some("User".to_string()),
            username: None,
            avatar_url: None,
            is_bot: false,
            role: None,
            language_code: None,
        },
        recipient: None,
        chat_id: "123".to_string(),
        chat_name: None,
        chat_type: ChatType::Dm,
        guild_id: None,
        thread_id: None,
        root_id: None,
        timestamp: 1000000,
        media: None,
        command: None,
        callback: None,
        reply_to: None,
        mentions: None,
        mentioned: None,
        metadata: None,
    }
}

#[tokio::test]
async fn test_message_store_and_list() {
    let pool = create_test_pool().await;
    let store = SqliteMessageStore::new(pool);

    let inbound = make_test_inbound();
    let stored = StoredMessage::from_inbound(&inbound);
    store.store_message(&stored).await.unwrap();

    let filter = MessageFilter {
        session_key: Some("telegram:123".to_string()),
        platform: None,
        chat_id: None,
        limit: Some(10),
        offset: None,
        before: None,
    };
    let msgs = store.list_messages(&filter).await.unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].text.as_deref(), Some("Hello"));
    assert_eq!(msgs[0].role, MessageRole::User);
}

#[tokio::test]
async fn outbound_delivery_finalization_is_atomic_with_message_history() {
    let pool = create_test_pool().await;
    let store = SqliteMessageStore::new(pool.clone());
    let delivery = OutboundDelivery {
        id: "delivery-1".into(),
        actor_id: "customer-key".into(),
        idempotency_key: Some("request-123".into()),
        platform: "telegram".into(),
        chat_id: "123".into(),
        request_json: serde_json::json!({"text": "hello"}),
        created_at: 1_000_000,
    };
    store.prepare_outbound_delivery(&delivery).await.unwrap();

    let send_result = SendResult {
        success: true,
        message_id: Some("platform-message-1".into()),
        timestamp: Some(1_000_001),
        error: None,
        error_code: None,
        retryable: false,
    };
    let message = StoredMessage::from_outbound("telegram", "123", None, "hello", &send_result);
    store
        .finalize_outbound_delivery(
            &delivery.id,
            OutboundDeliveryState::Succeeded,
            &serde_json::to_value(&send_result).unwrap(),
            Some(&message),
        )
        .await
        .unwrap();

    let state: String = sqlx::query_scalar("SELECT state FROM outbound_deliveries WHERE id = ?")
        .bind(&delivery.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let message_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id = ?")
        .bind(&message.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "succeeded");
    assert_eq!(message_count, 1);

    let unpublished = store.unpublished_outbound_events(10).await.unwrap();
    assert_eq!(unpublished.len(), 1);
    assert_eq!(unpublished[0].delivery_id, delivery.id);
    assert_eq!(unpublished[0].state, OutboundDeliveryState::Succeeded);
    store
        .mark_outbound_event_published(&delivery.id)
        .await
        .unwrap();
    assert!(
        store
            .unpublished_outbound_events(10)
            .await
            .unwrap()
            .is_empty()
    );

    let duplicate = store
        .finalize_outbound_delivery(
            &delivery.id,
            OutboundDeliveryState::Succeeded,
            &serde_json::json!({}),
            Some(&message),
        )
        .await;
    assert!(duplicate.is_err(), "a delivery can only be finalized once");

    let pending = OutboundDelivery {
        id: "delivery-pending".into(),
        actor_id: "customer-key".into(),
        idempotency_key: None,
        platform: "telegram".into(),
        chat_id: "456".into(),
        request_json: serde_json::json!({"text": "uncertain"}),
        created_at: 2_000_000,
    };
    store.prepare_outbound_delivery(&pending).await.unwrap();
    let stats = store.outbound_delivery_stats(3_000_000).await.unwrap();
    assert_eq!(stats.pending, 1);
    assert_eq!(stats.stale_pending, 1);
    assert!(
        store
            .list_outbound_deliveries("another-customer", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !store
            .reconcile_outbound_delivery(
                &pending.id,
                "another-customer",
                OutboundDeliveryState::Succeeded,
                "platform search found message 123",
                "another-customer",
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .reconcile_outbound_delivery(
                &pending.id,
                "customer-key",
                OutboundDeliveryState::Succeeded,
                "platform search found message 123",
                "customer-key",
            )
            .await
            .unwrap()
    );
    assert!(
        !store
            .reconcile_outbound_delivery(
                &pending.id,
                "customer-key",
                OutboundDeliveryState::Failed,
                "second finalization must be rejected",
                "customer-key",
            )
            .await
            .unwrap()
    );
    let records = store
        .list_outbound_deliveries("customer-key", 10)
        .await
        .unwrap();
    let reconciled = records.iter().find(|row| row.id == pending.id).unwrap();
    assert_eq!(reconciled.state, "succeeded");
    assert_eq!(
        reconciled.reconciliation_evidence.as_deref(),
        Some("platform search found message 123")
    );
    let stats = store.outbound_delivery_stats(3_000_000).await.unwrap();
    assert_eq!(stats.pending, 0);
    assert_eq!(stats.stale_pending, 0);
    assert_eq!(stats.unpublished_events, 1);
}

#[tokio::test]
async fn test_message_store_multiple() {
    let pool = create_test_pool().await;
    let store = SqliteMessageStore::new(pool);

    for i in 0..5 {
        let mut inbound = make_test_inbound();
        inbound.id = format!("msg{}", i);
        inbound.text = Some(format!("Message {}", i));
        inbound.timestamp = 1000000 + i;
        let stored = StoredMessage::from_inbound(&inbound);
        store.store_message(&stored).await.unwrap();
    }

    let filter = MessageFilter {
        session_key: Some("telegram:123".to_string()),
        platform: None,
        chat_id: None,
        limit: Some(3),
        offset: None,
        before: None,
    };
    let msgs = store.list_messages(&filter).await.unwrap();
    assert_eq!(msgs.len(), 3);
    // Should be newest first (timestamp desc)
    assert_eq!(msgs[0].text.as_deref(), Some("Message 4"));
}

#[tokio::test]
async fn test_message_delete() {
    let pool = create_test_pool().await;
    let store = SqliteMessageStore::new(pool);

    let stored = StoredMessage::from_inbound(&make_test_inbound());
    store.store_message(&stored).await.unwrap();

    assert!(store.delete_message(&stored.id).await.unwrap());
    assert!(!store.delete_message("nonexistent").await.unwrap());
}

#[tokio::test]
async fn test_inbound_to_stored_message() {
    let inbound = make_test_inbound();
    let stored = StoredMessage::from_inbound(&inbound);

    assert_eq!(stored.role, MessageRole::User);
    assert_eq!(stored.session_key, "telegram:123");
    assert_eq!(stored.platform, "telegram");
    assert_eq!(stored.chat_id, "123");
    assert!(stored.id.starts_with("inbound:"));
}

#[tokio::test]
async fn test_outbound_to_stored_message() {
    let result = SendResult::ok("out_msg_1".to_string());
    let stored = StoredMessage::from_outbound("telegram", "123", None, "Reply", &result);

    assert_eq!(stored.role, MessageRole::Assistant);
    assert_eq!(stored.session_key, "telegram:123");
    assert!(stored.id.starts_with("outbound:"));
    assert_eq!(stored.text.as_deref(), Some("Reply"));
}

#[test]
fn outbound_local_ids_are_unique_when_platform_reuses_message_id() {
    let result = SendResult {
        success: true,
        message_id: Some("platform-shared-id".into()),
        timestamp: Some(1),
        error: None,
        error_code: None,
        retryable: false,
    };
    let first = StoredMessage::from_outbound("telegram", "chat-1", None, "one", &result);
    let second = StoredMessage::from_outbound("telegram", "chat-2", None, "two", &result);
    assert_ne!(first.id, second.id);
    assert_eq!(first.raw_data["result"]["message_id"], "platform-shared-id");
    assert_eq!(
        second.raw_data["result"]["message_id"],
        "platform-shared-id"
    );
}

#[tokio::test]
async fn test_field_specific_query() {
    let pool = create_test_pool().await;
    let store = SqliteMessageStore::new(pool);

    // Store messages for two different chats
    let mut msg1 = make_test_inbound();
    msg1.chat_id = "111".to_string();
    msg1.text = Some("Chat 111 msg".to_string());
    store
        .store_message(&StoredMessage::from_inbound(&msg1))
        .await
        .unwrap();

    let mut msg2 = make_test_inbound();
    msg2.chat_id = "222".to_string();
    msg2.text = Some("Chat 222 msg".to_string());
    store
        .store_message(&StoredMessage::from_inbound(&msg2))
        .await
        .unwrap();

    // Filter by chat_id
    let filter = MessageFilter {
        session_key: None,
        platform: Some("telegram".to_string()),
        chat_id: Some("111".to_string()),
        limit: Some(10),
        offset: None,
        before: None,
    };
    let msgs = store.list_messages(&filter).await.unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].text.as_deref(), Some("Chat 111 msg"));
}

#[tokio::test]
async fn test_migration_adds_api_key_quota_to_existing_database() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE api_keys (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, prefix TEXT NOT NULL,
                created_at INTEGER NOT NULL, expires_at INTEGER, last_used_at INTEGER,
                revoked INTEGER NOT NULL DEFAULT 0, permissions TEXT NOT NULL DEFAULT '[]',
                hash TEXT NOT NULL
            )",
    )
    .execute(&pool)
    .await
    .unwrap();
    // v1 库同样具备 sessions 表（v3 迁移会为它添加 custom_name 列）
    sqlx::query(
        "CREATE TABLE sessions (
                key TEXT PRIMARY KEY, platform TEXT NOT NULL, chat_id TEXT NOT NULL,
                thread_id TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                source_json TEXT NOT NULL, reset_policy TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}', last_message TEXT,
                last_message_at INTEGER
            )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE api_usage_hourly (
                key_id TEXT NOT NULL, bucket_start INTEGER NOT NULL,
                status_class INTEGER NOT NULL, request_count INTEGER NOT NULL,
                PRIMARY KEY (key_id, bucket_start, status_class)
            )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_usage_hourly (key_id, bucket_start, status_class, request_count)
             VALUES ('legacy-key', 0, 2, 7)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_keys
             (id, name, prefix, created_at, revoked, permissions, hash)
             VALUES ('legacy-key', 'legacy', 'eb_old', 1, 0, '[]', 'hash')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Versioned v1 databases remain supported for automatic upgrades.
    sqlx::query(
        "CREATE TABLE _schema_version (
                version INTEGER NOT NULL,
                applied_at BIGINT NOT NULL,
                description TEXT NOT NULL
            )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO _schema_version (version, applied_at, description)
             VALUES (1, 0, 'Initial schema')",
    )
    .execute(&pool)
    .await
    .unwrap();

    run_migrations(&pool).await.unwrap();
    run_migrations(&pool).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
            .fetch_one(&pool)
            .await
            .unwrap(),
        crate::storage::migration::SCHEMA_VERSION
    );
    use sqlx::Row as _;
    let columns = sqlx::query("PRAGMA table_info(api_keys)")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(
        columns
            .iter()
            .any(|row| { row.get::<String, _>("name") == "requests_per_minute" })
    );
    let subject_id: String =
        sqlx::query_scalar("SELECT subject_id FROM api_keys WHERE id = 'legacy-key'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(subject_id, "legacy-key");
    let usage_subject: String =
        sqlx::query_scalar("SELECT subject_id FROM api_usage_hourly WHERE key_id = 'legacy-key'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(usage_subject, "legacy-key");
    let delivery_columns = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info('outbound_deliveries')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for expected in [
        "event_published",
        "reconciliation_evidence",
        "reconciled_by",
    ] {
        assert!(delivery_columns.iter().any(|column| column == expected));
    }
}

#[tokio::test]
async fn failed_multi_step_migration_rolls_back_prior_schema_changes() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE api_keys (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, prefix TEXT NOT NULL,
                created_at INTEGER NOT NULL, expires_at INTEGER, last_used_at INTEGER,
                revoked INTEGER NOT NULL DEFAULT 0, permissions TEXT NOT NULL DEFAULT '[]',
                hash TEXT NOT NULL
            )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("CREATE VIEW api_usage_hourly AS SELECT 'blocked' AS key_id")
        .execute(&pool)
        .await
        .unwrap();

    assert!(run_migrations(&pool).await.is_err());
    let columns = sqlx::query_scalar::<_, String>("SELECT name FROM pragma_table_info('api_keys')")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(!columns.iter().any(|column| column == "subject_id"));
    assert!(!columns.iter().any(|column| column == "requests_per_minute"));
}

#[tokio::test]
async fn migration_refuses_database_from_newer_schema_version() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    sqlx::query("CREATE TABLE future_data(value TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO future_data VALUES ('preserve-me')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE _schema_version (
                version INTEGER NOT NULL,
                applied_at INTEGER NOT NULL,
                description TEXT NOT NULL
            )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO _schema_version VALUES (999, 1, 'future schema')")
        .execute(&pool)
        .await
        .unwrap();

    let error = run_migrations(&pool).await.unwrap_err().to_string();
    assert!(error.contains("newer than supported"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(version), 0) FROM _schema_version")
            .fetch_one(&pool)
            .await
            .unwrap(),
        999
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM future_data")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "preserve-me"
    );
}

#[tokio::test]
async fn file_pool_applies_durable_security_pragmas_to_every_connection() {
    let path = std::env::temp_dir().join(format!("easybot-pool-{}.db", uuid::Uuid::new_v4()));
    let pool = create_pool(&path).await.unwrap();
    let mut connections = Vec::new();
    for _ in 0..5 {
        connections.push(pool.acquire().await.unwrap());
    }
    for connection in &mut connections {
        let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut **connection)
            .await
            .unwrap();
        assert_eq!(foreign_keys, 1);
        assert_eq!(busy_timeout, 5_000);
        assert_eq!(synchronous, 2);
    }
    sqlx::query("CREATE TABLE permission_probe(value TEXT)")
        .execute(&mut **connections.first_mut().unwrap())
        .await
        .unwrap();
    sqlx::query("INSERT INTO permission_probe VALUES ('secret')")
        .execute(&mut **connections.first_mut().unwrap())
        .await
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for protected_path in [
            path.clone(),
            std::path::PathBuf::from(format!("{}-wal", path.display())),
            std::path::PathBuf::from(format!("{}-shm", path.display())),
        ] {
            assert_eq!(
                std::fs::metadata(&protected_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{}",
                protected_path.display()
            );
        }
    }
    drop(connections);
    pool.close().await;
    for suffix in ["", "-wal", "-shm"] {
        let _ = tokio::fs::remove_file(format!("{}{suffix}", path.display())).await;
    }
}
