//! 主页路由

use axum::response::Html;

/// GET / — 项目主页
pub async fn home_page() -> Html<String> {
    let html = include_str!("../../templates/gen/home.html")
        .replace("__VERSION__", env!("CARGO_PKG_VERSION"));
    Html(html)
}
