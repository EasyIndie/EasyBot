//! API 服务器
//!
//! 基于 axum 构建的 HTTP 服务器，提供 REST API 和 WebSocket 端点。

use std::future::Future;
use std::sync::Arc;
use axum::{
    Router,
    extract::State,
    http::Request,
    middleware::{self, Next},
    response::{IntoResponse, Response},
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
use crate::response::ApiError;
use easybot_core::types::error::GatewayError;

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

    /// 构建路由器（委托给公共函数）
    fn build_router(&self) -> Router {
        create_router(self.state.clone())
    }

    /// Bearer Token 认证中间件
    ///
    /// 从 Authorization 头中提取 Bearer token，通过 ApiKeyManager 验证。
    /// 验证通过后将 AuthInfo 注入请求扩展，供下游处理器使用。
    /// 验证失败返回 401。
    async fn auth_middleware(
        State(state): State<AppState>,
        req: Request<axum::body::Body>,
        next: Next,
    ) -> Response {
        let auth_header = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        match auth_header {
            Some(key) => match state.auth_manager.authenticate(key).await {
                Ok(auth_info) => {
                    let mut req = req;
                    req.extensions_mut().insert(auth_info);
                    next.run(req).await
                }
                Err(_) => ApiError(GatewayError::AuthFailed("Invalid API key".into())).into_response(),
            },
            None => ApiError(GatewayError::AuthFailed(
                "Missing or invalid Authorization header. Expected: Bearer <api-key>".into(),
            ))
            .into_response(),
        }
    }

    /// 启动服务器（支持优雅关闭）
    ///
    /// `shutdown_signal` 是一个 Future，当它完成时服务器开始优雅关闭：
    /// 停止接受新连接，等待现有请求完成。
    /// 返回 JoinHandle 以便在关闭时等待服务器完全停止。
    ///
    /// # TLS 说明
    ///
    /// EasyBot 本身基于 TCP 提供服务，TLS 终止建议在基础设施层完成
    /// （如反向代理 nginx/caddy/traefik，或负载均衡器）。
    /// Docker 部署时可添加 TLS proxy sidecar。
    /// `TlsConfig` 中的证书路径用于文档化证书位置，不在应用层处理。
    pub async fn start(
        &self,
        shutdown_signal: impl Future<Output = ()> + Send + 'static,
    ) -> Result<tokio::task::JoinHandle<()>, std::io::Error> {
        let addr = format!("{}:{}", self.config.host, self.config.port);

        if self.config.tls.enabled {
            info!("TLS enabled in config. TLS termination should be handled by reverse proxy (nginx/caddy/traefik).");
            info!("Cert: {}, Key: {}", self.config.tls.cert_file, self.config.tls.key_file);
        }

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        let router = self.build_router();

        info!("API server listening on http://{}", addr);

        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown_signal)
                .await
                .expect("API server failed");
        });

        Ok(handle)
    }
}

/// 构建 axum Router 实例
///
/// 作为公共函数暴露，以便测试代码可以直接使用。
/// 构造包含所有路由（公共 + 受保护）、中间件（认证、限流）和 Swagger UI 的路由器。
pub fn create_router(state: AppState) -> Router {
    // ── 公共路由（无需认证）──

    // 健康检查
    let mut public_routes = Router::new()
        .route("/health", get(routes::health::health_check));

    // ── 指标端点（从 AppState 提取 MetricsRegistry）──
    if state.metrics.is_some() {
        public_routes = public_routes.route(
            &state.config.api.metrics.path,
            get(crate::metrics::metrics_handler),
        );
    }

    // ── 速率限制器 ──
    let rl_config = easybot_core::types::config::RateLimitConfig {
        enabled: state.config.api.rate_limit.enabled,
        requests_per_minute: state.config.api.rate_limit.requests_per_minute,
        burst_size: state.config.api.rate_limit.burst_size,
    };
    let rate_limiter = crate::middleware::rate_limit::RateLimiter::new(
        crate::middleware::rate_limit::RateLimitConfig {
            enabled: rl_config.enabled,
            requests_per_minute: rl_config.requests_per_minute,
            burst_size: rl_config.burst_size,
        },
    );

    // ── 受保护路由（需要 Bearer Token 认证）──

    let protected_routes = Router::new()
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
        .route("/ws", get(routes::ws::ws_handler))
        // 速率限制中间件（在认证之前）
        .route_layer(middleware::from_fn_with_state(
            rate_limiter.clone(),
            crate::middleware::rate_limit::rate_limit_middleware,
        ))
        // 认证中间件（作用于以上所有路由）
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            Server::auth_middleware,
        ));

    // 合并公共 + 受保护路由
    let api_routes = Router::new()
        .merge(public_routes)
        .merge(protected_routes);

    // OpenAPI 文档路径（Swagger UI，无需认证）
    let swagger = SwaggerUi::new("/swagger")
        .url("/openapi.json", ApiDoc::openapi());

    // 基础路径
    let base_path = &state.config.api.base_path;
    Router::new()
        .merge(swagger)
        .nest(base_path, api_routes)
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state.clone())
}
