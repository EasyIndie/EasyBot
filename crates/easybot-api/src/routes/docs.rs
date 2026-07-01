//! 文档页路由
//!
//! docs.html 由 build.rs 构建脚本在编译时自动生成。

use axum::response::Html;

/// GET /docs — 项目文档
pub async fn docs_page() -> Html<&'static str> {
    Html(include_str!("../../templates/gen/docs.html"))
}
