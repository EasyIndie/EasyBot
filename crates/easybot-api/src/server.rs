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

        // SECURITY: Use prefix-based matching against the API base path.
        // Strip the base path prefix to get the route path, then match exactly.
        let base = "/api/v1";
        let path = req.uri().path().to_string();
        let route_path = if let Some(stripped) = path.strip_prefix(base) {
            stripped
        } else {
            // Not an API route — allow through (e.g. /health, /admin)
            return next.run(req).await;
        };

        let required = match (req.method(), route_path) {
            (
                &Method::POST,
                "/adapters/telegram/start"
                | "/adapters/qq/start"
                | "/adapters/discord/start"
                | "/adapters/feishu/start"
                | "/adapters/wechat/start",
            ) => Permission::AdaptersManage,
            (
                &Method::POST,
                "/adapters/telegram/stop"
                | "/adapters/qq/stop"
                | "/adapters/discord/stop"
                | "/adapters/feishu/stop"
                | "/adapters/wechat/stop",
            ) => Permission::AdaptersManage,
            // Fallback: match by path prefix for dynamic segments
            (method, _p) if matches_adapter_action(method, route_path) => {
                Permission::AdaptersManage
            }
            (&Method::PUT, _) if route_path == "/config" => Permission::ConfigWrite,
            (&Method::GET, _) if route_path == "/config" => Permission::ConfigRead,
            (&Method::POST, _) if route_path == "/messages/send" => Permission::MessagesSend,
            (&Method::POST, _) if route_path == "/messages/batch-send" => Permission::MessagesSend,
            (&Method::PUT, _) if route_path.starts_with("/messages/") => Permission::MessagesSend,
            (&Method::DELETE, _) if route_path.starts_with("/messages/") => {
                Permission::MessagesSend
            }
            (&Method::GET, _) if route_path == "/messages" => Permission::MessagesRead,
            (&Method::DELETE, _) if route_path.starts_with("/sessions/") => {
                Permission::SessionsManage
            }
            (&Method::GET, _) if route_path.starts_with("/sessions") => Permission::SessionsRead,
            (&Method::GET, _) if route_path.starts_with("/adapters") => Permission::AdaptersRead,
            // WebSocket upgrade
            (&Method::GET, _) if route_path == "/ws" => Permission::WebSocketConnect,
            // API Key management
            (_, _) if route_path.starts_with("/api-keys") => Permission::ApiKeysManage,
            // System and logs endpoints require config read
            (&Method::GET, _)
                if route_path == "/system" || route_path == "/system/update-check" =>
            {
                Permission::ConfigRead
            }
            (&Method::GET, _) if route_path == "/logs" => Permission::ConfigRead,
            // Chats endpoints require adapters read
            (&Method::GET, _) if route_path.starts_with("/chats") => Permission::AdaptersRead,
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

/// Helper: match adapter start/stop actions on dynamic platform segments.
/// e.g. "/adapters/{platform}/start" or "/adapters/{platform}/stop"
fn matches_adapter_action(method: &Method, path: &str) -> bool {
    if !path.starts_with("/adapters/") {
        return false;
    }
    let parts: Vec<&str> = path.split('/').collect();
    // Expected: ["", "adapters", "{platform}", "start"|"stop"]
    if parts.len() == 4 && (method == Method::POST) {
        matches!(parts[3], "start" | "stop")
    } else {
        false
    }
}

/// CSP + security headers middleware
async fn security_headers_middleware(response: Response) -> Response {
    // NOTE: 'unsafe-inline' is required because all assets (CSS/JS) are embedded
    // inline in a single HTML file by build.rs. This is a local admin panel, not
    // a public-facing site — the inline content comes from trusted source files.
    const CSP_VALUE: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' https://static.cloudflareinsights.com; style-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss:; img-src 'self' data:; frame-ancestors 'none';";
    let (mut parts, body) = response.into_parts();

    // Content-Security-Policy
    parts.headers.insert(
        header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static(CSP_VALUE),
    );

    // Prevent clickjacking
    parts.headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        axum::http::HeaderValue::from_static("DENY"),
    );

    // Prevent MIME type sniffing
    parts.headers.insert(
        header::HeaderName::from_static("x-content-type-options"),
        axum::http::HeaderValue::from_static("nosniff"),
    );

    // HSTS (only meaningful over HTTPS, but safe to always include)
    parts.headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        axum::http::HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );

    Response::from_parts(parts, body)
}

/// 构建 axum Router 实例
///
/// 作为公共函数暴露，以便测试代码可以直接使用。
/// 构造包含所有路由（公共 + 受保护）、中间件（认证、限流）和 Swagger UI 的路由器。
pub fn create_router(state: AppState) -> Router {
    // ── 共享速率限制器桶池（所有路由共用同一 DashMap 和 cleanup 任务）──
    let shared_buckets: Arc<
        dashmap::DashMap<
            String,
            Arc<tokio::sync::RwLock<crate::middleware::rate_limit::SlidingWindow>>,
        >,
    > = Arc::new(dashmap::DashMap::new());
    crate::middleware::rate_limit::RateLimiter::start_shared_cleanup(&shared_buckets);

    // ── 公共路由（无需认证）──

    // 健康检查
    let mut public_routes = Router::new().route("/health", get(routes::health::health_check));

    // ── 公共路由速率限制器（宽松：120 req/min，突发 20）──
    const PUBLIC_RATE_LIMIT_RPM: u64 = 120;
    const PUBLIC_RATE_LIMIT_BURST: u32 = 20;
    let public_rate_limiter = crate::middleware::rate_limit::RateLimiter::with_shared_buckets(
        shared_buckets.clone(),
        crate::middleware::rate_limit::RateLimitConfig {
            enabled: state.config.api.rate_limit.enabled,
            requests_per_minute: PUBLIC_RATE_LIMIT_RPM,
            burst_size: PUBLIC_RATE_LIMIT_BURST,
        },
    );
    public_routes = public_routes.route_layer(middleware::from_fn_with_state(
        public_rate_limiter,
        crate::middleware::rate_limit::rate_limit_middleware,
    ));

    // ── 速率限制器（受保护路由，共用桶池）──
    let rl_config = easybot_core::types::config::RateLimitConfig {
        enabled: state.config.api.rate_limit.enabled,
        requests_per_minute: state.config.api.rate_limit.requests_per_minute,
        burst_size: state.config.api.rate_limit.burst_size,
    };
    let rate_limiter = crate::middleware::rate_limit::RateLimiter::with_shared_buckets(
        shared_buckets.clone(),
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
        // 版本更新检查
        .route("/system/update-check", get(routes::update::update_check))
        // 日志查询（管理后台日志页）
        .route("/logs", get(routes::logs::log_entries))
        // API Key 管理
        .route(
            "/api-keys",
            get(routes::admin::list_api_keys).post(routes::admin::create_api_key),
        )
        .route("/api-keys/types", get(routes::admin::list_api_key_types))
        .route("/api-keys/{id}", delete(routes::admin::revoke_api_key))
        .route("/api-keys/{id}/purge", delete(routes::admin::purge_api_key));

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

    // ── WebSocket 路由（共用桶池）──
    let ws_rate_limiter = crate::middleware::rate_limit::RateLimiter::with_shared_buckets(
        shared_buckets.clone(),
        crate::middleware::rate_limit::RateLimitConfig {
            enabled: state.config.api.rate_limit.enabled,
            requests_per_minute: rl_config.requests_per_minute,
            burst_size: rl_config.burst_size,
        },
    );
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
    // Use runtime flag instead of compile-time cfg!() to prevent
    // accidentally deploying debug builds with permissive CORS.
    let is_debug = std::env::var("EASYBOT_DEBUG_CORS").is_ok();
    let cors = if is_debug {
        tracing::warn!("Permissive CORS enabled via EASYBOT_DEBUG_CORS — not for production!");
        CorsLayer::permissive()
    } else {
        let origins: Vec<_> = state
            .config
            .server
            .cors_allowed_origins
            .iter()
            .filter_map(|o| {
                let parsed = o.parse::<axum::http::HeaderValue>();
                if parsed.is_err() {
                    tracing::warn!("Invalid CORS origin '{}' ignored", o);
                }
                parsed.ok()
            })
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
    };

    // ── Admin login rate limiter (strict: 5 attempts/min per IP, 共用桶池) ──
    const ADMIN_LOGIN_RPM: u64 = 5;
    const ADMIN_LOGIN_BURST: u32 = 2;
    let admin_login_rl = crate::middleware::rate_limit::RateLimiter::with_shared_buckets(
        shared_buckets,
        crate::middleware::rate_limit::RateLimitConfig {
            enabled: state.config.api.rate_limit.enabled,
            requests_per_minute: ADMIN_LOGIN_RPM,
            burst_size: ADMIN_LOGIN_BURST,
        },
    );

    // ── 静态页面路由（无需认证）──

    // 主页
    let homepage = Router::new().route("/", get(routes::home::home_page));
    // 文档页
    let docs = Router::new().route("/docs", get(routes::docs::docs_page));
    // 管理后台（SPA + 密码登录 API，带严格的速率限制）
    let admin = Router::new()
        .route("/admin", get(routes::admin::admin_page))
        .route("/admin/login", post(routes::admin::admin_login))
        .route_layer(middleware::from_fn_with_state(
            admin_login_rl,
            crate::middleware::rate_limit::rate_limit_middleware,
        ));

    // ── TraceLayer with sensitive header redaction ──
    // SECURITY: Authorization header values are not logged
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(tower_http::trace::DefaultMakeSpan::new().level(tracing::Level::DEBUG))
        .on_request(tower_http::trace::DefaultOnRequest::new().level(tracing::Level::DEBUG));

    // 基础路径
    let base_path = &state.config.api.base_path;
    Router::new()
        .merge(homepage)
        .merge(docs)
        .merge(admin)
        .merge(swagger)
        .nest(base_path, api_routes)
        .layer(trace_layer)
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024)) // 10MB
        .layer(cors)
        .route_layer(metrics_middleware)
        .layer(middleware::map_response(security_headers_middleware))
        .with_state(state.clone())
}
