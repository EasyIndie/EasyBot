# QQ 适配器验证指南

验证范围：`easybot-adapter-qq` crate。

## 当前测试现状

| 测试名 | 类型 | 依赖网络 | 需要真实凭证 |
|--------|------|---------|-------------|
| `test_create_adapter` | 单元测试 | ❌ | ❌ |
| `test_capabilities` | 单元测试 | ❌ | ❌ |
| `test_status_summary` | 单元测试 | ❌ | ❌ |
| `test_bot_token_uninitialized` | 单元测试 | ❌ | ❌ |
| `test_token_store_new_needs_refresh` | 单元测试 | ❌ | ❌ |
| `test_token_store_get_uninitialized_returns_err` | 单元测试 | ❌ | ❌ |
| `test_token_store_clone` | 单元测试 | ❌ | ❌ |
| `test_init_missing_config` | 单元测试 | ❌ | ❌ |
| `test_init_valid_config` | 单元测试 | ❌ | ❌ |
| `test_qq_user_deserialize_with_bot_field` | 类型反序列化 | ❌ | ❌ |
| `test_qq_user_deserialize_without_bot_field` | 类型反序列化 | ❌ | ❌ |
| `test_channel_message_event_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_channel_message_event_without_guild_id` | 类型反序列化 | ❌ | ❌ |
| `test_group_message_event_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_group_message_create_event_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_group_message_create_event_no_mentions` | 类型反序列化 | ❌ | ❌ |
| `test_c2c_message_event_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_c2c_message_event_without_content` | 类型反序列化 | ❌ | ❌ |
| `test_send_message_request_serialize` | 类型序列化 | ❌ | ❌ |
| `test_send_message_response_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_keyboard_serialization_callback_button` | 类型序列化 | ❌ | ❌ |
| `test_keyboard_serialization_url_button` | 类型序列化 | ❌ | ❌ |
| `test_keyboard_serialization_multi_row` | 类型序列化 | ❌ | ❌ |
| `test_qq_guild_deserialize` | 类型反序列化 | ❌ | ❌ |
| `test_handle_dispatch_at_message` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_group_at` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_group_message_create_mentioned` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_group_message_create_not_mentioned` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_c2c` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_c2c_self_not_filtered` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_self_filter_channel` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_malformed_data` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_ignored_event` | 事件处理 | ❌ | ❌ |
| `test_handle_dispatch_missing_data` | 事件处理 | ❌ | ❌ |
| `test_send_before_connect` | 单元测试 | ❌ | ❌ |
| `test_runtime_config_before_init` | 单元测试 | ❌ | ❌ |
| `test_runtime_config_after_init` | 单元测试 | ❌ | ❌ |
| `test_health_before_init` | 单元测试 | ❌ | ❌ |
| `test_disconnect_idempotent` | 单元测试 | ❌ | ❌ |
| `test_double_disconnect` | 单元测试 | ❌ | ❌ |
| `test_get_chat_info_uninitialized` | 单元测试 | ❌ | ❌ |
| 10 个 `send_mock` 测试 | 集成测试 | ✅ (mock) | ❌ |

## 前置条件

### 平台机器人与凭证获取

#### QQ 统一机器人创建步骤（完整流程）

1. **注册账号**
   - 打开 [QQ 开放平台](https://q.qq.com/)
   - 点击右上角 **立即注册**（支持个人/企业实名）
   - 完成注册并登录

2. **创建机器人应用**
   - 登录后点击 **创建机器人**
   - 填写机器人名称、简介、头像等信息
   - 选择 **私域机器人**（推荐测试用，无需审核）
   - 创建成功后进入机器人管理页

3. **获取 AppID 和 AppSecret**
   - 左侧菜单 → **开发设置**
   - **BotAppID**：直接复制（如 `123456789`）
   - **AppSecret**：点击 **查看**（仅首次查看时可复制，离开后不可见），格式如 `your_app_secret_here_xxxxxxxxxxxx`
   - ⚠️ **AppSecret 非常重要**，不要泄露到版本控制系统

4. **配置 IP 白名单**（必须）
   - 同一页面 → **IP白名单**
   - 添加你运行服务的服务器/机器的**公网 IP**
   - 可用以下命令查看本机公网 IP：
     ```bash
     curl -s https://api.ipify.org
     ```
   - ⚠️ 如果本机有代理工具（Surge/Clash 等），需在代理工具中也配置 `api.sgroup.qq.com` 和 `bots.qq.com` 走直连（DIRECT），或添加代理的出口 IP 到白名单

5. **沙箱环境配置**（测试用）
   - 左侧菜单 → **沙箱配置**
   - **创建测试 QQ 群**：群名必须包含"测试"二字（如 "EasyBot测试群"），你必须是群主
   - 在沙箱配置页面下拉选择该群 → 添加
   - (可选) **私聊白名单**：添加允许与机器人私聊的 QQ 号
   - (可选) **频道测试**：如果你有频道主权限的频道，可在沙箱中添加

6. **在 QQ App 中添加机器人到群**
   - 手机 QQ → 进入测试群 → 右上角菜单 → **群机器人**
   - 在列表底部找到你的机器人 → 点击 **添加**
   - 之后在群里 @机器人 即可触发消息

### 配置

```bash
# 环境变量
export QQ_APP_ID="你的BotAppID"
export QQ_CLIENT_SECRET="你的AppSecret"
```

```yaml
# ~/.easybot/gateway.local.yaml
adapters:
  qq:
    enabled: true
    token: "${QQ_CLIENT_SECRET}"
    extra:
      app_id: "${QQ_APP_ID}"
```

启动命令：
```bash
cargo run --features full -- --debug
```

### 常见调试问题

| 问题 | 原因 | 解决方法 |
|------|------|---------|
| `接口访问源IP不在白名单` | 服务器 IP 未在 QQ 平台白名单中 | 在"开发设置→IP白名单"中添加本机公网 IP |
| DNS 解析到 `198.18.0.x` | 本机代理工具（Surge/Clash）拦截 DNS | 代理工具中配置 `api.sgroup.qq.com` 走 DIRECT，或临时关闭代理 |
| `主动消息失败, 无权限` | QQ 限制机器人主动发消息 | 必须用 `msg_id` 被动回复（回复用户 5 分钟内收到的消息） |
| `频道不存在` | 群消息用了 `/channels/` 而非 `/groups/` | 已自动处理（先试 channel 端点，失败则降级到 group 端点） |
| `缺少 channel_id` 字段 | 群消息格式与频道消息不同 | 已修复（代码根据事件类型使用对应的解析结构） |
| `不支持的调用` | 用了旧版 `/groups/` 而非 `/v2/groups/` | 已修复（群消息发送使用 `/v2/groups/{openid}/messages`） |

### 获取测试用的频道/群 ID

启动服务后，在 QQ 群里 @机器人 发消息，然后：

```bash
API_KEY=$(grep "Dev API Key" <服务日志文件> | grep -oP 'key=\K\S+')
curl -s -H "Authorization: Bearer $API_KEY" \
  "http://localhost:8080/api/v1/messages?platform=qq" | jq '.messages[0].chat_id'
```

群聊的 ID 是 `group_openid` 格式（如 `74241426963B2AD398CF5DD01AE48EC8`），频道的 ID 则是 `channel_id` 格式。

## 验证结果

### 已验证功能清单

| 验证项 | 状态 | 说明 |
|--------|------|------|
| **鉴权** | | |
| `getAppAccessToken` | ✅ | 通过 AppID + clientSecret 获取 access_token |
| REST API `QQBot {token}` 鉴权 | ✅ | `/users/@me` 认证成功 |
| Gateway Identify `QQBot {token}` | ✅ | Gateway Ready 事件收到 |
| Token 定时刷新 | ✅ | 每 3500s 自动刷新 |
| **适配器管理** | | |
| 自动启动 | ✅ | REST API 认证 → Gateway WebSocket 连接 |
| 停止适配器 | ✅ | `POST /adapters/qq/stop` → `Stopped` |
| 重启适配器 | ✅ | `POST /adapters/qq/start` → `Connected` |
| **出站消息** | | |
| 群聊消息发送 | ✅ | 实机验证通过 (2026-06-21) |
| 频道消息发送 | ✅ | `try_send()` 三级降级: 频道→群→C2C |
| C2C 私聊消息发送 | ✅ | 实机验证通过, `/v2/users/{openid}/messages` |
| 交互式按钮发送 | ✅ | InlineKeyboard → QQ MessageKeyboard |
| 主动消息发送 | ❌ | QQ 限制（需特殊权限），需通过被动回复方式 |
| **入站消息** | | |
| 群聊 @消息接收 (旧协议) | ✅ | `GROUP_AT_MESSAGE_CREATE` |
| 群聊全量消息接收 (新协议) | ✅ | `GROUP_MESSAGE_CREATE` + `mentions[]` 判断, 实机验证通过 |
| 频道 @消息接收 | ✅ | `AT_MESSAGE_CREATE`, 实机验证通过 |
| C2C 私聊消息接收 | ✅ | `C2C_MESSAGE_CREATE`, 实机验证通过 |
| @mention 检测 | ✅ | `mentions[].is_you` 判断, 实机验证正确 |
| 自身消息过滤 | ❌ | 群消息不含 `bot` 字段，需另寻方案 |
| **其他** | | |
| list_chats | ✅ | GET /users/@me/guilds, 返回群聊+私聊列表 |
| Gateway WebSocket 自动重连 | ✅ | 外层循环, 每次重连前刷新 token |
| 通用健康监控 | ✅ | AdapterManager.start_health_monitor() 30s 间隔 |

### 适配器修复清单

| 修复项 | 说明 | 文件 |
|--------|------|------|
| 增加 `QqTokenStore` | 新增 token 管理模块，支持 access_token 获取/刷新/缓存 | `lib.rs` |
| 更换鉴权方式 | REST API: `QQBot {token}`, Gateway: `QQBot {token}` | `lib.rs` |
| 修复 `QqUser.bot` 字段 | 旧响应含 `bot` 字段，新统一平台不含，改为 `Option<bool>` | `types.rs` |
| TLS 修正 | 使用 `native-tls` 替代 `rustls + webpki-roots`（GlobalSign 新 CA 不在 webpki-roots 中） | `Cargo.toml`, `lib.rs` |
| Gateway WebSocket 连接 | 手动建立 DNS→TCP→TLS→WebSocket 连接，支持 `native-tls` | `lib.rs` |
| 配置更新 | `token` 字段现在存储 `clientSecret`（AppSecret），env var 改为 `QQ_CLIENT_SECRET` | `gateway.local.yaml` |

### 验证中发现并修复的问题

| 问题 | 修复 | 文件 |
|------|------|------|
| `Bot {appid}.{token}` 鉴权 401 | 更换为 `getAppAccessToken` + `QQBot {access_token}` | `lib.rs` |
| `/users/@me` JSON 不含 `bot` 字段反序列化失败 | `QqUser.bot` 改为 `Option<bool>` | `types.rs` |
| Gateway rustls 证书验证失败 | 改用 `native-tls`（系统 CA，支持 GlobalSign Atlas R3 CA） | `Cargo.toml`, `lib.rs` |
| `GROUP_AT_MESSAGE_CREATE` 缺少 `channel_id` 字段解析失败 | 拆分为 `QqChannelMessageEvent` / `QqGroupMessageEvent` / `QqC2cMessageEvent` 三种消息结构 | `types.rs` |
| 群聊消息发送使用 `/channels/` 路由返回"频道不存在" | 新增 `try_send()` 自动降级到 `/v2/groups/{openid}/messages` | `lib.rs` |
| 主动消息"无权限"错误 | `send()` 增加 `msg_id` 参数支持被动回复 | `lib.rs` |
| C2C 私聊消息发送失败 | 在 `try_send()` 中增加 `/v2/users/{openid}/messages` C2C 端点降级 | `lib.rs` |

## 测试方法

### 1. 纯离线单元测试

```bash
cargo test -p easybot-adapter-qq
```

### 2. 端到端验证

#### 2.1 启动服务

```bash
export QQ_APP_ID="你的BotAppID"
export QQ_CLIENT_SECRET="你的AppSecret"
cargo run --features full -- --debug
```

预期日志：
```
INFO  QQ access token refreshed, expires in 7200s
INFO  QQ adapter connected: YourBot (id=...)
INFO  QQ Gateway ready
```

#### 2.2 适配器管理

```bash
API_KEY=$(curl -s http://127.0.0.1:8080/api/v1/health | jq -r '...')

# 查看所有适配器
curl -s -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters | jq .

# 查看 QQ 状态
curl -s -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/qq/status | jq .

# 停止
curl -s -X POST -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/qq/stop | jq .

# 启动
curl -s -X POST -H "Authorization: Bearer <key>" http://127.0.0.1:8080/api/v1/adapters/qq/start | jq .
```

### 3. 发送消息

```bash
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"target": "qq:<channelId>", "text": "Hello from EasyBot!", "msg_type": 0}' | jq .
```

> 注意：QQ 频道消息发送需要机器人已在目标频道中，并且需要具备对应权限。

## 关键实现细节

| 属性 | 值 |
|------|-----|
| API 地址 | `https://api.sgroup.qq.com` |
| 鉴权地址 | `https://bots.qq.com/app/getAppAccessToken` |
| REST API 鉴权 | `Authorization: QQBot {access_token}` |
| Gateway Identiy | `"token": "QQBot {access_token}"` |
| Token 有效期 | 7200 秒，提前 60 秒触发刷新 |
| 连接方式 | Gateway WebSocket (`wss://api.sgroup.qq.com/websocket`) |
| TLS 方案 | `native-tls`（系统 CA 证书） |
| 支持的消息类型 | Text (0), Image (2), Markdown |
| 支持的能力 | Text, Image, Markdown, Interactive, Group, Thread, MessageEdit, MessageDelete, ChatList |
| 默认 Intents | `AT_MESSAGE \| C2C_MESSAGE \| GROUP_AT_MESSAGE` |
| 入站事件类型 | `AT_MESSAGE_CREATE` / `GROUP_AT_MESSAGE_CREATE` / `GROUP_MESSAGE_CREATE` (2026 新版) / `C2C_MESSAGE_CREATE` |
| 新字段 `mentioned` | 频道/旧版群@ → `Some(true)`, 新版全量群 → `Some(bool)`, C2C → `None` |
| 自动重连 | ✅ 外层循环自动重连（同时刷新 token） |
| Token 定时刷新 | ✅ Gateway 事件循环中每 3500s 刷新一次 |

## 后续改进建议

- [x] ~~**QQ 频道 / C2C 端到端验证**~~ — 已完成 (2026-06-21), 群聊/私聊入站出站全部实机验证通过
- [x] ~~**补充 list_chats 实现**~~ — 已完成, GET /users/@me/guilds 返回群聊+私聊列表
- [x] ~~**send_interactive 交互式按钮**~~ — 已完成, InlineKeyboard → QQ MessageKeyboard 映射
- [ ] 添加入站消息的 `chat_name` 字段填充
- [ ] 考虑 Docker Alpine 环境下 `native-tls` 需要 OpenSSL 支持
