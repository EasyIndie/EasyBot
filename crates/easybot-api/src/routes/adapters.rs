//! 适配器管理路由

use axum::{
    Json,
    extract::{State, Path},
};
use serde::Serialize;
use utoipa::ToSchema;
use crate::AppState;

/// 适配器列表响应
#[derive(Serialize, ToSchema)]
pub struct AdapterListResponse {
    pub adapters: Vec<AdapterItem>,
}

/// 适配器列表项
#[derive(Serialize, ToSchema)]
pub struct AdapterItem {
    #[schema(example = "telegram")]
    pub platform: String,
    #[schema(example = "Telegram")]
    pub display_name: String,
    #[schema(example = "Connected")]
    pub status: String,
    pub connected: bool,
}

/// 获取适配器列表
#[utoipa::path(
    get,
    path = "/api/v1/adapters",
    tag = "Adapters",
    responses(
        (status = 200, description = "List of all registered adapters", body = AdapterListResponse)
    )
)]
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

/// 启动适配器
#[utoipa::path(
    post,
    path = "/api/v1/adapters/{platform}/start",
    tag = "Adapters",
    params(
        ("platform" = String, Path, description = "Platform identifier, e.g. 'telegram'")
    ),
    responses(
        (status = 200, description = "Start result", body = serde_json::Value),
        (status = 404, description = "Platform not found"),
    )
)]
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

/// 停止适配器
#[utoipa::path(
    post,
    path = "/api/v1/adapters/{platform}/stop",
    tag = "Adapters",
    params(
        ("platform" = String, Path, description = "Platform identifier")
    ),
    responses(
        (status = 200, description = "Stop result", body = serde_json::Value)
    )
)]
pub async fn stop_adapter(
    State(state): State<AppState>,
    Path(platform): Path<String>,
) -> Json<serde_json::Value> {
    match state.adapter_manager.stop(&platform).await {
        Ok(()) => Json(serde_json::json!({ "ok": true, "platform": platform })),
        Err(e) => Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    }
}

/// 获取适配器详细状态
#[utoipa::path(
    get,
    path = "/api/v1/adapters/{platform}/status",
    tag = "Adapters",
    params(
        ("platform" = String, Path, description = "Platform identifier")
    ),
    responses(
        (status = 200, description = "Adapter status details", body = serde_json::Value),
        (status = 404, description = "Platform not found"),
    )
)]
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
