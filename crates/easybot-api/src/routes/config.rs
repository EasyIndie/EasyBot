//! 配置管理路由

use crate::AppState;
use crate::response::{ApiError, api_error};
use axum::{
    Json,
    extract::{Extension, State},
};
use easybot_core::types::error::GatewayError;

const MAX_CONFIG_UPDATE_BYTES: usize = 256 * 1024;
const MAX_CONFIG_DEPTH: usize = 16;
const MAX_CONFIG_NODES: usize = 10_000;
const MAX_WEBHOOKS: usize = 32;

fn validate_config_update_shape(body: &serde_json::Value) -> Result<(), GatewayError> {
    let object = body.as_object().ok_or_else(|| {
        GatewayError::InvalidRequest("configuration update must be a JSON object".into())
    })?;
    const ALLOWED_TOP_KEYS: &[&str] = &["logging", "api", "webhooks"];
    for key in object.keys() {
        if !ALLOWED_TOP_KEYS.contains(&key.as_str()) {
            return Err(GatewayError::InvalidRequest(format!(
                "configuration section is not writable through the API: {key}"
            )));
        }
    }
    if serde_json::to_vec(body).map_or(usize::MAX, |encoded| encoded.len())
        > MAX_CONFIG_UPDATE_BYTES
    {
        return Err(GatewayError::InvalidRequest(
            "configuration update exceeds 256 KiB".into(),
        ));
    }
    fn visit(value: &serde_json::Value, depth: usize, nodes: &mut usize) -> bool {
        *nodes = nodes.saturating_add(1);
        if depth > MAX_CONFIG_DEPTH || *nodes > MAX_CONFIG_NODES {
            return false;
        }
        match value {
            serde_json::Value::Array(items) => {
                items.iter().all(|item| visit(item, depth + 1, nodes))
            }
            serde_json::Value::Object(object) => {
                object.values().all(|item| visit(item, depth + 1, nodes))
            }
            _ => true,
        }
    }
    let mut nodes = 0;
    if !visit(body, 0, &mut nodes) {
        return Err(GatewayError::InvalidRequest(
            "configuration update is too deeply nested or complex".into(),
        ));
    }
    if object
        .get("webhooks")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|hooks| hooks.len() > MAX_WEBHOOKS)
    {
        return Err(GatewayError::InvalidRequest(
            "configuration supports at most 32 webhooks".into(),
        ));
    }
    Ok(())
}

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

/// Recursively strip credentials from config JSON before returning it.
fn sanitize_config_for_api(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                let normalized = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                let sensitive = normalized.contains("password")
                    || normalized.contains("secret")
                    || normalized.contains("token")
                    || matches!(
                        normalized.as_str(),
                        "apikey"
                            | "connectionstring"
                            | "databaseurl"
                            | "privatekey"
                            | "authorization"
                            | "cookie"
                    );
                if sensitive && !child.is_null() {
                    *child = serde_json::Value::String("***REDACTED***".into());
                } else if normalized.ends_with("url") {
                    sanitize_url(child);
                } else {
                    sanitize_config_for_api(child);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                sanitize_config_for_api(item);
            }
        }
        _ => {}
    }
}

fn sanitize_url(value: &mut serde_json::Value) {
    let Some(raw) = value.as_str() else { return };
    let Ok(mut url) = reqwest::Url::parse(raw) else {
        return;
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    *value = serde_json::Value::String(url.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursively_redacts_all_config_credentials_and_url_secrets() {
        let mut value = serde_json::json!({
            "server": {"adminPassword":"admin-secret"},
            "storage": {"connectionString":"postgres://user:pass@db/easybot"},
            "adapters": {"x":{"token":"bot-token","extra":{"clientSecret":"nested"}}},
            "webhooks": [{
                "url":"https://user:pass@example.com/hook?access_token=leak#fragment",
                "secret":"signing-secret"
            }]
        });
        sanitize_config_for_api(&mut value);
        let rendered = value.to_string();
        for secret in [
            "admin-secret",
            "postgres://",
            "bot-token",
            "nested",
            "pass@",
            "access_token",
            "signing-secret",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
        assert_eq!(value["webhooks"][0]["url"], "https://example.com/hook");
    }

    #[test]
    fn gateway_config_round_trips_after_hot_reload_merge() {
        let config = easybot_core::types::config::GatewayConfig::default();
        let mut value = serde_yaml::to_value(config).unwrap();
        let update = serde_yaml::from_str("logging:\n  level: warn\n").unwrap();
        easybot_core::config::merge_configs(&mut value, update);
        serde_yaml::from_value::<easybot_core::types::config::GatewayConfig>(value).unwrap();
    }

    #[test]
    fn config_update_shape_rejects_non_object_and_excessive_complexity() {
        assert!(validate_config_update_shape(&serde_json::json!([])).is_err());
        assert!(validate_config_update_shape(&serde_json::json!({"storage": {}})).is_err());
        let mut nested = serde_json::json!(true);
        for _ in 0..=MAX_CONFIG_DEPTH {
            nested = serde_json::json!({"child": nested});
        }
        assert!(validate_config_update_shape(&serde_json::json!({"api": nested})).is_err());
        assert!(
            validate_config_update_shape(&serde_json::json!({
                "webhooks": (0..=MAX_WEBHOOKS).map(|_| serde_json::json!({})).collect::<Vec<_>>()
            }))
            .is_err()
        );
    }
}

/// 更新配置（热重载）
#[utoipa::path(
    put,
    path = "/api/v1/config",
    tag = "Config",
    responses(
        (status = 409, description = "Runtime configuration changes require a reviewed restart", body = easybot_core::types::error::ApiErrorResponse),
    )
)]
pub async fn update_config(
    State(state): State<AppState>,
    Extension(actor): Extension<easybot_core::auth::AuthInfo>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    validate_config_update_shape(&body).map_err(api_error)?;
    let sections = body
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "config.update.rejected",
            "config",
            serde_json::json!({"sections":sections,"reason":"restart_required"}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    Err(api_error(GatewayError::Conflict(
        "runtime configuration updates are unsupported; apply a reviewed configuration and restart EasyBot"
            .into(),
    )))
}
