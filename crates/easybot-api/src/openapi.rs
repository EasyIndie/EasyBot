//! OpenAPI 规范文档
//!
//! 定义 EasyBot API 的 OpenAPI 3.1 规范，自动从路由处理函数和数据结构生成。
//! 通过 utoipa 的宏标注自动收集所有端点和类型定义。

use crate::routes;
use utoipa::openapi::SecurityRequirement;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

/// OpenAPI 修改器：添加 Bearer Token 安全方案
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "ApiKeyAuth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("eb_xxxx")
                        .description(Some(String::from(
                            "输入完整的 Bearer token，例如 `Bearer eb_xxxxxxxxxxxx`",
                        )))
                        .build(),
                ),
            );
        }
        let req = SecurityRequirement::new("ApiKeyAuth", Vec::<String>::new());
        openapi.security = Some(vec![req]);
    }
}

/// EasyBot API 的 OpenAPI 文档定义
#[derive(OpenApi)]
#[openapi(
    info(
        title = "EasyBot API",
        description = "EasyBot 即时通信网关服务 API\n\n\
            EasyBot 是一个独立的 IM 网关服务，连接多个即时通信平台（Telegram、Discord 等），\n\
            对外提供统一的 REST API 和 WebSocket 实时事件推送。\n\n\
            ## 目标格式\n\
            所有涉及消息发送的接口，目标格式为 `platform:chatId`，例如 `telegram:123456789`。\n\n\
            ## 认证\n\
            所有 API 请求（除 /health 外）需要携带 API Key。\n\
            认证方式：在 HTTP Header 中添加 `Authorization: Bearer <api-key>`。\n\
            在 Swagger UI 中点击右上角的 **Authorize** 按钮，输入 API Key（如 `eb_xxxxxxxx`）即可。",
        version = env!("CARGO_PKG_VERSION"),
        license(name = "GPL-3.0"),
    ),
    modifiers(&SecurityAddon),
    servers(
        (url = "http://localhost:8080", description = "Local development"),
    ),
    paths(
        routes::health::health_check,
        routes::adapters::list_adapters,
        routes::adapters::start_adapter,
        routes::adapters::stop_adapter,
        routes::adapters::adapter_status,
        routes::messages::send_message,
        routes::messages::batch_send,
        routes::messages::edit_message,
        routes::messages::delete_message,
        routes::messages::message_history,
        routes::sessions::list_sessions,
        routes::sessions::get_session,
        routes::sessions::delete_session,
        routes::chats::list_chats,
        routes::chats::get_chat,
        routes::config::get_config,
        routes::config::update_config,
        routes::ws::ws_handler,
        // Admin — API Key 管理
        routes::admin::list_api_keys,
        routes::admin::create_api_key,
        routes::admin::revoke_api_key,
        routes::admin::purge_api_key,
        routes::admin::list_api_key_types,
        routes::admin::admin_login,
        // System
        routes::system::system_info,
        // Logs
        routes::logs::log_entries,
    ),
    components(
        schemas(
            // Admin — API Keys
            routes::admin::ApiKeyResponse,
            routes::admin::CreateApiKeyRequest,
            routes::admin::CreateApiKeyResponse,
            routes::admin::RevokeResponse,
            routes::admin::ApiKeyTypesResponse,
            routes::admin::LoginRequest,
            routes::admin::LoginResponse,
            // Health
            routes::health::HealthResponse,
            routes::health::AdapterSummary,
            routes::health::SessionSummary,
            // Adapters
            routes::adapters::AdapterListResponse,
            routes::adapters::AdapterItem,
            // Messages
            routes::messages::SendMessageRequest,
            routes::messages::BatchSendRequest,
            routes::messages::EditMessageRequest,
            routes::messages::DeleteMessageRequest,
            routes::messages::MessageHistoryParams,
            routes::messages::MessageHistoryResponse,
            // Core types (API-facing)
            easybot_core::types::message::InboundMessage,
            easybot_core::types::message::OutboundMessage,
            easybot_core::types::message::SendTextParams,
            easybot_core::types::message::SendMediaParams,
            easybot_core::types::message::SendInteractiveParams,
            easybot_core::types::message::EditMessageParams,
            easybot_core::types::message::SendResult,
            easybot_core::types::message::EditResult,
            easybot_core::types::message::DeleteResult,
            easybot_core::types::message::ChatType,
            easybot_core::types::message::ParseMode,
            easybot_core::types::message::MessageSender,
            easybot_core::types::message::MessageType,
            easybot_core::types::message::MentionInfo,
            easybot_core::types::message::SenderRole,
            easybot_core::types::message::MediaAttachment,
            easybot_core::types::message::MediaType,
            easybot_core::types::message::CommandData,
            easybot_core::types::message::CallbackData,
            easybot_core::types::message::MessageReference,
            easybot_core::types::message::InlineKeyboard,
            easybot_core::types::message::KeyboardRow,
            easybot_core::types::message::Button,
            easybot_core::types::message::CallbackEvent,
            easybot_core::types::message::ChatInfo,
            easybot_core::types::message::ChatFilter,
            // Session
            easybot_core::types::session::Session,
            easybot_core::types::session::SessionSource,
            easybot_core::types::session::ResetPolicy,
            // Adapter
            easybot_core::types::adapter::AdapterConfig,
            easybot_core::types::adapter::AdapterStatusSummary,
            easybot_core::types::adapter::AdapterState,
            easybot_core::types::adapter::Capability,
            easybot_core::types::adapter::CapabilityName,
            easybot_core::types::adapter::CapabilityLimits,
            easybot_core::types::adapter::HealthStatus,
            easybot_core::types::adapter::HealthReport,
            easybot_core::types::adapter::BotInfo,
            easybot_core::types::adapter::AdapterRuntimeConfig,
            // Config
            easybot_core::types::config::GatewayConfig,
            easybot_core::types::config::ServerConfig,
            easybot_core::types::config::ApiConfig,
            easybot_core::types::config::WebSocketConfig,
            easybot_core::types::config::StorageConfig,
            easybot_core::types::config::LoggingConfig,
            easybot_core::types::config::TlsConfig,
            easybot_core::types::config::WebhookConfig,
            // Event
            easybot_core::types::event::GatewayEvent,
            easybot_core::types::event::EventMetadata,
            // Error
            easybot_core::types::error::ApiErrorResponse,
            easybot_core::types::error::ApiErrorDetail,
            // Logs
            routes::logs::LogQuery,
        )
    ),
    tags(
        (name = "Health", description = "服务健康检查"),
        (name = "Adapters", description = "适配器管理（启动/停止/状态查询）"),
        (name = "Messages", description = "消息发送与管理"),
        (name = "Sessions", description = "会话管理"),
        (name = "Chats", description = "聊天信息查询"),
        (name = "Config", description = "网关配置管理"),
        (name = "WebSocket", description = "WebSocket 实时事件推送"),
        (name = "API Keys", description = "API Key 管理（创建/列出/吊销/删除）"),
        (name = "System", description = "系统信息查询"),
        (name = "Logs", description = "日志查询"),
        (name = "Admin", description = "管理后台登录"),
    )
)]
pub struct ApiDoc;
