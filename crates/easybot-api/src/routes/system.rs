//! 系统信息 API

use crate::AppState;
use axum::{Json, extract::State};
use easybot_core::system::collect_system_info;

/// GET /api/v1/system — 获取服务器软硬件信息及性能负载
pub async fn system_info(State(_state): State<AppState>) -> Json<serde_json::Value> {
    let info = collect_system_info();
    Json(serde_json::to_value(info).unwrap_or_default())
}
