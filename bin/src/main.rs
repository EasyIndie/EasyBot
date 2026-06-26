//! EasyBot - 即时通信网关
//!
//! 独立运行的网关服务，连接多种 IM 平台，对外提供 API 接口。
//!
//! 使用:
//!   easybot --config ./gateway.yaml
//!   easybot --dir ~/.easybot
//!   easybot init

use clap::Parser;
#[allow(unused_imports)]
use easybot_core::PlatformAdapter;
use easybot_core::types::event::{GatewayEvent, event_types};
use std::sync::Arc;
/// EasyBot 命令行参数
#[derive(Parser)]
#[command(name = "easybot", version, about = "EasyBot - IM Gateway Service")]
struct Cli {
    /// 配置文件路径（优先级高于 --dir）
    #[arg(short, long)]
    config: Option<String>,

    /// 数据目录（默认为 ~/.easybot/ 或平台标准目录）
    #[arg(long)]
    dir: Option<String>,

    /// 初始化配置目录
    #[arg(long)]
    init: bool,

    /// 调试模式（启用 DEBUG 日志）
    #[arg(short, long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // 初始化日志（含内存日志收集器供管理后台使用）
    let log_level = if cli.debug { "debug" } else { "info" };
    let log_collector = Arc::new(easybot_api::log_collector::LogCollector::new(5000));
    // 克隆以共享同一个内部缓冲（Arc<RwLock<VecDeque>>）
    let tracing_collector = (*log_collector).clone();
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::Layer;
    use tracing_subscriber::Registry;
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    Registry::default()
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(EnvFilter::new(format!("easybot={}", log_level))),
        )
        .with(tracing_collector)
        .init();

    // 处理 init 命令
    if cli.init {
        return handle_init(cli).await;
    }

    // 解析配置根目录
    let home = easybot_core::config::resolve_home(cli.dir.map(std::path::PathBuf::from));
    let paths = easybot_core::config::EasyBotPaths::new(home.clone())?;
    tracing::info!("EasyBot home: {}", home.display());

    // 在加载任何配置之前，先从配置主目录加载 .env 文件。
    // Shell export / Docker environment 优先于 .env（dotenvy 默认行为）。
    easybot_core::config::load_env(&paths)?;

    // 加载配置
    let mut config = if let Some(config_path) = &cli.config {
        easybot_core::config::load_config(std::path::Path::new(config_path)).await?
    } else if paths.config_file.exists() {
        easybot_core::config::load_config(&paths.config_file).await?
    } else {
        tracing::warn!(
            "No configuration file found at {}. Using defaults.",
            paths.config_file.display()
        );
        tracing::info!("Run `easybot --init` to create a default configuration.");
        easybot_core::types::config::GatewayConfig::default()
    };

    // 合并 gateway.local.yaml（存在时覆盖基础配置）
    if paths.local_config_file.exists() {
        match easybot_core::config::load_config(&paths.local_config_file).await {
            Ok(local_config) => {
                // 通过 YAML Value 进行递归合并
                let base_val = serde_yaml::to_value(&config).unwrap_or_default();
                let local_val = serde_yaml::to_value(&local_config).unwrap_or_default();
                let mut merged = base_val.clone();
                easybot_core::config::merge_configs(&mut merged, local_val);
                match serde_yaml::from_value::<easybot_core::types::config::GatewayConfig>(merged) {
                    Ok(c) => {
                        tracing::info!(
                            "Merged local overrides from {}",
                            paths.local_config_file.display()
                        );
                        config = c;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to merge local config: {}. Using base config only.",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to load local config {}: {}",
                    paths.local_config_file.display(),
                    e
                );
            }
        }
    }

    // 创建核心组件
    let event_bus = Arc::new(easybot_core::bus::EventBus::new());

    // 初始化持久化存储
    let db_path = if !config.storage.path.is_empty() {
        let p = std::path::PathBuf::from(&config.storage.path);
        // 安全检查：拒绝含 .. 的路径穿越
        if p.components().any(|c| c == std::path::Component::ParentDir) {
            anyhow::bail!("storage.path 包含非法 '..' 组件: {}", config.storage.path);
        }
        // 相对路径解析到 EasyBot 配置目录下
        if p.is_relative() {
            paths.home.join(p)
        } else {
            // 绝对路径：显式记录
            tracing::info!("使用自定义绝对路径 storage.path: {}", p.display());
            p
        }
    } else {
        paths.db_path.clone()
    };
    let (message_store, session_manager) = match config.storage.storage_type.as_str() {
        "postgres" => {
            let conn_str = if !config.storage.connection_string.is_empty() {
                config.storage.connection_string.clone()
            } else {
                "postgresql://localhost:5432/easybot".to_string()
            };
            match easybot_core::storage::postgres::create_pool(&conn_str, config.storage.pool_size)
                .await
            {
                Ok(pool) => {
                    easybot_core::storage::postgres::run_migrations(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("PostgreSQL migration failed: {}", e))?;
                    // 脱敏连接字符串：仅日志 host/db，隐藏 user:password
                    let safe_conn = conn_str.split('@').next_back().unwrap_or(&conn_str);
                    tracing::info!("PostgreSQL storage initialized: {}", safe_conn);

                    let store: Arc<dyn easybot_core::storage::SessionStore> = Arc::new(
                        easybot_core::storage::postgres::PgSessionStore::new(pool.clone()),
                    );
                    let msg_store: Arc<dyn easybot_core::storage::MessageStore> =
                        Arc::new(easybot_core::storage::postgres::PgMessageStore::new(pool));

                    let sm = Arc::new(easybot_core::session::SessionManager::with_store(store));
                    let loaded = sm
                        .load_from_store()
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to load sessions: {}", e))?;
                    tracing::info!("Loaded {} sessions from database", loaded);

                    (msg_store, sm)
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("PostgreSQL connection failed: {}", e));
                }
            }
        }
        "sqlite" => {
            match easybot_core::storage::sqlite::create_pool(&db_path).await {
                Ok(pool) => {
                    easybot_core::storage::sqlite::run_migrations(&pool)
                        .await
                        .map_err(|e| anyhow::anyhow!("Migration failed: {}", e))?;
                    tracing::info!("SQLite storage initialized: {}", db_path.display());

                    let store: Arc<dyn easybot_core::storage::SessionStore> = Arc::new(
                        easybot_core::storage::sqlite::SqliteSessionStore::new(pool.clone()),
                    );
                    let msg_store: Arc<dyn easybot_core::storage::MessageStore> =
                        Arc::new(easybot_core::storage::sqlite::SqliteMessageStore::new(pool));

                    let sm = Arc::new(easybot_core::session::SessionManager::with_store(store));
                    let loaded = sm
                        .load_from_store()
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to load sessions: {}", e))?;
                    tracing::info!("Loaded {} sessions from database", loaded);

                    (msg_store, sm)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to initialize SQLite ({}), falling back to in-memory: {}",
                        db_path.display(),
                        e
                    );
                    // 使用内存数据库作为回退
                    let pool = easybot_core::storage::sqlite::create_pool(std::path::Path::new(
                        ":memory:",
                    ))
                    .await
                    .expect("In-memory SQLite should always work");
                    let msg_store: Arc<dyn easybot_core::storage::MessageStore> =
                        Arc::new(easybot_core::storage::sqlite::SqliteMessageStore::new(pool));
                    (
                        msg_store,
                        Arc::new(easybot_core::session::SessionManager::new()),
                    )
                }
            }
        }
        _ => {
            tracing::warn!(
                "Unknown storage type '{}', falling back to in-memory",
                config.storage.storage_type
            );
            let pool = easybot_core::storage::sqlite::create_pool(std::path::Path::new(":memory:"))
                .await
                .expect("In-memory SQLite should always work");
            let msg_store: Arc<dyn easybot_core::storage::MessageStore> =
                Arc::new(easybot_core::storage::sqlite::SqliteMessageStore::new(pool));
            (
                msg_store,
                Arc::new(easybot_core::session::SessionManager::new()),
            )
        }
    };

    let adapter_manager =
        Arc::new(easybot_core::adapter::AdapterManager::new().with_event_bus(event_bus.clone()));
    // 初始化自引用，使后台任务能安全持有 Arc<AdapterManager>
    easybot_core::adapter::AdapterManager::init_self_ref(&adapter_manager).await;
    let auth_manager = Arc::new(easybot_core::auth::ApiKeyManager::new());

    // 注册内置适配器
    register_builtin_adapters(&adapter_manager, event_bus.clone()).await;

    // 加载并注册插件适配器
    load_plugin_adapters(&adapter_manager, &paths, event_bus.clone()).await;

    // 创建默认 API Key
    let mut dev_api_key: Option<String> = None;
    match auth_manager
        .create_key("dev", vec!["*".to_string()], None)
        .await
    {
        Ok((_id, key)) => {
            // E2E 测试脚本通过 stdout 提取 key（仅 --debug 模式）
            if cli.debug {
                println!("E2E_API_KEY={}", key);
            }
            dev_api_key = Some(key);
        }
        Err(e) => tracing::warn!("Failed to create dev API key: {}", e),
    }

    // 解析管理后台密码（支持 .env 和 gateway.yaml 两种配置方式）
    let admin_password = if !config.server.admin_password.is_empty() {
        config.server.admin_password.clone()
    } else {
        std::env::var("EASYBOT_ADMIN_PASSWORD").unwrap_or_else(|_| "easybot".to_string())
    };
    if admin_password == "easybot" {
        tracing::warn!(
            "管理后台使用默认密码 'easybot'，请在 .env 或 gateway.yaml 中修改 admin_password"
        );
    }

    // 启动会话桥接器（入站消息 → 自动创建会话）
    easybot_core::session::SessionBridge::start(event_bus.clone(), session_manager.clone());

    // 启动消息持久化器（入站消息 → SQLite）
    easybot_core::session::MessagePersister::start(event_bus.clone(), message_store.clone());

    // 启动 Webhook 事件转发器
    easybot_core::webhook::WebhookDispatcher::start(event_bus.clone(), config.webhooks.clone());

    // 提取 TTL 清理所需数据（后续 config/session_manager 将被消费）
    let ttl_session_store = session_manager.store_ref();
    let ttl_config = easybot_core::storage::retention::RetentionConfig {
        message_ttl_days: config.storage.retention.message_ttl_days,
        session_ttl_days: config.storage.retention.session_ttl_days,
        cleanup_interval_secs: config.storage.retention.cleanup_interval_secs,
    };

    // 创建指标注册表
    let metrics_registry = if config.api.metrics.enabled {
        let reg = Arc::new(easybot_api::metrics::MetricsRegistry::new());
        tracing::info!("Prometheus metrics enabled at {}", config.api.metrics.path);
        Some(reg)
    } else {
        tracing::info!("Prometheus metrics disabled");
        None
    };

    // 提前暂存适配器配置（config 稍后会被移动进 AppState）
    let adapters_config = config.adapters.clone();

    // 创建配置管理器（用于热重载）
    // 优先使用 --config 指定的路径，否则使用默认配置路径
    let config_file_for_watch = cli
        .config
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            if paths.config_file.exists() {
                Some(paths.config_file.clone())
            } else {
                None
            }
        });
    let config_manager = if let Some(path) = config_file_for_watch {
        easybot_api::config_manager::ConfigManager::with_path(config.clone(), path)
    } else {
        easybot_api::config_manager::ConfigManager::new(config.clone())
    };

    // 构建应用状态
    let server_config = config.server.clone();
    let app_state = easybot_api::AppState::new(
        event_bus.clone(),
        adapter_manager.clone(),
        session_manager,
        message_store.clone(),
        auth_manager,
        config,
        config_manager,
        metrics_registry,
        log_collector,
        dev_api_key.clone(),
        admin_password,
    );

    // ── 启动指标事件监听器（自动更新消息计数和适配器状态）──
    if let Some(ref metrics) = app_state.metrics {
        easybot_api::metrics::start_metrics_event_listener(metrics.clone(), event_bus.clone());
        tracing::info!("Metrics event listener started");
    }

    // ── 生产环境安全检查：非 debug 模式下如未启用 TLS 则拒绝启动 ──
    if !server_config.tls.enabled && !cfg!(debug_assertions) {
        tracing::warn!(
            "TLS 未启用！生产环境请启用 TLS 或使用反向代理。\n\
             设置 tls.enabled = true 或设置环境变量 EASYBOT_ALLOW_PLAINTEXT=true 确认风险"
        );
        if std::env::var("EASYBOT_ALLOW_PLAINTEXT").is_err() {
            anyhow::bail!("生产环境必须启用 TLS，或设置 EASYBOT_ALLOW_PLAINTEXT=true 跳过此检查");
        }
        tracing::warn!("EASYBOT_ALLOW_PLAINTEXT 已设置，跳过 TLS 检查（不推荐）");
    }

    // 打印管理后台链接
    tracing::info!(
        "🌐 Admin dashboard: http://{}:{}/admin",
        server_config.host,
        server_config.port,
    );

    // 启动 API 服务器（支持优雅关闭）
    let server = easybot_api::server::Server::new(app_state.clone(), server_config);

    let shutdown = Arc::new(tokio::sync::Notify::new());
    let sig = shutdown.clone();
    let server_handle = server
        .start(async move {
            sig.notified().await;
        })
        .await?;

    // 启动配置文件轮询监听器（每 60 秒检查一次变更）
    easybot_api::config_manager::start_config_watcher(
        app_state.config_manager.clone(),
        event_bus.clone(),
        60,
    );
    tracing::info!("Config file watcher started (polling every 60s)");

    // 后台启动适配器和健康监控（不阻塞 HTTP 服务）
    let am = adapter_manager.clone();
    let cfg = adapters_config;
    tokio::spawn(async move {
        let start_result = am.start_all(cfg).await;
        if !start_result.succeeded.is_empty() {
            tracing::info!("Started adapters: {:?}", start_result.succeeded);
        }
        if !start_result.failed.is_empty() {
            tracing::warn!("Failed adapters: {:?}", start_result.failed);
        }
        am.start_health_monitor(tokio::time::Duration::from_secs(30))
            .await;
        tracing::info!("Adapter health monitor started");
    });

    // 启动 TTL 保留清理 worker（服务器启动后运行，避免启动时争用）
    if let Some(session_store) = ttl_session_store {
        easybot_core::storage::retention::RetentionWorker::start(
            message_store,
            session_store,
            ttl_config,
        );
    } else {
        tracing::info!("No session store available, TTL retention disabled");
    }

    // 发布网关启动事件
    event_bus.publish(GatewayEvent::new(
        event_types::GATEWAY_STARTED,
        "gateway",
        serde_json::json!({"version": env!("CARGO_PKG_VERSION")}),
    ));

    // 等待关闭信号
    tracing::info!("EasyBot started. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down...");

    // 发布网关关闭事件
    event_bus.publish(GatewayEvent::new(
        event_types::GATEWAY_STOPPING,
        "gateway",
        serde_json::json!({"reason": "user_interrupt"}),
    ));

    // 触发优雅关闭：停止接受新连接，等待现有请求完成
    shutdown.notify_waiters();
    let _ = server_handle.await;

    // 停止所有适配器（释放长连接、取消轮询）
    adapter_manager.stop_all().await;
    tracing::info!("EasyBot stopped.");

    Ok(())
}

/// 处理 init 命令：创建默认配置目录
async fn handle_init(cli: Cli) -> anyhow::Result<()> {
    let home = easybot_core::config::resolve_home(cli.dir.map(std::path::PathBuf::from));
    let paths = easybot_core::config::EasyBotPaths::new(home.clone())?;

    if !paths.config_file.exists() {
        let default_config = easybot_core::config::generate_default_config();
        tokio::fs::write(&paths.config_file, &default_config).await?;
        tracing::info!(
            "Created default configuration: {}",
            paths.config_file.display()
        );

        // 同时创建 .env（若不存在）— 所有变量注释掉，用户取消注释即可使用
        let env_path = &paths.env_path;
        if !env_path.exists() {
            let env_content = easybot_core::config::generate_env_example();
            tokio::fs::write(env_path, &env_content).await?;
            tracing::info!("Created .env: {}", env_path.display());
        }

        // 同时创建 gateway.local.yaml（若不存在）
        // 所有适配器以注释形式列出，用户取消注释即可启用
        if !paths.local_config_file.exists() {
            let local_config = easybot_core::config::generate_local_config_example();
            tokio::fs::write(&paths.local_config_file, &local_config).await?;
            tracing::info!(
                "Created local config: {}",
                paths.local_config_file.display()
            );
        }

        // 生成平台特定服务管理文件（配置 + 管理脚本）
        for (svc_path, content) in easybot_core::config::generate_service_files(&paths) {
            if !svc_path.exists() {
                tokio::fs::write(&svc_path, &content).await?;
                // Unix: 让 .sh 脚本可执行
                #[cfg(unix)]
                if svc_path.extension().is_some_and(|e| e == "sh") {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(perm) = std::fs::metadata(&svc_path).map(|m| m.permissions()) {
                        let mut perm = perm;
                        perm.set_mode(0o755);
                        let _ = std::fs::set_permissions(&svc_path, perm);
                    }
                }
                tracing::info!("Created: {}", svc_path.display());
            }
        }

        println!("\nEasyBot initialized at:");
        paths.print_tree();
        println!("\nNext steps:");
        println!(
            "  1. Edit {} — uncomment and fill in your tokens",
            paths.env_path.display()
        );
        println!("  2. Run `easybot --debug` to start locally");

        // 按平台显示服务安装指引
        if cfg!(target_os = "linux") {
            println!();
            println!("  3. Install as systemd service (recommended for production):");
            let bin_path = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "/usr/local/bin/easybot".to_string());
            println!("     Binary: {}", bin_path);
            println!(
                "     cd {} && sudo ./easybot.sh install",
                paths.home.display()
            );
            println!("     sudo ./easybot.sh status");
            println!("     sudo ./easybot.sh logs");
            println!("     sudo ./easybot.sh uninstall");
            println!();
            println!("  TLS: Release mode requires TLS or EASYBOT_ALLOW_PLAINTEXT.");
            println!("       .env already configured with EASYBOT_ALLOW_PLAINTEXT=true.");
            println!("       Edit gateway.yaml tls section to enable real certificates.");
        } else if cfg!(target_os = "macos") {
            println!();
            println!("  3. Install as launchd service (recommended for production):");
            println!("     cd {} && ./easybot.sh install", paths.home.display());
            println!("     ./easybot.sh status");
            println!("     ./easybot.sh logs");
            println!("     ./easybot.sh uninstall");
            println!();
            println!("  TLS: Release mode requires TLS or EASYBOT_ALLOW_PLAINTEXT.");
            println!("       .env already configured with EASYBOT_ALLOW_PLAINTEXT=true.");
            println!("       Edit gateway.yaml tls section to enable real certificates.");
        } else if cfg!(target_os = "windows") {
            println!();
            println!(
                "  3. Install as Windows Service (recommended for production, admin required):"
            );
            println!("     cd {}", paths.home.display());
            println!("     PowerShell -ExecutionPolicy Bypass -File manage-service.ps1 install");
            println!("     PowerShell -ExecutionPolicy Bypass -File manage-service.ps1 status");
            println!("     PowerShell -ExecutionPolicy Bypass -File manage-service.ps1 logs");
            println!("     PowerShell -ExecutionPolicy Bypass -File manage-service.ps1 uninstall");
            println!();
            println!("  TLS: Release mode requires TLS or EASYBOT_ALLOW_PLAINTEXT.");
            println!("       .env already configured with EASYBOT_ALLOW_PLAINTEXT=true.");
            println!("       Edit gateway.yaml tls section to enable real certificates.");
        }

        println!();
        println!("Docker Compose users:");
        println!("  cp .env.example .env && vim .env && docker compose up -d");
    } else {
        tracing::info!(
            "Configuration already exists: {}",
            paths.config_file.display()
        );
        println!("\nEasyBot is already initialized at:");
        println!("  {}", home.display());
        println!(
            "\nEdit {} to update configuration.",
            paths.config_file.display()
        );
    }

    Ok(())
}

/// 加载并注册插件适配器
#[cfg(feature = "plugin-system")]
async fn load_plugin_adapters(
    adapter_manager: &easybot_core::adapter::AdapterManager,
    paths: &easybot_core::config::EasyBotPaths,
    event_bus: Arc<easybot_core::bus::EventBus>,
) {
    use easybot_core::plugin::PluginLoader;

    if !paths.plugins_dir.exists() {
        tracing::info!(
            "No plugins directory at {}, skipping plugin loading",
            paths.plugins_dir.display()
        );
        return;
    }

    tracing::info!("Loading plugins from {}", paths.plugins_dir.display());
    let loader = PluginLoader::new(paths.plugins_dir.clone());
    let (succeeded, failed) = loader.load_all().await;

    for result in &succeeded {
        if let Some(factory) = loader
            .get_factory(&result.platform_name, event_bus.clone())
            .await
        {
            adapter_manager
                .registry()
                .register(&result.platform_name, &result.display_name, factory, &[])
                .await;
            tracing::info!(
                "Registered plugin adapter: {} ({})",
                result.platform_name,
                result.display_name
            );
        }
    }

    for (path, error) in &failed {
        tracing::warn!("Failed to load plugin from {}: {}", path.display(), error);
    }
}

/// 插件系统未启用时的空实现
#[cfg(not(feature = "plugin-system"))]
async fn load_plugin_adapters(
    _adapter_manager: &easybot_core::adapter::AdapterManager,
    _paths: &easybot_core::config::EasyBotPaths,
    _event_bus: Arc<easybot_core::bus::EventBus>,
) {
    tracing::info!("Plugin system not enabled (compile with --features plugin-system to enable)");
}

/// 注册单个内置适配器的宏
///
/// 消除 5 个适配器注册代码的重复模式：创建 factory → 注册到 registry → 日志输出。
#[allow(unused_macros)]
macro_rules! register_adapter {
    ($registry:expr, $eb:expr, $platform:literal, $display:literal, $ty:ty, $creds:expr) => {{
        let eb_cloned = $eb.clone();
        let factory: easybot_core::adapter::AdapterFactory = std::sync::Arc::new(move |config| {
            let eb = eb_cloned.clone();
            Box::pin(async move {
                let mut adapter = <$ty>::new();
                adapter.set_event_bus(eb);
                let init_result = adapter
                    .init(config)
                    .await
                    .map_err(|e| format!("{} init failed: {}", $platform, e))?;
                if !init_result.ok {
                    return Err(init_result
                        .error
                        .unwrap_or_else(|| "unknown init error".to_string()));
                }
                let boxed: Box<dyn easybot_core::PlatformAdapter> = Box::new(adapter);
                Ok(boxed)
            })
        });
        $registry
            .register($platform, $display, factory, $creds)
            .await;
        tracing::info!("Registered built-in adapter: {}", $platform);
    }};
}

/// 注册内置适配器工厂
#[allow(unused_variables)]
async fn register_builtin_adapters(
    adapter_manager: &easybot_core::adapter::AdapterManager,
    event_bus: Arc<easybot_core::bus::EventBus>,
) {
    let registry = adapter_manager.registry();

    #[cfg(feature = "adapter-telegram")]
    register_adapter!(
        registry,
        event_bus,
        "telegram",
        "Telegram",
        easybot_adapter_telegram::TelegramAdapter,
        &["TELEGRAM_BOT_TOKEN"]
    );

    #[cfg(feature = "adapter-discord")]
    register_adapter!(
        registry,
        event_bus,
        "discord",
        "Discord",
        easybot_adapter_discord::DiscordAdapter,
        &["DISCORD_BOT_TOKEN"]
    );

    #[cfg(feature = "adapter-feishu")]
    register_adapter!(
        registry,
        event_bus,
        "feishu",
        "飞书",
        easybot_adapter_feishu::FeishuAdapter,
        &["FEISHU_APP_ID", "FEISHU_APP_SECRET"]
    );

    #[cfg(feature = "adapter-qq")]
    register_adapter!(
        registry,
        event_bus,
        "qq",
        "QQ",
        easybot_adapter_qq::QqAdapter,
        &["QQ_APP_ID", "QQ_CLIENT_SECRET"]
    );

    #[cfg(feature = "adapter-wechat")]
    register_adapter!(
        registry,
        event_bus,
        "wechat",
        "个人微信",
        easybot_adapter_wechat::WeChatAdapter,
        &[] // 个人微信可通过扫码登录，无需强制凭据
    );

    #[cfg(not(any(
        feature = "adapter-telegram",
        feature = "adapter-discord",
        feature = "adapter-feishu",
        feature = "adapter-qq",
        feature = "adapter-wechat"
    )))]
    {
        let _ = event_bus;
        tracing::warn!(
            "No adapters enabled (compile with features to enable: adapter-telegram, adapter-discord, adapter-feishu, adapter-qq, adapter-wechat)"
        );
    }
}
