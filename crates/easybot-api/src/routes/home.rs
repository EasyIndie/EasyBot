//! 主页路由

use axum::response::Html;

/// GET / — 项目主页
pub async fn home_page() -> Html<&'static str> {
    Html(include_str!("../../templates/home.html"))
}
