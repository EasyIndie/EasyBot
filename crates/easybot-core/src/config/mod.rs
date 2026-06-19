//! 配置加载
//!
//! 负责从文件加载 YAML 配置、解析环境变量引用、配置合并。
//! 支持从 .env 文件加载环境变量（优先级低于 shell export）。

mod home;
pub use home::*;

use crate::types::config::GatewayConfig;
use regex::Regex;
use std::path::Path;
use tracing::info;

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

    info!("Loaded config from {}", path.display());
    Ok(config)
}

/// 合并配置
///
/// 将 local YAML 值递归合并到 base 中。
pub fn merge_configs(base: &mut serde_yaml::Value, local: serde_yaml::Value) {
    match (base, local) {
        (base @ serde_yaml::Value::Mapping(_), serde_yaml::Value::Mapping(local_map)) => {
            let base_map = base.as_mapping_mut().unwrap();
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
        return Ok(());
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

/// 生成 `.env.example` 文件内容
///
/// 列出所有已知环境变量及说明，供用户复制为 `.env` 后填入令牌和密钥。
pub fn generate_env_example() -> String {
    r#"# EasyBot 环境变量
#
# 取消注释并填入你的令牌/密钥以启用对应平台。
# 此文件不受版本控制（.env 已在 .gitignore 中）。
#
# Shell 中设置的环境变量（export VAR=value）或
# docker-compose.yml 中的设置优先于本文件的值。
#
# 提示: 同时还需在 gateway.local.yaml 中取消注释对应的适配器。

# Telegram Bot Token（从 @BotFather 获取）
# TELEGRAM_BOT_TOKEN=123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11

# Discord Bot Token
# DISCORD_BOT_TOKEN=your_discord_bot_token

# 飞书/Lark 应用凭据
# FEISHU_APP_ID=cli_xxxxxxxxxxxx
# FEISHU_APP_SECRET=xxxxxxxxxxxxxxxxxxxxxxxx

# QQ 机器人凭据（AppSecret 作为 token，AppId 作为 app_id）
# QQ_CLIENT_SECRET=your_qq_client_secret
# QQ_APP_ID=your_qq_app_id

# 个人微信 iLink Bot Token（可选，未设置时启动后扫码登录）
# WECHAT_BOT_TOKEN=your_wechat_bot_token

# PostgreSQL（可选，默认：SQLite）
# DATABASE_URL=postgresql://user:password@localhost:5432/easybot
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
# 环境变量优先级（从高到低）:
#   1. shell export / Docker environment:
#   2. {config_dir}/.env 文件
# 使用 gateway.local.yaml 覆盖本配置中的值（不上传到版本控制）

server:
  host: "127.0.0.1"
  port: 8080
  tls:
    enabled: false
    certFile: ""
    keyFile: ""

api:
  basePath: "/api/v1"
  websocket:
    enabled: true
    maxClients: 1000
    heartbeatInterval: 30

storage:
  type: "sqlite"
  # path 留空时使用默认值 {data_dir}/gateway.db
  path: ""

logging:
  level: "info"
  format: "text"
  output: "stdout"

adapters:
  telegram:
    enabled: false
    # token: "${TELEGRAM_BOT_TOKEN}"

# webhooks:
#   - name: "my-service"
#     url: "https://my-service.com/webhook"
#     secret: "${WEBHOOK_SECRET}"
#     events: ["message.inbound"]
#     platforms: ["telegram"]
"#
    .to_string()
}

/// 生成 `gateway.local.yaml` 示例内容
///
/// 列出所有支持的内置适配器及其配置方式，供用户复制为 `gateway.local.yaml`
/// 后启用对应的 IM 平台适配器。该文件不会上传到版本控制（已写入 .gitignore）。
///
/// secret 字段通过 `${VAR_NAME}` 引用环境变量，需在 `.env` 文件中设置对应值。
pub fn generate_local_config_example() -> String {
    r#"# EasyBot 本地配置覆盖
#
# 取消注释以启用对应适配器。
# 此文件不会被版本控制（已在 .gitignore 中），适合存放本地覆盖和密钥引用。
#
# Secret 值通过 ${VAR_NAME} 引用环境变量，需在 .env 中设置对应值。
#
# 使用方式:
#   取消下面所需适配器的注释，同时在 .env 中取消注释并填入对应 token
#
# 注意: 所有适配器默认 disabled，需显式设置 enabled: true。

# Telegram
# telegram:
#   enabled: true
#   token: "${TELEGRAM_BOT_TOKEN}"

# Discord
# discord:
#   enabled: true
#   token: "${DISCORD_BOT_TOKEN}"

# 飞书/Lark
# feishu:
#   enabled: true
#   token: "${FEISHU_APP_SECRET}"
#   extra:
#     app_id: "${FEISHU_APP_ID}"

# QQ 机器人
# qq:
#   enabled: true
#   token: "${QQ_CLIENT_SECRET}"
#   extra:
#     app_id: "${QQ_APP_ID}"

# 个人微信（可选，未配置 bot_token 时启动后扫码登录）
# wechat:
#   enabled: true
#   # bot_token: "${WECHAT_BOT_TOKEN}"
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_env_vars() {
        std::env::set_var("TEST_VAR", "hello");
        let result = resolve_env_vars("prefix_${TEST_VAR}_suffix");
        assert_eq!(result, "prefix_hello_suffix");
        std::env::remove_var("TEST_VAR");
    }

    #[test]
    fn test_resolve_env_vars_missing() {
        let result = resolve_env_vars("${MISSING_VAR}");
        assert_eq!(result, "");
    }

    #[test]
    fn test_resolve_env_vars_skips_comments() {
        std::env::set_var("MY_VAR", "hello");
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
        std::env::remove_var("MY_VAR");
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
    fn test_load_env_creates_variables() {
        use std::fs;
        let dir = std::env::temp_dir().join("easybot_env_test_basic");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join(".env"), "TEST_ENV_VAR=from_file\n").unwrap();

        let paths = EasyBotPaths::new(dir.clone()).unwrap();
        assert!(load_env(&paths).is_ok());
        assert_eq!(std::env::var("TEST_ENV_VAR").unwrap(), "from_file");

        std::env::remove_var("TEST_ENV_VAR");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_env_missing_file_returns_ok() {
        let dir = std::env::temp_dir().join("easybot_env_test_missing");
        let paths = EasyBotPaths::new(dir.clone()).unwrap();
        // .env 不存在 -> 应静默返回 Ok
        assert!(load_env(&paths).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_load_env_does_not_override_existing() {
        use std::fs;
        let dir = std::env::temp_dir().join("easybot_env_test_override");
        let _ = fs::create_dir_all(&dir);
        // .env 中说 "file_value"
        fs::write(dir.join(".env"), "OVERRIDE_ME=file_value\n").unwrap();

        // 但环境变量已被设置为 "shell_value"
        std::env::set_var("OVERRIDE_ME", "shell_value");

        let paths = EasyBotPaths::new(dir.clone()).unwrap();
        assert!(load_env(&paths).is_ok());
        // dotenvy 默认不覆盖已有变量
        assert_eq!(std::env::var("OVERRIDE_ME").unwrap(), "shell_value");

        std::env::remove_var("OVERRIDE_ME");
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
        assert!(content.contains("WECHAT_BOT_TOKEN"));
        assert!(content.contains("DATABASE_URL"));
    }

    #[test]
    fn test_generate_local_config_example_contains_all_platforms() {
        let content = generate_local_config_example();
        assert!(content.contains("telegram:"));
        assert!(content.contains("TELEGRAM_BOT_TOKEN"));
        assert!(content.contains("discord:"));
        assert!(content.contains("DISCORD_BOT_TOKEN"));
        assert!(content.contains("feishu:"));
        assert!(content.contains("FEISHU_APP_SECRET"));
        assert!(content.contains("FEISHU_APP_ID"));
        assert!(content.contains("qq:"));
        assert!(content.contains("QQ_CLIENT_SECRET"));
        assert!(content.contains("QQ_APP_ID"));
        assert!(content.contains("wechat:"));
        assert!(content.contains("WECHAT_BOT_TOKEN"));
    }
}
