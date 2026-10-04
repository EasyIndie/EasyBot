//! 目标授权（subject × platform:chat_id × action）。

use super::*;

impl ApiKeyManager {
    pub async fn create_target_grant(
        &self,
        subject_id: &str,
        platform: &str,
        chat_id: &str,
        actions: Vec<String>,
        created_by: &str,
    ) -> Result<TargetGrant, String> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "target authorization requires durable storage".to_string())?;
        let platform = platform.trim().to_ascii_lowercase();
        let chat_id = chat_id.trim();
        if subject_id.trim().is_empty() || platform.is_empty() || chat_id.is_empty() {
            return Err("subject_id, platform and chat_id are required".into());
        }
        if platform.len() > 64 || chat_id.len() > 255 {
            return Err("platform or chat_id exceeds policy limits".into());
        }
        if actions.is_empty() || actions.len() > target_actions::ALL.len() {
            return Err("at least one target action is required".into());
        }
        let unique: std::collections::HashSet<_> = actions.iter().collect();
        if unique.len() != actions.len()
            || actions
                .iter()
                .any(|action| !target_actions::ALL.contains(&action.as_str()))
        {
            return Err("target actions are invalid or duplicated".into());
        }
        let subject_exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM api_keys WHERE subject_id = ?1")
                .bind(subject_id)
                .fetch_one(pool)
                .await
                .map_err(|error| error.to_string())?;
        if subject_exists == 0 {
            return Err("subject does not exist".into());
        }
        let grant = TargetGrant {
            id: Uuid::now_v7().to_string(),
            subject_id: subject_id.to_string(),
            platform,
            chat_id: chat_id.to_string(),
            actions,
            created_at: chrono::Utc::now().timestamp_millis(),
            created_by: created_by.to_string(),
        };
        let actions_json = serde_json::to_string(&grant.actions).map_err(|e| e.to_string())?;
        sqlx::query(
            "INSERT INTO target_grants(id,subject_id,platform,chat_id,actions,created_at,created_by) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
        )
        .bind(&grant.id)
        .bind(&grant.subject_id)
        .bind(&grant.platform)
        .bind(&grant.chat_id)
        .bind(actions_json)
        .bind(grant.created_at)
        .bind(&grant.created_by)
        .execute(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(grant)
    }

    pub async fn list_target_grants(&self, subject_id: &str) -> Result<Vec<TargetGrant>, String> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "target authorization requires durable storage".to_string())?;
        use sqlx::Row as _;
        let rows = sqlx::query(
            "SELECT id,subject_id,platform,chat_id,actions,created_at,created_by \
             FROM target_grants WHERE subject_id = ?1 ORDER BY platform,chat_id,id",
        )
        .bind(subject_id)
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        rows.into_iter()
            .map(|row| {
                let actions_json: String = row.try_get("actions").map_err(|e| e.to_string())?;
                Ok(TargetGrant {
                    id: row.try_get("id").map_err(|e| e.to_string())?,
                    subject_id: row.try_get("subject_id").map_err(|e| e.to_string())?,
                    platform: row.try_get("platform").map_err(|e| e.to_string())?,
                    chat_id: row.try_get("chat_id").map_err(|e| e.to_string())?,
                    actions: serde_json::from_str(&actions_json).map_err(|e| e.to_string())?,
                    created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
                    created_by: row.try_get("created_by").map_err(|e| e.to_string())?,
                })
            })
            .collect()
    }

    pub async fn delete_target_grant(
        &self,
        subject_id: &str,
        grant_id: &str,
    ) -> Result<bool, String> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "target authorization requires durable storage".to_string())?;
        sqlx::query("DELETE FROM target_grants WHERE id = ?1 AND subject_id = ?2")
            .bind(grant_id)
            .bind(subject_id)
            .execute(pool)
            .await
            .map(|result| result.rows_affected() == 1)
            .map_err(|error| error.to_string())
    }

    pub async fn target_authorized(
        &self,
        subject_id: &str,
        platform: &str,
        chat_id: &str,
        action: &str,
    ) -> Result<bool, String> {
        if !target_actions::ALL.contains(&action) {
            return Err("unknown target action".into());
        }
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| "target authorization requires durable storage".to_string())?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT actions FROM target_grants WHERE subject_id = ?1 \
             AND (platform = ?2 OR platform = '*') AND (chat_id = ?3 OR chat_id = '*')",
        )
        .bind(subject_id)
        .bind(platform.trim().to_ascii_lowercase())
        .bind(chat_id.trim())
        .fetch_all(pool)
        .await
        .map_err(|error| error.to_string())?;
        Ok(rows.into_iter().any(|json| {
            serde_json::from_str::<Vec<String>>(&json)
                .is_ok_and(|actions| actions.iter().any(|candidate| candidate == action))
        }))
    }
}
