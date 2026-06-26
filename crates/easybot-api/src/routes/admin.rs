//! 管理后台路由

use crate::AppState;
use crate::response::ApiError;
use axum::{Json, extract::State, response::Html};
use easybot_core::types::error::GatewayError;
use serde::{Deserialize, Serialize};

/// GET /admin — 管理后台 SPA
pub async fn admin_page() -> Html<&'static str> {
    Html(include_str!("../../templates/admin.html"))
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
