//! 日志查询 API

use crate::AppState;
use axum::{
    Json,
    extract::{Query, State},
};
use serde::Deserialize;

/// 日志查询参数
#[derive(Debug, Deserialize)]
pub struct LogQuery {
    /// 过滤级别: ERROR / WARN / INFO / DEBUG
    pub level: Option<String>,
    /// 文本搜索
    pub search: Option<String>,
    /// 返回条数（默认 100，最大 500）
    pub limit: Option<usize>,
    /// 起始时间戳（Unix 毫秒），用于增量拉取
    pub since: Option<i64>,
}

/// GET /api/v1/logs — 查询日志条目
pub async fn log_entries(
    State(state): State<AppState>,
    Query(params): Query<LogQuery>,
) -> Json<serde_json::Value> {
    let entries = state.log_collector.query(
        params.level.as_deref(),
        params.search.as_deref(),
        params.limit.unwrap_or(100),
        params.since,
    );
    let total = state.log_collector.total();

    Json(serde_json::json!({
        "entries": entries,
        "total": total,
        "truncated": total > 5000,
    }))
}
