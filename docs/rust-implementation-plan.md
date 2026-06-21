# EasyBot – Rust 实现计划

> 本文档将架构设计翻译为 Rust 语言的实现计划，涵盖项目结构、crate 划分、关键类型设计、第三方依赖选型、分阶段实施路径。

---

## 第一章：技术选型

### 1.1 运行时与核心库

| 领域 | 库 | 理由 |
|------|----|------|
| 异步运行时 | **tokio** (1.x) | Rust 异步生态事实标准，支持多线程 work-stealing 调度器 |
| HTTP 服务端 | **axum** (0.8+) | 基于 tokio/tower/hyper，中间件模型简洁，与 tokio 深度集成 |
| WebSocket | **axum** 内置 + **tokio-tungstenite** | axum 的 `ws` 模块底层使用 tokio-tungstenite |
| 序列化 | **serde** + **serde_json** | Rust 序列化事实标准 |
| 配置解析 | **serde_yaml** | YAML 格式配置解析 |
| SQL | **sqlx** (async, compile-time checked) | 支持 SQLite/PostgreSQL/MySQL，编译期查询检查 |
| 日志 | **tracing** + **tracing-subscriber** | 结构化日志，支持 span 追踪，与 tokio/axum 生态一致 |
| 命令行 | **clap** (4.x) | 参数解析，自动生成帮助文档 |
| UUID | **uuid** (v7) | 消息/会话 ID |
| 时间 | **chrono** | 时间戳处理 |
| 正则 | **regex** | 消息解析 |
| 加密 | **argon2** (密码哈希) + **hmac** + **sha2** | API Key 哈希、Webhook 签名 |
| TLS | **rustls** | 原生 Rust TLS，无需 OpenSSL 依赖 |
| 跨平台路径 | **dirs** (5.x) | 获取各平台标准用户目录：Linux XDG / macOS ~/Library / Windows %APPDATA% |
| 热重载 | **notify** (文件监控) + **hot-lib** (实验) | 配置文件变更监听 |
| 测试 | **rstest** + **wiremock** | 参数化测试 + HTTP mock |
| 编译优化 | **mimalloc** 或 **jemalloc** allocator | 高性能内存分配器 |

### 1.2 Cargo.toml 核心依赖预览

```toml
[package]
name = "easybot"
version = "0.1.0"
edition = "2021"

[dependencies]
# 异步运行时
tokio = { version = "1", features = ["full"] }

# HTTP / WebSocket
axum = { version = "0.8", features = ["ws", "macros"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace", "limit"] }

# 序列化
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"

# 数据库
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "postgres", "chrono", "uuid"] }

# 日志
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }

# CLI
clap = { version = "4", features = ["derive"] }

# 工具
uuid = { version = "1", features = ["v7", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
regex = "1"
thiserror = "2"
anyhow = "1"
once_cell = "1"

# 加密
argon2 = "0.5"
hmac = "0.12"
sha2 = "0.10"

# 跨平台路径
dirs = "5"

# 异步工具
futures = "0.3"
dashmap = "6"         # 高性能并发 HashMap
parking_lot = "0.12"  # 更快的 Mutex/RwLock
```

---

## 第二章：跨平台部署与配置目录管理

### 2.1 各平台用户数据目录标准

| 平台 | 标准用户数据目录 | Rust 解析方式 |
|------|-----------------|--------------|
| **macOS** | `~/Library/Application Support/easybot/` | `dirs::data_dir()` → `/Users/xxx/Library/Application Support/` |
| **Linux** | `~/.local/share/easybot/` 或 `$XDG_DATA_HOME/easybot/` | `dirs::data_dir()` → `/home/xxx/.local/share/` |
| **Windows** | `%APPDATA%\easybot\` | `dirs::data_dir()` → `C:\Users\xxx\AppData\Roaming\` |

**统一用户配置目录 `~/.easybot/` 的方案：**

考虑到运维便利性和用户的统一管理习惯，我们支持两种配置目录模式，按优先级决定：

```
优先级 1: 命令行 --dir 参数显式指定
  例: easybot --dir /etc/my-gateway

优先级 2: 环境变量 EASYBOT_HOME
  例: export EASYBOT_HOME=~/.config/easybot

优先级 3: 传统路径 ~/.easybot/
  当该目录已存在时自动使用（方便迁移用户）

优先级 4: 平台标准目录
  macOS:   ~/Library/Application Support/easybot/
  Linux:   ~/.local/share/easybot/
  Windows: %APPDATA%\easybot\
```

### 2.2 配置目录结构

```
~/.easybot/                          # EasyBot 根目录
├── gateway.yaml                        # 主配置文件
├── gateway.local.yaml                  # 本地覆盖（不上传到版本控制）
├── .env                                # 环境变量（敏感信息，600 权限）
│
├── data/                               # 持久化数据
│   ├── gateway.db                      # SQLite 数据库（会话/消息历史/API Keys）
│   └── media_cache/                    # 媒体文件缓存
│
├── logs/                               # 日志文件
│   ├── gateway.log                     # 主日志
│   └── audit.log                       # 审计日志（API 调用记录）
│
├── plugins/                            # 第三方适配器插件
│   ├── my-custom-adapter/              # 每个插件一个子目录
│   │   ├── plugin.yaml
│   │   └── libadapter.so
│   └── ...
│
├── certs/                              # TLS 证书（可选）
│   ├── server.crt
│   └── server.key
│
└── secrets/                            # 密钥存储（可选，600 权限）
    └── api_keys.json                   # 持久化的 API Key 哈希
```

### 2.3 Home Directory 解析实现

```rust
// crate: easybot-core/src/config/home.rs

use std::path::PathBuf;

/// 获取 EasyBot 的根目录
///
/// 优先级:
///   1. --dir CLI 参数（由调用方传入）
///   2. EASYBOT_HOME 环境变量
///   3. ~/.easybot/（若存在）
///   4. 平台标准数据目录
pub fn resolve_gateway_home(cli_override: Option<PathBuf>) -> PathBuf {
    // 1. CLI 参数
    if let Some(dir) = cli_override {
        return dir;
    }

    // 2. 环境变量
    if let Ok(env_dir) = std::env::var("EASYBOT_HOME") {
        let path = PathBuf::from(env_dir);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }

    // 3. 传统路径 ~/.easybot/
    if let Some(home) = dirs::home_dir() {
        let legacy = home.join(".easybot");
        if legacy.exists() {
            return legacy;
        }
    }

    // 4. 平台标准目录
    platform_default_data_dir()
}

/// 按平台返回标准数据目录
fn platform_default_data_dir() -> PathBuf {
    if let Some(base) = dirs::data_dir() {
        base.join("easybot")
    } else {
        // 保底：当前目录
        PathBuf::from("./.easybot")
    }
}

/// 获取子目录（自动创建）
pub fn ensure_subdir(home: &std::path::Path, name: &str) -> std::io::Result<PathBuf> {
    let dir = home.join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 获取配置目录下各子路径
pub struct GatewayPaths {
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub plugins_dir: PathBuf,
    pub certs_dir: PathBuf,
    pub secrets_dir: PathBuf,
    pub config_file: PathBuf,
    pub local_config_file: PathBuf,
    pub db_path: PathBuf,
}

impl GatewayPaths {
    pub fn new(home: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&home)?;
        Ok(Self {
            data_dir: ensure_subdir(&home, "data")?,
            logs_dir: ensure_subdir(&home, "logs")?,
            plugins_dir: ensure_subdir(&home, "plugins")?,
            certs_dir: ensure_subdir(&home, "certs")?,
            secrets_dir: ensure_subdir(&home, "secrets")?,
            config_file: home.join("gateway.yaml"),
            local_config_file: home.join("gateway.local.yaml"),
            db_path: home.join("data").join("gateway.db"),
            home,
        })
    }

    /// 平台标准配置文件搜寻顺序：
    /// gateway.yaml ← gateway.local.yaml（覆盖前者）
    pub fn load_config_chain(&self) -> Result<(serde_yaml::Value, serde_yaml::Value), anyhow::Error> {
        use std::fs;

        let base: serde_yaml::Value = if self.config_file.exists() {
            let content = fs::read_to_string(&self.config_file)?;
            serde_yaml::from_str(&content)?
        } else {
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
        };

        let local: serde_yaml::Value = if self.local_config_file.exists() {
            let content = fs::read_to_string(&self.local_config_file)?;
            serde_yaml::from_str(&content)?
        } else {
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
        };

        Ok((base, local))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_with_env() {
        std::env::set_var("EASYBOT_HOME", "/tmp/test-gateway");
        let home = resolve_gateway_home(None);
        assert_eq!(home, std::path::PathBuf::from("/tmp/test-gateway"));
        std::env::remove_var("EASYBOT_HOME");
    }

    #[test]
    fn test_cli_override_takes_precedence() {
        std::env::set_var("EASYBOT_HOME", "/tmp/should-not-use");
        let home = resolve_gateway_home(Some(PathBuf::from("/opt/gateway")));
        assert_eq!(home, std::path::PathBuf::from("/opt/gateway"));
        std::env::remove_var("EASYBOT_HOME");
    }
}
```

### 2.4 平台配置分层体系

```yaml
# gateway.yaml（基础配置，可提交版本控制）

server:
  host: "127.0.0.1"
  port: 8080
  tls:
    enabled: false
    # certs/server.crt / certs/server.key 自动从配置目录加载
    certFile: ""
    keyFile: ""

storage:
  type: "sqlite"
  # dbPath 默认指向 {home}/data/gateway.db
  # dbPath: ""

adapters:
  telegram:
    enabled: true
    token: "${TELEGRAM_BOT_TOKEN}"

# ... 其余配置
```

```yaml
# gateway.local.yaml（本地覆盖，.gitignore，不上传版本控制）
# 此文件中的值会递归覆盖 gateway.yaml 中的对应键

adapters:
  telegram:
    token: "123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11"  # 本地测试用 token

server:
  port: 8081
```

**配置合并规则：**
```
1. 加载 gateway.yaml 得到基础配置
2. 若 gateway.local.yaml 存在，将其值递归 merge 到基础配置上
3. 对配置中所有 ${VAR_NAME} 执行环境变量替换
4. 验证配置结构的完整性
```

### 2.5 跨平台注意事项

| 关注点 | macOS | Linux | Windows |
|--------|-------|-------|---------|
| **路径分隔符** | `/` | `/` | `\`（用 `PathBuf` / `Path::join` 自动处理） |
| **换行符** | `\n` | `\n` | `\r\n`（Rust `fs` 在文本模式下自动转换） |
| **信号处理** | `SIGTERM`, `SIGINT`（ctrl+c） | `SIGTERM`, `SIGINT`, `SIGHUP` | `ctrl+c` 通过 `tokio::signal::ctrl_c()` |
| **文件权限** | `chmod 600 .env` | `chmod 600 .env` | 无需特殊处理 |
| **守护进程** | launchd / 直接前台运行 | systemd / 前台运行 | 注册为 Windows Service |
| **TLS 根证书** | 系统钥匙串（rustls 自动处理） | `/etc/ssl/certs`（rustls 自动处理） | 系统证书存储（rustls 自动处理） |
| **文件锁** | `flock()` | `flock()` | `LockFileEx()`（通过 `fs2` crate 抽象） |
| **SQLite 路径** | 正常 | 正常 | 需处理 `\` 转义 |
| **默认 Shell** | zsh | bash/dash | cmd/powershell（不影响本服务） |

### 2.6 CLI 命令设计

```bash
# 启动（从默认配置目录）
easybot start

# 启动并指定数据目录
easybot start --dir ~/my-gateway

# 启动并使用指定配置文件（同时保留 data/logs/plugins 在默认目录）
easybot start --config /etc/easybot/custom.yaml

# 初始化配置目录（创建默认 gateway.yaml）
easybot init
# → 创建 ~/.easybot/ 或 ~/Library/Application Support/easybot/
# → 生成默认 gateway.yaml 模板
# → 显示需要配置的环境变量

# 查看状态
easybot status

# 停止
easybot stop

# 验证配置
easybot config validate

# 打印配置路径
easybot config path

# 安装为系统服务
easybot service install
easybot service uninstall
```

### 2.7 平台原生服务集成

```rust
/// 安装系统服务（跨平台抽象）
pub enum ServicePlatform {
    Launchd,   // macOS: ~/Library/LaunchAgents/
    Systemd,   // Linux: /etc/systemd/system/
    WindowsService, // Windows: sc create
}

impl ServicePlatform {
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        { ServicePlatform::Launchd }
        #[cfg(target_os = "linux")]
        { ServicePlatform::Systemd }
        #[cfg(target_os = "windows")]
        { ServicePlatform::WindowsService }
    }

    pub fn install(&self, binary_path: &std::path::Path, config_dir: &std::path::Path) {
        match self {
            ServicePlatform::Launchd => {
                // 生成 ~/Library/LaunchAgents/com.easybot.plist
                // 包含: 二进制路径、配置目录参数、重启策略
            }
            ServicePlatform::Systemd => {
                // 生成 /etc/systemd/system/easybot.service
                // 包含: ExecStart、User、Restart=on-failure
            }
            ServicePlatform::WindowsService => {
                // 使用 sc.exe create 注册服务
            }
        }
    }
}
```

### 2.8 跨平台测试策略

```toml
# Cargo.toml（workspace）
[profile.release]
# 静态链接 C runtime，减少目标系统依赖
# macOS 目标不需要特殊配置
# Linux 目标可能需要 musl 静态编译
# Windows 目标使用 MSVC 工具链
```

```yaml
# .github/workflows/ci.yml
test:
  strategy:
    matrix:
      os: [ubuntu-latest, macos-latest, windows-latest]
  steps:
    - uses: actions/checkout@v4
    - run: cargo build --all-features
    - run: cargo test
    # 跨平台集成测试：
    - run: cargo test --test cross_platform_paths
```

```rust
#[cfg(test)]
mod cross_platform_tests {
    use super::*;

    #[test]
    fn test_platform_data_dir() {
        let home = resolve_gateway_home(None);
        #[cfg(target_os = "macos")]
        assert!(home.to_string_lossy().contains("Application Support"));
        #[cfg(target_os = "linux")]
        assert!(home.to_string_lossy().contains(".local/share"));
        #[cfg(target_os = "windows")]
        assert!(home.to_string_lossy().contains("AppData"));
    }

    #[test]
    fn test_paths_use_unix_separator_on_macos() {
        let home = PathBuf::from("/Users/test/.easybot");
        let paths = GatewayPaths::new(home).unwrap();
        // 确保 join 结果不出现双分隔符
        assert!(!paths.data_dir.to_string_lossy().contains("//"));
    }
}
```

---

## 第三章：项目结构

### 3.1 工作空间（Workspace）布局

```
easybot/
├── Cargo.toml                      # workspace root
├── Cargo.lock
├── gateway.yaml                     # 默认配置文件
├── docker-compose.yml
├── Dockerfile
├── scripts/
│   ├── dev-setup.sh
│   └── test-integration.sh
│
├── crates/
│   ├── easybot-core/             # 核心逻辑层 (库)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types/               # 所有数据类型定义
│   │       │   ├── mod.rs
│   │       │   ├── message.rs       # 消息模型
│   │       │   ├── adapter.rs       # 适配器接口 trait
│   │       │   ├── session.rs       # 会话模型
│   │       │   ├── event.rs         # 事件模型
│   │       │   ├── config.rs        # 配置结构体
│   │       │   └── error.rs         # 错误类型
│   │       ├── bus/                 # 消息总线
│   │       │   ├── mod.rs
│   │       │   └── event_bus.rs
│   │       ├── session/             # 会话管理器
│   │       │   ├── mod.rs
│   │       │   ├── manager.rs
│   │       │   └── store.rs         # 存储实现 trait
│   │       ├── router/              # 消息路由
│   │       │   ├── mod.rs
│   │       │   └── delivery_router.rs
│   │       ├── adapter/             # 适配器管理器
│   │       │   ├── mod.rs
│   │       │   ├── manager.rs
│   │       │   └── registry.rs      # 插件注册表
│   │       ├── auth/                # 认证授权
│   │       │   ├── mod.rs
│   │       │   ├── api_key.rs
│   │       │   └── permissions.rs
│   │       └── stats/               # 统计与健康
│   │           ├── mod.rs
│   │           └── metrics.rs
│   │
│   ├── easybot-api/              # API 服务层 (库)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs            # HTTP server 启动
│   │       ├── routes/              # 路由模块
│   │       │   ├── mod.rs
│   │       │   ├── health.rs
│   │       │   ├── messages.rs
│   │       │   ├── adapters.rs
│   │       │   ├── chats.rs
│   │       │   ├── sessions.rs
│   │       │   ├── config.rs
│   │       │   └── ws.rs            # WebSocket 端点
│   │       ├── middleware/           # 中间件
│   │       │   ├── mod.rs
│   │       │   ├── auth.rs
│   │       │   └── rate_limit.rs
│   │       ├── ws/                  # WebSocket 会话管理
│   │       │   ├── mod.rs
│   │       │   ├── session.rs
│   │       │   └── event_dispatch.rs
│   │       └── response.rs          # 统一响应格式
│   │
│   ├── easybot-adapter-telegram/ # Telegram 适配器 (库)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── adapter.rs
│   │       ├── client.rs            # Telegram Bot API 客户端
│   │       ├── types.rs             # Telegram 特有类型
│   │       └── format.rs            # 消息格式转换
│   │
│   ├── easybot-adapter-discord/  # Discord 适配器 (库)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── adapter.rs
│   │       └── ...
│   │
│   └── easybot-plugin-sdk/       # 插件 SDK (库)
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── adapter_trait.rs     # 导出适配器 trait
│           ├── types.rs             # 导出核心类型
│           └── macros.rs            # 插件注册宏
│
├── plugins/                         # 外部插件目录 (运行时加载)
│   └── .gitkeep
│
└── bin/                             # 二进制入口
    ├── Cargo.toml
    └── src/
        └── main.rs
```

### 3.2 二进制 Crate (`bin/`)

```toml
# bin/Cargo.toml
[package]
name = "easybot"
version = "0.1.0"
edition = "2021"

[dependencies]
easybot-core = { path = "../crates/easybot-core" }
easybot-api = { path = "../crates/easybot-api" }

# 内置适配器（编译时决定包含哪些）
easybot-adapter-telegram = { path = "../crates/easybot-adapter-telegram", optional = true }
easybot-adapter-discord = { path = "../crates/easybot-adapter-discord", optional = true }

# 可选：动态插件加载支持
# libloading = "0.8"

[features]
default = ["adapter-telegram", "adapter-discord"]
adapter-telegram = ["easybot-adapter-telegram"]
adapter-discord = ["easybot-adapter-discord"]
full = ["adapter-telegram", "adapter-discord"]

[[bin]]
name = "easybot"
path = "src/main.rs"
```

---

## 第四章：关键类型设计（Rust 代码草图）

### 3.1 适配器 Trait

```rust
// crate: easybot-core/src/types/adapter.rs

use async_trait::async_trait;
use crate::types::message::*;
use crate::types::error::*;
use std::fmt::Debug;

/// 适配器能力声明
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Capability {
    pub name: CapabilityName,
    pub supported: bool,
    pub limits: Option<CapabilityLimits>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CapabilityName {
    Text,
    Image,
    Audio,
    Video,
    Document,
    Interactive,
    Streaming,
    Voice,
    Markdown,
    Html,
    CodeBlock,
    Thread,
    Topic,
    Group,
    ChatList,
    MessageEdit,
    MessageDelete,
    TypingIndicator,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CapabilityLimits {
    pub max_text_length: Option<usize>,
    pub max_file_size: Option<u64>,
    pub max_buttons: Option<usize>,
}

/// 适配器状态
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum AdapterState {
    Created,
    Starting,
    Connected,
    Reconnecting,
    Failed,
    Stopped,
}

/// 健康报告
#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthReport {
    pub status: HealthStatus,
    pub connected: bool,
    pub last_connected_at: Option<i64>,
    pub last_error_at: Option<i64>,
    pub last_error: Option<String>,
    pub messages_in: u64,
    pub messages_out: u64,
    pub errors: u64,
    pub uptime: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Down,
}

/// 连接结果
#[derive(Debug)]
pub struct ConnectResult {
    pub ok: bool,
    pub error: Option<String>,
    pub bot_info: Option<BotInfo>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BotInfo {
    pub name: String,
    pub username: Option<String>,
    pub id: String,
}

/// 平台适配器 Trait
///
/// 每个 IM 平台的连接器必须实现此 trait。
/// 所有方法都是异步的，使用 tokio 运行时。
#[async_trait]
pub trait PlatformAdapter: Send + Sync + Debug {
    // ── 元数据 ──

    /// 平台唯一标识
    fn platform_name(&self) -> &str;

    /// 人类可读的平台名
    fn display_name(&self) -> &str;

    /// 能力列表
    fn capabilities(&self) -> &[Capability];

    // ── 生命周期 ──

    /// 初始化（不连接网络）
    async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError>;

    /// 连接到 IM 平台
    async fn connect(&mut self) -> Result<ConnectResult, GatewayError>;

    /// 断开连接
    async fn disconnect(&mut self) -> Result<(), GatewayError>;

    /// 当前连接状态
    fn state(&self) -> AdapterState;

    /// 健康检查
    async fn health(&self) -> HealthReport;

    // ── 消息发送 ──

    /// 发送文本消息
    async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError>;

    /// 发送媒体消息（可选）
    async fn send_media(&self, params: SendMediaParams) -> Result<SendResult, GatewayError> {
        Err(GatewayError::capability_not_supported("send_media"))
    }

    /// 发送交互式消息（可选）
    async fn send_interactive(
        &self,
        params: SendInteractiveParams,
    ) -> Result<SendResult, GatewayError> {
        Err(GatewayError::capability_not_supported("send_interactive"))
    }

    /// 发送输入指示（可选）
    async fn send_typing(&self, chat_id: &str) -> Result<(), GatewayError> {
        Err(GatewayError::capability_not_supported("send_typing"))
    }

    // ── 消息管理 ──

    /// 编辑消息（可选）
    async fn edit_message(
        &self,
        params: EditMessageParams,
    ) -> Result<EditResult, GatewayError> {
        Err(GatewayError::capability_not_supported("edit_message"))
    }

    /// 删除消息（可选）
    async fn delete_message(
        &self,
        chat_id: &str,
        message_id: &str,
    ) -> Result<DeleteResult, GatewayError> {
        Err(GatewayError::capability_not_supported("delete_message"))
    }

    // ── 流式发送 ──

    /// 发送流式草稿（可选）
    async fn send_draft(
        &self,
        chat_id: &str,
        draft_id: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<(), GatewayError> {
        Err(GatewayError::capability_not_supported("send_draft"))
    }

    // ── 查询 ──

    /// 获取聊天信息
    async fn get_chat_info(&self, chat_id: &str) -> Result<ChatInfo, GatewayError>;

    /// 列出聊天列表（可选）
    async fn list_chats(&self, filter: Option<ChatFilter>) -> Result<Vec<ChatInfo>, GatewayError> {
        Err(GatewayError::capability_not_supported("list_chats"))
    }

    // ── 配置 ──

    /// 返回运行时配置
    fn runtime_config(&self) -> AdapterRuntimeConfig;

    // ── 入站消息处理器注册 ──

    /// 设置入站消息处理器（由核心层调用）
    fn set_message_handler(&mut self, handler: Box<dyn MessageHandlerFn>);

    /// 设置按钮回调处理器（由核心层调用）
    fn set_callback_handler(&mut self, handler: Box<dyn CallbackHandlerFn>);
}

/// 入站消息处理器
pub type MessageHandlerFn =
    Arc<dyn Fn(InboundMessage) -> BoxFuture<'static, Result<(), GatewayError>> + Send + Sync>;

/// 按钮回调处理器
pub type CallbackHandlerFn =
    Arc<dyn Fn(CallbackEvent) -> BoxFuture<'static, Result<(), GatewayError>> + Send + Sync>;
```

### 3.2 消息模型

```rust
// crate: easybot-core/src/types/message.rs

use serde::{Deserialize, Serialize};

/// 入站消息（IM 平台 → 网关）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    pub id: String,
    pub platform: String,
    pub chat_id: String,
    pub chat_name: Option<String>,
    pub chat_type: ChatType,
    pub text: Option<String>,
    pub author: Author,
    pub timestamp: i64,
    pub media: Option<Vec<MediaAttachment>>,
    pub command: Option<CommandData>,
    pub callback: Option<CallbackData>,
    pub reply_to: Option<ReplyReference>,
    pub thread_id: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// 出站消息（网关 → IM 平台）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub text: String,
    #[serde(default)]
    pub parse_mode: ParseMode,
}

/// 发送文本参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendTextParams {
    pub chat_id: String,
    pub message: OutboundMessage,
    pub reply_to: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// 发送媒体参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMediaParams {
    pub chat_id: String,
    pub media: MediaAttachment,
    pub text: Option<String>,
    pub reply_to: Option<String>,
}

/// 发送交互式消息参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendInteractiveParams {
    pub chat_id: String,
    pub text: String,
    pub keyboard: InlineKeyboard,
    pub reply_to: Option<String>,
}

/// 编辑消息参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditMessageParams {
    pub chat_id: String,
    pub message_id: String,
    pub message: OutboundMessage,
    pub keyboard: Option<InlineKeyboard>,
}

/// 发送结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResult {
    pub success: bool,
    pub message_id: Option<String>,
    pub timestamp: Option<i64>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub retryable: bool,
}

/// 编辑结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditResult {
    pub success: bool,
    pub updated_at: Option<i64>,
    pub error: Option<String>,
}

/// 删除结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResult {
    pub success: bool,
    pub error: Option<String>,
}

// ── 支持类型 ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChatType {
    Dm,
    Group,
    Channel,
    Thread,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Author {
    pub id: String,
    pub name: Option<String>,
    pub is_bot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaAttachment {
    pub media_type: MediaType,
    pub url: Option<String>,
    pub data: Option<String>,
    pub mime_type: String,
    pub filename: Option<String>,
    pub caption: Option<String>,
    pub file_size: Option<u64>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MediaType {
    Image,
    Audio,
    Video,
    Document,
    Sticker,
    Animation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandData {
    pub name: String,
    pub args: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackData {
    pub data: String,
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyReference {
    pub message_id: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParseMode {
    Markdown,
    Html,
    None,
}

impl Default for ParseMode {
    fn default() -> Self {
        ParseMode::Markdown
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineKeyboard {
    pub rows: Vec<KeyboardRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardRow {
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Button {
    pub text: String,
    pub callback_data: Option<String>,
    pub url: Option<String>,
}

/// 按钮回调事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallbackEvent {
    pub id: String,
    pub platform: String,
    pub chat_id: String,
    pub user_id: String,
    pub data: String,
    pub message_id: String,
    pub metadata: Option<serde_json::Value>,
}
```

### 3.3 错误类型

```rust
// crate: easybot-core/src/types/error.rs

use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum GatewayError {
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Platform '{0}' not found")]
    PlatformNotFound(String),

    #[error("Chat '{0}' not found")]
    ChatNotFound(String),

    #[error("Adapter not connected: {0}")]
    AdapterNotConnected(String),

    #[error("Message too long: {current} > {max}")]
    MessageTooLong { current: usize, max: usize },

    #[error("Rate limited by platform: retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("Capability not supported: {0}")]
    CapabilityNotSupported(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Storage error: {0}")]
    StorageError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl GatewayError {
    pub fn capability_not_supported(name: &str) -> Self {
        GatewayError::CapabilityNotSupported(name.to_string())
    }

    pub fn to_error_code(&self) -> &'static str {
        match self {
            GatewayError::InvalidRequest(_) => "INVALID_REQUEST",
            GatewayError::PlatformNotFound(_) => "PLATFORM_NOT_FOUND",
            GatewayError::ChatNotFound(_) => "CHAT_NOT_FOUND",
            GatewayError::AdapterNotConnected(_) => "ADAPTER_NOT_CONNECTED",
            GatewayError::MessageTooLong { .. } => "MESSAGE_TOO_LONG",
            GatewayError::RateLimited { .. } => "RATE_LIMITED",
            GatewayError::CapabilityNotSupported(_) => "CAPABILITY_NOT_SUPPORTED",
            GatewayError::AuthFailed(_) => "AUTH_FAILED",
            GatewayError::Unauthorized(_) => "UNAUTHORIZED",
            GatewayError::ConfigError(_) => "CONFIG_ERROR",
            GatewayError::StorageError(_) => "STORAGE_ERROR",
            GatewayError::Internal(_) => "INTERNAL_ERROR",
        }
    }

    pub fn http_status(&self) -> axum::http::StatusCode {
        match self {
            GatewayError::InvalidRequest(_) => axum::http::StatusCode::BAD_REQUEST,
            GatewayError::AuthFailed(_) | GatewayError::Unauthorized(_) => {
                axum::http::StatusCode::UNAUTHORIZED
            }
            GatewayError::PlatformNotFound(_) | GatewayError::ChatNotFound(_) => {
                axum::http::StatusCode::NOT_FOUND
            }
            GatewayError::AdapterNotConnected(_) => {
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            }
            GatewayError::RateLimited { .. } => axum::http::StatusCode::TOO_MANY_REQUESTS,
            GatewayError::MessageTooLong { .. } | GatewayError::CapabilityNotSupported(_) => {
                axum::http::StatusCode::BAD_REQUEST
            }
            GatewayError::ConfigError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            GatewayError::StorageError(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            GatewayError::Internal(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
```

### 3.4 事件总线

```rust
// crate: easybot-core/src/bus/event_bus.rs

use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::{debug, error};

/// 网关内部事件
#[derive(Debug, Clone, serde::Serialize)]
pub struct GatewayEvent {
    pub event_type: String,         // "message.inbound" | "adapter.connected" | ...
    pub source: String,             // 事件源
    pub timestamp: i64,
    pub data: serde_json::Value,
    pub metadata: Option<EventMetadata>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct EventMetadata {
    pub correlation_id: Option<String>,
    pub session_key: Option<String>,
}

/// 消息总线
///
/// 使用 tokio broadcast channel 实现一对多分发。
/// 每个事件类型有独立的 channel，订阅者按需订阅。
pub struct EventBus {
    /// event_type → broadcast::Sender
    channels: DashMap<String, broadcast::Sender<GatewayEvent>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            channels: DashMap::new(),
        }
    }

    /// 发布事件
    pub fn publish(&self, event: GatewayEvent) {
        let event_type = event.event_type.clone();
        if let Some(tx) = self.channels.get(&event_type) {
            if let Err(e) = tx.send(event.clone()) {
                // 没有活跃订阅者不是错误
                if e.len() > 0 {
                    debug!("event '{}' had no receivers (dropped)", event_type);
                }
            }
        }
    }

    /// 订阅事件
    pub fn subscribe(&self, event_type: &str) -> broadcast::Receiver<GatewayEvent> {
        let entry = self
            .channels
            .entry(event_type.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(256);
                tx
            });
        entry.value().subscribe()
    }

    /// 创建按多个事件类型过滤的订阅
    pub fn subscribe_many(&self, event_types: &[&str]) -> broadcast::Receiver<GatewayEvent> {
        // 使用一个全局 channel 收所有事件
        let (global_tx, rx) = broadcast::channel(512);
        let tx = global_tx.clone();
        for et in event_types {
            let tx = tx.clone();
            let et = et.to_string();
            let sub = self.subscribe(&et);
            tokio::spawn(async move {
                let mut sub = sub;
                while let Ok(event) = sub.recv().await {
                    let _ = tx.send(event);
                }
            });
        }
        // 也订阅通配符 "*"
        // (简化处理，实际可优化)
        rx
    }
}
```

### 3.5 适配器管理器

```rust
// crate: easybot-core/src/adapter/manager.rs

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};

use crate::types::adapter::*;
use crate::types::error::GatewayError;

/// 适配器工厂
pub type AdapterFactory =
    Arc<dyn Fn(AdapterConfig) -> BoxFuture<'static, Result<Box<dyn PlatformAdapter>, GatewayError>>
        + Send + Sync>;

/// 适配器管理器
pub struct AdapterManager {
    /// 工厂注册表：platform_name → factory
    factories: RwLock<HashMap<String, AdapterFactory>>,
    /// 运行中的适配器实例
    adapters: RwLock<HashMap<String, Box<dyn PlatformAdapter>>>,
    /// 适配器状态
    statuses: RwLock<HashMap<String, AdapterStatus>>,
    /// 健康轮询间隔
    health_poll_interval: tokio::time::Duration,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AdapterStatus {
    pub platform: String,
    pub display_name: String,
    pub state: AdapterState,
    pub connected: bool,
    pub health: Option<HealthStatus>,
    pub last_error: Option<String>,
    pub uptime: Option<u64>,
}

impl AdapterManager {
    pub fn new(health_poll_interval: tokio::time::Duration) -> Self {
        Self {
            factories: RwLock::new(HashMap::new()),
            adapters: RwLock::new(HashMap::new()),
            statuses: RwLock::new(HashMap::new()),
            health_poll_interval,
        }
    }

    /// 注册适配器工厂
    pub async fn register(&self, name: &str, factory: AdapterFactory) {
        self.factories.write().await.insert(name.to_string(), factory);
        info!("Registered adapter factory: {}", name);
    }

    /// 启动适配器
    pub async fn start(
        &self,
        platform: &str,
        config: AdapterConfig,
    ) -> Result<StartResult, GatewayError> {
        let factory = {
            let factories = self.factories.read().await;
            factories.get(platform).cloned()
        };
        let factory = factory.ok_or_else(|| {
            GatewayError::PlatformNotFound(platform.to_string())
        })?;

        let mut adapter = factory(config).await?;
        let name = adapter.platform_name().to_string();

        // init
        let init_result = adapter.init(config).await?;
        if !init_result.ok {
            return Ok(StartResult {
                ok: false,
                platform: name,
                error: init_result.error,
                bot_info: None,
            });
        }

        // connect
        let connect_result = adapter.connect().await?;

        // 保存实例
        {
            let mut adapters = self.adapters.write().await;
            adapters.insert(name.clone(), adapter);
        }

        // 更新状态
        {
            let mut statuses = self.statuses.write().await;
            statuses.insert(
                name.clone(),
                AdapterStatus {
                    platform: name.clone(),
                    display_name: "...".to_string(),
                    state: AdapterState::Connected,
                    connected: connect_result.ok,
                    health: None,
                    last_error: None,
                    uptime: if connect_result.ok { Some(0) } else { None },
                },
            );
        }

        info!("Adapter '{}' started (connected: {})", name, connect_result.ok);

        Ok(StartResult {
            ok: connect_result.ok,
            platform: name,
            error: connect_result.error,
            bot_info: connect_result.bot_info,
        })
    }

    /// 停止适配器
    pub async fn stop(&self, platform: &str) -> Result<(), GatewayError> {
        let mut adapters = self.adapters.write().await;
        if let Some(mut adapter) = adapters.remove(platform) {
            adapter.disconnect().await?;
            info!("Adapter '{}' stopped", platform);
        }
        Ok(())
    }

    /// 获取适配器引用
    pub async fn get(&self, platform: &str) -> Option<impl Deref<Target = dyn PlatformAdapter> + '_> {
        // 简化处理，实际需要 RwLock 读锁
        let adapters = self.adapters.read().await;
        // Box<dyn PlatformAdapter> 不能直接 Deref 返回引用
        // 这里需要进一步设计生命周期管理
        todo!("need proper borrow management")
    }

    /// 列出所有适配器状态
    pub async fn list_statuses(&self) -> Vec<AdapterStatus> {
        let statuses = self.statuses.read().await;
        statuses.values().cloned().collect()
    }

    /// 启动所有已注册的适配器
    pub async fn start_all(
        &self,
        configs: HashMap<String, AdapterConfig>,
    ) -> (Vec<String>, Vec<(String, String)>) {
        let mut succeeded = Vec::new();
        let mut failed = Vec::new();

        let factories = self.factories.read().await;
        for (platform, config) in &configs {
            if factories.contains_key(platform) {
                match self.start(platform, config.clone()).await {
                    Ok(r) if r.ok => succeeded.push(platform.clone()),
                    Ok(r) => failed.push((platform.clone(), r.error.unwrap_or_default())),
                    Err(e) => failed.push((platform.clone(), e.to_string())),
                }
            }
        }

        (succeeded, failed)
    }

    /// 停止所有适配器
    pub async fn stop_all(&self) {
        let mut adapters = self.adapters.write().await;
        for (name, mut adapter) in adapters.drain() {
            if let Err(e) = adapter.disconnect().await {
                warn!("Error disconnecting adapter '{}': {}", name, e);
            }
        }
    }
}
```

### 3.6 API Server 路由

```rust
// crate: easybot-api/src/routes/messages.rs

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;

use easybot_core::types::message::*;
use easybot_core::types::error::GatewayError;
use crate::AppState;

/// POST /api/v1/messages/send
pub async fn send_message(
    State(state): State<AppState>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    // 解析目标
    let (platform, chat_id) = parse_target(&req.target)
        .ok_or_else(|| GatewayError::InvalidRequest("invalid target format".into()))?;

    // 找到适配器
    let adapter = state.adapter_manager.get(&platform).await
        .ok_or_else(|| GatewayError::PlatformNotFound(platform.clone()))?;

    // 检查适配器连接状态
    if !adapter.is_connected() {
        return Err(GatewayError::AdapterNotConnected(platform));
    }

    // 发送消息
    let result = adapter
        .send(SendTextParams {
            chat_id,
            message: OutboundMessage {
                text: req.text,
                parse_mode: req.parse_mode.unwrap_or(ParseMode::Markdown),
            },
            reply_to: req.reply_to,
            metadata: req.metadata,
        })
        .await?;

    // 发布事件
    state.event_bus.publish(GatewayEvent {
        event_type: "message.sent".into(),
        source: "api".into(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        data: serde_json::to_value(&result).unwrap_or_default(),
        metadata: None,
    });

    Ok(Json(serde_json::json!({
        "id": result.message_id,
        "status": if result.success { "sent" } else { "failed" },
        "platform": platform,
        "chatId": chat_id,
        "messageId": result.message_id,
        "timestamp": result.timestamp,
    })))
}

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub target: String,
    pub text: String,
    pub parse_mode: Option<ParseMode>,
    pub reply_to: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// 解析 "platform:chatId" 格式
fn parse_target(target: &str) -> Option<(String, String)> {
    let colon = target.find(':')?;
    let platform = target[..colon].to_string();
    let chat_id = target[colon + 1..].to_string();
    if platform.is_empty() || chat_id.is_empty() {
        return None;
    }
    Some((platform, chat_id))
}
```

```rust
// crate: easybot-api/src/routes/ws.rs

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};

use crate::AppState;
use easybot_core::types::event::GatewayEvent;

/// GET /api/v1/ws
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // 订阅事件总线
    let mut event_rx = state.event_bus.subscribe_many(&[
        "message.inbound",
        "message.sent",
        "message.failed",
        "adapter.connected",
        "adapter.disconnected",
        "adapter.error",
    ]);

    // 认证等待
    let mut authenticated = false;
    let mut event_seq = 0u64;

    loop {
        tokio::select! {
            // 接收客户端消息
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // 处理 WS 帧
                        if !authenticated {
                            if let Some(token) = extract_auth_token(&text) {
                                // 验证 API Key
                                match state.auth_manager.authenticate(&token).await {
                                    Ok(info) => {
                                        authenticated = true;
                                        let _ = sender.send(Message::Text(
                                            serde_json::json!({"type": "auth_ok", "info": info}).to_string().into()
                                        )).await;
                                    }
                                    Err(_) => {
                                        let _ = sender.send(Message::Text(
                                            r#"{"type":"error","message":"auth failed"}"#.into()
                                        )).await;
                                        break;
                                    }
                                }
                            }
                        } else {
                            // 处理业务帧（订阅、发送等）
                            handle_client_frame(&text, &state).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }

            // 推送网关事件到客户端
            event = event_rx.recv() => {
                if let Ok(event) = event {
                    event_seq += 1;
                    let frame = serde_json::json!({
                        "type": "event",
                        "event": event.event_type,
                        "data": event.data,
                        "seq": event_seq,
                    });
                    if sender.send(Message::Text(frame.to_string().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}
```

### 3.7 二进制入口

```rust
// bin/src/main.rs

use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "easybot", about = "EasyBot Service")]
struct Cli {
    #[arg(short, long, default_value = "./gateway.yaml")]
    config: String,

    #[arg(short, long)]
    daemon: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // 解析 CLI
    let cli = Cli::parse();

    // 加载配置
    let config = easybot_core::config::load_config(&cli.config).await?;

    // 创建组件
    let event_bus = Arc::new(easybot_core::bus::EventBus::new());
    let session_store = Arc::new(easybot_core::session::store::SqlSessionStore::new(&config.storage).await?);
    let session_manager = Arc::new(easybot_core::session::Manager::new(session_store));
    let adapter_manager = Arc::new(easybot_core::adapter::Manager::new(
        tokio::time::Duration::from_secs(30),
    ));
    let auth_manager = Arc::new(easybot_core::auth::ApiKeyManager::new(&config.storage).await?);
    let delivery_router = Arc::new(easybot_core::router::DeliveryRouter::new(
        adapter_manager.clone(),
    ));

    // 注册内置适配器工厂
    #[cfg(feature = "adapter-telegram")]
    {
        let factory = Arc::new(|config| {
            Box::pin(async move {
                Ok(Box::new(
                    easybot_adapter_telegram::TelegramAdapter::new(config),
                ) as Box<dyn PlatformAdapter>)
            })
        });
        adapter_manager.register("telegram", factory).await;
    }

    #[cfg(feature = "adapter-discord")]
    {
        let factory = Arc::new(|config| {
            Box::pin(async move {
                Ok(Box::new(
                    easybot_adapter_discord::DiscordAdapter::new(config),
                ) as Box<dyn PlatformAdapter>)
            })
        });
        adapter_manager.register("discord", factory).await;
    }

    // 启动适配器
    let (succeeded, failed) = adapter_manager.start_all(config.adapters.clone()).await;
    tracing::info!("Adapters started: {:?}, failed: {:?}", succeeded, failed);

    // 构建 API 应用状态
    let app_state = easybot_api::AppState {
        event_bus: event_bus.clone(),
        adapter_manager: adapter_manager.clone(),
        session_manager: session_manager.clone(),
        auth_manager: auth_manager.clone(),
        delivery_router: delivery_router.clone(),
    };

    // 启动 API 服务器
    let api_server = easybot_api::Server::new(config.server, app_state);
    let server_handle = api_server.start().await?;

    // 等待关闭信号
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down...");

    // 优雅关闭
    adapter_manager.stop_all().await;
    server_handle.shutdown().await;

    Ok(())
}
```

---

## 第五章：分阶段实施计划

### Phase 1：最小可行产品（MVP）—— 核心链路打通

**目标：** 实现 REST API 发送消息到单个 IM 平台（Telegram），验证核心链路。

**预计工时：** 2-3 周

| 任务 | 文件 / 模块 | 说明 |
|------|-------------|------|
| 1.1 项目骨架 | workspace, Cargo.toml | crate 布局、依赖配置 |
| 1.2 核心类型 | `core/types/*` | 所有数据类型定义（Message, Adapter, Error 等） |
| 1.3 配置加载 | `core/config.rs` | YAML 解析、环境变量替换 |
| 1.4 PlatformAdapter Trait | `core/types/adapter.rs` | 定义 trait（先只实现 send + connect） |
| 1.5 适配器管理器 | `core/adapter/manager.rs` | 工厂注册 + start/stop |
| 1.6 Telegram 适配器 | `adapter-telegram/` | 实现 Telegram Bot API 的发送与长轮询接收 |
| 1.7 API Server 启动 | `api/server.rs` | axum HTTP 服务器 |
| 1.8 `POST /messages/send` | `api/routes/messages.rs` | 单条消息发送端点 |
| 1.9 `GET /health` | `api/routes/health.rs` | 健康检查端点 |
| 1.10 主入口 | `bin/src/main.rs` | 加载配置、注册适配器、启动服务器 |
| 1.11 基础测试 | tests/ | 单元测试 + Telegram 集成测试 |

**Phase 1 完成时可验证场景：**
```bash
# 启动网关
easybot --config ./gateway.yaml

# 发送消息
curl -X POST http://localhost:8080/api/v1/messages/send \
  -H "Content-Type: application/json" \
  -d '{"target":"telegram:123456789","text":"Hello from EasyBot"}'

# 查看健康
curl http://localhost:8080/api/v1/health
```

---

### Phase 2：双向通信 + 消息接收

**目标：** 支持从 IM 平台接收消息并推送给外部客户端（WebSocket / Webhook）。

**预计工时：** 2-3 周

| 任务 | 模块 | 说明 |
|------|------|------|
| 2.1 事件总线 | `core/bus/` | 基于 broadcast channel 的事件总线 |
| 2.2 入站消息处理器 | `core/adapter/manager.rs` | 适配器接收到消息后的回调链路 |
| 2.3 WebSocket 端点 | `api/routes/ws.rs` | WS 握手、认证、事件推送 |
| 2.4 Webhook 推送 | `api/services/webhook.rs` | HTTP POST 到外部 URL + HMAC 签名 + 重试 |
| 2.5 适配器接收消息 | `adapter-telegram/src/adapter.rs` | Telegram polling 并转化为 InboundMessage |
| 2.6 `GET /messages/history` | `api/routes/messages.rs` | 消息历史查询 |
| 2.7 会话管理器 | `core/session/` | Session 创建/查找/存储 |
| 2.8 `GET /adapters` | `api/routes/adapters.rs` | 适配器状态列表 |
| 2.9 消息入库 | `core/session/store.rs` | 入站/出站消息写入 SQLite |
| 2.10 WebSocket 集成测试 | tests/ | 连接 WS、验证事件推送 |

**Phase 2 完成时可验证场景：**
```bash
# WebSocket 连接接收实时消息
wscat -c ws://localhost:8080/api/v1/ws
> {"type":"auth","token":"sk_xxx"}
< {"type":"auth_ok"}
# 然后在 Telegram 中给 bot 发消息
< {"type":"event","event":"message.inbound","data":{...}}

# 查询历史
curl http://localhost:8080/api/v1/messages?sessionKey=telegram:123456
```

---

### Phase 3：多平台支持

**目标：** 支持至少 3 个 IM 平台（Telegram / Discord / 飞书 / QQ / 微信），验证适配器接口的通用性。

**实际完成平台：5 个** (Telegram ✅, Discord ✅, 飞书 ✅, QQ ✅, 微信 ✅)，**WhatsApp 未实现**（原计划但被飞书/QQ/微信替代）。

**预计工时：** 3-4 周

| 任务 | 模块 | 说明 | 状态 |
|------|------|------|------|
| 3.1 Discord 适配器 | `adapter-discord/` | Discord Bot API，支持 gateway intents | ✅ 完成 |
| 3.2 飞书/Lark 适配器 | `adapter-feishu/` | 飞书 REST API + WebSocket 事件订阅 | ✅ 完成 (替代 WhatsApp) |
| 3.3 QQ 适配器 | `adapter-qq/` | 统一 QQBot 鉴权 + Gateway WebSocket | ✅ 完成 |
| 3.4 微信适配器 | `adapter-wechat/` | 个人微信 iLink Bot API 长轮询 | ✅ 完成 |
| 3.5 适配器接口评审 | `core/types/adapter.rs` | 根据多个实现调整 trait 设计 | ✅ 完成 |
| 3.6 发送媒体 | `adapter-telegram` / `adapter-discord` / etc. | 实现 send_media | ✅ 完成 |
| 3.7 消息格式转换 | `adapter-*/format.rs` | Markdown / HTML 按平台能力转换 | ✅ 完成 |
| 3.8 适配器状态持久化 | `core/adapter/manager.rs` | 状态写入数据库 | ✅ 完成 |
| 3.9 批量发送 | `api/routes/messages.rs` | POST /messages/batch-send | ✅ 完成 |
| — send_interactive | 各适配器 | 交互式按钮/键盘消息 | ✅ 完成 (Telegram/飞书/Discord/QQ; 微信 ❌ API 不支持) |
| — edit_message / delete_message | 各适配器 | 编辑/删除已发送消息 | ✅ 完成 (Telegram/Discord/飞书/QQ; 微信 ❌ API 不支持) |
| — list_chats | 各适配器 | 列出可用聊天列表 | ✅ 完成 (Discord/QQ; Telegram/飞书 stub; 微信 ❌ API 不支持) |

---

### Phase 4：生产级完善

**目标：** 达到生产可用标准。

**预计工时：** 4-6 周

| 任务 | 模块 | 说明 | 状态 |
|------|------|------|------|
| 4.1 API Key 管理 | `core/auth/` | 生成/验证/吊销 | ✅ 完成 (Argon2) |
| 4.2 权限模型 | `core/auth/permissions.rs` | 角色 + 权限检查中间件 | ❌ **未实现** |
| 4.3 速率限制 | `api/middleware/rate_limit.rs` | token bucket 或 sliding window | ✅ 完成 |
| 4.4 配置热重载 | `core/config.rs` + `notify` | 文件变更监听 + 动态重载 | ✅ 完成 (60s 轮询) |
| 4.5 优雅关闭 | `bin/src/main.rs` | signal handler + drain | ✅ 完成 |
| 4.6 PostgreSQL 支持 | `core/session/store.rs` | sqlx 连接池 + migration | ✅ 完成 |
| 4.7 适配器健康轮询 | `core/adapter/manager.rs` | 定时 health() 检查 + 自动重连 | ✅ 完成 (通用健康监控，5 个适配器全部集成 Heartbeat) |
| 4.8 Prometheus 指标 | `api/middleware/metrics.rs` | HTTP 请求数、延迟、错误率 | ✅ 完成 |
| 4.9 交互式按钮 | 各适配器 | 实现 send_interactive | ✅ 完成 (Telegram/Discord/飞书/QQ) |
| 4.10 流式草稿 | `adapter-telegram` / `adapter-discord` | 实现 send_draft | ✅ 完成 (Telegram + Discord; trait 方法 + 类型已添加) |
| 4.11 Docker 镜像 | `Dockerfile` | 多阶段构建 | ✅ 完成 |
| 4.12 HTTPS / WSS | `api/server.rs` | rustls 配置 | ⚠️ 暂缓 (TLS 配置存在但未在应用层处理) |
| 4.13 权限模型 RBAC | `core/auth/permissions.rs` | 角色 + 权限检查中间件 | ⚠️ 暂缓 |
| 4.14 Health 启动时间 | `api/routes/health.rs` | 进程启动时间和 uptime | ✅ 完成 |

---

### Phase 5：插件系统（可选）

**目标：** 支持运行时加载第三方适配器插件。

**预计工时：** 4 周

| 任务 | 模块 | 说明 |
|------|------|------|
| 5.1 Plugin SDK crate | `plugin-sdk/` | 导出 trait + 类型 + 注册宏 |
| 5.2 动态库加载 | `core/adapter/registry.rs` | `libloading` + dlopen |
| 5.3 插件描述文件解析 | `core/adapter/plugin.rs` | 读取 plugin.yaml |
| 5.4 插件目录扫描 | `core/adapter/plugin.rs` | `notify` + hot-reload |
| 5.5 `easybot plugins` CLI | `bin/src/main.rs` | 插件管理子命令 |
| 5.6 第三方示例插件 | `plugins/example/` | 完整示例 |

---

## 第六章：关键设计决策说明

### 5.1 为何选择 axum 而非 actix-web

- axum 基于 tokio（同一团队维护），与 Rust 异步生态更一致
- axum 的 `IntoResponse` trait 使得自定义响应更简洁
- actix-web 使用自己的 actor 运行时，与 tokio 生态集成需要额外适配
- axum + tower 中间件模型更灵活

### 5.2 为何选择 sqlx 而非 diesel/sea-orm

- sqlx 是异步原生的（直接使用 tokio）
- 编译期查询检查（`query_as!` 宏）在开发阶段捕获 SQL 错误
- 无需 codegen / build script
- SQLite + PostgreSQL 切换只需改连接字符串 + 少数类型差异
- ORM 对于网关这种以 JSON 存储为主的场景收益不大

### 5.3 动态分发策略

Rust 中 trait 对象的动态分发通过 `Box<dyn Trait>` 实现，但有以下考量：

```rust
// 适配器使用 Box<dyn PlatformAdapter>
// 好处：简单直接，编译快
// 代价：一次虚表间接调用
pub type BoxedAdapter = Box<dyn PlatformAdapter>;

// 或者使用 enum dispatch（静态分发表）
// 好处：零虚表开销
// 代价：新增平台需要修改 enum
pub enum AdapterEnum {
    Telegram(TelegramAdapter),
    Discord(DiscordAdapter),
}

impl PlatformAdapter for AdapterEnum {
    // 通过 match 分发
}
```

**推荐：Phase 1-3 使用 `Box<dyn PlatformAdapter>`，关注功能完善；后续如需要极致性能再考虑 enum dispatch。**

### 5.4 进程模型

```
easybot process
├── main thread (tokio runtime)
│   ├── API Server (axum, multi-threaded)
│   ├── Adapter Manager
│   │   ├── Telegram polling task
│   │   ├── Discord gateway task
│   │   ├── Feishu WebSocket task
│   │   ├── QQ Gateway WebSocket task
│   │   └── WeChat long-polling task
│   ├── Health poller (定期任务)
│   └── Config watcher (notify 异步监听)
└── signal handler (ctrl+c / SIGTERM)
```

### 5.5 Telegram 适配器实现关键点

```rust
// Token 验证 & 长轮询 (getUpdates)

impl TelegramAdapter {
    /// 使用 Bot API 的 getUpdates 长轮询
    /// 替代 webhook（便于本地开发和内网部署）
    async fn polling_loop(&self) {
        let mut offset = 0i64;
        loop {
            // 使用 tokio::time::interval 控制轮询频率
            let updates = self.client
                .get_updates(offset, Some(30))  // timeout=30s 长轮询
                .await;
            
            match updates {
                Ok(updates) => {
                    for update in updates {
                        if let Some(message) = update.message {
                            // 转换为 InboundMessage
                            let inbound = self.convert_message(message);
                            // 调用注册的处理器
                            if let Some(handler) = &self.message_handler {
                                handler(inbound).await;
                            }
                        }
                        offset = update.update_id + 1;
                    }
                }
                Err(e) => {
                    error!("Telegram polling error: {}", e);
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    /// 发送消息 (使用 Bot API sendMessage)
    async fn send_message(&self, chat_id: &str, text: &str) -> Result<serde_json::Value> {
        let resp = self.client
            .post(format!(
                "https://api.telegram.org/bot{token}/sendMessage",
                token = self.config.token
            ))
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": text,
                "parse_mode": "MarkdownV2",
            }))
            .send()
            .await?;
        
        Ok(resp.json().await?)
    }
}
```

### 5.6 配置文件中的环境变量替换

```rust
// 支持 ${VAR_NAME} 或 $VAR_NAME 语法
fn resolve_env_vars(value: &str) -> String {
    let re = regex::Regex::new(r"\$\{([^}]+)\}|\$([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    re.replace_all(value, |caps: &regex::Captures| {
        let var_name = caps.get(1).or_else(|| caps.get(2))
            .map(|m| m.as_str())
            .unwrap_or("");
        std::env::var(var_name).unwrap_or_else(|_| {
            tracing::warn!("Environment variable '{}' not set, using empty string", var_name);
            String::new()
        })
    })
    .to_string()
}
```

---

## 第七章：目录结构总览（最终形态）

```
easybot/
├── Cargo.toml                          # workspace
├── Cargo.lock
├── README.md
├── gateway.yaml                         # 默认配置
├── gateway.dev.yaml                     # 开发配置
├── docker-compose.yml
├── Dockerfile
├── .env.example
│
├── bin/
│   ├── Cargo.toml
│   └── src/main.rs                      # 主入口
│
├── crates/
│   ├── easybot-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types/                   # 数据类型
│   │       ├── bus/                     # 事件总线
│   │       ├── session/                 # 会话管理
│   │       ├── adapter/                 # 适配器管理器
│   │       ├── router/                  # 消息路由
│   │       ├── auth/                    # 认证授权
│   │       ├── config/                  # 配置加载
│   │       └── stats/                   # 统计指标
│   │
│   ├── easybot-api/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs
│   │       ├── routes/                  # HTTP 路由
│   │       ├── middleware/              # 中间件
│   │       └── ws/                      # WebSocket
│   │
│   ├── easybot-adapter-telegram/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   │
│   ├── easybot-adapter-discord/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   │
│   ├── easybot-adapter-feishu/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   │
│   ├── easybot-adapter-qq/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   │
│   ├── easybot-adapter-wechat/
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   │
│   └── easybot-plugin-sdk/
│       ├── Cargo.toml
│       └── src/lib.rs
│
├── tests/
│   ├── integration/                     # 集成测试
│   │   └── integration-tests/
│   └── plugins/
│       └── mock-adapter/                # 插件系统测试适配器
│
└── docs/
    ├── im-gateway-architecture.md       # 架构设计文档
    ├── rust-implementation-plan.md      # 本文件
    ├── TEST_PLAN.md                     # 测试计划
    └── TODO.md                          # 待办事项清单
```

---

## 第八章：总结

### 实施路线图 (当前状态)

```
                 Phase 1              Phase 2              Phase 3              Phase 4               Phase 5
                (2-3 周)             (2-3 周)             (3-4 周)             (4-6 周)              (4 周)
                ─────────            ─────────            ─────────            ─────────              ─────────
                 ✅ 完成              ✅ 完成              ✅ 100%               ✅ 95%                 ✅ 完成

REST 单发        ██
Telegram         ██        ██
WebSocket                    ██
Webhook                      ██
Discord                                ██
飞书/QQ/微信                            ██
5 平台                                   ██
API Key / 权限                                        ██  (RBAC ⚠️暂缓)
速率限制                                                 ██
热重载                                                    ██
健康轮询 + 自动重连                                        ██
HTTPS/WSS                                                  ██ (⚠️暂缓)
Prometheus                                                  ██
Docker                                                       ██
交互式按钮 + 流式                                              ██
PostgreSQL                                                     ██
插件 SDK                                                                   ██
动态加载                                                                     ██
```

### 关键数字 (实际 vs 计划)

| 指标 | Phase 1 计划 | Phase 4 计划 | 实际当前 |
|------|-------------|-------------|---------|
| 支持平台数 | 1 | 3+ | **5** (Telegram, Discord, 飞书, QQ, 微信) |
| 代码行数估计 | ~3,000 | ~20,000 | **~30,000+** |
| Rust 文件数 | ~30 | ~150 | **~200+** |
| 第三方依赖数 | ~15 | ~25 | ~30 |

### 剩余工作 (详情见 docs/TODO.md)

**P3 补完 (100% 完成):**
- ✅ Discord: send_media, send_interactive, list_chats — 全部完成
- ✅ 微信: edit_message, delete_message, send_interactive, list_chats — 全部确认为平台限制
- ✅ QQ: send_interactive, list_chats, C2C/频道实机验证 — 全部完成

**P4 补完 (仅剩 2 项暂缓):**
- ⚠️ 权限模型 RBAC (暂缓)
- ⚠️ TLS/HTTPS 应用层处理 (暂缓)
- ✅ 其余全部完成: send_draft, 通用健康轮询+自动重连, Health 启动时间, QQ 实机验证, 状态缓存修复
