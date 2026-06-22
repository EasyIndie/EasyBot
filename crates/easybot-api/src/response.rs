//! 统一响应格式
//!
//! 使用 newtype 模式包装 GatewayError，实现 axum IntoResponse。

use axum::{
    Json,
    response::{IntoResponse, Response},
};
use easybot_core::types::error::GatewayError;

/// API 错误响应包装
pub struct ApiError(pub GatewayError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = self.0.to_api_error();
        let status = axum::http::StatusCode::from_u16(self.0.http_status())
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(body)).into_response()
    }
}

/// 便捷构造 ApiError
pub fn api_error<E: Into<GatewayError>>(err: E) -> ApiError {
    ApiError(err.into())
}

/// 类型别名：API 路由的标准返回类型
pub type ApiResult<T = serde_json::Value> = Result<Json<T>, ApiError>;
