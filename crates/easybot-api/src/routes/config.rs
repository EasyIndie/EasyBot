//! 配置管理路由

use crate::AppState;
use crate::response::{ApiError, api_error};
use axum::{Json, extract::State};
use easybot_core::types::error::GatewayError;
use std::path::Path;

/// 获取当前配置（已脱敏：隐藏适配器凭据字段）
#[utoipa::path(
    get,
    path = "/api/v1/config",
    tag = "Config",
    responses(
        (status = 200, description = "Current gateway configuration", body = serde_json::Value),
    )
)]
pub async fn get_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    async {
        let config = state.config_manager.get().await;
        let mut value = serde_json::to_value(&*config).unwrap_or_default();
        // SECURITY: Redact adapter credentials from API output
        sanitize_config_for_api(&mut value);
        value
    }
    .await
    .into()
}

/// Strip sensitive fields (token, api_key) from config JSON before returning to clients.
fn sanitize_config_for_api(value: &mut serde_json::Value) {
    if let Some(adapters) = value.get_mut("adapters").and_then(|a| a.as_object_mut()) {
        for (_name, cfg) in adapters.iter_mut() {
            if let Some(obj) = cfg.as_object_mut() {
                if obj.contains_key("token") {
                    obj.insert(
                        "token".to_string(),
                        serde_json::Value::String("***REDACTED***".into()),
                    );
                }
                if obj.contains_key("apiKey") || obj.contains_key("api_key") {
                    obj.insert(
                        "apiKey".to_string(),
                        serde_json::Value::String("***REDACTED***".into()),
                    );
                    obj.insert(
                        "api_key".to_string(),
                        serde_json::Value::String("***REDACTED***".into()),
                    );
                }
            }
        }
    }
    // Also redact admin_password from server config
    if let Some(server) = value.get_mut("server").and_then(|s| s.as_object_mut())
        && (server.contains_key("adminPassword") || server.contains_key("admin_password"))
    {
        server.insert(
            "adminPassword".to_string(),
            serde_json::Value::String("***REDACTED***".into()),
        );
        server.insert(
            "admin_password".to_string(),
            serde_json::Value::String("***REDACTED***".into()),
        );
    }
}

/// 更新配置（热重载）
#[utoipa::path(
    put,
    path = "/api/v1/config",
    tag = "Config",
    responses(
        (status = 200, description = "Configuration updated", body = serde_json::Value),
    )
)]
pub async fn update_config(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // 获取当前配置（YAML 值）
    let current = state.config_manager.get().await;
    let current_val: serde_yaml::Value = serde_yaml::to_value(&*current)
        .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

    // 将 JSON 请求体转换为 YAML 值（用于合并）
    let update_val: serde_yaml::Value =
        serde_yaml::from_str(&serde_json::to_string(&body).unwrap_or_default())
            .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

    // 合并配置
    let mut merged = current_val.clone();
    easybot_core::config::merge_configs(&mut merged, update_val);

    // 解析为 GatewayConfig
    let new_config = serde_yaml::from_value::<easybot_core::types::config::GatewayConfig>(merged)
        .map_err(|e| {
        api_error(GatewayError::ConfigError(format!(
            "Invalid configuration: {}",
            e
        )))
    })?;

    // ── 安全校验 ──
    // 1. 白名单校验：禁止通过 API 修改敏感字段
    const ALLOWED_TOP_KEYS: &[&str] = &["logging", "api", "webhooks"];

    if let serde_json::Value::Object(ref update_obj) = body {
        for key in update_obj.keys() {
            if !ALLOWED_TOP_KEYS.contains(&key.as_str()) {
                return Err(api_error(GatewayError::ConfigError(format!(
                    "不允许通过 API 修改 '{}' 配置段，请直接编辑配置文件后重启",
                    key
                ))));
            }
        }

        // Block modification of sensitive sub-fields
        if let Some(api_obj) = update_obj.get("api").and_then(|v| v.as_object()) {
            // Block disabling rate limiting
            if let Some(rl) = api_obj
                .get("rateLimit")
                .or_else(|| api_obj.get("rate_limit"))
                && let Some(enabled) = rl.get("enabled").and_then(|v| v.as_bool())
                && !enabled
            {
                return Err(api_error(GatewayError::ConfigError(
                    "不允许通过 API 关闭速率限制".to_string(),
                )));
            }
            // Block disabling raw_payload control
            if let Some(raw) = api_obj
                .get("rawPayloadEnabled")
                .or_else(|| api_obj.get("raw_payload_enabled"))
                && raw.as_bool() == Some(true)
            {
                tracing::warn!("通过 API 开启 raw_payload_enabled");
            }
        }
    }

    // 2. Validate webhook URLs (SSRF prevention)
    for wh in &new_config.webhooks {
        if let Err(e) = easybot_core::config::validate_webhook_url(&wh.url) {
            return Err(ApiError(e));
        }
    }

    // 3. 防止存储路径穿越
    if !new_config.storage.path.is_empty() {
        let p = Path::new(&new_config.storage.path);
        if p.components().any(|c| c == std::path::Component::ParentDir) {
            return Err(api_error(GatewayError::ConfigError(
                "storage.path 包含非法 '..' 组件".to_string(),
            )));
        }
    }

    // 4. 禁止通过 API 修改数据库连接字符串（防止凭据注入）
    if let serde_json::Value::Object(ref update_obj) = body
        && let Some(storage) = update_obj.get("storage").and_then(|v| v.as_object())
        && (storage.contains_key("connectionString") || storage.contains_key("connection_string"))
    {
        return Err(api_error(GatewayError::ConfigError(
            "不允许通过 API 修改数据库连接字符串".to_string(),
        )));
    }

    // 5. 禁止通过 API 关闭速率限制
    if !new_config.api.rate_limit.enabled {
        tracing::warn!("尝试通过 API 关闭限流，已拒绝");
        return Err(api_error(GatewayError::ConfigError(
            "不允许通过 API 关闭速率限制".to_string(),
        )));
    }

    let _old = state.config_manager.swap(new_config).await;

    // 发布配置变更事件
    state
        .event_bus
        .publish(easybot_core::types::event::GatewayEvent::new(
            easybot_core::types::event::event_types::CONFIG_CHANGED,
            "config",
            serde_json::json!({"reload_type": "api"}),
        ));

    tracing::info!("Configuration hot-reloaded via API");

    Ok(Json(serde_json::json!({
        "ok": true,
        "message": "Configuration updated"
    })))
}
