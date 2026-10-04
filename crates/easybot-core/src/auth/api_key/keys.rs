//! API Key 生命周期：创建/轮换/鉴权/配额/吊销/列表。

use super::*;

impl ApiKeyManager {
    /// 创建新的 API Key
    ///
    /// 返回 (key_id, raw_key)。raw_key 仅在创建时返回，不再持久化存储。
    ///
    /// Keys created through this method are durable when a storage pool is configured.
    /// Short-lived, memory-only credentials must use `create_ephemeral_key` explicitly.
    pub async fn create_key(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: Option<i64>,
    ) -> Result<(String, String), String> {
        self.create_key_with_quota(name, permissions, expires_at, None)
            .await
    }

    /// Create an API key with an optional independently enforced minute quota.
    pub async fn create_key_with_quota(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: Option<i64>,
        requests_per_minute: Option<u32>,
    ) -> Result<(String, String), String> {
        self.create_key_internal(
            name,
            permissions,
            expires_at,
            requests_per_minute,
            true,
            None,
            None,
        )
        .await
    }

    /// Create a replacement credential that preserves the caller identity.
    pub async fn create_rotated_key(
        &self,
        source: &ApiKeyInfo,
        expires_at: i64,
    ) -> Result<(String, String), String> {
        self.create_key_internal(
            &source.name,
            source.permissions.clone(),
            Some(expires_at),
            source.requests_per_minute,
            true,
            Some(source.subject_id.clone()),
            Some(source.id.clone()),
        )
        .await
    }

    /// Create a short-lived key that is never written to SQLite.
    pub async fn create_ephemeral_key(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: i64,
    ) -> Result<(String, String), String> {
        const MAX_ACTIVE_PER_EPHEMERAL_NAME: usize = 8;
        // Remove expired ephemeral/session keys while issuing a replacement.
        let now = chrono::Utc::now().timestamp_millis();
        self.keys
            .write()
            .await
            .retain(|_, stored| stored.info.expires_at.is_none_or(|expires| expires > now));
        let created = self
            .create_key_internal(name, permissions, Some(expires_at), None, false, None, None)
            .await?;
        let created_index = sha256_index(&created.1);
        // Browser reloads and repeated logins must not create an unbounded set of
        // simultaneously valid management credentials. Keep a small bounded set
        // so multiple tabs/operators still work, evicting the oldest sessions.
        let mut keys = self.keys.write().await;
        let mut same_name = keys
            .iter()
            .filter(|(_, stored)| !stored.manageable && stored.info.name == name)
            .map(|(index, stored)| {
                (
                    index.clone(),
                    index == &created_index,
                    stored.info.created_at,
                )
            })
            .collect::<Vec<_>>();
        same_name.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.2.cmp(&left.2)));
        for (index, _, _) in same_name.into_iter().skip(MAX_ACTIVE_PER_EPHEMERAL_NAME) {
            keys.remove(&index);
        }
        Ok(created)
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_key_internal(
        &self,
        name: &str,
        permissions: Vec<String>,
        expires_at: Option<i64>,
        requests_per_minute: Option<u32>,
        persist: bool,
        subject_id: Option<String>,
        rotation_source_id: Option<String>,
    ) -> Result<(String, String), String> {
        let is_rotation = rotation_source_id.is_some();
        if permissions.len() > 32 {
            return Err("permissions exceed policy limits".into());
        }
        let unique_permissions: std::collections::HashSet<_> = permissions.iter().collect();
        if unique_permissions.len() != permissions.len() {
            return Err("permissions must not contain duplicates".into());
        }
        let key_id = Uuid::new_v4().to_string();
        let subject_id = subject_id.unwrap_or_else(|| key_id.clone());
        let raw_key = format!("eb_{}", Uuid::new_v4().to_string().replace("-", ""));
        let prefix = raw_key.chars().take(8).collect::<String>();

        // 生成 Argon2 哈希 (CPU 密集型，使用 spawn_blocking)
        // argon2 0.6：hash_password 内部用 getrandom 生成随机 salt，无需显式 salt
        let raw_key_clone = raw_key.clone();
        let phc_hash = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let argon2 = Argon2::default();
            argon2
                .hash_password(raw_key_clone.as_bytes())
                .map(|h| h.to_string())
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))??;

        let now = chrono::Utc::now().timestamp_millis();
        let info = ApiKeyInfo {
            id: key_id.clone(),
            subject_id: subject_id.clone(),
            name: name.to_string(),
            prefix: prefix.clone(),
            created_at: now,
            expires_at,
            last_used_at: None,
            revoked: false,
            permissions: permissions.clone(),
            requests_per_minute,
        };

        let stored = StoredKey {
            info,
            hash: phc_hash.clone(),
            manageable: persist,
        };

        // 内存索引（SHA-256 快速查找）
        let index_hash = sha256_index(&raw_key);
        self.keys.write().await.insert(index_hash.clone(), stored);

        // Persist every durable user-managed API key. Ephemeral management
        // sessions authenticate from memory and are deliberately excluded.
        if persist && let Some(pool) = &self.pool {
            let perms_json = serde_json::to_string(&permissions).map_err(|e| e.to_string())?;
            let mut connection = match pool.acquire().await {
                Ok(connection) => connection,
                Err(error) => {
                    self.keys.write().await.remove(&index_hash);
                    return Err(format!("failed to acquire API key database: {error}"));
                }
            };
            if let Err(error) = sqlx::query("BEGIN IMMEDIATE")
                .execute(&mut *connection)
                .await
            {
                self.keys.write().await.remove(&index_hash);
                return Err(format!("failed to begin API key transaction: {error}"));
            }
            let active_count = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM api_keys WHERE revoked = 0 AND (expires_at IS NULL OR expires_at > ?1)",
            )
            .bind(now)
            .fetch_one(&mut *connection)
            .await;
            match active_count {
                Ok(count) if count >= if is_rotation { 101 } else { 100 } => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    self.keys.write().await.remove(&index_hash);
                    return Err(if is_rotation {
                        "API Key rotation transition slot is occupied".into()
                    } else {
                        "active API Key limit reached (100)".into()
                    });
                }
                Err(error) => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    self.keys.write().await.remove(&index_hash);
                    return Err(format!("failed to count active API keys: {error}"));
                }
                Ok(_) => {}
            }
            if let Err(error) = sqlx::query(
                    "INSERT INTO api_keys (id, subject_id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, requests_per_minute, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
                )
                .bind(&key_id).bind(&subject_id).bind(name).bind(&prefix).bind(now).bind(expires_at).bind(None::<i64>).bind(0).bind(&perms_json).bind(requests_per_minute.map(i64::from)).bind(&phc_hash)
                .execute(&mut *connection)
                .await
            {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                self.keys.write().await.remove(&index_hash);
                return Err(format!("failed to persist API key: {error}"));
            }
            if let Some(source_id) = &rotation_source_id
                && let Err(error) = sqlx::query(
                    "INSERT INTO api_key_rotation_transitions(source_id,replacement_id,state,created_at) VALUES (?1,?2,'created',?3)",
                )
                .bind(source_id)
                .bind(&key_id)
                .bind(now)
                .execute(&mut *connection)
                .await
            {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                self.keys.write().await.remove(&index_hash);
                return Err(format!("failed to persist API key rotation transition: {error}"));
            }
            if let Err(error) = sqlx::query("COMMIT").execute(&mut *connection).await {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                self.keys.write().await.remove(&index_hash);
                return Err(format!("failed to commit API key transaction: {error}"));
            }
        }

        Ok((key_id, raw_key))
    }

    pub async fn mark_rotation_prepared(
        &self,
        source_id: &str,
        replacement_id: &str,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        let updated = sqlx::query(
            "UPDATE api_key_rotation_transitions SET state='prepared' WHERE source_id=?1 AND replacement_id=?2 AND state='created'",
        )
        .bind(source_id)
        .bind(replacement_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        if updated.rows_affected() != 1 {
            return Err("API key rotation transition was not in created state".into());
        }
        Ok(())
    }

    pub async fn clear_rotation_transition(
        &self,
        source_id: &str,
        replacement_id: &str,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Ok(());
        };
        sqlx::query(
            "DELETE FROM api_key_rotation_transitions WHERE source_id=?1 AND replacement_id=?2",
        )
        .bind(source_id)
        .bind(replacement_id)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub async fn rotation_transition_count(&self) -> Result<i64, String> {
        let Some(pool) = &self.pool else {
            return Ok(0);
        };
        sqlx::query_scalar("SELECT COUNT(*) FROM api_key_rotation_transitions")
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn rotation_transitions(&self) -> Result<Vec<ApiKeyRotationTransition>, String> {
        let Some(pool) = &self.pool else {
            return Ok(Vec::new());
        };
        use sqlx::Row as _;
        let rows = sqlx::query(
            "SELECT t.source_id,t.replacement_id,t.state,t.created_at,
                    s.revoked AS source_revoked,r.revoked AS replacement_revoked
             FROM api_key_rotation_transitions t
             JOIN api_keys s ON s.id=t.source_id
             JOIN api_keys r ON r.id=t.replacement_id
             ORDER BY t.created_at,t.source_id",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .map(|row| ApiKeyRotationTransition {
                source_id: row.get("source_id"),
                replacement_id: row.get("replacement_id"),
                state: row.get("state"),
                created_at: row.get("created_at"),
                source_revoked: row.get::<i64, _>("source_revoked") != 0,
                replacement_revoked: row.get::<i64, _>("replacement_revoked") != 0,
            })
            .collect())
    }

    /// Atomically resolve a crash-left rotation transition. `cancel` is only
    /// valid before the prepared audit gate; `complete` is only valid after it.
    pub async fn reconcile_rotation_transition(
        &self,
        source_id: &str,
        replacement_id: &str,
        action: &str,
    ) -> Result<(), String> {
        let Some(pool) = &self.pool else {
            return Err("durable API key storage is required".into());
        };
        let _lifecycle = self.key_lifecycle_lock.lock().await;
        let mut connection = pool.acquire().await.map_err(|error| error.to_string())?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(|error| error.to_string())?;
        let outcome = async {
            let state = sqlx::query_scalar::<_, String>(
                "SELECT state FROM api_key_rotation_transitions WHERE source_id=?1 AND replacement_id=?2",
            )
            .bind(source_id)
            .bind(replacement_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or(sqlx::Error::RowNotFound)?;
            let target_id = match (action, state.as_str()) {
                ("cancel", "created") => replacement_id,
                ("complete", "prepared") => source_id,
                _ => {
                    return Err(sqlx::Error::Protocol(
                        "rotation action does not match transition state".into(),
                    ));
                }
            };
            let updated = sqlx::query("UPDATE api_keys SET revoked=1 WHERE id=?1 AND revoked=0")
                .bind(target_id)
                .execute(&mut *connection)
                .await?;
            if updated.rows_affected() != 1 {
                return Err(sqlx::Error::Protocol(
                    "rotation reconciliation target is not active".into(),
                ));
            }
            sqlx::query(
                "DELETE FROM api_key_rotation_transitions WHERE source_id=?1 AND replacement_id=?2",
            )
            .bind(source_id)
            .bind(replacement_id)
            .execute(&mut *connection)
            .await?;
            Ok::<_, sqlx::Error>(target_id.to_string())
        }
        .await;
        match outcome {
            Ok(revoked_id) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(|error| error.to_string())?;
                self.keys
                    .write()
                    .await
                    .retain(|_, stored| stored.info.id != revoked_id);
                self.loaded
                    .write()
                    .await
                    .retain(|stored| stored.info.id != revoked_id);
                Ok(())
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error.to_string())
            }
        }
    }

    /// 验证 API Key
    ///
    /// 优先使用 SHA-256 快速定位（运行时创建的 Key），
    /// 未命中则遍历 DB 加载的历史 Key 并用 Argon2 验证。
    pub async fn authenticate(&self, key: &str) -> Result<AuthInfo, String> {
        let Some(prefix) = api_key_prefix(key) else {
            return Err("Invalid API key".to_string());
        };
        let index_hash = sha256_index(key);

        // 快速路径：SHA-256 索引查找
        let indexed = {
            let keys = self.keys.read().await;
            keys.get(&index_hash).cloned()
        };
        if let Some(stored) = indexed {
            let auth = Self::verify_and_build_auth(&stored, key).await?;
            self.record_successful_use(&auth.id).await;
            return Ok(auth);
        }

        // Persisted keys cannot be SHA-256 indexed after restart because the raw
        // secret is intentionally unavailable. The random public prefix narrows
        // Argon2 work to collision candidates instead of all historical keys.
        {
            // Clone candidates so Argon2 verification never holds an async read
            // lock and usage updates cannot deadlock.
            let loaded = self
                .loaded
                .read()
                .await
                .iter()
                .filter(|stored| stored.info.prefix == prefix)
                .cloned()
                .collect::<Vec<_>>();
            for stored in loaded.iter() {
                if stored.info.revoked {
                    continue;
                }
                if let Some(expires) = stored.info.expires_at
                    && chrono::Utc::now().timestamp_millis() > expires
                {
                    continue;
                }
                // 尝试 Argon2 验证
                let phc_hash = stored.hash.clone();
                let key_owned = key.to_string();
                let verified = tokio::task::spawn_blocking(move || {
                    let parsed_hash = PasswordHash::new(&phc_hash).map_err(|e| e.to_string())?;
                    let argon2 = Argon2::default();
                    argon2
                        .verify_password(key_owned.as_bytes(), &parsed_hash)
                        .map_err(|_| "Invalid API key".to_string())
                })
                .await
                .map_err(|e| format!("Task join error: {}", e))?;

                if verified.is_ok() {
                    let auth_info = self
                        .promote_verified_loaded_key(index_hash, &stored.info.id)
                        .await?;
                    self.record_successful_use(&auth_info.id).await;
                    return Ok(auth_info);
                }
            }
        }

        Err("Invalid API key".to_string())
    }

    pub(super) async fn promote_verified_loaded_key(
        &self,
        index_hash: String,
        key_id: &str,
    ) -> Result<AuthInfo, String> {
        // Re-check authoritative current state while serialized with
        // revoke/delete. The Argon2 candidate passed here is a stale clone.
        let _lifecycle = self.key_lifecycle_lock.lock().await;
        let current = self
            .loaded
            .read()
            .await
            .iter()
            .find(|candidate| candidate.info.id == key_id)
            .cloned();
        let Some(current) = current else {
            return Err("Invalid API key".to_string());
        };
        if current.info.revoked
            || current
                .info
                .expires_at
                .is_some_and(|expires| chrono::Utc::now().timestamp_millis() > expires)
        {
            return Err("Invalid API key".to_string());
        }
        let auth_info = AuthInfo {
            id: current.info.id.clone(),
            subject_id: current.info.subject_id.clone(),
            name: current.info.name.clone(),
            permissions: current.info.permissions.clone(),
            requests_per_minute: current.info.requests_per_minute,
        };
        self.keys.write().await.insert(index_hash, current);
        Ok(auth_info)
    }

    /// Atomically consume one request from a subject-level 60-second window.
    /// Persistent managers keep the window in auth.db so restarts cannot reset it.
    pub async fn consume_subject_quota(
        &self,
        subject_id: &str,
        limit: u32,
    ) -> Result<QuotaConsumption, String> {
        const WINDOW_MS: i64 = 60_000;
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Some(pool) = &self.pool {
            let mut connection = pool
                .acquire()
                .await
                .map_err(|error| format!("failed to acquire quota database: {error}"))?;
            sqlx::query("BEGIN IMMEDIATE")
                .execute(&mut *connection)
                .await
                .map_err(|error| format!("failed to begin quota transaction: {error}"))?;
            let outcome = async {
                sqlx::query("DELETE FROM api_quota_events WHERE occurred_at <= ?1")
                .bind(now_ms - WINDOW_MS)
                .execute(&mut *connection)
                .await?;
                let (count, oldest): (i64, Option<i64>) = sqlx::query_as(
                    "SELECT COUNT(*), MIN(occurred_at) FROM api_quota_events WHERE subject_id = ?1",
                )
                .bind(subject_id)
                .fetch_one(&mut *connection)
                .await?;
                if count >= i64::from(limit) {
                    return Ok::<_, sqlx::Error>((false, count, oldest));
                }
                sqlx::query(
                    "INSERT INTO api_quota_events (id, subject_id, occurred_at) VALUES (?1, ?2, ?3)",
                )
                .bind(Uuid::now_v7().to_string())
                .bind(subject_id)
                .bind(now_ms)
                .execute(&mut *connection)
                .await?;
                Ok((true, count + 1, oldest.or(Some(now_ms))))
            }
            .await;
            match outcome {
                Ok((allowed, count, oldest)) => {
                    sqlx::query("COMMIT")
                        .execute(&mut *connection)
                        .await
                        .map_err(|error| format!("failed to commit quota transaction: {error}"))?;
                    let retry_after_secs = oldest
                        .map(|oldest| {
                            ((WINDOW_MS - (now_ms - oldest)).max(1) as u64).div_ceil(1_000)
                        })
                        .unwrap_or(60)
                        .max(1);
                    return Ok(QuotaConsumption {
                        allowed,
                        remaining: if allowed {
                            limit.saturating_sub(count as u32)
                        } else {
                            0
                        },
                        retry_after_secs,
                    });
                }
                Err(error) => {
                    let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                    return Err(format!("failed to persist quota decision: {error}"));
                }
            }
        }

        let mut windows = self.quota_windows.lock().await;
        let timestamps = windows.entry(subject_id.to_string()).or_default();
        let now = Instant::now();
        while timestamps
            .front()
            .is_some_and(|timestamp| now.duration_since(*timestamp).as_secs() >= 60)
        {
            timestamps.pop_front();
        }
        if timestamps.len() >= limit as usize {
            let retry_after_secs = timestamps
                .front()
                .map(|oldest| 60u64.saturating_sub(now.duration_since(*oldest).as_secs()))
                .unwrap_or(60)
                .max(1);
            return Ok(QuotaConsumption {
                allowed: false,
                remaining: 0,
                retry_after_secs,
            });
        }
        timestamps.push_back(now);
        Ok(QuotaConsumption {
            allowed: true,
            remaining: limit.saturating_sub(timestamps.len() as u32),
            retry_after_secs: 60,
        })
    }

    /// Update the in-memory and persisted last-used timestamp after successful
    /// authentication. Persistence failure must not turn valid authentication
    /// into an outage, but is logged for operators.
    async fn record_successful_use(&self, key_id: &str) {
        let now = chrono::Utc::now().timestamp_millis();
        const PERSIST_INTERVAL_MS: i64 = 60_000;
        let mut should_persist = false;
        {
            let mut keys = self.keys.write().await;
            for stored in keys.values_mut().filter(|stored| stored.info.id == key_id) {
                should_persist |= stored
                    .info
                    .last_used_at
                    .is_none_or(|last| now - last >= PERSIST_INTERVAL_MS);
                stored.info.last_used_at = Some(now);
            }
        }
        {
            let mut loaded = self.loaded.write().await;
            for stored in loaded.iter_mut().filter(|stored| stored.info.id == key_id) {
                should_persist |= stored
                    .info
                    .last_used_at
                    .is_none_or(|last| now - last >= PERSIST_INTERVAL_MS);
                stored.info.last_used_at = Some(now);
            }
        }
        // A one-minute write-behind window avoids one database write per API
        // request while keeping operator-visible activity reasonably fresh.
        if should_persist
            && let Some(pool) = &self.pool
            && let Err(error) = sqlx::query("UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2")
                .bind(now)
                .bind(key_id)
                .execute(pool)
                .await
        {
            tracing::warn!(key_id, %error, "Failed to persist API key usage timestamp");
        }
    }

    /// Argon2 验证并构建 AuthInfo
    async fn verify_and_build_auth(stored: &StoredKey, key: &str) -> Result<AuthInfo, String> {
        // SECURITY: Use unified error message to prevent user enumeration
        // (distinguishing revoked vs invalid keys leaks information)
        if stored.info.revoked {
            return Err("Invalid API key".to_string());
        }

        // SECURITY: Use unified error message to prevent user enumeration
        if let Some(expires) = stored.info.expires_at
            && chrono::Utc::now().timestamp_millis() > expires
        {
            return Err("Invalid API key".to_string());
        }

        let auth_info = AuthInfo {
            id: stored.info.id.clone(),
            subject_id: stored.info.subject_id.clone(),
            name: stored.info.name.clone(),
            permissions: stored.info.permissions.clone(),
            requests_per_minute: stored.info.requests_per_minute,
        };
        let phc_hash = stored.hash.clone();
        let key_owned = key.to_string();

        tokio::task::spawn_blocking(move || {
            let parsed_hash = PasswordHash::new(&phc_hash).map_err(|e| e.to_string())?;
            let argon2 = Argon2::default();
            argon2
                .verify_password(key_owned.as_bytes(), &parsed_hash)
                .map_err(|_| "Invalid API key".to_string())?;
            Ok(auth_info)
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }

    /// 吊销 API Key
    pub async fn revoke_key(&self, key_id: &str) -> Result<bool, String> {
        let _lifecycle = self.key_lifecycle_lock.lock().await;
        // Persist first. A failed durable revocation must never be reported as
        // successful while the key could become active again after restart.
        if let Some(pool) = &self.pool {
            let result =
                sqlx::query("UPDATE api_keys SET revoked = 1 WHERE id = ?1 AND revoked = 0")
                    .bind(key_id)
                    .execute(pool)
                    .await
                    .map_err(|error| format!("failed to persist API key revocation: {error}"))?;
            if result.rows_affected() != 1 {
                return Ok(false);
            }
        } else {
            let found = self
                .keys
                .read()
                .await
                .values()
                .any(|stored| stored.info.id == key_id)
                || self
                    .loaded
                    .read()
                    .await
                    .iter()
                    .any(|stored| stored.info.id == key_id);
            if !found {
                return Ok(false);
            }
        }

        if self.pool.is_some() {
            self.keys
                .write()
                .await
                .retain(|_, stored| stored.info.id != key_id);
            self.loaded
                .write()
                .await
                .retain(|stored| stored.info.id != key_id);
        } else {
            for stored in self.keys.write().await.values_mut() {
                if stored.info.id == key_id {
                    stored.info.revoked = true;
                }
            }
            for stored in self.loaded.write().await.iter_mut() {
                if stored.info.id == key_id {
                    stored.info.revoked = true;
                }
            }
        }
        Ok(true)
    }

    /// Revoke a memory-only session without consulting the durable API Key table.
    pub async fn revoke_ephemeral_key(&self, key_id: &str) -> bool {
        let mut keys = self.keys.write().await;
        let before = keys.len();
        keys.retain(|_, stored| stored.manageable || stored.info.id != key_id);
        keys.len() != before
    }

    /// 永久删除已吊销的 API Key
    ///
    /// 仅允许删除已吊销的 Key，防止误删活跃 Key。
    pub async fn delete_key(&self, key_id: &str) -> Result<bool, String> {
        let _lifecycle = self.key_lifecycle_lock.lock().await;
        if let Some(pool) = &self.pool {
            let deleted = sqlx::query("DELETE FROM api_keys WHERE id = ?1 AND revoked = 1")
                .bind(key_id)
                .execute(pool)
                .await
                .map_err(|error| format!("failed to persist API key deletion: {error}"))?;
            if deleted.rows_affected() != 1 {
                return Ok(false);
            }
            self.keys
                .write()
                .await
                .retain(|_, stored| stored.info.id != key_id);
            self.loaded
                .write()
                .await
                .retain(|stored| stored.info.id != key_id);
            return Ok(true);
        }
        // 检查是否已吊销
        let mut revoked = false;
        {
            let keys = self.keys.read().await;
            if let Some(stored) = keys.values().find(|s| s.info.id == key_id) {
                revoked = stored.info.revoked;
            }
        }
        if !revoked {
            let loaded = self.loaded.read().await;
            if let Some(stored) = loaded.iter().find(|s| s.info.id == key_id) {
                revoked = stored.info.revoked;
            }
        }

        if !revoked {
            return Ok(false); // 不允许删除未吊销的 Key
        }

        // Delete durably before removing the in-memory copy. Otherwise a DB
        // failure could be reported as success and the key would reappear.
        // 从内存中移除
        {
            let mut keys = self.keys.write().await;
            keys.retain(|_, s| s.info.id != key_id);
        }
        {
            let mut loaded = self.loaded.write().await;
            loaded.retain(|s| s.info.id != key_id);
        }

        Ok(true)
    }

    /// 列出所有 API Key（合并内存和 DB 加载的）
    pub async fn list_keys(&self) -> Vec<ApiKeyInfo> {
        self.list_keys_result().await.unwrap_or_default()
    }

    pub async fn list_keys_result(&self) -> Result<Vec<ApiKeyInfo>, String> {
        self.key_infos_page(10_000, 0).await
    }

    pub async fn key_infos_page(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ApiKeyInfo>, String> {
        if let Some(pool) = &self.pool {
            use sqlx::Row as _;
            let rows = sqlx::query(
                "SELECT id, subject_id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, requests_per_minute FROM api_keys ORDER BY created_at DESC, id LIMIT ?1 OFFSET ?2",
            )
            .bind(limit.min(10_000) as i64)
            .bind(offset as i64)
            .fetch_all(pool)
            .await
            .map_err(|error| error.to_string())?;
            let mut all = rows
                .into_iter()
                .map(|row| {
                    Ok(ApiKeyInfo {
                        id: row.get("id"),
                        subject_id: row.get("subject_id"),
                        name: row.get("name"),
                        prefix: row.get("prefix"),
                        created_at: row.get("created_at"),
                        expires_at: row.get("expires_at"),
                        last_used_at: row.get("last_used_at"),
                        revoked: row.get::<i64, _>("revoked") != 0,
                        permissions: serde_json::from_str(&row.get::<String, _>("permissions"))
                            .map_err(|error| error.to_string())?,
                        requests_per_minute: row
                            .get::<Option<i64>, _>("requests_per_minute")
                            .map(|value| value as u32),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let persisted = all
                .iter()
                .map(|key| key.id.clone())
                .collect::<std::collections::HashSet<_>>();
            let keys = self.keys.read().await;
            all.extend(
                keys.values()
                    .filter(|stored| stored.manageable && !persisted.contains(&stored.info.id))
                    .map(|stored| stored.info.clone()),
            );
            return Ok(all);
        }
        let keys = self.keys.read().await;
        let loaded = self.loaded.read().await;

        let mut all: Vec<ApiKeyInfo> = keys
            .values()
            .filter(|stored| stored.manageable)
            .map(|stored| stored.info.clone())
            .collect();
        // 追加 DB 加载的 Key（去重：以 id 为准）
        let seen: std::collections::HashSet<String> = all.iter().map(|k| k.id.clone()).collect();
        for s in loaded.iter().filter(|stored| stored.manageable) {
            if !seen.contains(&s.info.id) {
                all.push(s.info.clone());
            }
        }
        Ok(all)
    }

    pub async fn find_key_info(&self, key_id: &str) -> Result<Option<ApiKeyInfo>, String> {
        if let Some(pool) = &self.pool {
            use sqlx::Row as _;
            let row = sqlx::query(
                "SELECT id, subject_id, name, prefix, created_at, expires_at, last_used_at, revoked, permissions, requests_per_minute FROM api_keys WHERE id = ?1",
            )
            .bind(key_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| error.to_string())?;
            return row
                .map(|row| {
                    Ok(ApiKeyInfo {
                        id: row.get("id"),
                        subject_id: row.get("subject_id"),
                        name: row.get("name"),
                        prefix: row.get("prefix"),
                        created_at: row.get("created_at"),
                        expires_at: row.get("expires_at"),
                        last_used_at: row.get("last_used_at"),
                        revoked: row.get::<i64, _>("revoked") != 0,
                        permissions: serde_json::from_str(&row.get::<String, _>("permissions"))
                            .map_err(|error| error.to_string())?,
                        requests_per_minute: row
                            .get::<Option<i64>, _>("requests_per_minute")
                            .map(|value| value as u32),
                    })
                })
                .transpose();
        }
        Ok(self
            .list_keys()
            .await
            .into_iter()
            .find(|key| key.id == key_id))
    }

    pub async fn active_key_count(&self) -> Result<i64, String> {
        if let Some(pool) = &self.pool {
            return sqlx::query_scalar(
                "SELECT COUNT(*) FROM api_keys WHERE revoked = 0 AND (expires_at IS NULL OR expires_at > ?1)",
            )
            .bind(chrono::Utc::now().timestamp_millis())
            .fetch_one(pool)
            .await
            .map_err(|error| error.to_string());
        }
        let now = chrono::Utc::now().timestamp_millis();
        Ok(self
            .list_keys()
            .await
            .iter()
            .filter(|key| !key.revoked && key.expires_at.is_none_or(|expires| expires > now))
            .count() as i64)
    }
}
