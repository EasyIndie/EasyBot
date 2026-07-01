//! 管理后台路由

use crate::AppState;
use crate::response::ApiError;
use axum::{
    Json,
    extract::{Path, State},
    response::Html,
};
use easybot_core::types::error::GatewayError;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// GET /admin — 管理后台 SPA
pub async fn admin_page() -> Html<&'static str> {
    Html(include_str!("../../templates/gen/admin.html"))
}

/// 登录请求体
#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

/// 登录成功响应
#[derive(Serialize)]
pub struct LoginResponse {
    pub key: String,
}

/// POST /admin/login — 密码登录，返回 API Key
pub async fn admin_login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    if body.password != state.admin_password {
        return Err(ApiError(GatewayError::Unauthorized("密码错误".into())));
    }
    match &state.dev_api_key {
        Some(key) => Ok(Json(LoginResponse { key: key.clone() })),
        None => Err(ApiError(GatewayError::Internal("API Key 未就绪".into()))),
    }
}

// ─── API Key 管理 ──────────────────────────────────────────

/// API Key 列表响应项（不含 raw key）
#[derive(Serialize, ToSchema)]
pub struct ApiKeyResponse {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked: bool,
    pub permissions: Vec<String>,
    pub event_filters: Vec<String>,
}

/// 创建 API Key 请求体
#[derive(Deserialize, ToSchema)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub permissions: Vec<String>,
    #[serde(default)]
    pub event_filters: Vec<String>,
}

/// 创建 API Key 响应（含 raw key，仅返回一次）
#[derive(Serialize, ToSchema)]
pub struct CreateApiKeyResponse {
    pub id: String,
    pub key: String,
    pub name: String,
    pub permissions: Vec<String>,
    pub event_filters: Vec<String>,
    pub created_at: i64,
}

/// 吊销/删除响应
#[derive(Serialize, ToSchema)]
pub struct RevokeResponse {
    pub success: bool,
    pub message: String,
}

/// 可用事件类型和权限列表
#[derive(Serialize, ToSchema)]
pub struct ApiKeyTypesResponse {
    pub event_types: Vec<&'static str>,
    pub permissions: Vec<&'static str>,
}

/// GET /api/v1/api-keys — 列出所有 Key
#[utoipa::path(
    get,
    path = "/api/v1/api-keys",
    tag = "API Keys",
    responses(
        (status = 200, description = "API Key 列表", body = [ApiKeyResponse])
    )
)]
pub async fn list_api_keys(State(state): State<AppState>) -> Json<Vec<ApiKeyResponse>> {
    let keys = state.auth_manager.list_keys().await;
    Json(
        keys.into_iter()
            .map(|k| ApiKeyResponse {
                id: k.id,
                name: k.name,
                prefix: k.prefix,
                created_at: k.created_at,
                expires_at: k.expires_at,
                last_used_at: k.last_used_at,
                revoked: k.revoked,
                permissions: k.permissions,
                event_filters: k.event_filters,
            })
            .collect(),
    )
}

/// POST /api/v1/api-keys — 创建 Key
#[utoipa::path(
    post,
    path = "/api/v1/api-keys",
    tag = "API Keys",
    request_body = CreateApiKeyRequest,
    responses(
        (status = 200, description = "创建成功，返回 raw key（仅此一次）", body = CreateApiKeyResponse),
        (status = 400, description = "请求参数无效")
    )
)]
pub async fn create_api_key(
    State(state): State<AppState>,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<CreateApiKeyResponse>, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError(GatewayError::InvalidRequest(
            "name 不能为空".into(),
        )));
    }

    let (id, raw_key) = state
        .auth_manager
        .create_key(body.name.trim(), body.permissions, None, body.event_filters)
        .await
        .map_err(|e| ApiError(GatewayError::InvalidRequest(e)))?;

    // 从 list_keys 获取完整信息（包含 created_at 等）
    let keys = state.auth_manager.list_keys().await;
    let info = keys
        .iter()
        .find(|k| k.id == id)
        .ok_or_else(|| ApiError(GatewayError::Internal("Key 创建后未找到".into())))?;

    Ok(Json(CreateApiKeyResponse {
        id,
        key: raw_key,
        name: info.name.clone(),
        permissions: info.permissions.clone(),
        event_filters: info.event_filters.clone(),
        created_at: info.created_at,
    }))
}

/// DELETE /api/v1/api-keys/{id} — 吊销 Key
#[utoipa::path(
    delete,
    path = "/api/v1/api-keys/{id}",
    tag = "API Keys",
    params(
        ("id" = String, Path, description = "API Key ID")
    ),
    responses(
        (status = 200, description = "吊销成功", body = RevokeResponse),
        (status = 400, description = "Key 未找到")
    )
)]
pub async fn revoke_api_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RevokeResponse>, ApiError> {
    if state.auth_manager.revoke_key(&id).await {
        Ok(Json(RevokeResponse {
            success: true,
            message: "API Key 已吊销".into(),
        }))
    } else {
        Err(ApiError(GatewayError::InvalidRequest(
            "API Key 未找到".into(),
        )))
    }
}

/// DELETE /api/v1/api-keys/{id}/purge — 永久删除已吊销的 Key
#[utoipa::path(
    delete,
    path = "/api/v1/api-keys/{id}/purge",
    tag = "API Keys",
    params(
        ("id" = String, Path, description = "API Key ID")
    ),
    responses(
        (status = 200, description = "永久删除成功", body = RevokeResponse),
        (status = 400, description = "Key 未吊销或不存在")
    )
)]
pub async fn purge_api_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RevokeResponse>, ApiError> {
    if state.auth_manager.delete_key(&id).await {
        Ok(Json(RevokeResponse {
            success: true,
            message: "API Key 已永久删除".into(),
        }))
    } else {
        Err(ApiError(GatewayError::InvalidRequest(
            "只能删除已吊销的 Key，且 Key 必须存在".into(),
        )))
    }
}

/// GET /api/v1/api-keys/types — 获取可用事件类型和权限列表
#[utoipa::path(
    get,
    path = "/api/v1/api-keys/types",
    tag = "API Keys",
    responses(
        (status = 200, description = "可用事件类型和权限列表", body = ApiKeyTypesResponse)
    )
)]
pub async fn list_api_key_types() -> Json<ApiKeyTypesResponse> {
    Json(ApiKeyTypesResponse {
        event_types: easybot_core::types::event::event_types::all().to_vec(),
        permissions: vec![
            "*",
            "messagesread",
            "messagessend",
            "adaptersread",
            "adaptersmanage",
            "configread",
            "configwrite",
            "sessionsread",
            "sessionsmanage",
            "websocketconnect",
            "apikeysmanage",
        ],
    })
}
