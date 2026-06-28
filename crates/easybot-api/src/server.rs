//! API 服务器
//!
//! 基于 axum 构建的 HTTP 服务器，提供 REST API 和 WebSocket 端点。

use axum::{
    Router,
    extract::State,
    http::{Method, Request, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use std::future::Future;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::AppState;
use crate::openapi::ApiDoc;
use crate::response::ApiError;
use crate::routes;
use easybot_core::auth::permissions::{Permission, require_permission};
use easybot_core::types::error::GatewayError;

/// API 服务器
pub struct Server {
    state: AppState,
    config: Arc<easybot_core::types::config::ServerConfig>,
}

impl Server {
    /// 创建服务器实例
    pub fn new(state: AppState, config: easybot_core::types::config::ServerConfig) -> Self {
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
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        match auth_header {
            Some(key) => match state.auth_manager.authenticate(key).await {
                Ok(auth_info) => {
                    let mut req = req;
                    req.extensions_mut().insert(auth_info);
                    next.run(req).await
                }
                Err(_) => {
                    ApiError(GatewayError::AuthFailed("Invalid API key".into())).into_response()
                }
            },
            None => ApiError(GatewayError::AuthFailed(
                "Missing or invalid Authorization header. Expected: Bearer <api-key>".into(),
            ))
            .into_response(),
        }
    }

    /// 权限检查中间件
    ///
    /// 在认证中间件之后运行，根据请求路径和方法判断所需权限，
    /// 从 request extensions 中读取 AuthInfo 进行权限校验。
    /// 认证失败返回 401，权限不足返回 403。
    async fn permission_middleware(req: Request<axum::body::Body>, next: Next) -> Response {
        let auth = match req.extensions().get::<easybot_core::auth::AuthInfo>() {
            Some(auth) => auth.clone(),
            None => {
                return ApiError(GatewayError::AuthFailed("Authentication required".into()))
                    .into_response();
            }
        };

        let required = match (req.method(), req.uri().path()) {
            (&Method::POST, p) if p.ends_with("/start") => Permission::AdaptersManage,
            (&Method::POST, p) if p.ends_with("/stop") => Permission::AdaptersManage,
            (&Method::PUT, p) if p.contains("/config") => Permission::ConfigWrite,
            (&Method::GET, p) if p.contains("/config") => Permission::ConfigRead,
            (&Method::POST, p) if p.contains("/messages/send") => Permission::MessagesSend,
            (&Method::POST, p) if p.contains("/messages/batch-send") => Permission::MessagesSend,
            (&Method::PUT, p) if p.contains("/messages/") => Permission::MessagesSend,
            (&Method::DELETE, p) if p.contains("/messages/") => Permission::MessagesSend,
            (&Method::GET, p) if p.contains("/messages") => Permission::MessagesRead,
            (&Method::DELETE, p) if p.contains("/sessions/") => Permission::SessionsManage,
            (&Method::GET, p) if p.contains("/sessions") => Permission::SessionsRead,
            (&Method::GET, p) if p.contains("/adapters") => Permission::AdaptersRead,
            // WebSocket upgrade
            (&Method::GET, p) if p.ends_with("/ws") => Permission::WebSocketConnect,
            _ => return next.run(req).await,
        };

        match require_permission(&auth, required) {
            Ok(()) => next.run(req).await,
            Err(e) => ApiError(e).into_response(),
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
            info!(
                "TLS enabled in config. TLS termination should be handled by reverse proxy (nginx/caddy/traefik)."
            );
            info!(
                "Cert: {}, Key: {}",
                self.config.tls.cert_file, self.config.tls.key_file
            );
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

/// CSP 中间件：为所有响应添加 Content-Security-Policy header
async fn csp_middleware(response: Response) -> Response {
    const CSP_VALUE: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss:; img-src 'self' data:;";
    let (mut parts, body) = response.into_parts();
    parts.headers.insert(
        header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static(CSP_VALUE),
    );
    Response::from_parts(parts, body)
}

/// 构建 axum Router 实例
///
/// 作为公共函数暴露，以便测试代码可以直接使用。
/// 构造包含所有路由（公共 + 受保护）、中间件（认证、限流）和 Swagger UI 的路由器。
pub fn create_router(state: AppState) -> Router {
    // ── 公共路由（无需认证）──

    // 健康检查
    let mut public_routes = Router::new().route("/health", get(routes::health::health_check));

    // ── 公共路由速率限制器（宽松：120 req/min，突发 20）──
    // health 端点开销极低，但大量请求仍可造成 DoS。
    // 120 req/min（每秒 2 次）对监控探测足够宽松，同时防止滥用。
    const PUBLIC_RATE_LIMIT_RPM: u64 = 120;
    const PUBLIC_RATE_LIMIT_BURST: u32 = 20;
    let public_rate_limiter = crate::middleware::rate_limit::RateLimiter::new(
        crate::middleware::rate_limit::RateLimitConfig {
            enabled: state.config.api.rate_limit.enabled,
            requests_per_minute: PUBLIC_RATE_LIMIT_RPM,
            burst_size: PUBLIC_RATE_LIMIT_BURST,
        },
    );
    public_rate_limiter.start_cleanup();
    public_routes = public_routes.route_layer(middleware::from_fn_with_state(
        public_rate_limiter,
        crate::middleware::rate_limit::rate_limit_middleware,
    ));

    // ── 速率限制器（受保护路由）──
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
    rate_limiter.start_cleanup();

    // ── 受保护路由（需要 Bearer Token 认证）──

    let protected_routes = Router::new()
        // 适配器管理
        .route("/adapters", get(routes::adapters::list_adapters))
        .route(
            "/adapters/{platform}/start",
            post(routes::adapters::start_adapter),
        )
        .route(
            "/adapters/{platform}/stop",
            post(routes::adapters::stop_adapter),
        )
        .route(
            "/adapters/{platform}/status",
            get(routes::adapters::adapter_status),
        )
        // 消息
        .route("/messages/send", post(routes::messages::send_message))
        .route("/messages/batch-send", post(routes::messages::batch_send))
        .route(
            "/messages/{message_id}",
            put(routes::messages::edit_message),
        )
        .route(
            "/messages/{message_id}",
            delete(routes::messages::delete_message),
        )
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
        // 系统信息（管理后台概览页）
        .route("/system", get(routes::system::system_info))
        // 日志查询（管理后台日志页）
        .route("/logs", get(routes::logs::log_entries));

    // ── 指标端点（需认证：Prometheus 抓取可能不支持 Bearer token，
    // 生产环境建议通过反向代理 IP 白名单控制访问）──
    let protected_routes = if state.metrics.is_some() {
        protected_routes.route(
            &state.config.api.metrics.path,
            get(crate::metrics::metrics_handler),
        )
    } else {
        protected_routes
    };

    let protected_routes = protected_routes
        // 速率限制中间件（最内层，最后执行）
        .route_layer(middleware::from_fn_with_state(
            rate_limiter.clone(),
            crate::middleware::rate_limit::rate_limit_middleware,
        ))
        // 权限中间件（在认证之后执行，根据路径+方法检查权限）
        .route_layer(middleware::from_fn(Server::permission_middleware))
        // 认证中间件（最外层，最先执行，注入 AuthInfo 到 extensions）
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            Server::auth_middleware,
        ));

    // 合并公共 + 受保护路由
    let api_routes = Router::new().merge(public_routes).merge(protected_routes);

    // ── WebSocket 路由（单独处理：无需 Bearer 认证——WSC 不支持自定义 HTTP 头，
    // 连接后在 handle_ws() 内通过 JSON 帧 {"token":"..."} 二次认证）──
    let ws_rate_limiter = crate::middleware::rate_limit::RateLimiter::new(
        crate::middleware::rate_limit::RateLimitConfig {
            enabled: state.config.api.rate_limit.enabled,
            requests_per_minute: rl_config.requests_per_minute,
            burst_size: rl_config.burst_size,
        },
    );
    ws_rate_limiter.start_cleanup();
    let ws_routes = Router::new()
        .route("/ws", get(routes::ws::ws_handler))
        .route_layer(middleware::from_fn_with_state(
            ws_rate_limiter,
            crate::middleware::rate_limit::rate_limit_middleware,
        ));
    let api_routes = api_routes.merge(ws_routes);

    // OpenAPI 文档路径（Swagger UI，无需认证）
    let swagger = SwaggerUi::new("/swagger").url("/openapi.json", ApiDoc::openapi());

    // ── HTTP 指标中间件（记录请求计数和耗时）──
    let metrics_middleware =
        middleware::from_fn_with_state(state.clone(), crate::metrics::http_metrics_middleware);

    // ── CORS 配置 ──
    // debug 模式保持 permissive 方便开发；release 模式使用配置白名单
    let cors = if cfg!(debug_assertions) {
        CorsLayer::permissive()
    } else {
        let origins: Vec<_> = state
            .config
            .server
            .cors_allowed_origins
            .iter()
            .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
    };

    // ── 静态页面路由（无需认证）──

    // 主页
    let homepage = Router::new().route("/", get(routes::home::home_page));
    // 文档页
    let docs = Router::new().route("/docs", get(routes::docs::docs_page));
    // 管理后台（SPA + 密码登录 API）
    let admin = Router::new()
        .route("/admin", get(routes::admin::admin_page))
        .route("/admin/login", post(routes::admin::admin_login));

    // 基础路径
    let base_path = &state.config.api.base_path;
    Router::new()
        .merge(homepage)
        .merge(docs)
        .merge(admin)
        .merge(swagger)
        .nest(base_path, api_routes)
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024)) // 10MB
        .layer(cors)
        .route_layer(metrics_middleware)
        .layer(middleware::map_response(csp_middleware))
        .with_state(state.clone())
}
