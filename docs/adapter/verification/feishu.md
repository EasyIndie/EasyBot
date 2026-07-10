# 飞书（Feishu/Lark）适配器验证指南

验证范围：`easybot-adapter-feishu` crate。

## 平台机器人与凭证获取

### 飞书应用创建步骤

1. **打开飞书开放平台**
   - 访问 [飞书开放平台](https://open.feishu.cn/)
   - 点击 **创建应用** → **企业自建应用**

2. **填写应用信息**
   - 应用名称（如 `EasyBotTest`）
   - 应用描述
   - 上传图标

3. **获取凭证**
   - 左侧菜单 → **凭证与基础信息**
   - **App ID**：格式如 `cli_xxxxxxxxxxxx`
   - **App Secret**：点击 **查看** 获取

4. **配置权限**
   - 左侧菜单 → **权限管理**
   - 添加以下权限（搜索并开启）：
     - `im:message` — 发送和接收消息
     - `im:message.p2p_msg:readonly` — 读取发给机器人的单聊消息
     - `im:message.group_at_msg:readonly` — 读取群聊中 @机器人的消息（默认）
     - `im:message.group_msg` — **【敏感权限】获取群组中所有消息**（接收非 @消息必需）
     - `im:resource` — 下载文件
     - `im:chat` — 获取群聊信息
     - `contact:user.base` — 获取用户信息
   - ⚠️ `im:message.group_msg` 是敏感权限，需要管理员审核通过
   - ⚠️ 权限修改后需要**发布新版本**并**审核通过**（自建应用可由管理员直接通过）

5. **订阅事件**
   - 左侧菜单 → **事件与回调**
   - 添加事件：`im.message.receive_v1`（接收消息）
   - ⚠️ 事件订阅时会弹窗要求确认权限，确保已勾选 `im:message.group_msg` 才能接收全量群消息
   - 发布新版本并审核通过

6. **获取 Chat ID（测试用）**
   - 飞书 → 目标群聊 → 右键群聊名称 → **复制群ID**
   - 或通过 API `GET /open-apis/im/v1/chats` 获取

### 配置方式

```bash
# 环境变量
export FEISHU_APP_ID="cli_xxxxxxxxxxxx"
export FEISHU_APP_SECRET="your_app_secret"
```

或者在 `~/.easybot/gateway.local.yaml` 中配置：

```yaml
adapters:
  feishu:
    enabled: true
    token: "${FEISHU_APP_SECRET}"
    extra:
      app_id: "${FEISHU_APP_ID}"
```

## 验证记录

| 功能 | 状态 | 备注 |
|------|------|------|
| Adapter init / connect | ✅ 通过 | 自动获取 tenant_access_token |
| 适配器生命期管理 (start/stop/status) | ✅ 通过 | REST API 正常控制 |
| 文本消息发送 | ✅ 通过 | 使用 `im/v1/messages` API |
| 媒体消息发送 | ✅ 通过 | 上传文件 + 获取 file_key (base64 解码 bug 已修复) |
| 交互式消息（按钮） | ✅ 通过 | 支持单按钮行 / 多按钮 action 组 |
| 消息编辑 | ✅ 通过 | 使用 `PUT /im/v1/messages/{id}` |
| 消息删除 | ✅ 通过 | `DELETE /im/v1/messages/{id}` (24h 内可撤回) |
| 入站消息接收 | ✅ 通过 | WebSocket 事件订阅 + SDK |
| 消息持久化 | ✅ 通过 | SQLite 存储发送和接收消息 |
| 会话管理 | ✅ 通过 | 自动创建 feishu 会话 |
| WebSocket 事件推送 | ✅ 通过 | EventBus + WS 推送 |

## 当前测试现状

| 测试名 | 类型 | 依赖网络 | 需要真实凭证 |
|--------|------|---------|-------------|
| `test_create_adapter` | 单元测试 | ❌ | ❌ |
| `test_capabilities` | 单元测试 | ❌ | ❌ |
| `test_status_summary` | 单元测试 | ❌ | ❌ |
| `test_init_missing_config` | 单元测试 | ❌ | ❌ |
| `test_init_missing_app_id` | 单元测试 | ❌ | ❌ |
| `test_init_valid_config` | 单元测试 | ❌ | ❌ |

## 实现细节

| 属性 | 值 |
|------|-----|
| 连接方式 | REST API + WebSocket 事件订阅（protobuf 二进制） |
| 鉴权方式 | `tenant_access_token`（自动刷新，7200s 过期） |
| 入站 SDK | `larksuite-oapi-sdk-rs` v0.1.2 + `ws` feature |
| 事件协议 | 飞书 v2.0 事件协议 (schema: "2.0") |
| 事件类型 | `im.message.receive_v1` |
| 群聊消息权限 | 默认 `group_at_msg:readonly`（仅 @消息）；全量需 `group_msg` 敏感权限 |
| 字段名 | `message_type`（非 `msg_type`）|
| Token 刷新 | ✅ 自动刷新，300s 提前量 |
| 能力声明 | Text、Image、Audio、Video、Document、Interactive、Markdown、Group、MessageEdit、MessageDelete |

### WebSocket 事件接收

使用飞书官方 SDK (`larksuite-oapi-sdk-rs`) 的 WebSocket 客户端：

1. `Client::builder(app_id, app_secret).build()` 创建 SDK 客户端
2. `EventDispatcher::new("", "").skip_sign_verify().on_event("im.message.receive_v1", handler)` 注册事件处理器
3. `ws_client.start()` 启动长连接，自动处理 protobuf 帧、分片重组、ping/pong
4. 收到 `im.message.receive_v1` 事件后，解析 `event` 字段为 `FeishuMessageReceiveEvent`
5. 构建 `InboundMessage` 并通过 `EventBus::publish()` 发布

### 入站消息字段映射

| 飞书事件字段 | InboundMessage 字段 |
|-------------|-------------------|
| `message.message_id` | `id` |
| `message.chat_id` | `chat_id` |
| `message.chat_type` ("group"/"p2p") | `chat_type` (Group/Dm) |
| `message.message_type` ("text") | `text` (从 content JSON 中提取) |
| `message.content` | `text` (JSON 解码) |
| `sender.sender_id.open_id` | `author.id`, `author.name` |
| `message.create_time` | `timestamp` (毫秒) |

## 调试步骤

### 启动服务

```bash
FEISHU_APP_ID="cli_xxx" FEISHU_APP_SECRET="xxx" cargo run -- --debug
```

### 验证适配器状态

```bash
# 获取 API Key（从启动日志中）
# Dev API Key created: ... key=eb_xxx

API_KEY="eb_xxx"

# 查看适配器列表
curl -s http://localhost:8080/api/v1/adapters \
  -H "Authorization: Bearer $API_KEY" | jq .

# 发送消息
curl -s -X POST http://localhost:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"target": "feishu:oc_xxx", "text": "Hello from EasyBot"}' | jq .

# 查看消息历史
curl -s "http://localhost:8080/api/v1/messages?platform=feishu" \
  -H "Authorization: Bearer $API_KEY" | jq .
```

## 后续改进建议

- [x] ~~修复 `upload_media` 中 base64 未解码的 bug（已改为 `STANDARD.decode()`）~~
- [x] ~~在入站消息处理时递增适配器内部 `messages_in` 计数器~~（已通过 `Arc<AtomicU64>` 传递并递增）
- [ ] 添加飞书 `im.message.receive_v1` 事件签名的真实验证（当前使用 `skip_sign_verify()`）
- [ ] 支持更多消息类型（图片、文件等入站消息的 content 解析）
- [ ] 合并两套独立 token 管理系统（适配器实例的 `access_token` + WebSocket 任务的 `token_cache`）
