//! 配置管理路由

use crate::AppState;
use crate::response::{ApiError, api_error};
use axum::{Json, extract::State};
use easybot_core::types::error::GatewayError;
use std::path::Path;

/// 获取当前配置
#[utoipa::path(
    get,
    path = "/api/v1/config",
    tag = "Config",
    responses(
        (status = 200, description = "Current gateway configuration", body = serde_json::Value),
    )
)]
pub async fn get_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    // 从 ConfigManager 获取最新配置，再与运行时实际值 reconcile
    async {
        let config = state.config_manager.get().await;
        let val = serde_json::to_value(&*config).unwrap_or_default();
        state.reconcile_config_json(val)
    }
    .await
    .into()
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
    // 1. 防止存储路径穿越
    if !new_config.storage.path.is_empty() {
        let p = Path::new(&new_config.storage.path);
        if p.components().any(|c| c == std::path::Component::ParentDir) {
            return Err(api_error(GatewayError::ConfigError(
                "storage.path 包含非法 '..' 组件".to_string(),
            )));
        }
    }

    // 2. 禁止通过 API 关闭速率限制
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
