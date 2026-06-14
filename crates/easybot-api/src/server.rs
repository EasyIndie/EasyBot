//! API 服务器
//!
//! 基于 axum 构建的 HTTP 服务器，提供 REST API 和 WebSocket 端点。

use std::sync::Arc;
use axum::{
    Router,
    routing::{get, post, put, delete},
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::AppState;
use crate::routes;
use crate::openapi::ApiDoc;

/// API 服务器
pub struct Server {
    state: AppState,
    config: Arc<easybot_core::types::config::ServerConfig>,
}

impl Server {
    /// 创建服务器实例
    pub fn new(
        state: AppState,
        config: easybot_core::types::config::ServerConfig,
    ) -> Self {
        Self {
            state,
            config: Arc::new(config),
        }
    }

    /// 构建路由器
    fn build_router(&self) -> Router {
        let api_routes = Router::new()
            // 健康检查
            .route("/health", get(routes::health::health_check))
            // 适配器管理
            .route("/adapters", get(routes::adapters::list_adapters))
            .route("/adapters/{platform}/start", post(routes::adapters::start_adapter))
            .route("/adapters/{platform}/stop", post(routes::adapters::stop_adapter))
            .route("/adapters/{platform}/status", get(routes::adapters::adapter_status))
            // 消息
            .route("/messages/send", post(routes::messages::send_message))
            .route("/messages/batch-send", post(routes::messages::batch_send))
            .route("/messages/{message_id}", put(routes::messages::edit_message))
            .route("/messages/{message_id}", delete(routes::messages::delete_message))
            .route("/messages", get(routes::messages::message_history))
            // 会话
            .route("/sessions", get(routes::sessions::list_sessions))
            .route("/sessions/{key}", get(routes::sessions::get_session))
            .route("/sessions/{key}", delete(routes::sessions::delete_session))
            // 聊天
            .route("/chats/{platform}", get(routes::chats::list_chats))
            .route("/chats/{platform}/{chat_id}", get(routes::chats::get_chat))
            // 配置
            .route("/config", get(routes::config::get_config))
            .route("/config", put(routes::config::update_config))
            // WebSocket
            .route("/ws", get(routes::ws::ws_handler));

        // OpenAPI 文档路径（Swagger UI）
        let openapi_json_path = "/openapi.json";
        let swagger = SwaggerUi::new("/swagger")
            .url(openapi_json_path, ApiDoc::openapi());

        // 基础路径
        let base_path = &self.state.config.api.base_path;
        Router::new()
            .merge(swagger)
            .nest(base_path, api_routes)
            .layer(TraceLayer::new_for_http())
            .layer(CorsLayer::permissive())
            .with_state(self.state.clone())
    }

    /// 启动服务器
    ///
    /// 返回 JoinHandle 以便在关闭时等待。
    pub async fn start(&self) -> Result<tokio::task::JoinHandle<()>, std::io::Error> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        let router = self.build_router();

        info!("API server listening on {}", addr);

        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("API server failed");
        });

        Ok(handle)
    }
}
