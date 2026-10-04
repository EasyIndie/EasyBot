//! 用量账本、计费事件与幂等预留。

use super::*;

impl ApiKeyManager {
    /// Atomically add one authenticated request to the durable hourly ledger.
    pub async fn record_usage(
        &self,
        key_id: &str,
        subject_id: &str,
        status: u16,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        let now = chrono::Utc::now().timestamp_millis();
        let hour_ms = 60 * 60 * 1_000;
        let bucket_start = now - now.rem_euclid(hour_ms);
        let status_class = i32::from(status / 100).clamp(1, 5);
        let result = async {
            let mut transaction = pool.begin().await?;
            for statement in [
                "INSERT INTO api_usage_hourly (key_id, subject_id, bucket_start, status_class, request_count) \
                 VALUES (?1, ?2, ?3, ?4, 1) \
                 ON CONFLICT(key_id, bucket_start, status_class) \
                 DO UPDATE SET request_count = request_count + 1, subject_id = excluded.subject_id",
                "INSERT INTO api_usage_integrity (key_id, subject_id, bucket_start, status_class, request_count) \
                 VALUES (?1, ?2, ?3, ?4, 1) \
                 ON CONFLICT(key_id, bucket_start, status_class) \
                 DO UPDATE SET request_count = request_count + 1, subject_id = excluded.subject_id",
            ] {
                sqlx::query(statement)
                    .bind(key_id)
                    .bind(subject_id)
                    .bind(bucket_start)
                    .bind(status_class)
                    .execute(&mut *transaction)
                    .await?;
            }
            let anchor = sqlx::query(
                "UPDATE api_usage_ledger_state SET total_requests = total_requests + 1 WHERE singleton = 1",
            )
            .execute(&mut *transaction)
            .await?;
            if anchor.rows_affected() != 1 {
                return Err(sqlx::Error::Protocol("usage ledger anchor is missing".into()));
            }
            transaction.commit().await
        }
        .await;
        match result {
            Ok(_) => {
                self.metering_healthy.store(true, Ordering::Release);
                Ok(())
            }
            Err(error) => {
                self.metering_healthy.store(false, Ordering::Release);
                Err(error.to_string())
            }
        }
    }

    /// Validate the mutable usage aggregate against an independently updated
    /// mirror and cumulative request anchor before any invoice export.
    pub async fn verify_usage_ledger_integrity(&self) -> Result<bool, String> {
        let Some(pool) = &self.pool else {
            return Err("durable usage storage is required".into());
        };
        let mismatch: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT key_id, subject_id, bucket_start, status_class, request_count FROM api_usage_hourly
                EXCEPT
                SELECT key_id, subject_id, bucket_start, status_class, request_count FROM api_usage_integrity
             ) OR EXISTS(
                SELECT key_id, subject_id, bucket_start, status_class, request_count FROM api_usage_integrity
                EXCEPT
                SELECT key_id, subject_id, bucket_start, status_class, request_count FROM api_usage_hourly
             )",
        )
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())?;
        if mismatch != 0 {
            return Ok(false);
        }
        let totals = sqlx::query_as::<_, (i64, i64)>(
            "SELECT COALESCE(SUM(u.request_count), 0), s.total_requests
             FROM api_usage_ledger_state s LEFT JOIN api_usage_hourly u ON 1 = 1
             WHERE s.singleton = 1 GROUP BY s.total_requests",
        )
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(matches!(totals, Some((actual, anchor)) if actual == anchor))
    }

    /// Read a bounded UTC time range from the durable usage ledger.
    pub async fn usage_records(
        &self,
        from: i64,
        to: i64,
        key_id: Option<&str>,
        subject_id: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<UsageRecord>, String> {
        let Some(pool) = &self.pool else {
            return Ok(Vec::new());
        };
        use sqlx::Row as _;
        let rows = if let Some(key_id) = key_id {
            sqlx::query(
                "SELECT key_id, subject_id, bucket_start, status_class, request_count \
                 FROM api_usage_hourly WHERE bucket_start >= ?1 AND bucket_start < ?2 \
                 AND key_id = ?3 ORDER BY bucket_start, key_id, status_class LIMIT ?4 OFFSET ?5",
            )
            .bind(from)
            .bind(to)
            .bind(key_id)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await
        } else if let Some(subject_id) = subject_id {
            sqlx::query(
                "SELECT key_id, subject_id, bucket_start, status_class, request_count \
                 FROM api_usage_hourly WHERE bucket_start >= ?1 AND bucket_start < ?2 \
                 AND subject_id = ?3 ORDER BY bucket_start, key_id, status_class LIMIT ?4 OFFSET ?5",
            )
            .bind(from)
            .bind(to)
            .bind(subject_id)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await
        } else {
            sqlx::query(
                "SELECT key_id, subject_id, bucket_start, status_class, request_count \
                 FROM api_usage_hourly WHERE bucket_start >= ?1 AND bucket_start < ?2 \
                 ORDER BY bucket_start, key_id, status_class LIMIT ?3 OFFSET ?4",
            )
            .bind(from)
            .bind(to)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await
        }
        .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| UsageRecord {
                key_id: row.get("key_id"),
                subject_id: row.get("subject_id"),
                bucket_start: row.get("bucket_start"),
                status_class: row.get("status_class"),
                request_count: row.get("request_count"),
            })
            .collect())
    }

    pub async fn usage_total(
        &self,
        from: i64,
        to: i64,
        key_id: Option<&str>,
        subject_id: Option<&str>,
    ) -> Result<i64, String> {
        let Some(pool) = &self.pool else {
            return Ok(0);
        };
        let total = if let Some(key_id) = key_id {
            sqlx::query_scalar(
                "SELECT COALESCE(SUM(request_count), 0) FROM api_usage_hourly
                 WHERE bucket_start >= ?1 AND bucket_start < ?2 AND key_id = ?3",
            )
            .bind(from)
            .bind(to)
            .bind(key_id)
            .fetch_one(pool)
            .await
        } else if let Some(subject_id) = subject_id {
            sqlx::query_scalar(
                "SELECT COALESCE(SUM(request_count), 0) FROM api_usage_hourly
                 WHERE bucket_start >= ?1 AND bucket_start < ?2 AND subject_id = ?3",
            )
            .bind(from)
            .bind(to)
            .bind(subject_id)
            .fetch_one(pool)
            .await
        } else {
            sqlx::query_scalar(
                "SELECT COALESCE(SUM(request_count), 0) FROM api_usage_hourly
                 WHERE bucket_start >= ?1 AND bucket_start < ?2",
            )
            .bind(from)
            .bind(to)
            .fetch_one(pool)
            .await
        };
        total.map_err(|error| error.to_string())
    }

    pub async fn record_billing_event(
        &self,
        mut event: BillingEvent,
    ) -> Result<BillingEventWrite, String> {
        let Some(pool) = &self.pool else {
            return Err("durable billing storage is required".into());
        };
        event.received_at = chrono::Utc::now().timestamp_millis();
        event.event_hash = billing_event_hash(&event);
        let mut transaction = pool.begin().await.map_err(|error| error.to_string())?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO billing_events \
             (provider,event_id,event_type,object_id,customer_ref,amount_minor,currency,occurred_at,received_at,event_hash) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        )
        .bind(&event.provider).bind(&event.event_id).bind(&event.event_type)
        .bind(&event.object_id).bind(&event.customer_ref).bind(event.amount_minor)
        .bind(&event.currency).bind(event.occurred_at).bind(event.received_at)
        .bind(&event.event_hash).execute(&mut *transaction).await.map_err(|error| error.to_string())?;
        if inserted.rows_affected() == 1 {
            let updated = sqlx::query(
                "UPDATE billing_ledger_state SET event_count = event_count + 1 WHERE singleton = 1",
            )
            .execute(&mut *transaction)
            .await
            .map_err(|error| error.to_string())?;
            if updated.rows_affected() != 1 {
                return Err("billing ledger anchor is missing".into());
            }
            transaction
                .commit()
                .await
                .map_err(|error| error.to_string())?;
            return Ok(BillingEventWrite::Created);
        }
        use sqlx::Row as _;
        let existing =
            sqlx::query("SELECT event_hash FROM billing_events WHERE provider=?1 AND event_id=?2")
                .bind(&event.provider)
                .bind(&event.event_id)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|error| error.to_string())?;
        let disposition = if existing.get::<String, _>("event_hash") == event.event_hash {
            BillingEventWrite::Duplicate
        } else {
            BillingEventWrite::Conflict
        };
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        Ok(disposition)
    }

    pub async fn reserve_idempotency(
        &self,
        key_id: &str,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<IdempotencyReservation, String> {
        let Some(pool) = &self.pool else {
            return Err("durable idempotency storage is required".into());
        };
        let now = chrono::Utc::now().timestamp_millis();
        let expires_at = now + 24 * 60 * 60 * 1_000;
        let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("DELETE FROM api_idempotency WHERE expires_at <= ?1")
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO api_idempotency \
             (key_id,idempotency_key,request_hash,state,created_at,expires_at) \
             VALUES (?1,?2,?3,'pending',?4,?5)",
        )
        .bind(key_id)
        .bind(idempotency_key)
        .bind(request_hash)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        if inserted.rows_affected() == 1 {
            tx.commit().await.map_err(|e| e.to_string())?;
            return Ok(IdempotencyReservation::Acquired);
        }
        use sqlx::Row as _;
        let row = sqlx::query(
            "SELECT request_hash,state,http_status,response_json FROM api_idempotency \
             WHERE key_id=?1 AND idempotency_key=?2",
        )
        .bind(key_id)
        .bind(idempotency_key)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let existing_hash: String = row.get("request_hash");
        let state: String = row.get("state");
        let status: Option<i64> = row.get("http_status");
        let response: Option<String> = row.get("response_json");
        tx.commit().await.map_err(|e| e.to_string())?;
        if existing_hash != request_hash {
            Ok(IdempotencyReservation::Conflict)
        } else if state == "completed" {
            Ok(IdempotencyReservation::Replay {
                status: status.unwrap_or(500) as u16,
                response_json: response.unwrap_or_else(|| "{}".into()),
            })
        } else {
            Ok(IdempotencyReservation::InProgress)
        }
    }

    pub async fn complete_idempotency(
        &self,
        key_id: &str,
        idempotency_key: &str,
        request_hash: &str,
        status: u16,
        response_json: &str,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Err("durable idempotency storage is required".into());
        };
        let result = sqlx::query(
            "UPDATE api_idempotency SET state='completed',http_status=?1,response_json=?2 \
             WHERE key_id=?3 AND idempotency_key=?4 AND request_hash=?5 AND state='pending'",
        )
        .bind(i64::from(status))
        .bind(response_json)
        .bind(key_id)
        .bind(idempotency_key)
        .bind(request_hash)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
        if result.rows_affected() != 1 {
            return Err("idempotency reservation was not pending".into());
        }
        Ok(())
    }

    pub async fn billing_events(
        &self,
        from: i64,
        to: i64,
        filter: BillingEventFilter<'_>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<BillingEvent>, String> {
        let Some(pool) = &self.pool else {
            return Ok(Vec::new());
        };
        use sqlx::Row as _;
        let mut query =
            sqlx::QueryBuilder::new("SELECT * FROM billing_events WHERE occurred_at >= ");
        query
            .push_bind(from)
            .push(" AND occurred_at < ")
            .push_bind(to);
        if let Some(customer_ref) = filter.customer_ref {
            query.push(" AND customer_ref = ").push_bind(customer_ref);
        }
        if let Some(provider) = filter.provider {
            query.push(" AND provider = ").push_bind(provider);
        }
        if let Some(event_type) = filter.event_type {
            query.push(" AND event_type = ").push_bind(event_type);
        }
        query
            .push(" ORDER BY occurred_at DESC, provider, event_id LIMIT ")
            .push_bind(limit as i64)
            .push(" OFFSET ")
            .push_bind(offset as i64);
        let rows = query
            .build()
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| BillingEvent {
                provider: row.get("provider"),
                event_id: row.get("event_id"),
                event_type: row.get("event_type"),
                object_id: row.get("object_id"),
                customer_ref: row.get("customer_ref"),
                amount_minor: row.get("amount_minor"),
                currency: row.get("currency"),
                occurred_at: row.get("occurred_at"),
                received_at: row.get("received_at"),
                event_hash: row.get("event_hash"),
            })
            .collect())
    }

    pub async fn billing_event_count(
        &self,
        from: i64,
        to: i64,
        filter: BillingEventFilter<'_>,
    ) -> Result<i64, String> {
        let Some(pool) = &self.pool else {
            return Ok(0);
        };
        let mut query =
            sqlx::QueryBuilder::new("SELECT COUNT(*) FROM billing_events WHERE occurred_at >= ");
        query
            .push_bind(from)
            .push(" AND occurred_at < ")
            .push_bind(to);
        if let Some(customer_ref) = filter.customer_ref {
            query.push(" AND customer_ref = ").push_bind(customer_ref);
        }
        if let Some(provider) = filter.provider {
            query.push(" AND provider = ").push_bind(provider);
        }
        if let Some(event_type) = filter.event_type {
            query.push(" AND event_type = ").push_bind(event_type);
        }
        query
            .build_query_scalar()
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())
    }

    /// Verify immutable financial event fields against their stored content hashes.
    pub fn verify_billing_event_integrity(events: &[BillingEvent]) -> bool {
        events
            .iter()
            .all(|event| billing_event_hash(event) == event.event_hash)
    }

    /// Verify every persisted financial event and the independent row-count
    /// anchor so deletion cannot be hidden by returning only the remaining rows.
    pub async fn verify_billing_ledger_integrity(&self) -> Result<bool, String> {
        let Some(pool) = &self.pool else {
            return Err("durable billing storage is required".into());
        };
        use sqlx::Row as _;
        let mut rows =
            sqlx::query("SELECT * FROM billing_events ORDER BY received_at, rowid").fetch(pool);
        let mut count = 0_i64;
        while let Some(row) = rows.try_next().await.map_err(|error| error.to_string())? {
            let event = BillingEvent {
                provider: row.get("provider"),
                event_id: row.get("event_id"),
                event_type: row.get("event_type"),
                object_id: row.get("object_id"),
                customer_ref: row.get("customer_ref"),
                amount_minor: row.get("amount_minor"),
                currency: row.get("currency"),
                occurred_at: row.get("occurred_at"),
                received_at: row.get("received_at"),
                event_hash: row.get("event_hash"),
            };
            if billing_event_hash(&event) != event.event_hash {
                return Ok(false);
            }
            count += 1;
        }
        let anchor = sqlx::query_scalar::<_, i64>(
            "SELECT event_count FROM billing_ledger_state WHERE singleton = 1",
        )
        .fetch_optional(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(anchor == Some(count))
    }
}
