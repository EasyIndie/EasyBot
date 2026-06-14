//! 适配器管理路由

use axum::{
    Json,
    extract::{State, Path},
};
use serde::Serialize;
use crate::AppState;

/// 适配器列表响应
#[derive(Serialize)]
pub struct AdapterListResponse {
    pub adapters: Vec<AdapterItem>,
}

#[derive(Serialize)]
pub struct AdapterItem {
    pub platform: String,
    pub display_name: String,
    pub status: String,
    pub connected: bool,
}

/// GET /api/v1/adapters
pub async fn list_adapters(
    State(state): State<AppState>,
) -> Json<AdapterListResponse> {
    let adapters = state.adapter_manager.list_statuses().await;
    let items: Vec<AdapterItem> = adapters
        .into_iter()
        .map(|s| AdapterItem {
            platform: s.platform,
            display_name: s.display_name,
            status: format!("{:?}", s.state),
            connected: s.connected,
        })
        .collect();

    Json(AdapterListResponse { adapters: items })
}

/// POST /api/v1/adapters/{platform}/start
pub async fn start_adapter(
    State(state): State<AppState>,
    Path(platform): Path<String>,
) -> Json<serde_json::Value> {
    // Phase 1: 简化处理，实际需要从配置读取 AdapterConfig
    match state.adapter_manager.start(&platform, easybot_core::types::adapter::AdapterConfig {
        enabled: true,
        token: None,
        api_key: None,
        extra: serde_json::json!({}),
    }).await {
        Ok(result) => Json(serde_json::json!({
            "ok": result.ok,
            "platform": result.platform,
            "error": result.error,
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": e.to_string(),
        })),
    }
}

/// POST /api/v1/adapters/{platform}/stop
pub async fn stop_adapter(
    State(state): State<AppState>,
    Path(platform): Path<String>,
) -> Json<serde_json::Value> {
    match state.adapter_manager.stop(&platform).await {
        Ok(()) => Json(serde_json::json!({ "ok": true, "platform": platform })),
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}

/// GET /api/v1/adapters/{platform}/status
pub async fn adapter_status(
    State(state): State<AppState>,
    Path(platform): Path<String>,
) -> Json<serde_json::Value> {
    let statuses = state.adapter_manager.list_statuses().await;
    for s in statuses {
        if s.platform == platform {
            return Json(serde_json::to_value(s).unwrap_or_default());
        }
    }
    Json(serde_json::json!({
        "error": "PLATFORM_NOT_FOUND",
        "message": format!("Platform '{}' not found", platform),
    }))
}
