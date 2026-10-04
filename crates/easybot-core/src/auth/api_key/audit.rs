//! 管理审计账本（链式哈希）。

use super::*;

impl ApiKeyManager {
    /// Append a tamper-evident audit event. Metadata must already be redacted.
    pub async fn record_audit(
        &self,
        actor_id: &str,
        action: &str,
        resource: &str,
        metadata: serde_json::Value,
    ) -> Result<AuditEvent, String> {
        let _guard = self.audit_lock.lock().await;
        let timestamp = chrono::Utc::now().timestamp_millis();
        let id = Uuid::new_v4().to_string();
        let metadata_json = serde_json::to_string(&metadata).map_err(|e| e.to_string())?;
        let previous_hash = if self.pool.is_some() {
            self.audit_head
                .read()
                .await
                .clone()
                .unwrap_or_else(|| "GENESIS".to_string())
        } else {
            self.audit_events
                .read()
                .await
                .last()
                .map(|event| event.event_hash.clone())
                .unwrap_or_else(|| "GENESIS".to_string())
        };
        let event_hash = audit_hash(
            &id,
            timestamp,
            actor_id,
            action,
            resource,
            &metadata_json,
            &previous_hash,
        );
        let event = AuditEvent {
            id,
            timestamp,
            actor_id: actor_id.to_string(),
            action: action.to_string(),
            resource: resource.to_string(),
            metadata,
            previous_hash,
            event_hash,
        };
        if let Some(pool) = &self.pool {
            let mut transaction = pool.begin().await.map_err(|e| e.to_string())?;
            sqlx::query("INSERT INTO audit_events (id, timestamp, actor_id, action, resource, metadata_json, previous_hash, event_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
                .bind(&event.id).bind(event.timestamp).bind(&event.actor_id).bind(&event.action)
                .bind(&event.resource).bind(&metadata_json).bind(&event.previous_hash).bind(&event.event_hash)
                .execute(&mut *transaction).await.map_err(|e| e.to_string())?;
            let updated = sqlx::query(
                "UPDATE audit_chain_state SET head_hash = ?1, event_count = event_count + 1 WHERE singleton = 1 AND head_hash = ?2",
            )
            .bind(&event.event_hash)
            .bind(&event.previous_hash)
            .execute(&mut *transaction)
            .await
            .map_err(|e| e.to_string())?;
            if updated.rows_affected() != 1 {
                return Err("audit chain anchor does not match current head".into());
            }
            transaction.commit().await.map_err(|e| e.to_string())?;
        }
        if self.pool.is_some() {
            *self.audit_head.write().await = Some(event.event_hash.clone());
        } else {
            self.audit_events.write().await.push(event.clone());
        }
        Ok(event)
    }

    pub async fn list_audit_events(&self, limit: usize) -> Vec<AuditEvent> {
        self.query_audit_events(limit).await.unwrap_or_default()
    }

    pub async fn query_audit_events(&self, limit: usize) -> Result<Vec<AuditEvent>, String> {
        if let Some(pool) = &self.pool {
            let rows = sqlx::query(
                "SELECT id, timestamp, actor_id, action, resource, metadata_json, previous_hash, event_hash FROM audit_events ORDER BY timestamp DESC, rowid DESC LIMIT ?1",
            )
            .bind(limit.min(1_000) as i64)
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
            return rows
                .into_iter()
                .map(|row| audit_event_from_row(&row))
                .collect();
        }
        Ok(self
            .audit_events
            .read()
            .await
            .iter()
            .rev()
            .take(limit.min(1_000))
            .cloned()
            .collect())
    }

    pub async fn verify_audit_chain(&self) -> bool {
        let _guard = self.audit_lock.lock().await;
        if let Some(pool) = &self.pool {
            let mut rows = sqlx::query(
                "SELECT id, timestamp, actor_id, action, resource, metadata_json, previous_hash, event_hash FROM audit_events ORDER BY timestamp, rowid",
            )
            .fetch(pool);
            let mut previous = "GENESIS".to_string();
            let mut count = 0_i64;
            loop {
                let row = match rows.try_next().await {
                    Ok(Some(row)) => row,
                    Ok(None) => break,
                    Err(error) => {
                        tracing::error!(%error, "failed to stream audit ledger for verification");
                        return false;
                    }
                };
                let event = match audit_event_from_row(&row) {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::error!(%error, "invalid persisted audit event");
                        return false;
                    }
                };
                if !audit_event_follows(&event, &previous) {
                    return false;
                }
                previous = event.event_hash;
                count += 1;
            }
            let anchor = sqlx::query_as::<_, (String, i64)>(
                "SELECT head_hash, event_count FROM audit_chain_state WHERE singleton = 1",
            )
            .fetch_optional(pool)
            .await;
            return matches!(anchor, Ok(Some((head, anchor_count))) if head == previous && anchor_count == count);
        }
        let events = self.audit_events.read().await;
        let mut previous = "GENESIS".to_string();
        for event in events.iter() {
            if !audit_event_follows(event, &previous) {
                return false;
            }
            previous = event.event_hash.clone();
        }
        true
    }
}
