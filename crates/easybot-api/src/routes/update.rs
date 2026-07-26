//! 版本更新检查 API
//!
//! 提供 `GET /api/v1/system/update-check` 端点，
//! 返回当前版本和最新可用版本信息。

use crate::AppState;
use axum::{Json, extract::State};
use serde::Serialize;
use utoipa::ToSchema;

/// 更新检查响应
#[derive(Debug, Serialize, ToSchema)]
pub struct UpdateCheckResponse {
    #[schema(example = "0.0.16")]
    pub current_version: String,
    /// 数据库 schema 版本
    pub schema_version: i64,
    /// 最新可用版本（来自 GitHub API，可能为 None 如果检查失败）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    /// 最新版本对应的 schema 版本
    pub latest_schema_version: Option<i64>,
    /// 是否有可用更新
    pub update_available: bool,
    /// 是否需要数据库迁移
    pub requires_db_migration: bool,
    /// 破坏性变更列表
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub breaking_changes: Vec<String>,
    /// 上次检查时间戳（毫秒）
    pub last_checked: Option<i64>,
}

/// 检查版本更新
///
/// 返回当前版本号和最新可用版本信息（从 GitHub Releases API 获取）。
/// 用于管理后台显示更新提示和自动化运维。
#[utoipa::path(
    get,
    path = "/api/v1/system/update-check",
    tag = "System",
    responses(
        (status = 200, description = "Update availability check result", body = UpdateCheckResponse),
    )
)]
pub async fn update_check(State(_state): State<AppState>) -> Json<UpdateCheckResponse> {
    // GitHub API 调用可能失败（离线/速率限制），降级返回仅版本信息
    let home =
        std::path::PathBuf::from(std::env::var("EASYBOT_HOME").unwrap_or_else(|_| ".".to_string()));
    let mut updater = easybot_core::updater::Updater::new(home);

    match updater.check_update().await {
        Ok(plan) => Json(UpdateCheckResponse {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: easybot_core::storage::migration::SCHEMA_VERSION,
            latest_version: Some(plan.target_version),
            latest_schema_version: Some(plan.target_schema_version),
            update_available: true,
            requires_db_migration: plan.requires_db_migration,
            breaking_changes: plan.breaking_changes,
            last_checked: Some(chrono::Utc::now().timestamp_millis()),
        }),
        Err(e) => match &e {
            easybot_core::updater::types::UpdateError::AlreadyUpToDate(v) => {
                Json(UpdateCheckResponse {
                    current_version: env!("CARGO_PKG_VERSION").to_string(),
                    schema_version: easybot_core::storage::migration::SCHEMA_VERSION,
                    latest_version: Some(v.clone()),
                    latest_schema_version: None,
                    update_available: false,
                    requires_db_migration: false,
                    breaking_changes: vec![],
                    last_checked: Some(chrono::Utc::now().timestamp_millis()),
                })
            }
            _ => Json(UpdateCheckResponse {
                current_version: env!("CARGO_PKG_VERSION").to_string(),
                schema_version: easybot_core::storage::migration::SCHEMA_VERSION,
                latest_version: None,
                latest_schema_version: None,
                update_available: false,
                requires_db_migration: false,
                breaking_changes: vec![],
                last_checked: None,
            }),
        },
    }
}
