//! 配置加载
//!
//! 负责从文件加载 YAML 配置、解析环境变量引用、配置合并。

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

/// 加载配置链
///
/// 先加载基础配置（gateway.yaml），然后在上面合并本地覆盖（gateway.local.yaml）。
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

/// 解析内容中的环境变量引用
///
/// 支持语法:
/// - ${VAR_NAME}
/// - $VAR_NAME
fn resolve_env_vars(content: &str) -> String {
    let re = Regex::new(r"\$\{([^}]+)\}|\$([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    re.replace_all(content, |caps: &regex::Captures| {
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
}

/// 生成默认配置
pub fn generate_default_config() -> String {
    r#"# EasyBot 默认配置
#
# 配置文件支持环境变量引用语法: ${VAR_NAME}
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
}
