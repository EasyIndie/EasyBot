#![allow(unused_qualifications)]
//! 日志查询 API

use crate::{
    AppState,
    response::{ApiError, api_error},
};
use axum::{
    Json,
    extract::{Query, State},
};
use easybot_core::types::error::GatewayError;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

/// 日志查询参数
#[derive(Debug, Deserialize, ToSchema, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LogQuery {
    /// 过滤级别: ERROR / WARN / INFO / DEBUG / TRACE
    pub level: Option<String>,
    /// 文本搜索
    pub search: Option<String>,
    /// 返回条数（默认 100，最大 500）
    pub limit: Option<usize>,
    /// 起始时间戳（Unix 毫秒），用于增量拉取
    pub since: Option<i64>,
}

/// 查询日志条目
#[utoipa::path(
    get,
    path = "/api/v1/logs",
    tag = "Logs",
    params(LogQuery),
    responses(
        (status = 200, description = "日志条目列表", body = serde_json::Value),
        (status = 400, description = "Invalid level, search, or pagination", body = easybot_core::types::error::ApiErrorResponse),
    )
)]
pub async fn log_entries(
    State(state): State<AppState>,
    Query(params): Query<LogQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let limit = params.limit.unwrap_or(100);
    if !(1..=500).contains(&limit) {
        return Err(api_error(GatewayError::InvalidRequest(
            "log limit must be between 1 and 500".into(),
        )));
    }
    if params
        .search
        .as_ref()
        .is_some_and(|search| search.len() > 128)
    {
        return Err(api_error(GatewayError::InvalidRequest(
            "log search must not exceed 128 bytes".into(),
        )));
    }
    if params.level.as_ref().is_some_and(|level| {
        !matches!(
            level.to_ascii_uppercase().as_str(),
            "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE"
        )
    }) {
        return Err(api_error(GatewayError::InvalidRequest(
            "invalid log level".into(),
        )));
    }
    let entries = state.log_collector.query(
        params.level.as_deref(),
        params.search.as_deref(),
        limit,
        params.since,
    );
    let total = state.log_collector.total();

    Ok(Json(serde_json::json!({
        "entries": entries,
        "total": total,
        "truncated": total > 5000,
    })))
}
