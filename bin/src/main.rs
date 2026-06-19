//! EasyBot - 即时通信网关
//!
//! 独立运行的网关服务，连接多种 IM 平台，对外提供 API 接口。
//!
//! 使用:
//!   easybot --config ./gateway.yaml
//!   easybot --dir ~/.easybot
//!   easybot init

use clap::Parser;
use easybot_core::types::event::{event_types, GatewayEvent};
#[allow(unused_imports)]
use easybot_core::PlatformAdapter;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

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

    // 初始化日志
    let log_level = if cli.debug { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(format!("easybot={}", log_level)))
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
        std::path::PathBuf::from(&config.storage.path)
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
                    tracing::info!("PostgreSQL storage initialized: {}", conn_str);

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
    let auth_manager = Arc::new(easybot_core::auth::ApiKeyManager::new());

    // 注册内置适配器
    register_builtin_adapters(&adapter_manager, event_bus.clone()).await;

    // 加载并注册插件适配器
    load_plugin_adapters(&adapter_manager, &paths, event_bus.clone()).await;

    // 创建默认 API Key（仅开发环境）
    if cli.debug {
        match auth_manager
            .create_key("dev", vec!["*".to_string()], None)
            .await
        {
            Ok((id, key)) => tracing::info!("Dev API Key created: id={}, key={}", id, key),
            Err(e) => tracing::warn!("Failed to create dev API key: {}", e),
        }
    }

    // 启动适配器
    let start_result = adapter_manager.start_all(config.adapters.clone()).await;
    if !start_result.succeeded.is_empty() {
        tracing::info!("Started adapters: {:?}", start_result.succeeded);
    }
    if !start_result.failed.is_empty() {
        tracing::warn!("Failed adapters: {:?}", start_result.failed);
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

        println!("\nEasyBot initialized at:");
        paths.print_tree();
        println!("\nNext steps:");
        println!(
            "  1. Edit {} — uncomment the adapters you need",
            paths.config_file.display()
        );
        println!(
            "  2. Edit {} — uncomment and fill in your tokens",
            paths.env_path.display()
        );
        println!("  3. Run `easybot --debug` to start");
        println!();
        println!("Docker Compose 用户使用同样的方式: 编辑 gateway.yaml 并 docker compose up -d");
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
    event_bus: std::sync::Arc<easybot_core::bus::EventBus>,
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
                .register(&result.platform_name, &result.display_name, factory)
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
    _event_bus: std::sync::Arc<easybot_core::bus::EventBus>,
) {
    tracing::info!("Plugin system not enabled (compile with --features plugin-system to enable)");
}

/// 注册内置适配器工厂
#[allow(unused_variables)]
async fn register_builtin_adapters(
    adapter_manager: &easybot_core::adapter::AdapterManager,
    event_bus: std::sync::Arc<easybot_core::bus::EventBus>,
) {
    #[cfg(feature = "adapter-telegram")]
    {
        let registry = adapter_manager.registry();
        let eb = event_bus.clone();
        let factory: easybot_core::adapter::AdapterFactory = std::sync::Arc::new(move |config| {
            let eb = eb.clone();
            Box::pin(async move {
                let mut adapter = easybot_adapter_telegram::TelegramAdapter::new();
                adapter.set_event_bus(eb);
                let init_result = adapter
                    .init(config)
                    .await
                    .map_err(|e| format!("telegram init failed: {}", e))?;
                if !init_result.ok {
                    return Err(init_result
                        .error
                        .unwrap_or_else(|| "unknown init error".to_string()));
                }
                Ok(Box::new(adapter) as Box<dyn easybot_core::PlatformAdapter>)
            })
        });
        registry.register("telegram", "Telegram", factory).await;
        tracing::info!("Registered built-in adapter: telegram");
    }

    #[cfg(feature = "adapter-discord")]
    {
        let registry = adapter_manager.registry();
        let eb = event_bus.clone();
        let factory: easybot_core::adapter::AdapterFactory = std::sync::Arc::new(move |config| {
            let eb = eb.clone();
            Box::pin(async move {
                let mut adapter = easybot_adapter_discord::DiscordAdapter::new();
                adapter.set_event_bus(eb);
                let init_result = adapter
                    .init(config)
                    .await
                    .map_err(|e| format!("discord init failed: {}", e))?;
                if !init_result.ok {
                    return Err(init_result
                        .error
                        .unwrap_or_else(|| "unknown init error".to_string()));
                }
                Ok(Box::new(adapter) as Box<dyn easybot_core::PlatformAdapter>)
            })
        });
        registry.register("discord", "Discord", factory).await;
        tracing::info!("Registered built-in adapter: discord");
    }

    #[cfg(feature = "adapter-feishu")]
    {
        let registry = adapter_manager.registry();
        let eb = event_bus.clone();
        let factory: easybot_core::adapter::AdapterFactory = std::sync::Arc::new(move |config| {
            let eb = eb.clone();
            Box::pin(async move {
                let mut adapter = easybot_adapter_feishu::FeishuAdapter::new();
                adapter.set_event_bus(eb);
                let init_result = adapter
                    .init(config)
                    .await
                    .map_err(|e| format!("feishu init failed: {}", e))?;
                if !init_result.ok {
                    return Err(init_result
                        .error
                        .unwrap_or_else(|| "unknown init error".to_string()));
                }
                Ok(Box::new(adapter) as Box<dyn easybot_core::PlatformAdapter>)
            })
        });
        registry.register("feishu", "飞书", factory).await;
        tracing::info!("Registered built-in adapter: feishu");
    }

    #[cfg(feature = "adapter-qq")]
    {
        let registry = adapter_manager.registry();
        let eb = event_bus.clone();
        let factory: easybot_core::adapter::AdapterFactory = std::sync::Arc::new(move |config| {
            let eb = eb.clone();
            Box::pin(async move {
                let mut adapter = easybot_adapter_qq::QqAdapter::new();
                adapter.set_event_bus(eb);
                let init_result = adapter
                    .init(config)
                    .await
                    .map_err(|e| format!("qq init failed: {}", e))?;
                if !init_result.ok {
                    return Err(init_result
                        .error
                        .unwrap_or_else(|| "unknown init error".to_string()));
                }
                Ok(Box::new(adapter) as Box<dyn easybot_core::PlatformAdapter>)
            })
        });
        registry.register("qq", "QQ", factory).await;
        tracing::info!("Registered built-in adapter: qq");
    }

    #[cfg(feature = "adapter-wechat")]
    {
        let registry = adapter_manager.registry();
        let eb = event_bus.clone();
        let factory: easybot_core::adapter::AdapterFactory = std::sync::Arc::new(move |config| {
            let eb = eb.clone();
            Box::pin(async move {
                let mut adapter = easybot_adapter_wechat::WeChatAdapter::new_with_event_bus(eb);
                let init_result = adapter
                    .init(config)
                    .await
                    .map_err(|e| format!("wechat init failed: {}", e))?;
                if !init_result.ok {
                    return Err(init_result
                        .error
                        .unwrap_or_else(|| "unknown init error".to_string()));
                }
                Ok(Box::new(adapter) as Box<dyn easybot_core::PlatformAdapter>)
            })
        });
        registry.register("wechat", "个人微信", factory).await;
        tracing::info!("Registered built-in adapter: wechat");
    }

    #[cfg(not(any(
        feature = "adapter-telegram",
        feature = "adapter-discord",
        feature = "adapter-feishu",
        feature = "adapter-qq",
        feature = "adapter-wechat"
    )))]
    {
        let _ = event_bus;
        tracing::warn!("No adapters enabled (compile with features to enable: adapter-telegram, adapter-discord, adapter-feishu, adapter-qq, adapter-wechat)");
    }
}
