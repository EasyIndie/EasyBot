# 个人微信 (WeChat) 适配器验证指南

验证范围：`easybot-adapter-wechat` crate。

## 平台机器人与凭证获取

### iLink Bot API 接入

个人微信适配器使用 **腾讯官方 iLink Bot API** (`ilinkai.weixin.qq.com`)，这是腾讯在 2026 年开放的官方个人微信 Bot 协议。

**与 Openclaw/Hermes 对齐**：
- Openclaw：`@tencent-weixin/openclaw-weixin` 官方插件
- Hermes：`weixin.py` 原生适配器（PR #7268）
- EasyBot：`easybot-adapter-wechat`（Rust 实现）

### 登录流程

首次启动时自动打印 QR 码，扫码后自动保存凭据。

```
启动服务 → 终端打印 QQ 码链接
  → 手机打开链接 → 微信扫码确认
  → 凭据保存到 ~/.easybot/.wechat-credentials.json
  → 下次启动自动加载，无需扫码
```

### 配置

```yaml
# ~/.easybot/gateway.local.yaml
adapters:
  wechat:
    enabled: true
```

凭据文件 `~/.easybot/.wechat-credentials.json` 自动管理（扫码登录后保存、过期后清除）。

## 当前测试现状

| 测试名 | 类型 | 依赖网络 | 需要真实凭证 |
|--------|------|---------|-------------|
| `test_create_adapter` | 单元测试 | ❌ | ❌ |
| `test_capabilities` | 单元测试 | ❌ | ❌ |
| `test_init` | 单元测试 | ❌ | ❌ |
| `test_init_with_saved_credentials` | 单元测试 | ❌ | ❌ |
| `test_status_summary` | 单元测试 | ❌ | ❌ |
| `test_base64_encode_uin` | 单元测试 | ❌ | ❌ |
| `test_base64_encode_uin_zero` | 单元测试 | ❌ | ❌ |
| `test_base64_encode_uin_max` | 单元测试 | ❌ | ❌ |
| `test_default` | 单元测试 | ❌ | ❌ |
| `test_runtime_config_before_init` | 单元测试 | ❌ | ❌ |
| `test_runtime_config_after_init` | 单元测试 | ❌ | ❌ |
| `test_health_before_init` | 单元测试 | ❌ | ❌ |
| `test_get_chat_info_always_dm` | 单元测试 | ❌ | ❌ |
| ~~`test_new_with_event_bus`~~ | 已移除（P2-3 重构） | — | — |
| `test_double_disconnect` | 单元测试 | ❌ | ❌ |
| `test_disconnect_idempotent` | 单元测试 | ❌ | ❌ |
| `test_send_before_connect_errors` | 单元测试 | ❌ | ❌ |
| `test_send_media_before_connect_errors` | 单元测试 | ❌ | ❌ |
| `test_convert_text_message` | 消息转换 | ❌ | ❌ |
| `test_convert_image_message` | 消息转换 | ❌ | ❌ |
| `test_convert_file_message` | 消息转换 | ❌ | ❌ |
| `test_convert_group_message` | 消息转换 | ❌ | ❌ |
| `test_convert_unknown_message_type` | 消息转换 | ❌ | ❌ |
| `test_convert_empty_item_list` | 消息转换 | ❌ | ❌ |
| `test_deserialize_weixin_message_from_json` | 反序列化 | ❌ | ❌ |
| `test_deserialize_image_weixin_message` | 反序列化 | ❌ | ❌ |
| `test_deserialize_empty_getupdates_response` | 反序列化 | ❌ | ❌ |
| `test_credentials_serialization_roundtrip` | 序列化 | ❌ | ❌ |

## 验证结果

### 已验证功能清单

| 验证项 | 状态 | 说明 |
|--------|------|------|
| **登录/鉴权** | | |
| QR 码获取 | ✅ | `GET /ilink/bot/get_bot_qrcode` 成功返回 |
| 扫码状态轮询 | ✅ | wait → scaned → confirmed |
| bot_token 获取 | ✅ | 扫码确认后成功获取 |
| 凭据自动保存 | ✅ | 保存到 `~/.easybot/.wechat-credentials.json` |
| 凭据自动加载 | ✅ | 重启后自动加载，无需重复扫码 |
| **适配器管理** | | |
| 自动启动 | ✅ | QR 登录后 connected: true |
| 适配器停止 | ✅ | `POST /adapters/wechat/stop` |
| **出站消息** | | |
| 文本消息发送 | ✅ | `POST /ilink/bot/sendmessage` |
| 媒体消息发送 | ✅ | AES-128-ECB 加密 + CDN 上传（Image/Audio/Video/Document） |
| **入站消息** | | |
| 长轮询接收 | ✅ | `POST /ilink/bot/getupdates` 每 ~18s 轮询 |
| 文本消息解析 | ✅ | 通过 `item_list[].text_item.text` 提取 |
| 图片消息解析 | ✅ | 含 `file_url`、`file_size` 等元数据 |
| 文件消息解析 | ✅ | 含文件名、大小、CDN URL |
| 消息入库 | ✅ | SQLite 持久化 |
| **连接方式** | | |
| HTTP 长轮询 | ✅ | 35s 超时，自动重连 |
| Session 过期处理 | ✅ | 连续 10 次失败清除凭据 |

### 已验证的端到端流程

```
用户微信 → 发消息 → iLink API → 长轮询接收 → EventBus → SQLite 入库
EasyBot API → send → iLink API → 用户微信收到
```

## 测试方法

### 1. 纯离线单元测试

```bash
cargo test -p easybot-adapter-wechat
```

### 2. 端到端验证

#### 2.1 启动服务

```bash
cargo run --features "adapter-wechat" -- --debug
```

首次启动输出 QR 码链接，手机打开链接扫码确认。

#### 2.2 验证登录

```log
INFO  个人微信适配器：需要扫码登录        # 首次启动
INFO  扫描以下二维码...
INFO  等待扫码...                          # 等待用户扫码
INFO  个人微信登录成功                     # 扫码确认
INFO  个人微信凭据已保存                   # 凭据写入磁盘
INFO  个人微信适配器已连接                 # 连接成功
```

第二次启动自动加载凭据：

```log
INFO  个人微信适配器：从磁盘加载保存的凭据  # 自动加载
INFO  个人微信适配器已连接                 # 直接连接
```

#### 2.3 适配器管理

```bash
API_KEY="eb_xxx"  # 从启动日志中获取

# 查看适配器
curl -s -H "Authorization: Bearer $API_KEY" http://127.0.0.1:8080/api/v1/adapters

# 发送消息
curl -s -X POST http://127.0.0.1:8080/api/v1/messages/send \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"target": "wechat:<user_id>", "text": "测试消息"}'

# 查看消息历史
curl -s "http://localhost:8080/api/v1/messages?platform=wechat" \
  -H "Authorization: Bearer $API_KEY"
```

## 关键实现细节

| 属性 | 值 |
|------|-----|
| API 地址 | `https://ilinkai.weixin.qq.com/ilink/bot/*` |
| 登录方式 | QR 码扫码 + bot_token |
| 消息接收 | HTTP 长轮询 (35s timeout) |
| 消息发送 | `POST /ilink/bot/sendmessage` |
| 凭据有效期 | ~24 小时（过期后需重新扫码） |
| 凭据持久化 | `~/.easybot/.wechat-credentials.json` (0600) |
| 鉴权头 | `Authorization: Bearer <bot_token>` |
| | `AuthorizationType: ilink_bot_token` |
| | `X-WeChat-Uin: <base64_uin>`（防重放）|
| 消息格式 | `item_list[]` 数组（支持多项）|
| 媒体加密 | AES-128-ECB（getuploadurl → 加密 → CDN POST → 提取 x-encrypted-param）|
| 支持的能力 | Text / Image / Audio / Video / Document（全部支持）|

## 已知限制

- **入站消息支持群聊** — 群聊消息通过 `group_id` 识别并设置 `is_group: true`，但发送群聊消息的能力取决于 iLink Bot API 权限
- **无 Markdown** — 微信客户端不渲染
- **Session 24h 过期** — 过期后需重启服务重新扫码
- **无历史消息 API** — 仅游标式实时拉取

## 后续改进建议

- [x] 媒体消息发送（AES-128-ECB 加密 + CDN 上传）
- [x] 修复非文本消息占位符（已改为空文本 + `MediaAttachment`）
- [x] 修复 `save_sync_buf` 同步 I/O（已用 `spawn_blocking` 包裹）
- [x] ~~CDN x-encrypted-param 日志脱敏~~
- [ ] 凭据过期时自动重启适配器触发重新登录
- [x] ~~将 `save_context_tokens` 移出异步热路径~~（send/send_media 2 处 session 过期路径已用 `spawn_blocking` 包裹）
- [ ] 添加入站消息的 `chat_name` 字段填充（iLink API 仅提供 `from_user_id`/`group_id` 等不透明标识符，无名名字段。需额外 API 或映射表）
- [ ] 支持语音消息转录文本
- [ ] 支持引用/回复消息
