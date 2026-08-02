//! 管理后台路由

use crate::AppState;
use crate::response::ApiError;
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
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
#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    /// 管理后台密码
    #[schema(example = "your-password")]
    pub password: String,
}

/// 登录成功响应
#[derive(Serialize, ToSchema)]
pub struct LoginResponse {
    /// 用于 API 认证的 Bearer token
    pub key: String,
}

/// POST /admin/login — 密码登录，返回短期管理 Session
///
/// SECURITY: Uses constant-time comparison to prevent timing side-channel attacks.
/// Rate limiting is handled by the dedicated admin login rate limiter in server.rs.
#[utoipa::path(
    post,
    path = "/admin/login",
    tag = "Admin",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "登录成功，返回短期管理 Session", body = LoginResponse),
        (status = 400, description = "密码字段为空或超过 1024 字节", body = easybot_core::types::error::ApiErrorResponse),
        (status = 401, description = "密码错误或未配置"),
        (status = 413, description = "请求体超过 4 KiB"),
    )
)]
pub async fn admin_login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    if body.password.is_empty() || body.password.len() > 1_024 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "password must be 1-1024 bytes".into(),
        )));
    }
    // SECURITY: Reject empty/default password (admin panel disabled)
    if state.admin_password.is_empty() {
        return Err(ApiError(GatewayError::Unauthorized(
            "管理后台未配置密码".into(),
        )));
    }

    // SECURITY: Constant-time comparison to prevent timing attacks
    if !constant_time_eq(body.password.as_bytes(), state.admin_password.as_bytes()) {
        tracing::warn!("AUDIT: Admin login failed (incorrect password)");
        if let Err(error) = state
            .auth_manager
            .record_audit(
                "anonymous",
                "admin.login.failed",
                "admin",
                serde_json::json!({"reason":"invalid_credentials"}),
            )
            .await
        {
            tracing::error!(%error, "failed to persist rejected admin login audit event");
        }
        return Err(ApiError(GatewayError::Unauthorized("密码错误".into())));
    }
    tracing::info!("AUDIT: Admin login successful");
    if let Err(error) = state
        .auth_manager
        .record_audit(
            "admin",
            "admin.login.succeeded",
            "admin",
            serde_json::json!({}),
        )
        .await
    {
        return Err(ApiError(GatewayError::Internal(format!(
            "failed to audit admin login: {error}"
        ))));
    }
    // Management sessions are deliberately independent from every durable API
    // key. This keeps the admin panel available while a bootstrap key rotates.
    let expires_at = chrono::Utc::now().timestamp_millis() + 60 * 60 * 1_000;
    let (session_id, key) = state
        .auth_manager
        .create_ephemeral_key("admin-session", vec!["*".into()], expires_at)
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    if let Err(error) = state
        .auth_manager
        .record_audit(
            "admin",
            "admin.session.issued",
            &format!("api_key:{session_id}"),
            serde_json::json!({"expires_at": expires_at}),
        )
        .await
    {
        let _ = state.auth_manager.revoke_key(&session_id).await;
        return Err(ApiError(GatewayError::Internal(format!(
            "failed to audit admin session issuance: {error}"
        ))));
    }
    Ok(Json(LoginResponse { key }))
}

/// POST /api/v1/admin/logout — revoke the current memory-only management Session.
#[utoipa::path(
    post,
    path = "/api/v1/admin/logout",
    tag = "Admin",
    responses(
        (status = 200, description = "管理 Session 已注销", body = RevokeResponse),
        (status = 400, description = "当前凭据不是管理 Session")
    )
)]
pub async fn admin_logout(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
) -> Result<Json<RevokeResponse>, ApiError> {
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "admin.session.revoked",
            &format!("api_key:{}", actor.id),
            serde_json::json!({}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    if !state.auth_manager.revoke_ephemeral_key(&actor.id).await {
        return Err(ApiError(GatewayError::InvalidRequest(
            "current credential is not an ephemeral management session".into(),
        )));
    }
    Ok(Json(RevokeResponse {
        success: true,
        message: "管理 Session 已注销".into(),
    }))
}

/// Constant-time byte comparison to prevent timing side-channel attacks.
/// Both slices must have the same length for the comparison to be meaningful.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

// ─── API Key 管理 ──────────────────────────────────────────

/// API Key 列表响应项（不含 raw key）
#[derive(Serialize, ToSchema)]
pub struct ApiKeyResponse {
    pub id: String,
    pub subject_id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked: bool,
    pub permissions: Vec<String>,
    pub requests_per_minute: Option<u32>,
}

#[derive(Deserialize, ToSchema)]
pub struct ApiKeyListQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// 创建 API Key 请求体
#[derive(Deserialize, ToSchema)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub permissions: Vec<String>,
    /// Optional per-subject quota. Rotated credentials share the same window.
    pub requests_per_minute: Option<u32>,
    /// UTC Unix timestamp in milliseconds. Commercial keys should expire.
    pub expires_at: Option<i64>,
}

/// 创建 API Key 响应（含 raw key，仅返回一次）
#[derive(Serialize, ToSchema)]
pub struct CreateApiKeyResponse {
    pub id: String,
    pub subject_id: String,
    pub key: String,
    pub name: String,
    pub permissions: Vec<String>,
    pub requests_per_minute: Option<u32>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateTargetGrantRequest {
    pub platform: String,
    pub chat_id: String,
    pub actions: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DeleteTargetGrantResponse {
    pub deleted: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/subjects/{subject_id}/target-grants",
    tag = "Target Grants",
    params(("subject_id" = String, Path)),
    responses((status = 200, body = [easybot_core::auth::TargetGrant]))
)]
pub async fn list_target_grants(
    State(state): State<AppState>,
    Path(subject_id): Path<String>,
) -> Result<Json<Vec<easybot_core::auth::TargetGrant>>, ApiError> {
    state
        .auth_manager
        .list_target_grants(&subject_id)
        .await
        .map(Json)
        .map_err(|error| ApiError(GatewayError::StorageError(error)))
}

#[utoipa::path(
    post,
    path = "/api/v1/subjects/{subject_id}/target-grants",
    tag = "Target Grants",
    params(("subject_id" = String, Path)),
    request_body = CreateTargetGrantRequest,
    responses((status = 200, body = easybot_core::auth::TargetGrant))
)]
pub async fn create_target_grant(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(subject_id): Path<String>,
    Json(body): Json<CreateTargetGrantRequest>,
) -> Result<Json<easybot_core::auth::TargetGrant>, ApiError> {
    let grant = state
        .auth_manager
        .create_target_grant(
            &subject_id,
            &body.platform,
            &body.chat_id,
            body.actions,
            &actor.subject_id,
        )
        .await
        .map_err(|error| ApiError(GatewayError::InvalidRequest(error)))?;
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "target_grant.created",
            &format!("target_grant:{}", grant.id),
            serde_json::json!({
                "subject_id": grant.subject_id,
                "platform": grant.platform,
                "chat_id": grant.chat_id,
                "actions": grant.actions,
            }),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    Ok(Json(grant))
}

#[utoipa::path(
    delete,
    path = "/api/v1/subjects/{subject_id}/target-grants/{grant_id}",
    tag = "Target Grants",
    params(("subject_id" = String, Path), ("grant_id" = String, Path)),
    responses((status = 200, body = DeleteTargetGrantResponse))
)]
pub async fn delete_target_grant(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path((subject_id, grant_id)): Path<(String, String)>,
) -> Result<Json<DeleteTargetGrantResponse>, ApiError> {
    let deleted = state
        .auth_manager
        .delete_target_grant(&subject_id, &grant_id)
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?;
    if deleted {
        state
            .auth_manager
            .record_audit(
                &actor.id,
                "target_grant.deleted",
                &format!("target_grant:{grant_id}"),
                serde_json::json!({"subject_id": subject_id}),
            )
            .await
            .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    }
    Ok(Json(DeleteTargetGrantResponse { deleted }))
}

#[derive(Deserialize, ToSchema)]
pub struct RotateApiKeyRequest {
    /// UTC Unix timestamp in milliseconds for the replacement key.
    pub expires_at: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct ReconcileRotationRequest {
    /// `cancel` for a created transition; `complete` for a prepared transition.
    pub action: String,
}

#[derive(Serialize, ToSchema)]
pub struct RotationReconcileResponse {
    pub success: bool,
    pub action: String,
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

#[derive(Deserialize, ToSchema)]
pub struct UsageQuery {
    pub from: i64,
    pub to: i64,
    pub key_id: Option<String>,
    pub subject_id: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub struct UsageResponse {
    pub from: i64,
    pub to: i64,
    pub records: Vec<easybot_core::auth::UsageRecord>,
    /// Sum across the entire filtered time range, not only this page.
    pub total_requests: i64,
    pub page_requests: i64,
    pub truncated: bool,
    pub next_offset: Option<usize>,
}

/// GET /api/v1/usage — export durable hourly usage for invoice reconciliation.
#[utoipa::path(
    get,
    path = "/api/v1/usage",
    tag = "Billing",
    params(
        ("from" = i64, Query, description = "Inclusive UTC Unix milliseconds"),
        ("to" = i64, Query, description = "Exclusive UTC Unix milliseconds"),
        ("key_id" = Option<String>, Query, description = "Optional internal API key ID"),
        ("subject_id" = Option<String>, Query, description = "Optional stable caller subject ID"),
        ("limit" = Option<usize>, Query, description = "Page size, 1-1000; default 500"),
        ("offset" = Option<usize>, Query, description = "Stable row offset")
    ),
    responses(
        (status = 200, description = "Hourly usage records", body = UsageResponse),
        (status = 400, description = "Invalid or excessive range")
    )
)]
pub async fn list_usage(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<UsageResponse>, ApiError> {
    const MAX_RANGE_MS: i64 = 366 * 24 * 60 * 60 * 1_000;
    if query.from < 0 || query.to <= query.from || query.to - query.from > MAX_RANGE_MS {
        return Err(ApiError(GatewayError::InvalidRequest(
            "usage range must be positive, ordered, and no longer than 366 days".into(),
        )));
    }
    let limit = query.limit.unwrap_or(500);
    let offset = query.offset.unwrap_or(0);
    if !(1..=1_000).contains(&limit) || offset > 10_000_000 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "usage pagination requires limit 1-1000 and offset <= 10000000".into(),
        )));
    }
    if query.key_id.is_some() && query.subject_id.is_some() {
        return Err(ApiError(GatewayError::InvalidRequest(
            "key_id and subject_id are mutually exclusive".into(),
        )));
    }
    if query
        .key_id
        .as_ref()
        .or(query.subject_id.as_ref())
        .is_some_and(|id| {
            id.len() > 64
                || !id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
    {
        return Err(ApiError(GatewayError::InvalidRequest(
            "invalid key_id".into(),
        )));
    }
    if !state
        .auth_manager
        .verify_usage_ledger_integrity()
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?
    {
        state
            .auth_manager
            .record_audit(
                &actor.id,
                "billing.usage.integrity_failed",
                "usage-ledger",
                serde_json::json!({"scope":"full_ledger"}),
            )
            .await
            .map_err(|error| ApiError(GatewayError::Internal(error)))?;
        return Err(ApiError(GatewayError::StorageError(
            "usage ledger integrity validation failed".into(),
        )));
    }
    let records = state
        .auth_manager
        .usage_records(
            query.from,
            query.to,
            query.key_id.as_deref(),
            query.subject_id.as_deref(),
            limit + 1,
            offset,
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    let truncated = records.len() > limit;
    let records: Vec<_> = records.into_iter().take(limit).collect();
    let page_requests = records.iter().map(|record| record.request_count).sum();
    let total_requests = state
        .auth_manager
        .usage_total(
            query.from,
            query.to,
            query.key_id.as_deref(),
            query.subject_id.as_deref(),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    let next_offset = truncated.then_some(offset.saturating_add(limit));
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "billing.usage.exported",
            "usage-ledger",
            serde_json::json!({
                "from": query.from,
                "to": query.to,
                "key_id": query.key_id,
                "subject_id": query.subject_id,
                "record_count": records.len(),
                "offset": offset,
                "limit": limit,
                "truncated": truncated,
                "total_requests": total_requests,
            }),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    Ok(Json(UsageResponse {
        from: query.from,
        to: query.to,
        records,
        total_requests,
        page_requests,
        truncated,
        next_offset,
    }))
}

/// GET /api/v1/api-keys — 列出所有 Key
#[utoipa::path(
    get,
    path = "/api/v1/api-keys",
    tag = "API Keys",
    params(
        ("limit" = Option<usize>, Query, description = "Page size, 1-1000; default 100"),
        ("offset" = Option<usize>, Query, description = "Stable history offset, max 10000000")
    ),
    responses(
        (status = 200, description = "API Key 列表", body = [ApiKeyResponse])
    )
)]
pub async fn list_api_keys(
    State(state): State<AppState>,
    Query(query): Query<ApiKeyListQuery>,
) -> Result<Json<Vec<ApiKeyResponse>>, ApiError> {
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    if !(1..=1_000).contains(&limit) || offset > 10_000_000 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "API Key pagination requires limit 1-1000 and offset <= 10000000".into(),
        )));
    }
    let keys = state
        .auth_manager
        .key_infos_page(limit, offset)
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?;
    Ok(Json(
        keys.into_iter()
            .map(|k| ApiKeyResponse {
                id: k.id,
                subject_id: k.subject_id,
                name: k.name,
                prefix: k.prefix,
                created_at: k.created_at,
                expires_at: k.expires_at,
                last_used_at: k.last_used_at,
                revoked: k.revoked,
                permissions: k.permissions,
                requests_per_minute: k.requests_per_minute,
            })
            .collect(),
    ))
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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Json(body): Json<CreateApiKeyRequest>,
) -> Result<Json<CreateApiKeyResponse>, ApiError> {
    // Validate name
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError(GatewayError::InvalidRequest(
            "name 不能为空".into(),
        )));
    }
    if name.len() > 128 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "name 不能超过 128 个字符".into(),
        )));
    }
    if name == "admin-session" {
        return Err(ApiError(GatewayError::InvalidRequest(
            "API Key name is reserved for the internal management session".into(),
        )));
    }
    // Only allow alphanumeric, hyphens, underscores, and spaces
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == ' ')
    {
        return Err(ApiError(GatewayError::InvalidRequest(
            "name 只能包含字母、数字、横线、下划线和空格".into(),
        )));
    }

    // SECURITY: Validate permissions against allowlist
    if body.permissions.len() > 32 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "permissions 数量超过上限".into(),
        )));
    }
    let unique_permissions: std::collections::HashSet<_> = body.permissions.iter().collect();
    if unique_permissions.len() != body.permissions.len() {
        return Err(ApiError(GatewayError::InvalidRequest(
            "permissions 不得包含重复项".into(),
        )));
    }
    let valid_permissions: Vec<&str> = vec![
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
        "metricsread",
        "auditread",
        "billingread",
        "billingwrite",
        "logsread",
        "systemread",
    ];
    for perm in &body.permissions {
        if perm == "*" {
            continue; // wildcard is always valid
        }
        if !valid_permissions.contains(&perm.as_str()) {
            return Err(ApiError(GatewayError::InvalidRequest(format!(
                "无效的权限: {}",
                perm
            ))));
        }
    }

    // SECURITY: Limit maximum number of API keys
    let active_key_count = state
        .auth_manager
        .active_key_count()
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?;
    if active_key_count >= 100 {
        return Err(ApiError(GatewayError::InvalidRequest(
            "API Key 数量已达到上限 (100)".into(),
        )));
    }
    if body
        .requests_per_minute
        .is_some_and(|limit| limit == 0 || limit > 1_000_000)
    {
        return Err(ApiError(GatewayError::InvalidRequest(
            "requests_per_minute 必须在 1 到 1000000 之间".into(),
        )));
    }
    validate_key_expiry(body.expires_at)?;

    let quota = body.requests_per_minute;
    let expires_at = body.expires_at;
    let (id, raw_key) = state
        .auth_manager
        .create_key_with_quota(&name, body.permissions, expires_at, quota)
        .await
        .map_err(|e| ApiError(GatewayError::InvalidRequest(e)))?;

    tracing::info!("AUDIT: API key created — id={}, name={}", id, name);
    if let Err(error) = state
        .auth_manager
        .record_audit(
            &actor.id,
            "api_key.created",
            &format!("api_key:{id}"),
            serde_json::json!({"name":name,"requests_per_minute":quota,"expires_at":expires_at}),
        )
        .await
    {
        let _ = state.auth_manager.revoke_key(&id).await;
        return Err(ApiError(GatewayError::Internal(format!(
            "failed to audit API key creation; new key was revoked: {error}"
        ))));
    }
    let info = state
        .auth_manager
        .find_key_info(&id)
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?
        .ok_or_else(|| ApiError(GatewayError::Internal("Key 创建后未找到".into())))?;

    Ok(Json(CreateApiKeyResponse {
        id,
        subject_id: info.subject_id,
        key: raw_key,
        name: info.name,
        permissions: info.permissions,
        requests_per_minute: info.requests_per_minute,
        created_at: info.created_at,
        expires_at: info.expires_at,
    }))
}

fn validate_key_expiry(expires_at: Option<i64>) -> Result<(), ApiError> {
    let Some(expires_at) = expires_at else {
        return Ok(());
    };
    let now = chrono::Utc::now().timestamp_millis();
    const MIN_LIFETIME_MS: i64 = 5 * 60 * 1_000;
    const MAX_LIFETIME_MS: i64 = 366 * 24 * 60 * 60 * 1_000;
    if expires_at < now + MIN_LIFETIME_MS || expires_at > now + MAX_LIFETIME_MS {
        return Err(ApiError(GatewayError::InvalidRequest(
            "expires_at must be between 5 minutes and 366 days from now".into(),
        )));
    }
    Ok(())
}

/// Replace a key while preserving its subject, permissions and quota.
#[utoipa::path(
    post,
    path = "/api/v1/api-keys/{id}/rotate",
    tag = "API Keys",
    params(("id" = String, Path)),
    request_body = RotateApiKeyRequest,
    responses(
        (status = 200, body = CreateApiKeyResponse),
        (status = 400, description = "Invalid expiry or key state"),
        (status = 404, description = "Key not found")
    )
)]
pub async fn rotate_api_key(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(id): Path<String>,
    Json(body): Json<RotateApiKeyRequest>,
) -> Result<Json<CreateApiKeyResponse>, ApiError> {
    validate_key_expiry(Some(body.expires_at))?;
    if actor.id == id {
        return Err(ApiError(GatewayError::InvalidRequest(
            "cannot rotate the key used to authorize this request".into(),
        )));
    }
    let source = state
        .auth_manager
        .find_key_info(&id)
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?
        .ok_or_else(|| ApiError(GatewayError::InvalidRequest("API Key 未找到".into())))?;
    if source.revoked {
        return Err(ApiError(GatewayError::InvalidRequest(
            "cannot rotate a revoked API Key".into(),
        )));
    }
    let (replacement_id, raw_key) = state
        .auth_manager
        .create_rotated_key(&source, body.expires_at)
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    // The durable prepared record is the audit gate. If it cannot be written,
    // the original key is still active and the replacement can be safely revoked.
    if let Err(error) = state
        .auth_manager
        .record_audit(
            &actor.id,
            "api_key.rotation.prepared",
            &format!("api_key:{id}"),
            serde_json::json!({
                "replacement_id": replacement_id,
                "expires_at": body.expires_at
            }),
        )
        .await
    {
        if let Err(cleanup_error) = state.auth_manager.revoke_key(&replacement_id).await {
            tracing::error!(%cleanup_error, replacement_id, "failed to revoke replacement after rotation audit gate failed");
        }
        if let Err(cleanup_error) = state
            .auth_manager
            .clear_rotation_transition(&id, &replacement_id)
            .await
        {
            tracing::error!(%cleanup_error, replacement_id, "failed to clear cancelled rotation transition");
        }
        return Err(ApiError(GatewayError::Internal(format!(
            "failed to prepare API key rotation audit; original key remains active: {error}"
        ))));
    }
    if let Err(error) = state
        .auth_manager
        .mark_rotation_prepared(&id, &replacement_id)
        .await
    {
        let _ = state.auth_manager.revoke_key(&replacement_id).await;
        let _ = state
            .auth_manager
            .clear_rotation_transition(&id, &replacement_id)
            .await;
        return Err(ApiError(GatewayError::Internal(format!(
            "failed to persist prepared rotation state; original key remains active: {error}"
        ))));
    }
    let original_revoked = match state.auth_manager.revoke_key(&id).await {
        Ok(revoked) => revoked,
        Err(error) => {
            let _ = state.auth_manager.revoke_key(&replacement_id).await;
            let _ = state
                .auth_manager
                .clear_rotation_transition(&id, &replacement_id)
                .await;
            return Err(ApiError(GatewayError::Internal(format!(
                "failed to revoke original API Key; rotation requires reconciliation: {error}"
            ))));
        }
    };
    if !original_revoked {
        let _ = state.auth_manager.revoke_key(&replacement_id).await;
        let _ = state
            .auth_manager
            .clear_rotation_transition(&id, &replacement_id)
            .await;
        return Err(ApiError(GatewayError::Internal(
            "failed to revoke original API Key".into(),
        )));
    }
    if let Err(error) = state
        .auth_manager
        .clear_rotation_transition(&id, &replacement_id)
        .await
    {
        tracing::error!(original_id = %id, replacement_id, %error, "rotation succeeded but durable transition cleanup failed");
    }
    // Completion is supplemental: the prepared event plus durable key states are
    // sufficient to reconstruct the outcome. Never revoke the only usable new
    // credential after the original has already been revoked.
    if let Err(error) = state
        .auth_manager
        .record_audit(
            &actor.id,
            "api_key.rotated",
            &format!("api_key:{id}"),
            serde_json::json!({
                "replacement_id": replacement_id,
                "expires_at": body.expires_at
            }),
        )
        .await
    {
        tracing::error!(original_id = %id, replacement_id, %error, "rotation completed but completion audit could not be written; prepared audit remains authoritative");
    }
    let replacement = state
        .auth_manager
        .find_key_info(&replacement_id)
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?
        .ok_or_else(|| ApiError(GatewayError::Internal("replacement key not found".into())))?;
    Ok(Json(CreateApiKeyResponse {
        id: replacement_id,
        subject_id: replacement.subject_id,
        key: raw_key,
        name: replacement.name,
        permissions: replacement.permissions,
        requests_per_minute: replacement.requests_per_minute,
        created_at: replacement.created_at,
        expires_at: replacement.expires_at,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/api-keys/rotations",
    tag = "API Keys",
    responses((status = 200, body = [easybot_core::auth::ApiKeyRotationTransition]))
)]
pub async fn list_rotation_transitions(
    State(state): State<AppState>,
) -> Result<Json<Vec<easybot_core::auth::ApiKeyRotationTransition>>, ApiError> {
    Ok(Json(
        state
            .auth_manager
            .rotation_transitions()
            .await
            .map_err(|error| ApiError(GatewayError::StorageError(error)))?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/v1/api-keys/rotations/{source_id}/{replacement_id}/reconcile",
    tag = "API Keys",
    params(("source_id" = String, Path), ("replacement_id" = String, Path)),
    request_body = ReconcileRotationRequest,
    responses(
        (status = 200, body = RotationReconcileResponse),
        (status = 400, description = "Action does not match durable transition state")
    )
)]
pub async fn reconcile_rotation_transition(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path((source_id, replacement_id)): Path<(String, String)>,
    Json(body): Json<ReconcileRotationRequest>,
) -> Result<Json<RotationReconcileResponse>, ApiError> {
    if !matches!(body.action.as_str(), "cancel" | "complete") {
        return Err(ApiError(GatewayError::InvalidRequest(
            "rotation action must be cancel or complete".into(),
        )));
    }
    let transition = state
        .auth_manager
        .rotation_transitions()
        .await
        .map_err(|error| ApiError(GatewayError::StorageError(error)))?
        .into_iter()
        .find(|transition| {
            transition.source_id == source_id && transition.replacement_id == replacement_id
        })
        .ok_or_else(|| {
            ApiError(GatewayError::InvalidRequest(
                "rotation transition not found".into(),
            ))
        })?;
    let expected_state = if body.action == "cancel" {
        "created"
    } else {
        "prepared"
    };
    if transition.state != expected_state {
        return Err(ApiError(GatewayError::InvalidRequest(format!(
            "{} action requires {} transition state",
            body.action, expected_state
        ))));
    }
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "api_key.rotation.reconcile.requested",
            &format!("api_key:{source_id}"),
            serde_json::json!({"replacement_id":replacement_id,"action":body.action}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    state
        .auth_manager
        .reconcile_rotation_transition(&source_id, &replacement_id, &body.action)
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    if let Err(error) = state
        .auth_manager
        .record_audit(
            &actor.id,
            "api_key.rotation.reconciled",
            &format!("api_key:{source_id}"),
            serde_json::json!({"replacement_id":replacement_id,"action":body.action}),
        )
        .await
    {
        tracing::error!(%error, source_id, replacement_id, action = %body.action, "rotation reconciliation completed but completion audit failed; requested audit and key states remain authoritative");
    }
    Ok(Json(RotationReconcileResponse {
        success: true,
        action: body.action,
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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(id): Path<String>,
) -> Result<Json<RevokeResponse>, ApiError> {
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "api_key.revoke.requested",
            &format!("api_key:{id}"),
            serde_json::json!({}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    if state
        .auth_manager
        .revoke_key(&id)
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?
    {
        tracing::info!("AUDIT: API key revoked — id={}", id);
        state
            .auth_manager
            .record_audit(
                &actor.id,
                "api_key.revoked",
                &format!("api_key:{id}"),
                serde_json::json!({}),
            )
            .await
            .map_err(|error| ApiError(GatewayError::Internal(error)))?;
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
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Path(id): Path<String>,
) -> Result<Json<RevokeResponse>, ApiError> {
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "api_key.purge.requested",
            &format!("api_key:{id}"),
            serde_json::json!({}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    if state
        .auth_manager
        .delete_key(&id)
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?
    {
        state
            .auth_manager
            .record_audit(
                &actor.id,
                "api_key.purged",
                &format!("api_key:{id}"),
                serde_json::json!({}),
            )
            .await
            .map_err(|error| ApiError(GatewayError::Internal(error)))?;
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
            "metricsread",
            "auditread",
            "billingread",
            "billingwrite",
            "logsread",
            "systemread",
        ],
    })
}

#[derive(Deserialize)]
pub struct AuditQuery {
    pub limit: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub struct AuditEventsResponse {
    pub integrity_valid: bool,
    pub events: Vec<easybot_core::auth::AuditEvent>,
}

/// Query newest management audit events and verify the full hash chain.
#[utoipa::path(
    get,
    path = "/api/v1/audit-events",
    tag = "Audit",
    params(("limit" = Option<usize>, Query, description = "Newest events to return, max 1000")),
    responses(
        (status = 200, body = AuditEventsResponse),
        (status = 400, description = "limit must be between 1 and 1000", body = easybot_core::types::error::ApiErrorResponse),
        (status = 500, description = "The full audit hash chain failed integrity validation", body = easybot_core::types::error::ApiErrorResponse),
    )
)]
pub async fn list_audit_events(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Query(query): Query<AuditQuery>,
) -> axum::response::Response {
    use axum::response::IntoResponse as _;
    if let Err(error) = easybot_core::auth::permissions::require_permission(
        &actor,
        easybot_core::auth::permissions::Permission::AuditRead,
    ) {
        return ApiError(error).into_response();
    }
    let limit = query.limit.unwrap_or(100);
    if !(1..=1_000).contains(&limit) {
        return ApiError(GatewayError::InvalidRequest(
            "audit limit must be between 1 and 1000".into(),
        ))
        .into_response();
    }
    if !state.auth_manager.verify_audit_chain().await {
        tracing::error!("management audit hash chain integrity validation failed");
        return ApiError(GatewayError::Internal(
            "audit ledger integrity validation failed".into(),
        ))
        .into_response();
    }
    Json(AuditEventsResponse {
        integrity_valid: true,
        events: match state.auth_manager.query_audit_events(limit).await {
            Ok(events) => events,
            Err(error) => {
                tracing::error!(%error, "failed to query management audit ledger");
                return ApiError(GatewayError::Internal(
                    "failed to query audit ledger".into(),
                ))
                .into_response();
            }
        },
    })
    .into_response()
}
