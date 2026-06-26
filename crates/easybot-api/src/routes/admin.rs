//! 管理后台路由

use axum::response::Html;

/// GET /admin — 管理后台（SPA）
pub async fn admin_page() -> Html<&'static str> {
    Html(include_str!("../../templates/admin.html"))
}
