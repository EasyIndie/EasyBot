//! QQ Access Token 管理
//!
//! QQ 统一机器人平台使用 AppId + ClientSecret 换取 Access Token，
//! 有效期 7200 秒。Token 通过 `Arc<Mutex>` 在适配器与 Gateway 事件循环间共享。

use parking_lot::Mutex;
use std::sync::Arc;

use easybot_core::types::error::GatewayError;

/// QQ 统一机器人平台 Access Token 存储
///
/// 按需调用 `refresh()` 从 QQ 鉴权端点获取新 token。
/// Token 有效期 7200 秒，提前 60 秒触发刷新。
#[derive(Clone)]
pub(crate) struct QqTokenStore {
    pub(crate) state: Arc<Mutex<Option<(String, tokio::time::Instant)>>>,
    app_id: String,
    client_secret: String,
    auth_base_url: String,
}

impl QqTokenStore {
    pub(crate) fn new(app_id: String, client_secret: String, auth_base_url: String) -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
            app_id,
            client_secret,
            auth_base_url,
        }
    }

    /// 从 QQ 鉴权端点获取新 token
    pub(crate) async fn refresh(&self) -> Result<(), GatewayError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| GatewayError::Internal(format!("Failed to create HTTP client: {}", e)))?;

        let body = serde_json::json!({
            "appId": self.app_id,
            "clientSecret": self.client_secret,
        });

        let resp = client
            .post(format!("{}/app/getAppAccessToken", self.auth_base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                GatewayError::Internal(format!("QQ getAppAccessToken request failed: {}", e))
            })?;

        let data: serde_json::Value = resp.json().await.map_err(|e| {
            GatewayError::Internal(format!("QQ getAppAccessToken parse failed: {}", e))
        })?;

        let access_token = data["access_token"].as_str().ok_or_else(|| {
            GatewayError::Internal("QQ getAppAccessToken: missing access_token".to_string())
        })?;

        let expires_in = data["expires_in"].as_u64().unwrap_or(7200);
        let expires_at = tokio::time::Instant::now() + std::time::Duration::from_secs(expires_in)
            - std::time::Duration::from_secs(60);

        let mut guard = self.state.lock();
        *guard = Some((access_token.to_string(), expires_at));

        tracing::info!("QQ access token refreshed, expires in {}s", expires_in);

        Ok(())
    }

    /// 获取 `QQBot {access_token}` 格式的鉴权字符串
    pub(crate) fn get(&self) -> Result<String, GatewayError> {
        let guard = self.state.lock();
        let (token, expires_at) = guard
            .as_ref()
            .ok_or_else(|| GatewayError::Internal("QQ access token not initialized".to_string()))?;

        if tokio::time::Instant::now() >= *expires_at {
            // 过期了但尚未刷新 — 返回当前 token 并记录警告
            tracing::warn!("QQ access token may be expired");
        }

        Ok(format!("QQBot {}", token))
    }

    /// 检查是否需要刷新
    pub(crate) fn needs_refresh(&self) -> bool {
        let guard = self.state.lock();
        match guard.as_ref() {
            Some((_, expires_at)) => tokio::time::Instant::now() >= *expires_at,
            None => true,
        }
    }
}
