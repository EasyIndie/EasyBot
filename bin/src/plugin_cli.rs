//! `easybot plugin` 子命令：插件市场与管理（T12）
//!
//! 市场安装（GitHub Releases + ed25519 验签 + 发布者信任）、本地管理
//! （list/inspect/enable/disable/uninstall）、显式更新（默认 pin 当前版本）。
//!
//! 信任语义（对齐 VS Code 1.97）：`--yes` 跳过首次安装的信任确认但**不自动**
//! 写入 `.trust`；显式 `plugin trust <publisher>` 才加入。更新默认显式、
//! 默认 pin 当前版本，`--latest`/`--channel` 才跨版本。

use clap::Subcommand;
use easybot_core::plugin::registry::types::PluginChannel;
use easybot_core::plugin::{InstallRequest, PluginManager, UpdateOptions};
use easybot_core::types::config::GatewayConfig;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::plugin_scaffold::{ScaffoldOptions, scaffold};

/// `easybot plugin` 子命令
#[derive(Subcommand)]
pub enum PluginCmd {
    /// 创建插件工程脚手架（生成独立可构建的适配器项目）
    New {
        /// 插件名（kebab-case，如 my-adapter；同时用作平台名与 Cargo 包名）
        name: String,
        /// 目标父目录（默认当前目录；工程建在 <target-dir>/<name>/）
        #[arg(long)]
        target_dir: Option<PathBuf>,
        /// 作者署名（写入 plugin.yaml / LICENSE）
        #[arg(long)]
        author: Option<String>,
    },
    /// 列出已安装插件
    List,
    /// 搜索市场目录（catalog.json，多注册源合并去重）
    Search {
        /// 关键字（缺省列出全部）
        query: Option<String>,
    },
    /// 查看插件详情（已装版本 + 市场版本）
    Info { name: String },
    /// 安装插件（支持 `publisher/name` 限定；首次安装未信任发布者需确认）
    Install {
        name: String,
        /// 发布渠道：stable | beta
        #[arg(long, default_value = "stable")]
        channel: String,
        /// 接受发布者信任确认（不自动写入 .trust）
        #[arg(long)]
        yes: bool,
        /// 离线安装源目录（含 plugin.yaml + 库文件 + plugin.sig.json）
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// 卸载插件
    Uninstall { name: String },
    /// 启用插件（下次启动生效）
    Enable { name: String },
    /// 禁用插件（立即停止适配器）
    Disable { name: String },
    /// 更新插件（默认 pin 当前版本；--latest 跨版本）
    Update {
        name: String,
        /// 跨到渠道最新版本
        #[arg(long)]
        latest: bool,
        /// 发布渠道：stable | beta
        #[arg(long)]
        channel: Option<String>,
    },
    /// 信任发布者（写入 `{home}/plugins/.trust`）
    Trust {
        publisher: String,
        /// 发布者验证公钥（base64）
        #[arg(long)]
        public_key: String,
    },
    /// 检查插件（清单/签名状态/加载错误）
    Inspect { name: String },
}

/// 分发到各子命令处理器
pub async fn run(cmd: &PluginCmd, dir: Option<String>) -> anyhow::Result<()> {
    // 脚手架不依赖市场/宿主状态，先于 manager 构造处理（避免无谓加载配置）。
    if let PluginCmd::New {
        name,
        target_dir,
        author,
    } = cmd
    {
        scaffold(&ScaffoldOptions {
            name: name.clone(),
            target_dir: target_dir.clone(),
            author: author.clone(),
        })?;
        return Ok(());
    }

    let home = easybot_core::config::resolve_home(dir.map(PathBuf::from));
    let paths = easybot_core::config::EasyBotPaths::new(home.clone())?;
    easybot_core::config::load_env(&paths)?;
    let config = load_merged_config(&paths).await;

    let manager = PluginManager::new(
        paths.plugins_dir.clone(),
        Arc::new(RwLock::new(config.plugins.clone())),
        Arc::new(easybot_core::adapter::AdapterManager::new()),
        Arc::new(easybot_core::bus::EventBus::new()),
    )
    .await;

    match cmd {
        PluginCmd::New { .. } => unreachable!("handled before manager construction"),
        PluginCmd::List => list_plugins(&manager).await,
        PluginCmd::Search { query } => search_catalog(&manager, query.as_deref()).await,
        PluginCmd::Info { name } => show_info(&manager, name).await,
        PluginCmd::Install {
            name,
            channel,
            yes,
            file,
        } => install(&manager, name, channel, *yes, file.as_ref()).await,
        PluginCmd::Uninstall { name } => uninstall(&manager, name).await,
        PluginCmd::Enable { name } => set_enabled(&manager, name, true).await,
        PluginCmd::Disable { name } => set_enabled(&manager, name, false).await,
        PluginCmd::Update {
            name,
            latest,
            channel,
        } => update(&manager, name, *latest, channel.as_deref()).await,
        PluginCmd::Trust {
            publisher,
            public_key,
        } => trust(&manager, publisher, public_key).await,
        PluginCmd::Inspect { name } => inspect(&manager, name).await,
    }
}

// ══════════════════════════════════════════════════════════════════
// 子命令实现
// ══════════════════════════════════════════════════════════════════

async fn list_plugins(manager: &PluginManager) -> anyhow::Result<()> {
    let installed = manager.list_installed().await;
    if installed.is_empty() {
        println!("No plugins installed.");
        println!("  Run `easybot plugin search <query>` to browse the marketplace.");
        return Ok(());
    }
    println!(
        "{:<22} {:<10} {:<4} {:<8} {:<9} {:<16} STATUS",
        "NAME", "VERSION", "SDK", "ENABLED", "SIGNATURE", "PUBLISHER"
    );
    for p in &installed {
        let signed = match (p.signed, p.signature_valid) {
            (false, _) => "none".to_string(),
            (true, Some(true)) => "valid".into(),
            (true, Some(false)) => "invalid".into(),
            (true, None) => "unverified".into(),
        };
        let status = if let Some(plat) = &p.platform {
            format!("loaded as {plat}")
        } else if p.enabled {
            p.load_error
                .as_deref()
                .map_or_else(|| "not loaded".into(), |e| format!("error: {e}"))
        } else {
            "disabled".into()
        };
        println!(
            "{:<22} {:<10} {:<4} {:<8} {:<9} {:<16} {}",
            p.name,
            p.version,
            p.sdk_version,
            p.enabled,
            signed,
            p.publisher.as_deref().unwrap_or("-"),
            status
        );
    }
    Ok(())
}

async fn search_catalog(manager: &PluginManager, query: Option<&str>) -> anyhow::Result<()> {
    let results = manager.search_catalog(query).await;
    if results.is_empty() {
        println!(
            "No plugins found{}",
            query.map(|q| format!(" for '{q}'")).unwrap_or_default()
        );
        println!(
            "  Check network access to the market catalog, or `easybot plugin list` for installed plugins."
        );
        return Ok(());
    }
    for s in &results {
        let badge = if s.verified {
            "✓ verified"
        } else {
            "community"
        };
        println!("{}/{}  [{badge}]", s.publisher, s.name);
        if let Some(d) = &s.display_name {
            println!("  {d}");
        }
        if let Some(desc) = &s.description {
            println!("  {desc}");
        }
        if !s.tags.is_empty() {
            println!("  tags: {}", s.tags.join(", "));
        }
        println!(
            "  install: easybot plugin install {}/{}",
            s.publisher, s.name
        );
        println!();
    }
    Ok(())
}

async fn show_info(manager: &PluginManager, name: &str) -> anyhow::Result<()> {
    let info = match manager.plugin_info(name).await {
        Ok(i) => i,
        Err(e) => {
            anyhow::bail!("cannot fetch info for '{name}': {e}");
        }
    };
    let s = &info.source;
    println!("{}/{}", s.publisher, s.name);
    if let Some(d) = &s.display_name {
        println!("  display name: {d}");
    }
    if let Some(desc) = &s.description {
        println!("  description: {desc}");
    }
    println!("  repo: {}/{}", s.owner, s.repo);
    println!("  verified: {}", s.verified);
    if !s.tags.is_empty() {
        println!("  tags: {}", s.tags.join(", "));
    }
    match &info.installed_version {
        Some(v) => println!("  installed: v{v}"),
        None => println!("  installed: (not installed)"),
    }
    println!("  releases:");
    for v in &info.versions {
        let marker = if Some(&v.version) == info.installed_version.as_ref() {
            "  ← installed"
        } else {
            ""
        };
        println!(
            "    v{}  [{}]  sdk={}{}",
            v.version,
            channel_label(v.channel),
            v.sdk_version,
            marker
        );
    }
    Ok(())
}

async fn install(
    manager: &PluginManager,
    name: &str,
    channel: &str,
    yes: bool,
    file: Option<&PathBuf>,
) -> anyhow::Result<()> {
    let req = InstallRequest {
        qualified: name.to_string(),
        channel: parse_channel(channel),
        trust: yes,
        file: file.cloned(),
        ..Default::default()
    };
    let outcome = manager.install(req).await?;
    if outcome.needs_trust {
        anyhow::bail!(
            "plugin '{name}' is published by '{}', which is not in your trusted publishers.\n  Run again with `--yes` to confirm this install (does NOT auto-trust the publisher).\n  To permanently trust: `easybot plugin trust {} --public-key <base64>`",
            outcome.publisher,
            outcome.publisher
        );
    }
    let verb = if outcome.upgraded {
        "Updated"
    } else {
        "Installed"
    };
    println!("✓ {verb} {} v{}", outcome.name, outcome.version);
    println!("  This install is signature-verified.");
    Ok(())
}

async fn uninstall(manager: &PluginManager, name: &str) -> anyhow::Result<()> {
    manager.uninstall(name).await?;
    println!("✓ Uninstalled {name}");
    Ok(())
}

async fn set_enabled(manager: &PluginManager, name: &str, enabled: bool) -> anyhow::Result<()> {
    manager.set_enabled(name, enabled).await?;
    if enabled {
        println!("✓ Enabled {name} (takes effect on next start)");
    } else {
        println!("✓ Disabled {name} (adapter stopped)");
    }
    Ok(())
}

async fn update(
    manager: &PluginManager,
    name: &str,
    latest: bool,
    channel: Option<&str>,
) -> anyhow::Result<()> {
    let opts = UpdateOptions {
        latest,
        channel: channel.map(parse_channel),
    };
    let outcome = manager.update(name, opts).await?;
    println!(
        "✓ Updated {} → v{} (signature-verified)",
        outcome.name, outcome.version
    );
    Ok(())
}

async fn trust(manager: &PluginManager, publisher: &str, public_key: &str) -> anyhow::Result<()> {
    manager.trust_publisher(publisher, public_key).await?;
    println!("✓ Trusted publisher {publisher}");
    println!("  Trust entries are stored per-user in `{{home}}/plugins/.trust`.");
    Ok(())
}

async fn inspect(manager: &PluginManager, name: &str) -> anyhow::Result<()> {
    let installed = manager.list_installed().await;
    let Some(p) = installed.into_iter().find(|p| p.name == name) else {
        anyhow::bail!("plugin '{name}' is not installed");
    };
    println!(
        "Plugin: {}/{}",
        p.publisher.as_deref().unwrap_or("-"),
        p.name
    );
    println!("  version: v{}  (sdk {})", p.version, p.sdk_version);
    println!("  enabled: {}", p.enabled);
    match (p.signed, p.signature_valid) {
        (false, _) => println!("  signature: none"),
        (true, Some(true)) => println!("  signature: valid"),
        (true, Some(false)) => println!("  signature: INVALID"),
        (true, None) => println!("  signature: present (unverified)"),
    }
    if let Some(plat) = &p.platform {
        println!("  loaded: yes (as {plat})");
    } else if let Some(e) = &p.load_error {
        println!("  loaded: no — {e}");
    } else {
        println!("  loaded: no");
    }
    Ok(())
}

// ══════════════════════════════════════════════════════════════════
// 内部工具
// ══════════════════════════════════════════════════════════════════

fn parse_channel(s: &str) -> PluginChannel {
    match s.to_ascii_lowercase().as_str() {
        "beta" => PluginChannel::Beta,
        _ => PluginChannel::Stable,
    }
}

fn channel_label(c: PluginChannel) -> &'static str {
    match c {
        PluginChannel::Stable => "stable",
        PluginChannel::Beta => "beta",
    }
}

/// 加载基础配置并合并 `gateway.local.yaml`（plugins 段：registries / trusted_publishers）
async fn load_merged_config(paths: &easybot_core::config::EasyBotPaths) -> GatewayConfig {
    let mut config = easybot_core::config::load_config(&paths.config_file)
        .await
        .unwrap_or_default();
    if paths.local_config_file.exists()
        && let Ok(local) = easybot_core::config::load_config(&paths.local_config_file).await
    {
        let base_val = serde_yaml::to_value(&config).unwrap_or_default();
        let local_val = serde_yaml::to_value(&local).unwrap_or_default();
        let mut merged = base_val;
        easybot_core::config::merge_configs(&mut merged, local_val);
        if let Ok(c) = serde_yaml::from_value::<GatewayConfig>(merged) {
            config = c;
        }
    }
    config
}
