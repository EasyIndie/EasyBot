use super::*;

#[tokio::test]
async fn test_create_and_authenticate() {
    let mgr = ApiKeyManager::new(None);
    let (id, key) = mgr
        .create_key("test", vec!["message:send".to_string()], None)
        .await
        .unwrap();

    assert!(!id.is_empty());
    assert!(key.starts_with("eb_"));

    let auth = mgr.authenticate(&key).await.unwrap();
    assert_eq!(auth.name, "test");
    assert_eq!(auth.permissions, vec!["message:send"]);
    let keys = mgr.list_keys().await;
    assert!(keys[0].last_used_at.is_some());
}

#[tokio::test]
async fn ephemeral_sessions_are_authenticatable_hidden_and_bounded() {
    let manager = ApiKeyManager::new(None);
    let expires_at = chrono::Utc::now().timestamp_millis() + 60 * 60 * 1_000;
    let mut sessions = Vec::new();
    for _ in 0..10 {
        sessions.push(
            manager
                .create_ephemeral_key("admin-session", vec!["*".into()], expires_at)
                .await
                .unwrap()
                .1,
        );
    }

    assert!(manager.list_keys().await.is_empty());
    assert!(manager.authenticate(sessions.last().unwrap()).await.is_ok());
    let valid_count =
        futures::future::join_all(sessions.iter().map(|session| manager.authenticate(session)))
            .await
            .into_iter()
            .filter(Result::is_ok)
            .count();
    assert_eq!(valid_count, 8);
}

#[test]
fn api_key_locator_rejects_malformed_or_unbounded_secrets() {
    assert_eq!(
        api_key_prefix("eb_0123456789abcdef0123456789abcdef"),
        Some("eb_01234")
    );
    for invalid in [
        "invalid_key",
        "eb_0123",
        "eb_0123456789abcdef0123456789abcdeg",
        "EB_0123456789abcdef0123456789abcdef",
    ] {
        assert_eq!(api_key_prefix(invalid), None);
    }
}

#[tokio::test]
async fn test_authentication_persists_last_used_at() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let mgr = ApiKeyManager::new(Some(pool.clone()));
    let (id, key) = mgr
        .create_key_with_quota("customer", vec!["messagesread".into()], None, Some(250))
        .await
        .unwrap();

    mgr.authenticate(&key).await.unwrap();

    use sqlx::Row as _;
    let row = sqlx::query("SELECT last_used_at, requests_per_minute FROM api_keys WHERE id = ?1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let last_used_at: Option<i64> = row.get("last_used_at");
    assert!(last_used_at.is_some());
    let quota: Option<i64> = row.get("requests_per_minute");
    assert_eq!(quota, Some(250));

    let reloaded = ApiKeyManager::new(Some(pool));
    reloaded.load_from_db().await;
    let auth = reloaded.authenticate(&key).await.unwrap();
    assert_eq!(auth.requests_per_minute, Some(250));
}

#[tokio::test]
async fn stale_argon_result_cannot_resurrect_revoked_loaded_key() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let issuer = ApiKeyManager::new(Some(pool.clone()));
    let (key_id, raw_key) = issuer.create_key("customer", vec![], None).await.unwrap();
    let reloaded = ApiKeyManager::new(Some(pool));
    reloaded.load_from_db().await;

    let stale_index = sha256_index(&raw_key);
    reloaded.loaded.write().await[0].info.revoked = true;
    let result = reloaded
        .promote_verified_loaded_key(stale_index.clone(), &key_id)
        .await;
    assert_eq!(result.unwrap_err(), "Invalid API key");
    assert!(!reloaded.keys.read().await.contains_key(&stale_index));
}

#[tokio::test]
async fn restart_keeps_inactive_key_history_out_of_authentication_memory() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let issuer = ApiKeyManager::new(Some(pool.clone()));
    let (revoked_id, _) = issuer.create_key("revoked", vec![], None).await.unwrap();
    issuer.revoke_key(&revoked_id).await.unwrap();
    issuer.create_key("expired", vec![], Some(1)).await.unwrap();

    let reloaded = ApiKeyManager::new(Some(pool));
    reloaded.load_from_db().await;
    assert!(reloaded.loaded.read().await.is_empty());
    let history = reloaded.list_keys_result().await.unwrap();
    assert_eq!(history.len(), 2);
    assert!(
        history
            .iter()
            .any(|key| key.id == revoked_id && key.revoked)
    );
    assert!(reloaded.delete_key(&revoked_id).await.unwrap());
    assert_eq!(reloaded.list_keys_result().await.unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_key_creation_cannot_oversell_active_limit() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    sqlx::query(
            "WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 99)
             INSERT INTO api_keys(id,subject_id,name,prefix,created_at,revoked,permissions,hash)
             SELECT 'seed-' || value, 'subject-' || value, 'seed', 'eb_seed', 1, 0, '[]', 'unused' FROM n",
        )
        .execute(&pool)
        .await
        .unwrap();
    let first = ApiKeyManager::new(Some(pool.clone()));
    let second = ApiKeyManager::new(Some(pool.clone()));
    let (a, b) = tokio::join!(
        first.create_key("concurrent-a", vec![], None),
        second.create_key("concurrent-b", vec![], None),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM api_keys WHERE revoked = 0 AND (expires_at IS NULL OR expires_at > ?1)",
            )
            .bind(chrono::Utc::now().timestamp_millis())
            .fetch_one(&pool)
            .await
            .unwrap(),
            100
        );
}

#[tokio::test]
async fn full_capacity_allows_only_one_safe_rotation_transition() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    sqlx::query(
            "WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value + 1 FROM n WHERE value < 99)
             INSERT INTO api_keys(id,subject_id,name,prefix,created_at,revoked,permissions,hash)
             SELECT 'seed-' || value, 'subject-' || value, 'seed', 'eb_seed', 1, 0, '[]', 'unused' FROM n",
        )
        .execute(&pool)
        .await
        .unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    let (source_id, _) = manager.create_key("source", vec![], None).await.unwrap();
    let source = manager.find_key_info(&source_id).await.unwrap().unwrap();
    let (replacement_id, _) = manager
        .create_rotated_key(&source, chrono::Utc::now().timestamp_millis() + 86_400_000)
        .await
        .unwrap();
    assert_eq!(manager.active_key_count().await.unwrap(), 101);
    assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM api_key_rotation_transitions WHERE source_id=?1 AND replacement_id=?2",
            )
            .bind(&source_id)
            .bind(&replacement_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            "created"
        );

    assert!(
        manager
            .create_key("normal-overflow", vec![], None)
            .await
            .is_err()
    );
    assert!(
        manager
            .create_rotated_key(&source, chrono::Utc::now().timestamp_millis() + 86_400_000)
            .await
            .is_err()
    );

    manager
        .mark_rotation_prepared(&source_id, &replacement_id)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM api_key_rotation_transitions WHERE source_id=?1",
        )
        .bind(&source_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "prepared"
    );
    assert!(manager.revoke_key(&source_id).await.unwrap());
    manager
        .clear_rotation_transition(&source_id, &replacement_id)
        .await
        .unwrap();
    assert_eq!(manager.active_key_count().await.unwrap(), 100);
    assert!(
        manager
            .find_key_info(&replacement_id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_key_rotation_transitions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn rotation_reconciliation_enforces_state_specific_atomic_action() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool));
    let (source_id, _) = manager.create_key("source", vec![], None).await.unwrap();
    let source = manager.find_key_info(&source_id).await.unwrap().unwrap();

    let (cancelled_id, _) = manager
        .create_rotated_key(&source, chrono::Utc::now().timestamp_millis() + 86_400_000)
        .await
        .unwrap();
    assert!(
        manager
            .reconcile_rotation_transition(&source_id, &cancelled_id, "complete")
            .await
            .is_err()
    );
    manager
        .reconcile_rotation_transition(&source_id, &cancelled_id, "cancel")
        .await
        .unwrap();
    assert!(
        !manager
            .find_key_info(&source_id)
            .await
            .unwrap()
            .unwrap()
            .revoked
    );
    assert!(
        manager
            .find_key_info(&cancelled_id)
            .await
            .unwrap()
            .unwrap()
            .revoked
    );

    let (completed_id, _) = manager
        .create_rotated_key(&source, chrono::Utc::now().timestamp_millis() + 86_400_000)
        .await
        .unwrap();
    manager
        .mark_rotation_prepared(&source_id, &completed_id)
        .await
        .unwrap();
    manager
        .reconcile_rotation_transition(&source_id, &completed_id, "complete")
        .await
        .unwrap();
    assert!(
        manager
            .find_key_info(&source_id)
            .await
            .unwrap()
            .unwrap()
            .revoked
    );
    assert!(
        !manager
            .find_key_info(&completed_id)
            .await
            .unwrap()
            .unwrap()
            .revoked
    );
    assert!(manager.rotation_transitions().await.unwrap().is_empty());
}

#[tokio::test]
async fn durable_subject_quota_survives_manager_restart() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let first = ApiKeyManager::new(Some(pool.clone()));
    assert!(
        first
            .consume_subject_quota("customer-subject", 2)
            .await
            .unwrap()
            .allowed
    );
    assert!(
        first
            .consume_subject_quota("customer-subject", 2)
            .await
            .unwrap()
            .allowed
    );

    let restarted = ApiKeyManager::new(Some(pool));
    let decision = restarted
        .consume_subject_quota("customer-subject", 2)
        .await
        .unwrap();
    assert!(!decision.allowed);
    assert_eq!(decision.remaining, 0);
    assert!(decision.retry_after_secs > 0 && decision.retry_after_secs <= 60);
}

#[tokio::test]
async fn durable_subject_quota_does_not_oversell_under_concurrency() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = std::sync::Arc::new(ApiKeyManager::new(Some(pool)));
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let manager = manager.clone();
        tasks.push(tokio::spawn(async move {
            manager
                .consume_subject_quota("concurrent-subject", 5)
                .await
                .unwrap()
                .allowed
        }));
    }
    let mut allowed = 0;
    for task in tasks {
        allowed += usize::from(task.await.unwrap());
    }
    assert_eq!(allowed, 5);
}

#[tokio::test]
async fn test_revoke_key() {
    let mgr = ApiKeyManager::new(None);
    let (id, key) = mgr.create_key("test", vec![], None).await.unwrap();

    assert!(mgr.revoke_key(&id).await.unwrap());
    assert!(mgr.authenticate(&key).await.is_err());
}

#[tokio::test]
async fn test_delete_revoked_key() {
    let mgr = ApiKeyManager::new(None);
    let (id, _key) = mgr.create_key("test", vec![], None).await.unwrap();

    // 未吊销不能删除
    assert!(!mgr.delete_key(&id).await.unwrap());

    // 吊销后可删除
    assert!(mgr.revoke_key(&id).await.unwrap());
    assert!(mgr.delete_key(&id).await.unwrap());

    // 列表里不再有
    assert!(mgr.list_keys().await.is_empty());
}

#[tokio::test]
async fn test_invalid_key() {
    let mgr = ApiKeyManager::new(None);
    assert!(mgr.authenticate("invalid_key").await.is_err());
}

#[tokio::test]
async fn test_expired_key() {
    let mgr = ApiKeyManager::new(None);
    let (_id, key) = mgr.create_key("expired", vec![], Some(1)).await.unwrap();
    // expires_at is 1ms after epoch — definitely expired
    assert!(mgr.authenticate(&key).await.is_err());
}

#[tokio::test]
async fn test_create_key_rejects_duplicate_policy_entries() {
    let mgr = ApiKeyManager::new(None);
    let duplicate_permissions = mgr
        .create_key(
            "duplicate-permissions",
            vec!["messagesread".to_string(), "messagesread".to_string()],
            None,
        )
        .await;
    assert!(duplicate_permissions.is_err());
}

#[tokio::test]
async fn test_audit_chain_persists_and_detects_tampering() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    manager
        .record_audit(
            "actor-1",
            "api_key.created",
            "api_key:1",
            serde_json::json!({"plan":"starter"}),
        )
        .await
        .unwrap();
    manager
        .record_audit(
            "actor-1",
            "api_key.revoked",
            "api_key:1",
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert!(manager.verify_audit_chain().await);

    let reloaded = ApiKeyManager::new(Some(pool.clone()));
    reloaded.load_from_db().await;
    assert!(reloaded.audit_events.read().await.is_empty());
    assert_eq!(reloaded.list_audit_events(10).await.len(), 2);
    assert!(reloaded.verify_audit_chain().await);
    reloaded
        .record_audit(
            "actor-2",
            "api_key.deleted",
            "api_key:1",
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(reloaded.list_audit_events(10).await.len(), 3);
    assert!(reloaded.verify_audit_chain().await);

    sqlx::query("UPDATE audit_events SET metadata_json = ?1 WHERE action = ?2")
        .bind(r#"{"plan":"enterprise"}"#)
        .bind("api_key.created")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!reloaded.verify_audit_chain().await);
}

#[tokio::test]
async fn audit_chain_anchor_detects_tail_deletion_and_full_truncation() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    for action in ["first", "second"] {
        manager
            .record_audit("actor", action, "resource", serde_json::json!({}))
            .await
            .unwrap();
    }
    assert!(manager.verify_audit_chain().await);

    sqlx::query("DELETE FROM audit_events WHERE action = 'second'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!manager.verify_audit_chain().await);

    sqlx::query("DELETE FROM audit_events")
        .execute(&pool)
        .await
        .unwrap();
    let reloaded = ApiKeyManager::new(Some(pool));
    reloaded.load_from_db().await;
    assert!(!reloaded.verify_audit_chain().await);
}

#[tokio::test]
async fn usage_ledger_is_atomic_persistent_and_filterable() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));

    manager
        .record_usage("key-a", "subject-a", 200)
        .await
        .unwrap();
    manager
        .record_usage("key-a", "subject-a", 201)
        .await
        .unwrap();
    manager
        .record_usage("key-a", "subject-a", 429)
        .await
        .unwrap();
    manager
        .record_usage("key-b", "subject-b", 500)
        .await
        .unwrap();
    manager
        .record_usage("key-a-rotated", "subject-a", 200)
        .await
        .unwrap();

    let now = chrono::Utc::now().timestamp_millis();
    let records = manager
        .usage_records(
            now - 3_600_000,
            now + 3_600_000,
            Some("key-a"),
            None,
            100,
            0,
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records.iter().map(|r| r.request_count).sum::<i64>(), 3);
    assert_eq!(records[0].status_class, 2);
    assert_eq!(records[0].request_count, 2);
    assert_eq!(records[1].status_class, 4);

    let subject_records = manager
        .usage_records(
            now - 3_600_000,
            now + 3_600_000,
            None,
            Some("subject-a"),
            100,
            0,
        )
        .await
        .unwrap();
    assert_eq!(
        subject_records
            .iter()
            .map(|record| record.request_count)
            .sum::<i64>(),
        4
    );
    assert!(
        subject_records
            .iter()
            .all(|record| record.subject_id == "subject-a")
    );
    let key_ids = subject_records
        .iter()
        .map(|record| record.key_id.as_str())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        key_ids,
        std::collections::HashSet::from(["key-a", "key-a-rotated"])
    );
    let first_page = manager
        .usage_records(
            now - 3_600_000,
            now + 3_600_000,
            None,
            Some("subject-a"),
            1,
            0,
        )
        .await
        .unwrap();
    let second_page = manager
        .usage_records(
            now - 3_600_000,
            now + 3_600_000,
            None,
            Some("subject-a"),
            1,
            1,
        )
        .await
        .unwrap();
    assert_eq!(first_page.len(), 1);
    assert_eq!(second_page.len(), 1);
    assert_ne!(
        (first_page[0].key_id.as_str(), first_page[0].status_class),
        (second_page[0].key_id.as_str(), second_page[0].status_class)
    );
    assert_eq!(
        manager
            .usage_total(now - 3_600_000, now + 3_600_000, None, Some("subject-a"))
            .await
            .unwrap(),
        4
    );
    assert!(manager.verify_usage_ledger_integrity().await.unwrap());
    sqlx::query(
        "UPDATE api_usage_hourly SET request_count = request_count + 1 WHERE key_id = 'key-a'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(!manager.verify_usage_ledger_integrity().await.unwrap());
}

#[tokio::test]
async fn usage_ledger_integrity_detects_row_deletion_from_both_copies() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    manager.record_usage("key", "subject", 200).await.unwrap();
    assert!(manager.verify_usage_ledger_integrity().await.unwrap());
    sqlx::query("DELETE FROM api_usage_hourly")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM api_usage_integrity")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!manager.verify_usage_ledger_integrity().await.unwrap());
}

#[tokio::test]
async fn usage_subject_survives_credential_purge() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool));
    let (key_id, raw_key) = manager
        .create_key("purged-customer", vec![], None)
        .await
        .unwrap();
    let subject_id = manager.authenticate(&raw_key).await.unwrap().subject_id;
    manager
        .record_usage(&key_id, &subject_id, 200)
        .await
        .unwrap();
    assert!(manager.revoke_key(&key_id).await.unwrap());
    assert!(manager.delete_key(&key_id).await.unwrap());

    let now = chrono::Utc::now().timestamp_millis();
    let records = manager
        .usage_records(
            now - 3_600_000,
            now + 3_600_000,
            None,
            Some(&subject_id),
            100,
            0,
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].key_id, key_id);
    assert_eq!(records[0].subject_id, subject_id);
    assert_eq!(records[0].request_count, 1);
}

#[tokio::test]
async fn storage_readiness_detects_closed_pool() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    assert!(manager.storage_ready().await);
    pool.close().await;
    assert!(!manager.storage_ready().await);
}

#[tokio::test]
async fn metering_failure_blocks_and_real_write_probe_recovers() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    sqlx::query("DROP TABLE api_usage_hourly")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        manager
            .record_usage("customer", "customer", 200)
            .await
            .is_err()
    );
    assert!(!manager.metering_ready());
    assert!(!manager.probe_metering_ready().await);
    sqlx::query(
        "CREATE TABLE api_usage_hourly (
                key_id TEXT NOT NULL,
                subject_id TEXT NOT NULL,
                bucket_start INTEGER NOT NULL,
                status_class INTEGER NOT NULL,
                request_count INTEGER NOT NULL CHECK (request_count >= 0),
                PRIMARY KEY (key_id, bucket_start, status_class)
            )",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(manager.probe_metering_ready().await);
    assert!(manager.metering_ready());
}

#[tokio::test]
async fn billing_events_are_idempotent_and_conflicting_replays_are_rejected() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    let event = BillingEvent {
        provider: "stripe".into(),
        event_id: "evt_1".into(),
        event_type: "payment_succeeded".into(),
        object_id: "pi_1".into(),
        customer_ref: "customer-1".into(),
        amount_minor: 9900,
        currency: "USD".into(),
        occurred_at: 1_700_000_000_000,
        received_at: 0,
        event_hash: String::new(),
    };
    assert_eq!(
        manager.record_billing_event(event.clone()).await.unwrap(),
        BillingEventWrite::Created
    );
    assert_eq!(
        manager.record_billing_event(event.clone()).await.unwrap(),
        BillingEventWrite::Duplicate
    );
    let mut conflict = event.clone();
    conflict.amount_minor = 10_000;
    assert_eq!(
        manager.record_billing_event(conflict).await.unwrap(),
        BillingEventWrite::Conflict
    );
    let rows = manager
        .billing_events(
            1_600_000_000_000,
            1_800_000_000_000,
            BillingEventFilter {
                customer_ref: Some("customer-1"),
                provider: Some("stripe"),
                event_type: Some("payment_succeeded"),
            },
            10,
            0,
        )
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].amount_minor, 9900);
    for (event_id, event_type, occurred_at) in [
        ("evt_refund", "refund_succeeded", 1_700_000_000_001),
        ("evt_chargeback", "chargeback_opened", 1_700_000_000_002),
    ] {
        let mut followup = event.clone();
        followup.event_id = event_id.into();
        followup.event_type = event_type.into();
        followup.object_id = format!("object-{event_id}");
        followup.occurred_at = occurred_at;
        assert_eq!(
            manager.record_billing_event(followup).await.unwrap(),
            BillingEventWrite::Created
        );
    }
    let first_page = manager
        .billing_events(
            1_600_000_000_000,
            1_800_000_000_000,
            BillingEventFilter {
                customer_ref: Some("customer-1"),
                provider: Some("stripe"),
                event_type: None,
            },
            2,
            0,
        )
        .await
        .unwrap();
    let second_page = manager
        .billing_events(
            1_600_000_000_000,
            1_800_000_000_000,
            BillingEventFilter {
                customer_ref: Some("customer-1"),
                provider: Some("stripe"),
                event_type: None,
            },
            2,
            2,
        )
        .await
        .unwrap();
    assert_eq!(first_page.len(), 2);
    assert_eq!(second_page.len(), 1);
    assert_eq!(
        manager
            .billing_event_count(
                1_600_000_000_000,
                1_800_000_000_000,
                BillingEventFilter {
                    customer_ref: Some("customer-1"),
                    provider: Some("stripe"),
                    event_type: None,
                },
            )
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        manager
            .billing_event_count(
                1_600_000_000_000,
                1_800_000_000_000,
                BillingEventFilter {
                    customer_ref: Some("customer-1"),
                    provider: Some("stripe"),
                    event_type: Some("refund_succeeded"),
                },
            )
            .await
            .unwrap(),
        1
    );
    let all_events = manager
        .billing_events(
            1_600_000_000_000,
            1_800_000_000_000,
            BillingEventFilter {
                customer_ref: Some("customer-1"),
                provider: Some("stripe"),
                event_type: None,
            },
            100,
            0,
        )
        .await
        .unwrap();
    assert!(ApiKeyManager::verify_billing_event_integrity(&all_events));
    assert!(manager.verify_billing_ledger_integrity().await.unwrap());
    sqlx::query(
        "UPDATE billing_events SET amount_minor = amount_minor + 1 WHERE event_id = 'evt_1'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let tampered = manager
        .billing_events(
            1_600_000_000_000,
            1_800_000_000_000,
            BillingEventFilter {
                customer_ref: Some("customer-1"),
                provider: Some("stripe"),
                event_type: Some("payment_succeeded"),
            },
            10,
            0,
        )
        .await
        .unwrap();
    assert!(!ApiKeyManager::verify_billing_event_integrity(&tampered));
    assert!(!manager.verify_billing_ledger_integrity().await.unwrap());
}

#[tokio::test]
async fn billing_ledger_anchor_detects_deleted_valid_event() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    manager
        .record_billing_event(BillingEvent {
            provider: "provider".into(),
            event_id: "event-to-delete".into(),
            event_type: "invoice_paid".into(),
            object_id: "invoice-1".into(),
            customer_ref: "customer-1".into(),
            amount_minor: 500,
            currency: "CNY".into(),
            occurred_at: 1_700_000_000_000,
            received_at: 0,
            event_hash: String::new(),
        })
        .await
        .unwrap();
    assert!(manager.verify_billing_ledger_integrity().await.unwrap());
    sqlx::query("DELETE FROM billing_events WHERE event_id = 'event-to-delete'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!manager.verify_billing_ledger_integrity().await.unwrap());
}

#[tokio::test]
async fn idempotency_reservation_replays_and_rejects_changed_requests() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool.clone()));
    assert_eq!(
        manager
            .reserve_idempotency("key-1", "request-123", "hash-a")
            .await
            .unwrap(),
        IdempotencyReservation::Acquired
    );
    assert_eq!(
        manager
            .reserve_idempotency("key-1", "request-123", "hash-a")
            .await
            .unwrap(),
        IdempotencyReservation::InProgress
    );
    assert_eq!(
        manager
            .reserve_idempotency("key-1", "request-123", "hash-b")
            .await
            .unwrap(),
        IdempotencyReservation::Conflict
    );
    manager
        .complete_idempotency(
            "key-1",
            "request-123",
            "hash-a",
            200,
            r#"{"id":"message-1"}"#,
        )
        .await
        .unwrap();
    assert_eq!(
        manager
            .reserve_idempotency("key-1", "request-123", "hash-a")
            .await
            .unwrap(),
        IdempotencyReservation::Replay {
            status: 200,
            response_json: r#"{"id":"message-1"}"#.into()
        }
    );
    let reloaded = ApiKeyManager::new(Some(pool));
    assert!(matches!(
        reloaded
            .reserve_idempotency("key-1", "request-123", "hash-a")
            .await
            .unwrap(),
        IdempotencyReservation::Replay { .. }
    ));
}

#[tokio::test]
async fn target_grants_are_subject_scoped_action_scoped_and_fail_closed() {
    let pool = SqlitePool::connect(":memory:").await.unwrap();
    crate::storage::sqlite::run_migrations(&pool).await.unwrap();
    let manager = ApiKeyManager::new(Some(pool));
    let (_, raw_key) = manager
        .create_key("target-owner", vec!["websocketconnect".into()], None)
        .await
        .unwrap();
    let subject = manager.authenticate(&raw_key).await.unwrap().subject_id;
    let grant = manager
        .create_target_grant(
            &subject,
            "QQ",
            "group-a",
            vec![target_actions::INBOUND_READ.into()],
            "admin",
        )
        .await
        .unwrap();

    assert!(
        manager
            .target_authorized(&subject, "qq", "group-a", target_actions::INBOUND_READ,)
            .await
            .unwrap()
    );
    assert!(
        !manager
            .target_authorized(&subject, "qq", "group-a", target_actions::MESSAGES_READ,)
            .await
            .unwrap()
    );
    assert!(
        !manager
            .target_authorized(&subject, "qq", "group-b", target_actions::INBOUND_READ,)
            .await
            .unwrap()
    );
    assert_eq!(manager.list_target_grants(&subject).await.unwrap().len(), 1);
    assert!(
        manager
            .delete_target_grant(&subject, &grant.id)
            .await
            .unwrap()
    );
    assert!(
        !manager
            .target_authorized(&subject, "qq", "group-a", target_actions::INBOUND_READ,)
            .await
            .unwrap()
    );
}
