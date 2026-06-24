//! 权限管理
//!
//! 提供细粒度权限位标志和权限检查函数。
//! API Key 的 `permissions` 字段支持 `"*"` 通配符授予全部权限。

use crate::types::error::GatewayError;

/// 权限位标志
///
/// 每个受保护路由需要特定权限。`"*"` 通配符授予所有权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// 读取消息历史
    MessagesRead,
    /// 发送/编辑/删除消息
    MessagesSend,
    /// 查看适配器列表和状态
    AdaptersRead,
    /// 启动/停止适配器
    AdaptersManage,
    /// 读取配置
    ConfigRead,
    /// 修改配置（热重载）
    ConfigWrite,
    /// 查看会话列表和详情
    SessionsRead,
    /// 删除会话
    SessionsManage,
    /// 建立 WebSocket 连接
    WebSocketConnect,
}

/// 检查 AuthInfo 是否持有所需权限
///
/// `"*"` 通配符或对应的权限名称均视为授权通过。
/// 权限名称匹配不区分大小写。
pub fn require_permission(
    auth: &crate::auth::AuthInfo,
    required: Permission,
) -> Result<(), GatewayError> {
    let perm_name = format!("{:?}", required).to_lowercase();
    if auth.permissions.contains(&"*".to_string())
        || auth
            .permissions
            .iter()
            .any(|p| p.to_lowercase() == perm_name)
    {
        Ok(())
    } else {
        Err(GatewayError::Forbidden(format!("需要权限 {perm_name}")))
    }
}
