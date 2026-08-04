//! 适配器管理路由

use crate::AppState;
use crate::response::ApiError;
use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
};
use easybot_core::types::error::GatewayError;
use serde::Serialize;
use utoipa::ToSchema;

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
    /// 传输层健康状态：Healthy, Degraded, Down
    /// 用于区分"适配器在运行"和"消息流是否正常"
    #[schema(example = "Healthy")]
    pub health: Option<String>,
    pub connected: bool,
    /// 最近一次错误（连接/重连失败原因）；None 表示无错误
    pub last_error: Option<String>,
    /// 是否永久停用（凭据拒绝等，不再自动重试，需人工介入）
    pub permanent_failure: bool,
    /// 连续失败重试次数（瞬时失败自动重试中会递增）
    pub retry_attempt: u32,
    /// 距下次自动重试的毫秒数；None 表示当前无排定的重试
    pub next_retry_in_ms: Option<u64>,
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
pub async fn list_adapters(State(state): State<AppState>) -> Json<AdapterListResponse> {
    let adapters = state.adapter_manager.list_statuses().await;
    let mut items = Vec::with_capacity(adapters.len());
    for s in adapters {
        // 重连/重试状态由健康监测器维护，与适配器自报的状态分离
        let info = state.adapter_manager.get_reconnect_info(&s.platform).await;
        items.push(AdapterItem {
            platform: s.platform,
            display_name: s.display_name,
            status: format!("{:?}", s.state),
            health: s.health.map(|h| format!("{:?}", h)),
            connected: s.connected,
            last_error: s.last_error,
            permanent_failure: info.permanent_failure,
            retry_attempt: info.retry_attempt,
            next_retry_in_ms: info.next_retry_in_ms,
        });
    }

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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(platform): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "adapter.start.requested",
            &format!("adapter:{platform}"),
            serde_json::json!({}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    // 从当前配置中读取该平台的 AdapterConfig（包含 token 等凭证）
    let adapter_config = state
        .config
        .adapters
        .get(&platform)
        .cloned()
        .unwrap_or_else(|| easybot_core::types::adapter::AdapterConfig {
            enabled: Some(true),
            token: None,
            api_key: None,
            base_url: None,
            extra: serde_json::json!({}),
        });
    let response = match state.adapter_manager.start(&platform, adapter_config).await {
        Ok(result) => {
            if let Some(error) = result.error.as_deref() {
                tracing::warn!(%platform, detail = %crate::log_collector::redact_sensitive_text(error), "adapter start failed");
            }
            serde_json::json!({
                "ok": result.ok,
                "pending": result.pending,
                "platform": result.platform,
                "error": result.error.as_ref().map(|_| "Adapter failed to start"),
            })
        }
        Err(e) => {
            tracing::warn!(%platform, detail = %crate::log_collector::redact_sensitive_text(&e.to_string()), "adapter start failed");
            serde_json::json!({
                "ok": false,
                "error": "Adapter failed to start",
            })
        }
    };
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "adapter.start.completed",
            &format!("adapter:{platform}"),
            serde_json::json!({"ok":response["ok"],"pending":response["pending"]}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    Ok(Json(response))
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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(platform): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "adapter.stop.requested",
            &format!("adapter:{platform}"),
            serde_json::json!({}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    let response = match state.adapter_manager.stop(&platform).await {
        Ok(()) => serde_json::json!({ "ok": true, "platform": platform }),
        Err(e) => {
            tracing::warn!(%platform, detail = %crate::log_collector::redact_sensitive_text(&e.to_string()), "adapter stop failed");
            serde_json::json!({ "ok": false, "error": "Adapter failed to stop" })
        }
    };
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "adapter.stop.completed",
            &format!("adapter:{platform}"),
            serde_json::json!({"ok":response["ok"]}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    Ok(Json(response))
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
) -> (StatusCode, Json<serde_json::Value>) {
    // 使用新的 O(1) get_status 方法，绕过 Vec 遍历
    match state.adapter_manager.get_status(&platform).await {
        Some(s) => (
            StatusCode::OK,
            Json(serde_json::to_value(s).unwrap_or_default()),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "PLATFORM_NOT_FOUND",
                "message": format!("Platform '{}' not found", platform),
            })),
        ),
    }
}
