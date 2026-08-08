//! 配置加载
//!
//! 负责从文件加载 YAML 配置、解析环境变量引用、配置合并。
//! 支持从 .env 文件加载环境变量（优先级低于 shell export）。

mod home;
pub use home::*;

pub mod service;
pub use service::*;

use crate::types::config::GatewayConfig;
use regex::Regex;
use std::path::Path;
use tracing::info;

/// Validate security-sensitive settings before exposing EasyBot as a service.
///
/// This deliberately returns every problem in one pass so deployment pipelines
/// can report an actionable checklist instead of failing one setting at a time.
pub fn validate_production_config(
    config: &GatewayConfig,
    admin_password: &str,
    allow_plaintext: bool,
    debug_cors: bool,
    debug_logging: bool,
) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    // The Axum listener is HTTP-only today. `server.tls` documents certificate
    // locations but does not terminate TLS, so it must never satisfy the
    // production transport-security gate.
    if !allow_plaintext {
        errors.push(
            "the application listener is HTTP-only; terminate TLS at a trusted reverse proxy \
             and set EASYBOT_ALLOW_PLAINTEXT=true to acknowledge the private HTTP hop"
                .to_string(),
        );
    }
    if config.server.tls.enabled {
        errors.push(
            "server.tls.enabled is not supported by the HTTP listener; configure TLS on a trusted reverse proxy"
                .to_string(),
        );
    }
    if debug_cors {
        errors.push("EASYBOT_DEBUG_CORS is forbidden in production".to_string());
    }
    if debug_logging {
        errors.push(
            "--debug is forbidden in production because logs may expose customer metadata"
                .to_string(),
        );
    }
    if admin_password.chars().count() < 12 {
        errors.push("admin password must contain at least 12 characters".to_string());
    }
    if !config.api.rate_limit.enabled {
        errors.push("API rate limiting must be enabled".to_string());
    }
    if config.api.rate_limit.requests_per_minute == 0 {
        errors.push("API rate limit must be greater than zero".to_string());
    }
    if config.api.raw_payload_enabled {
        errors.push("raw platform payload forwarding must be disabled".to_string());
    }
    if !config.api.metrics.enabled {
        errors.push("Prometheus metrics must be enabled".to_string());
    }
    if !(1..=10_000).contains(&config.api.websocket.max_clients) {
        errors.push("WebSocket max_clients must be between 1 and 10000".to_string());
    }
    if !(5..=300).contains(&config.api.websocket.heartbeat_interval_secs) {
        errors.push("WebSocket heartbeat interval must be between 5 and 300 seconds".to_string());
    }
    if config.server.cors_allowed_origins.is_empty() {
        errors.push("at least one explicit CORS origin is required".to_string());
    }
    for origin in &config.server.cors_allowed_origins {
        let origin = origin.trim().to_ascii_lowercase();
        if origin == "*" || origin == "null" {
            errors.push("wildcard/null CORS origins are forbidden".to_string());
        }
        if origin.starts_with("http://")
            && !origin.starts_with("http://localhost")
            && !origin.starts_with("http://127.0.0.1")
        {
            errors.push(format!("CORS origin must use HTTPS: {origin}"));
        }
    }
    for webhook in &config.webhooks {
        if !webhook.url.to_ascii_lowercase().starts_with("https://") {
            errors.push(format!(
                "production webhook must use HTTPS: {}",
                webhook.url
            ));
        }
    }

    errors.sort();
    errors.dedup();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// 加载配置文件
///
/// 从指定路径加载 YAML 配置，解析环境变量引用。
/// 支持递归的 ${VAR_NAME} 和 $VAR_NAME 语法。
pub async fn load_config(path: &Path) -> Result<GatewayConfig, crate::types::error::GatewayError> {
    let content = tokio::fs::read_to_string(path).await.map_err(|e| {
        crate::types::error::GatewayError::ConfigError(format!(
            "failed to read config file {}: {}",
            path.display(),
            e
        ))
    })?;

    // 解析环境变量引用
    let resolved = resolve_env_vars(&content);

    let config: GatewayConfig = serde_yaml::from_str(&resolved).map_err(|e| {
        crate::types::error::GatewayError::ConfigError(format!("failed to parse config: {}", e))
    })?;

    // SECURITY: Validate webhook URLs
    for wh in &config.webhooks {
        validate_webhook_url(&wh.url)?;
    }

    info!("Loaded config from {}", path.display());
    Ok(config)
}

/// SECURITY: Validate webhook URL to prevent SSRF.
///
/// Rejects non-HTTP(S) schemes and common internal target patterns.
pub fn validate_webhook_url(url: &str) -> Result<(), crate::types::error::GatewayError> {
    validate_url_for_ssrf(url).map_err(|_| {
        crate::types::error::GatewayError::ConfigError(format!(
            "Webhook URL targets an internal/blocked host: {}",
            url
        ))
    })
}

/// SSRF validation error — URL targets an internal/blocked host.
#[derive(Debug)]
pub struct SsrError;

impl std::fmt::Display for SsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "URL targets a blocked internal host")
    }
}

impl std::error::Error for SsrError {}

/// SECURITY: Validate a URL to prevent SSRF attacks.
///
/// Returns `Err(SsrError)` if the URL targets a known internal/blocked host.
/// Used by both webhook config and media download paths.
pub fn validate_url_for_ssrf(url: &str) -> Result<(), SsrError> {
    let lower = url.to_lowercase();
    // Reject non-HTTP(S) schemes
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return Err(SsrError);
    }

    // Reject common SSRF targets
    let blocked_hosts: [&str; 5] = [
        "localhost",
        "127.0.0.1",
        "0.0.0.0",
        "169.254.169.254",          // AWS metadata
        "metadata.google.internal", // GCP metadata
    ];

    let host_with_path = if let Some(h) = lower.strip_prefix("https://") {
        h
    } else if let Some(h) = lower.strip_prefix("http://") {
        h
    } else {
        return Err(SsrError);
    };

    let host = host_with_path.split('/').next().unwrap_or(host_with_path);
    let host_clean = host.split(':').next().unwrap_or(host);

    if blocked_hosts.contains(&host_clean) {
        return Err(SsrError);
    }

    Ok(())
}

/// 合并配置
///
/// 将 local YAML 值递归合并到 base 中。
pub fn merge_configs(base: &mut serde_yaml::Value, local: serde_yaml::Value) {
    match (base, local) {
        (base @ serde_yaml::Value::Mapping(_), serde_yaml::Value::Mapping(local_map)) => {
            // SAFETY: Already matched as Mapping in the outer pattern
            let Some(base_map) = base.as_mapping_mut() else {
                tracing::warn!(
                    "merge_configs: invariant violation — Mapping match but as_mapping_mut failed"
                );
                return;
            };
            for (key, value) in local_map {
                if base_map.contains_key(&key) && base_map[&key].is_mapping() && value.is_mapping()
                {
                    // 递归合并嵌套对象
                    merge_configs(&mut base_map[&key], value);
                } else {
                    base_map.insert(key, value);
                }
            }
        }
        (base, local) => *base = local,
    }
}

/// 从配置主目录加载 `.env` 文件
///
/// 将 `.env` 中的变量注入进程环境，但不会覆盖已存在的环境变量。
/// 这意味着 shell `export`、Docker `environment:` 等方式设置的变量
/// 优先级高于 `.env` 文件中的同名变量。
///
/// 若 `.env` 文件不存在，静默返回 Ok(())。
pub fn load_env(paths: &EasyBotPaths) -> Result<(), crate::types::error::GatewayError> {
    let env_path = &paths.env_path;
    if !env_path.exists() {
        tracing::info!(".env file not found at {}, skipping", env_path.display());
    } else {
        load_dotenv_file(env_path)?;
    }

    load_secret_files()?;
    Ok(())
}

fn load_dotenv_file(env_path: &Path) -> Result<(), crate::types::error::GatewayError> {
    // SECURITY: Check file permissions on Unix (should be 0600)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(env_path) {
            let mode = metadata.permissions().mode();
            // Check if group or other have read access
            if mode & 0o077 != 0 {
                tracing::warn!(
                    ".env file {} has overly permissive permissions ({:o}). \
                     Run: chmod 600 {}",
                    env_path.display(),
                    mode & 0o777,
                    env_path.display()
                );
            }
        }
    }

    // dotenvy::from_path 默认不覆盖已存在的环境变量
    dotenvy::from_path(env_path).map_err(|e| {
        crate::types::error::GatewayError::ConfigError(format!(
            "failed to load .env file {}: {}",
            env_path.display(),
            e
        ))
    })?;

    tracing::info!("Loaded environment variables from {}", env_path.display());
    Ok(())
}

/// Load deployment secrets from files without exposing their values through
/// the container environment. Only the documented allowlist is accepted.
fn load_secret_files() -> Result<(), crate::types::error::GatewayError> {
    const SECRET_VARS: &[&str] = &[
        "EASYBOT_ADMIN_PASSWORD",
        "DATABASE_URL",
        "TELEGRAM_BOT_TOKEN",
        "DISCORD_BOT_TOKEN",
        "FEISHU_APP_ID",
        "FEISHU_APP_SECRET",
        "QQ_APP_ID",
        "QQ_CLIENT_SECRET",
    ];

    for variable in SECRET_VARS {
        let file_variable = format!("{variable}_FILE");
        let Ok(path) = std::env::var(&file_variable) else {
            continue;
        };
        if path.trim().is_empty() {
            continue;
        }
        if std::env::var_os(variable).is_some() {
            return Err(crate::types::error::GatewayError::ConfigError(format!(
                "{variable} and {file_variable} are mutually exclusive"
            )));
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| {
            crate::types::error::GatewayError::ConfigError(format!(
                "failed to inspect {file_variable} path {path}: {e}"
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(crate::types::error::GatewayError::ConfigError(format!(
                "{file_variable} must reference a regular file, not a symlink"
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(crate::types::error::GatewayError::ConfigError(format!(
                    "{file_variable} path must not grant group or other permissions"
                )));
            }
        }
        if metadata.len() > 64 * 1024 {
            return Err(crate::types::error::GatewayError::ConfigError(format!(
                "{file_variable} exceeds the 64 KiB secret size limit"
            )));
        }
        let value = std::fs::read_to_string(&path).map_err(|e| {
            crate::types::error::GatewayError::ConfigError(format!(
                "failed to read {file_variable} path {path}: {e}"
            ))
        })?;
        let value = value.trim_end_matches(['\r', '\n']);
        if value.is_empty() || value.contains(['\0', '\r', '\n']) {
            return Err(crate::types::error::GatewayError::ConfigError(format!(
                "{file_variable} must contain a non-empty single-line text secret"
            )));
        }
        let escaped = value
            .replace('\\', "\\\\")
            .replace('$', "\\$")
            .replace('"', "\\\"");
        dotenvy::from_read(format!("{variable}=\"{escaped}\"\n").as_bytes()).map_err(|e| {
            crate::types::error::GatewayError::ConfigError(format!(
                "failed to load {file_variable}: {e}"
            ))
        })?;
    }
    Ok(())
}

/// 生成 `.env.example` 文件内容
///
/// 列出所有已知环境变量及说明，供用户复制为 `.env` 后填入令牌和密钥。
pub fn generate_env_example() -> String {
    r#"# EasyBot 环境变量
#
# 取消注释并填入令牌/密钥，对应平台适配器会自动启用。
# 无需修改 gateway.yaml。
#
# 此文件不受版本控制（.env 已在 .gitignore 中）。
# Shell export / Docker environment 优先于本文件的值。

# Telegram Bot Token（从 @BotFather 获取）
# TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11

# Discord Bot Token
# DISCORD_BOT_TOKEN=your_discord_bot_token

# 飞书/Lark 应用凭据
# FEISHU_APP_ID=cli_xxxxxxxxxxxx
# FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxx

# QQ 机器人凭据（AppId 作为 app_id，AppSecret 作为 token）
# QQ_APP_ID=your_qq_app_id
# QQ_CLIENT_SECRET=your_qq_client_secret

# 管理后台登录密码（用于浏览器访问 /admin 时验证身份；生产环境至少 12 个字符）
# EASYBOT_ADMIN_PASSWORD=replace-with-a-long-random-password

# PostgreSQL（可选，默认：SQLite）
# DATABASE_URL=postgresql://user:password@localhost:5432/easybot

# 透传各平台原始 payload 到 WebSocket 事件（开发调试用）
# EASYBOT_RAW_PAYLOAD_ENABLED=true

# 生产环境安全：使用 --production（或 EASYBOT_ENV=production）启用完整门禁
# 仅当可信反向代理已终止 TLS 时才取消下一行注释
# EasyBot 当前监听器不直接终止 TLS，禁止将其直接暴露到公网
# EASYBOT_ALLOW_PLAINTEXT=true

# EASYBOT_ENV=production
"#
    .to_string()
}

/// 解析内容中的环境变量引用
///
/// 支持语法:
/// - ${VAR_NAME}
/// - $VAR_NAME
///
/// 跳过 YAML 注释行（以 # 开头，可含前导空格），防止注释中的 ${...} 被误解析。
fn resolve_env_vars(content: &str) -> String {
    let re = Regex::new(r"\$\{([^}]+)\}|\$([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    content
        .lines()
        .map(|line| {
            // 跳过注释行
            if line.trim_start().starts_with('#') {
                return line.to_string();
            }
            re.replace_all(line, |caps: &regex::Captures| {
                let var_name = caps
                    .get(1)
                    .or_else(|| caps.get(2))
                    .map(|m| m.as_str())
                    .unwrap_or("");
                std::env::var(var_name).unwrap_or_else(|_| {
                    tracing::warn!(
                        "Environment variable '{}' not set, using empty string",
                        var_name
                    );
                    String::new()
                })
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 生成默认配置
pub fn generate_default_config() -> String {
    r#"# EasyBot 默认配置
#
# 配置文件支持环境变量引用语法: ${VAR_NAME}
# 使用 gateway.local.yaml 覆盖本配置中的值（不上传到版本控制）
#
# 启用适配器: 在 .env 或 Shell 中设置对应平台的环境变量即可自动启用。
# 无需在配置文件中声明适配器 — 系统会自动检测已注册的平台。
#
# 各平台所需环境变量:
#   Telegram:  TELEGRAM_BOT_TOKEN
#   Discord:   DISCORD_BOT_TOKEN
#   飞书:      FEISHU_APP_ID + FEISHU_APP_SECRET
#   QQ:        QQ_APP_ID + QQ_CLIENT_SECRET
#   个人微信:  扫码登录（无需环境变量）

server:
  host: "127.0.0.1"
  port: 8080
  tls:
    enabled: false
    certFile: ""
    keyFile: ""
  # 管理后台登录密码（也可通过 EASYBOT_ADMIN_PASSWORD 环境变量覆盖）。
  # 留空 = 禁用管理后台登录；生产环境建议至少 12 个字符。
  adminPassword: ""

api:
  basePath: "/api/v1"
  websocket:
    enabled: true
    maxClients: 1000
    heartbeatInterval: 30
  # 透传各平台原始 payload（开发调试用），也可通过环境变量 EASYBOT_RAW_PAYLOAD 覆盖
  # raw_payload_enabled: ${EASYBOT_RAW_PAYLOAD_ENABLED:-false}

storage:
  storageType: "sqlite"
  path: ""

logging:
  level: "info"
  format: "text"
  output: "stdout"
"#
    .to_string()
}

/// 生成 `gateway.local.yaml` 示例内容
///
/// 高级用法：覆盖 gateway.yaml 中的默认值或显式控制适配器启用/禁用。
/// 一般情况下无需此文件 — 在 .env 中设置令牌即可自动启用适配器。
/// 该文件不会上传到版本控制（已写入 .gitignore）。
pub fn generate_local_config_example() -> String {
    r#"# EasyBot 本地配置覆盖（高级用法）
#
# 此文件用于覆盖 gateway.yaml 中的默认配置。
# 一般情况下无需此文件 — 在 .env 中设置令牌即可自动启用适配器。
#
# 高级场景:
#   - 显式禁用某个已配置凭据的适配器: enabled: false
#   - 覆盖服务器端口: server.port: 9090
#   - 设置自定义 API 地址: base_url: "https://custom-api.example.com"
#
# 此文件不会被版本控制（已在 .gitignore 中）。

# ── 适配器控制 ──────────────────────────────
# 每个适配器支持以下字段：
#   enabled:    true | false          — 强制启用/禁用（不写则自动检测凭据）
#   token:      "xxx"                 — 覆盖凭据（通常从 .env 读取）
#   base_url:   "https://..."         — 自定义 API 地址（测试/代理场景）
#
# 注意：适配器配置必须放在 adapters: 下方，不可直接放在 YAML 顶层。

# 注意：适配器配置必须在 adapters: 下，不可直接放在 YAML 顶层。
# 取消注释以下块来显式禁用或覆盖适配器：

# adapters:
#   # Telegram（凭据: TELEGRAM_BOT_TOKEN）
#   telegram:
#     enabled: false
#   # Discord（凭据: DISCORD_BOT_TOKEN）
#   discord:
#     enabled: false
#   # 飞书/Lark（凭据: FEISHU_APP_ID + FEISHU_APP_SECRET）
#   feishu:
#     enabled: false
#   # QQ（凭据: QQ_APP_ID + QQ_CLIENT_SECRET）
#   qq:
#     enabled: false
#   # 个人微信（无需强制凭据，支持扫码登录）
#   wechat:
#     enabled: false
#     base_url: "http://192.168.1.100:8080"

# ── 服务端覆盖 ─────────────────────────────
# server:
#   port: 9090
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    // Environment variables are process-global. Keep tests that call `load_env`
    // serialized so temporary *_FILE variables cannot leak into another test.
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("environment test lock poisoned")
    }

    #[test]
    fn test_resolve_env_vars() {
        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::set_var("TEST_VAR", "hello") };
        let result = resolve_env_vars("prefix_${TEST_VAR}_suffix");
        assert_eq!(result, "prefix_hello_suffix");
        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("TEST_VAR") };
    }

    #[test]
    fn test_resolve_env_vars_missing() {
        let result = resolve_env_vars("${MISSING_VAR}");
        assert_eq!(result, "");
    }

    #[test]
    fn test_resolve_env_vars_skips_comments() {
        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::set_var("MY_VAR", "hello") };
        // 注释行中的 ${MY_VAR} 不应被解析
        let content = "# ${MY_VAR}\nkey: \"${MY_VAR}\"\n# secret: \"${WEBHOOK_SECRET}\"\n";
        let result = resolve_env_vars(content);
        // 注释行保持原样
        assert!(
            result.starts_with("# ${MY_VAR}"),
            "comment line should not be resolved"
        );
        assert!(
            result.contains("# secret: \"${WEBHOOK_SECRET}\""),
            "comment line should not be resolved"
        );
        // 非注释行正常解析
        assert!(
            result.contains("key: \"hello\""),
            "non-comment line should be resolved"
        );
        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("MY_VAR") };
    }

    #[test]
    fn test_merge_configs() {
        let mut base: serde_yaml::Value = serde_yaml::from_str(
            r#"
server:
  port: 8080
  host: "0.0.0.0"
adapters:
  telegram:
    enabled: false
"#,
        )
        .unwrap();

        let local: serde_yaml::Value = serde_yaml::from_str(
            r#"
server:
  port: 9090
adapters:
  telegram:
    enabled: true
    token: "test-token"
"#,
        )
        .unwrap();

        merge_configs(&mut base, local);

        assert_eq!(base["server"]["port"], 9090);
        assert_eq!(base["server"]["host"], "0.0.0.0"); // 未覆盖
        assert_eq!(base["adapters"]["telegram"]["enabled"], true);
        assert_eq!(base["adapters"]["telegram"]["token"], "test-token");
    }

    #[test]
    fn test_merge_configs_preserves_enabled_false_through_deserialize() {
        // 模拟用户场景：
        //   gateway.yaml: adapter 有 token 但无 enabled（auto-detect）
        //   gateway.local.yaml: enabled: false（显式禁用）
        // 合并后反序列化回 GatewayConfig，AdapterConfig.enabled 应为 Some(false)
        let base_yaml = r#"
server:
  port: 8080
adapters:
  telegram:
    token: "${TELEGRAM_BOT_TOKEN}"
"#;
        let local_yaml = r#"
adapters:
  telegram:
    enabled: false
"#;

        let base_val: serde_yaml::Value =
            serde_yaml::from_str(base_yaml).expect("base yaml should parse");
        let local_val: serde_yaml::Value =
            serde_yaml::from_str(local_yaml).expect("local yaml should parse");

        let mut merged = base_val.clone();
        merge_configs(&mut merged, local_val);

        // 验证 merged Value 包含 enabled: false
        assert_eq!(merged["adapters"]["telegram"]["enabled"], false);
        assert_eq!(
            merged["adapters"]["telegram"]["token"],
            "${TELEGRAM_BOT_TOKEN}"
        );

        // 反序列化回 GatewayConfig
        let config: GatewayConfig =
            serde_yaml::from_value(merged).expect("merged value should deserialize");

        let telegram_cfg = config.adapters.get("telegram").unwrap();

        // 关键断言：enabled 应为 Some(false)，而非 None
        assert!(
            matches!(telegram_cfg.enabled, Some(false)),
            "expected Some(false), got {:?}",
            telegram_cfg.enabled
        );
    }

    #[test]
    fn test_admin_password_accepts_camelcase_yaml_key() {
        // 回归保护：gateway.yaml 中的 adminPassword（camelCase）必须正确映射到
        // Rust 字段 server.admin_password（snake_case）。该映射由 ServerConfig 上的
        // #[serde(rename_all = "camelCase")] 提供，防止未来改动误删此属性。
        let yaml = r#"
server:
  host: "127.0.0.1"
  port: 8080
  adminPassword: "543859230"
"#;
        let config: GatewayConfig = serde_yaml::from_str(yaml).expect("yaml should parse");
        assert_eq!(config.server.admin_password, "543859230");
    }

    #[test]
    fn test_load_env_creates_variables() {
        let _env_lock = env_lock();
        use std::fs;
        let dir = std::env::temp_dir().join("easybot_env_test_basic");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join(".env"), "TEST_ENV_VAR=from_file\n").unwrap();

        let paths = EasyBotPaths::new(dir.clone()).unwrap();
        assert!(load_env(&paths).is_ok());
        assert_eq!(std::env::var("TEST_ENV_VAR").unwrap(), "from_file");

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("TEST_ENV_VAR") };
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_env_missing_file_returns_ok() {
        let _env_lock = env_lock();
        let dir = std::env::temp_dir().join("easybot_env_test_missing");
        let paths = EasyBotPaths::new(dir.clone()).unwrap();
        // .env 不存在 -> 应静默返回 Ok
        assert!(load_env(&paths).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_env_does_not_override_existing() {
        let _env_lock = env_lock();
        use std::fs;
        let dir = std::env::temp_dir().join("easybot_env_test_override");
        let _ = fs::create_dir_all(&dir);
        // .env 中说 "file_value"
        fs::write(dir.join(".env"), "OVERRIDE_ME=file_value\n").unwrap();

        // 但环境变量已被设置为 "shell_value"
        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::set_var("OVERRIDE_ME", "shell_value") };

        let paths = EasyBotPaths::new(dir.clone()).unwrap();
        assert!(load_env(&paths).is_ok());
        // dotenvy 默认不覆盖已有变量
        assert_eq!(std::env::var("OVERRIDE_ME").unwrap(), "shell_value");

        // SAFETY: 测试环境，单线程执行
        unsafe { std::env::remove_var("OVERRIDE_ME") };
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_env_reads_secret_file() {
        let _env_lock = env_lock();
        use std::fs;
        let dir =
            std::env::temp_dir().join(format!("easybot_secret_file_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let secret = dir.join("telegram_token");
        fs::write(&secret, "token-from-file\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        }
        // SAFETY: this test owns these variables for the duration of the call.
        unsafe {
            std::env::remove_var("TELEGRAM_BOT_TOKEN");
            std::env::set_var("TELEGRAM_BOT_TOKEN_FILE", &secret);
        }

        let paths = EasyBotPaths::new(dir.clone()).unwrap();
        assert!(load_env(&paths).is_ok());
        assert_eq!(
            std::env::var("TELEGRAM_BOT_TOKEN").unwrap(),
            "token-from-file"
        );

        unsafe {
            std::env::remove_var("TELEGRAM_BOT_TOKEN");
            std::env::remove_var("TELEGRAM_BOT_TOKEN_FILE");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_generate_env_example_contains_expected_vars() {
        let content = generate_env_example();
        assert!(content.contains("TELEGRAM_BOT_TOKEN"));
        assert!(content.contains("DISCORD_BOT_TOKEN"));
        assert!(content.contains("FEISHU_APP_ID"));
        assert!(content.contains("FEISHU_APP_SECRET"));
        assert!(content.contains("QQ_CLIENT_SECRET"));
        assert!(content.contains("QQ_APP_ID"));
        assert!(content.contains("DATABASE_URL"));
    }

    #[test]
    fn test_generate_env_example_contains_allow_plaintext() {
        let content = generate_env_example();
        assert!(content.contains("EASYBOT_ALLOW_PLAINTEXT"));
    }

    #[test]
    fn test_generate_env_example_contains_admin_password() {
        let content = generate_env_example();
        assert!(content.contains("EASYBOT_ADMIN_PASSWORD"));
    }

    #[test]
    fn test_generate_env_example_does_not_enable_plaintext_by_default() {
        let content = generate_env_example();
        assert!(
            !content
                .lines()
                .any(|line| line == "EASYBOT_ALLOW_PLAINTEXT=true")
        );
    }

    #[test]
    fn test_generate_local_config_example_contains_override_examples() {
        let content = generate_local_config_example();
        // 必须包含 adapters: 父级键（之前模板误将适配器放在顶层导致 serde 静默忽略）
        assert!(
            content.contains("adapters:"),
            "template must show adapters: wrapper, got:\n{}",
            content
        );
        assert!(content.contains("本地配置覆盖"));
        assert!(content.contains("enabled: false"));
        assert!(content.contains("server:"));
    }

    #[test]
    fn production_config_accepts_hardened_reverse_proxy_deployment() {
        let mut config = GatewayConfig::default();
        config.server.cors_allowed_origins = vec!["https://console.example.com".into()];

        assert!(
            validate_production_config(&config, "a-strong-admin-password", true, false, false)
                .is_ok()
        );
    }

    #[test]
    fn production_config_reports_all_unsafe_settings() {
        let mut config = GatewayConfig::default();
        config.server.cors_allowed_origins = vec!["*".into(), "http://console.example.com".into()];
        config.api.rate_limit.enabled = false;
        config.api.raw_payload_enabled = true;

        let errors = validate_production_config(&config, "short", false, true, true).unwrap_err();
        assert!(errors.iter().any(|e| e.contains("HTTP-only")));
        assert!(errors.iter().any(|e| e.contains("12 characters")));
        assert!(errors.iter().any(|e| e.contains("rate limiting")));
        assert!(errors.iter().any(|e| e.contains("raw platform payload")));
        assert!(errors.iter().any(|e| e.contains("wildcard/null")));
        assert!(errors.iter().any(|e| e.contains("must use HTTPS")));
        assert!(errors.iter().any(|e| e.contains("DEBUG_CORS")));
        assert!(errors.iter().any(|e| e.contains("--debug")));
    }

    #[test]
    fn production_config_rejects_unsupported_application_tls() {
        let mut config = GatewayConfig::default();
        config.server.tls.enabled = true;
        config.server.cors_allowed_origins = vec!["https://console.example.com".into()];

        let errors =
            validate_production_config(&config, "a-strong-admin-password", true, false, false)
                .unwrap_err();
        assert!(errors.iter().any(|e| e.contains("not supported")));
    }
}
