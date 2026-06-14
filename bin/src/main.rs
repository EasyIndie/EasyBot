//! EasyBot - 即时通信网关
//!
//! 独立运行的网关服务，连接多种 IM 平台，对外提供 API 接口。
//!
//! 使用:
//!   easybot --config ./gateway.yaml
//!   easybot --dir ~/.easybot
//!   easybot init

use std::sync::Arc;
use clap::Parser;
use tracing_subscriber::EnvFilter;
use easybot_core::PlatformAdapter;

/// EasyBot 命令行参数
#[derive(Parser)]
#[command(name = "easybot", about = "EasyBot - IM Gateway Service")]
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

    // 加载配置
    let config = if let Some(config_path) = &cli.config {
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

    // 创建核心组件
    let event_bus = Arc::new(easybot_core::bus::EventBus::new());
    let session_manager = Arc::new(easybot_core::session::SessionManager::new());
    let adapter_manager = Arc::new(easybot_core::adapter::AdapterManager::new());
    let auth_manager = Arc::new(easybot_core::auth::ApiKeyManager::new());

    // 注册内置适配器
    register_builtin_adapters(&adapter_manager, event_bus.clone()).await;

    // 创建默认 API Key（仅开发环境）
    if cli.debug {
        match auth_manager.create_key("dev", vec!["*".to_string()], None).await {
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

    // 构建应用状态
    let server_config = config.server.clone();
    let app_state = easybot_api::AppState::new(
        event_bus.clone(),
        adapter_manager.clone(),
        session_manager,
        auth_manager,
        config,
    );

    // 启动 API 服务器
    let server = easybot_api::server::Server::new(
        app_state,
        server_config,
    );
    let server_handle = server.start().await?;

    // 等待关闭信号
    tracing::info!("EasyBot started. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down...");

    // 优雅关闭
    drop(server_handle);
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
        tracing::info!("Created default configuration: {}", paths.config_file.display());
    } else {
        tracing::info!("Configuration file already exists: {}", paths.config_file.display());
    }

    println!("\nEasyBot initialized at:");
    paths.print_tree();
    println!("\nEdit {} to configure platforms, then run `easybot`.", paths.config_file.display());

    Ok(())
}

/// 注册内置适配器工厂
async fn register_builtin_adapters(
    adapter_manager: &easybot_core::adapter::AdapterManager,
    event_bus: std::sync::Arc<easybot_core::bus::EventBus>,
) {
    #[cfg(feature = "adapter-telegram")]
    {
        let registry = adapter_manager.registry();
        let eb = event_bus.clone();
        let factory: easybot_core::adapter::AdapterFactory =
            std::sync::Arc::new(move |config| {
                let eb = eb.clone();
                Box::pin(async move {
                    let mut adapter = easybot_adapter_telegram::TelegramAdapter::new();
                    // 设置事件总线，使适配器能推送入站消息
                    adapter.set_event_bus(eb);
                    let init_result = adapter.init(config).await
                        .map_err(|e| format!("telegram init failed: {}", e))?;
                    if !init_result.ok {
                        return Err(init_result.error.unwrap_or_else(|| "unknown init error".to_string()));
                    }
                    Ok(Box::new(adapter) as Box<dyn easybot_core::PlatformAdapter>)
                })
            });
        registry.register("telegram", "Telegram", factory).await;
        tracing::info!("Registered built-in adapter: telegram");
    }

    #[cfg(not(feature = "adapter-telegram"))]
    {
        let _ = event_bus; // suppress unused warning
        tracing::warn!("Telegram adapter not enabled (compile with --features adapter-telegram)");
    }
}
