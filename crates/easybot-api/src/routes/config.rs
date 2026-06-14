//! 配置管理路由

use axum::{Json, extract::State};
use crate::AppState;

/// 获取当前配置
#[utoipa::path(
    get,
    path = "/api/v1/config",
    tag = "Config",
    responses(
        (status = 200, description = "Current gateway configuration", body = serde_json::Value),
    )
)]
pub async fn get_config(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    // 从 ConfigManager 获取最新配置
    async {
        let config = state.config_manager.get().await;
        serde_json::to_value(&*config).unwrap_or_default()
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
) -> Json<serde_json::Value> {
    // 获取当前配置（YAML 值）
    let current = state.config_manager.get().await;
    let current_val: serde_yaml::Value = serde_yaml::to_value(&*current)
        .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

    // 将 JSON 请求体转换为 YAML 值（用于合并）
    let update_val: serde_yaml::Value = serde_yaml::from_str(
        &serde_json::to_string(&body).unwrap_or_default()
    ).unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));

    // 合并配置
    let mut merged = current_val.clone();
    easybot_core::config::merge_configs(&mut merged, update_val);

    // 解析为 GatewayConfig
    match serde_yaml::from_value::<easybot_core::types::config::GatewayConfig>(merged) {
        Ok(new_config) => {
            let _old = state.config_manager.swap(new_config).await;

            // 发布配置变更事件
            state.event_bus.publish(easybot_core::types::event::GatewayEvent::new(
                easybot_core::types::event::event_types::CONFIG_CHANGED,
                "config",
                serde_json::json!({"reload_type": "api"}),
            ));

            tracing::info!("Configuration hot-reloaded via API");

            Json(serde_json::json!({
                "ok": true,
                "message": "Configuration updated"
            }))
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "error": "PARSE_ERROR",
            "message": format!("Invalid configuration: {}", e)
        })),
    }
}
