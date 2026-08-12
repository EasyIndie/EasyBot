//! 插件管理路由（plugin-system 特性）
//!
//! 已装插件列表（含加载失败原因）、市场目录搜索、安装（含发布者信任确认）、
//! 卸载、启停。权限：读操作 `PluginsRead`，管理操作 `PluginsManage`。
//!
//! 信任语义对齐 CLI/`PluginManager`：首次安装未受信任发布者的插件时返回
//! `needs_trust_confirmation`（非错误），UI 弹确认后带 `trust: true` 重试；
//! `trust: true` **不自动**写入 `.trust`（显式信任走 CLI/API 未来接口）。

use crate::AppState;
use crate::response::ApiError;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use easybot_core::auth::AuthInfo;
use easybot_core::plugin::manager::{InstallRequest, PluginManager};
use easybot_core::plugin::registry::types::PluginChannel;
use easybot_core::types::error::GatewayError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

/// 已安装插件列表项（镜像 `InstalledPlugin`，含加载失败原因）
#[derive(Serialize, ToSchema)]
pub struct PluginItem {
    #[schema(example = "slack")]
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    #[schema(example = "1.0.0")]
    pub version: String,
    pub sdk_version: u32,
    pub enabled: bool,
    /// 是否存在 `plugin.sig.json`
    pub signed: bool,
    /// 签名是否通过校验（None = 未签名无法校验）
    pub signature_valid: Option<bool>,
    /// 发布者标识（签名文件优先，其次 manifest.author）
    pub publisher: Option<String>,
    /// 已加载时的平台名（未加载为 None）
    pub platform: Option<String>,
    /// 加载失败原因（未加载且非禁用时）
    pub load_error: Option<String>,
}

impl From<easybot_core::plugin::manager::InstalledPlugin> for PluginItem {
    fn from(p: easybot_core::plugin::manager::InstalledPlugin) -> Self {
        Self {
            name: p.name,
            display_name: p.display_name,
            description: p.description,
            version: p.version,
            sdk_version: p.sdk_version,
            enabled: p.enabled,
            signed: p.signed,
            signature_valid: p.signature_valid,
            publisher: p.publisher,
            platform: p.platform,
            load_error: p.load_error,
        }
    }
}

/// 市场目录列表项
#[derive(Serialize, ToSchema)]
pub struct CatalogItem {
    #[schema(example = "slack")]
    pub name: String,
    #[schema(example = "easybot")]
    pub publisher: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    /// 发布者是否通过官方验证（徽标；**只证身份，不证安全**）
    pub verified: bool,
    /// 源码仓库（owner/repo）
    pub repo: String,
}

impl From<easybot_core::plugin::registry::types::PluginSource> for CatalogItem {
    fn from(s: easybot_core::plugin::registry::types::PluginSource) -> Self {
        Self {
            name: s.name,
            publisher: s.publisher,
            display_name: s.display_name,
            description: s.description,
            tags: s.tags,
            verified: s.verified,
            repo: format!("{}/{}", s.owner, s.repo),
        }
    }
}

/// GET /api/v1/plugins 响应
#[derive(Serialize, ToSchema)]
pub struct PluginsListResponse {
    pub plugins: Vec<PluginItem>,
}

/// GET /api/v1/plugins/catalog 响应
#[derive(Serialize, ToSchema)]
pub struct CatalogResponse {
    pub plugins: Vec<CatalogItem>,
}

/// GET /api/v1/plugins/catalog 查询参数
#[derive(Deserialize, ToSchema)]
pub struct CatalogQuery {
    pub query: Option<String>,
}

/// POST /api/v1/plugins/install 请求体
#[derive(Deserialize, ToSchema)]
pub struct PluginInstallRequest {
    /// 插件限定名（`publisher/name` 或裸 `name`）
    pub name: String,
    /// 发布渠道：stable | beta（默认 stable）
    #[serde(default = "default_channel")]
    pub channel: String,
    /// 接受发布者信任确认（跳过 needs_trust；**不自动**写入 .trust）
    #[serde(default)]
    pub trust: bool,
}

fn default_channel() -> String {
    "stable".to_string()
}

/// POST /api/v1/plugins/install 响应
#[derive(Serialize, ToSchema)]
pub struct PluginInstallResponse {
    pub ok: bool,
    pub name: String,
    pub publisher: String,
    #[schema(example = "1.0.0")]
    pub version: String,
    /// 是否为升级（覆盖已安装版本）
    pub upgraded: bool,
    /// 需要用户确认发布者信任（非错误；UI 弹确认后带 `trust: true` 重试）
    pub needs_trust_confirmation: bool,
}

/// 获取已安装插件列表
#[utoipa::path(
    get,
    path = "/api/v1/plugins",
    tag = "Plugins",
    responses(
        (status = 200, description = "List of installed plugins (including load-failure reasons)", body = PluginsListResponse)
    )
)]
pub async fn list_plugins(
    State(state): State<AppState>,
) -> Result<Json<PluginsListResponse>, ApiError> {
    let manager = plugin_manager(&state)?;
    let plugins = manager
        .list_installed()
        .await
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(PluginsListResponse { plugins }))
}

/// 搜索市场目录（多源合并去重，5min 缓存由注册表实现）
#[utoipa::path(
    get,
    path = "/api/v1/plugins/catalog",
    tag = "Plugins",
    params(
        ("query" = Option<String>, Query, description = "Search keyword (name/description/tags)"),
    ),
    responses(
        (status = 200, description = "Marketplace catalog entries", body = CatalogResponse)
    )
)]
pub async fn catalog_plugins(
    State(state): State<AppState>,
    Query(query): Query<CatalogQuery>,
) -> Result<Json<CatalogResponse>, ApiError> {
    let manager = plugin_manager(&state)?;
    let plugins = manager
        .search_catalog(query.query.as_deref())
        .await
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(CatalogResponse { plugins }))
}

/// 安装插件（市场下载 或 离线 `--file` 不在 API 提供，走 CLI）
#[utoipa::path(
    post,
    path = "/api/v1/plugins/install",
    tag = "Plugins",
    request_body = PluginInstallRequest,
    responses(
        (status = 200, description = "Install result (may require trust confirmation)", body = PluginInstallResponse),
        (status = 400, description = "Invalid request / plugin error"),
        (status = 409, description = "Already installed or downgrade"),
    )
)]
pub async fn install_plugin(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthInfo>,
    Json(req): Json<PluginInstallRequest>,
) -> Result<Json<PluginInstallResponse>, ApiError> {
    let manager = plugin_manager(&state)?;
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "plugin.install.requested",
            &req.name,
            serde_json::json!({"channel": req.channel}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;

    let core_req = InstallRequest {
        qualified: req.name.clone(),
        channel: parse_channel(&req.channel),
        trust: req.trust,
        ..Default::default()
    };
    match manager.install(core_req).await {
        Ok(outcome) => {
            state
                .auth_manager
                .record_audit(
                    &actor.id,
                    "plugin.install.completed",
                    &outcome.name,
                    serde_json::json!({
                        "ok": !outcome.needs_trust,
                        "publisher": outcome.publisher,
                        "version": outcome.version,
                        "needs_trust_confirmation": outcome.needs_trust,
                    }),
                )
                .await
                .map_err(|error| ApiError(GatewayError::Internal(error)))?;
            Ok(Json(PluginInstallResponse {
                ok: !outcome.needs_trust,
                name: outcome.name,
                publisher: outcome.publisher,
                version: outcome.version,
                upgraded: outcome.upgraded,
                needs_trust_confirmation: outcome.needs_trust,
            }))
        }
        Err(e) => Err(plugin_error(e)),
    }
}

/// 卸载插件
#[utoipa::path(
    delete,
    path = "/api/v1/plugins/{name}",
    tag = "Plugins",
    params(
        ("name" = String, Path, description = "Plugin name"),
    ),
    responses(
        (status = 200, description = "Uninstall result"),
        (status = 404, description = "Plugin not found"),
    )
)]
pub async fn uninstall_plugin(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthInfo>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = plugin_manager(&state)?;
    manager.uninstall(&name).await.map_err(plugin_error)?;
    state
        .auth_manager
        .record_audit(
            &actor.id,
            "plugin.uninstall.completed",
            &name,
            serde_json::json!({}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    Ok(Json(serde_json::json!({ "ok": true, "name": name })))
}

/// 启用插件（写 `enabled:true`，下次启动生效）
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{name}/enable",
    tag = "Plugins",
    params(
        ("name" = String, Path, description = "Plugin name"),
    ),
    responses(
        (status = 200, description = "Enable result"),
        (status = 404, description = "Plugin not found"),
    )
)]
pub async fn enable_plugin(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthInfo>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    set_enabled(state, actor, name, true).await
}

/// 禁用插件（写 `enabled:false` 并立即停止适配器）
#[utoipa::path(
    post,
    path = "/api/v1/plugins/{name}/disable",
    tag = "Plugins",
    params(
        ("name" = String, Path, description = "Plugin name"),
    ),
    responses(
        (status = 200, description = "Disable result"),
        (status = 404, description = "Plugin not found"),
    )
)]
pub async fn disable_plugin(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthInfo>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    set_enabled(state, actor, name, false).await
}

async fn set_enabled(
    state: AppState,
    actor: AuthInfo,
    name: String,
    enabled: bool,
) -> Result<Json<serde_json::Value>, ApiError> {
    let manager = plugin_manager(&state)?;
    let event = if enabled {
        "plugin.enable.completed"
    } else {
        "plugin.disable.completed"
    };
    manager
        .set_enabled(&name, enabled)
        .await
        .map_err(plugin_error)?;
    state
        .auth_manager
        .record_audit(
            &actor.id,
            event,
            &name,
            serde_json::json!({"enabled": enabled}),
        )
        .await
        .map_err(|error| ApiError(GatewayError::Internal(error)))?;
    Ok(Json(
        serde_json::json!({ "ok": true, "name": name, "enabled": enabled }),
    ))
}

/// 获取插件管理器（未注入时返回 400）
fn plugin_manager(state: &AppState) -> Result<Arc<PluginManager>, ApiError> {
    state.plugin_manager.clone().ok_or_else(|| {
        ApiError(GatewayError::InvalidRequest(
            "Plugin system is not enabled (rebuild with --features plugin-system)".into(),
        ))
    })
}

/// 解析发布渠道（缺省 stable）
fn parse_channel(s: &str) -> PluginChannel {
    match s.to_ascii_lowercase().as_str() {
        "beta" => PluginChannel::Beta,
        _ => PluginChannel::Stable,
    }
}

/// 将插件管理器错误映射为 API 错误
///
/// 所有 `PluginManagerError` 消息均由服务端产生，可安全返回给已认证的管理端。
fn plugin_error(e: easybot_core::plugin::error::PluginManagerError) -> ApiError {
    let gateway_error = match &e {
        easybot_core::plugin::error::PluginManagerError::NotFound(_) => {
            GatewayError::InvalidRequest(e.to_string())
        }
        easybot_core::plugin::error::PluginManagerError::AlreadyInstalled(_)
        | easybot_core::plugin::error::PluginManagerError::DowngradeNotAllowed { .. } => {
            GatewayError::Conflict(e.to_string())
        }
        _ => GatewayError::InvalidRequest(e.to_string()),
    };
    ApiError(gateway_error)
}
