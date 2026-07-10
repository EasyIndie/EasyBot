# 即时通信网关架构设计说明书

> **摘要：** 一份语言无关的即时通信网关（EasyBot）架构设计。该网关作为独立服务运行，连接多种 IM 平台，对外暴露统一的 API 供第三方客户端调用，支持消息发送与接收的双向通信。设计遵循契约驱动、分层隔离、插件扩展的原则。
>
> **当前实现**: EasyBot 已用 Rust（tokio + axum tower + sqlx）实现，详见 `CLAUDE.md` 和源码。

---

## 第一章：总体架构

### 1.1 系统定位

EasyBot 是连接 **IM 平台**与**业务系统**之间的独立中间层服务：

- **南向（Southbound）**：接入 Telegram、Discord、飞书、QQ、微信等 IM 平台
- **北向（Northbound）**：对外暴露 RESTful API + WebSocket，供业务系统调用
- **双向转发**：IM 消息 → 业务系统（WebSocket/Webhook），业务系统消息 → IM 平台

### 1.2 架构分层

```
   ┌───────────────────────────────────────────────┐
   │          Third-party Client / Service         │
   │             (REST API / WS Client)            │
   └───────────────────────┬───────────────────────┘
                           │                        
                           ↕  REST / WebSocket      
                           │                        
   ┌───────────────────────┴───────────────────────┐
   │                EasyBot Gateway                │
   ├───────────────────────────────────────────────┤
   │             API Server (REST / WS)            │
   ├───────────────────────────────────────────────┤
   │                  Gateway Core                 │
   │   ┌──────┐  ┌──────┐  ┌──────┐                │
   │   │ Msg  │  │ Ses  │  │ Auth │                │
   │   │ Mgr  │  │ Mgr  │  │      │                │
   │   └──────┘  └──────┘  └──────┘                │
   ├───────────────────────────────────────────────┤
   │                Adapter Manager                │
   └───────┬───────┬───────┬───────┬───────┬───────┘
           │       │       │       │       │        
           ↕       ↕       ↕       ↕       ↕        
       Telegram Discord  Lark     QQ    WeChat      
           │       │       │       │       │        
           ↕       ↕       ↕       ↕       ↕        
   ┌───────┴───────┴───────┴───────┴───────┴───────┐
   │           End Users (chatting on IM)          │
   └───────────────────────────────────────────────┘
```

### 1.3 核心概念

| 概念 | 说明 |
|------|------|
| **IM 平台** | 即时通信服务商，如 Telegram、Discord |
| **平台适配器** | 连接特定 IM 平台的模块，处理协议差异 |
| **聊天** | IM 平台上的一个对话（私聊/群聊/频道） |
| **消息** | 在聊天中传递的内容单元 |
| **会话** | 以 `platform:chatId` 标识的持久化对话上下文 |
| **外部客户端** | 调用网关 API 的第三方业务系统 |
| **目标** | 消息投递的目的地描述 `platform:chatId` |

---

## 第二章：API 接口设计

### 2.1 总览

外部客户端通过 **REST API** 进行命令式交互，通过 **WebSocket** 接收实时推送。

基础 URL：`http(s)://<host>:<port>/api/v1`

### 2.2 REST API 路由

| 方法 | 路径 | 说明 | 认证 |
|------|------|------|------|
| GET | `/health` | 健康检查 | ❌ |
| GET | `/adapters` | 适配器状态列表 | ✅ |
| POST | `/adapters/{platform}/start` | 启动适配器 | ✅ |
| POST | `/adapters/{platform}/stop` | 停止适配器 | ✅ |
| GET | `/adapters/{platform}/status` | 适配器详细状态 | ✅ |
| POST | `/messages/send` | 发送消息 | ✅ |
| POST | `/messages/batch-send` | 批量发送 | ✅ |
| PUT | `/messages/{message_id}` | 编辑消息 | ✅ |
| DELETE | `/messages/{message_id}` | 删除消息 | ✅ |
| GET | `/messages` | 消息历史（支持 `?sessionKey=`、`?platform=` 过滤） | ✅ |
| GET | `/sessions` | 会话列表 | ✅ |
| GET | `/sessions/{key}` | 会话详情 | ✅ |
| DELETE | `/sessions/{key}` | 删除会话 | ✅ |
| GET | `/chats/{platform}` | 聊天列表 | ✅ |
| GET | `/chats/{platform}/{chat_id}` | 聊天详情 | ✅ |
| GET/PUT | `/config` | 获取/更新配置 | ✅ |
| GET/POST | `/api-keys` | 列出/创建 API Key | ✅ |
| GET | `/api-keys/types` | API Key 类型列表 | ✅ |
| DELETE | `/api-keys/{id}` | 吊销 API Key | ✅ |
| DELETE | `/api-keys/{id}/purge` | 彻底删除 API Key | ✅ |
| GET | `/metrics` | Prometheus 指标 | ✅ |
| GET | `/logs` | 实时日志流（环形缓冲） | ✅ |
| GET | `/ws` | WebSocket 升级 | ✅ |
| GET | `/swagger` | Swagger UI | ❌ |
| GET | `/openapi.json` | OpenAPI 3.1 Schema | ❌ |

#### 2.2.1 消息发送

```
POST /api/v1/messages/send
Content-Type: application/json
Authorization: Bearer <api-key>

{
  "target": "telegram:123456789",
  "text": "Hello from API!",
  "parse_mode": "markdown",
  "media": {
    "media_type": "Image",
    "url": "https://example.com/photo.jpg",
    "caption": "照片说明"
  },
  "reply_to": "98765",
  "metadata": { "disable_web_page_preview": true }
}

Response 200:
{
  "id": "msg_abc123",
  "status": "sent",
  "messageId": "msg_telegram_98765",
  "timestamp": 1718000000000
}
```

> `parse_mode`: `markdown` / `html` / `none`（默认 `none`）

#### 2.2.2 批量发送

```
POST /api/v1/messages/batch-send
Content-Type: application/json

{
  "targets": [
    "telegram:123456",
    "discord:789012"
  ],
  "text": "群发公告：系统维护通知...",
  "parse_mode": "markdown"
}

Response 200:
{
  "total": 2,
  "results": {
    "telegram:123456": { "status": "sent", "messageId": "msg_1" },
    "discord:789012": { "status": "failed", "error": "chat not found" }
  }
}
```

> 最大并发 5 个，整体超时 30 秒。

#### 2.2.3 消息编辑与删除

```
PUT /api/v1/messages/{message_id}
{
  "target": "telegram:123456",
  "text": "修改后的消息",
  "parse_mode": "markdown"
}

Response 200: { "ok": true, "updated_at": 1718000100000, "error": null }

DELETE /api/v1/messages/{message_id}
{
  "target": "telegram:123456"
}

Response 200: { "ok": true, "error": null }
```

> 编辑/删除能力因平台而异（微信不支持，QQ 仅频道消息支持）。

#### 2.2.4 适配器管理

```
GET /api/v1/adapters
Response 200:
{
  "adapters": [
    {
      "platform": "telegram",
      "display_name": "Telegram",
      "status": "Connected",
      "connected": true
    },
    {
      "platform": "discord",
      "display_name": "Discord",
      "status": "Failed",
      "connected": false
    }
  ]
}

POST /api/v1/adapters/{platform}/start
Response 200: { "ok": true, "pending": false, "platform": "telegram", "error": null }

POST /api/v1/adapters/{platform}/stop
Response 200: { "ok": true, "platform": "telegram" }

GET /api/v1/adapters/{platform}/status
Response 200:
{
  "platform": "telegram",
  "display_name": "Telegram",
  "state": "Connected",
  "connected": true,
  "health": "Healthy",
  "last_error": null,
  "uptime": 3600,
  "messages_in": 150,
  "messages_out": 200,
  "errors": 2
}
```

#### 2.2.5 聊天管理

```
GET /api/v1/chats/telegram
Response 200:
{
  "chats": [
    { "id": "123456", "name": "Alice", "chat_type": "Dm" },
    { "id": "-789012", "name": "Team Chat", "chat_type": "Group", "member_count": 15 }
  ]
}

GET /api/v1/chats/telegram/123456
Response 200:
{
  "chat_id": "123456",
  "name": "Alice",
  "chat_type": "Dm"
}
```

#### 2.2.6 消息历史

```
GET /api/v1/messages?session_key=telegram:123456&limit=50&before=1717000000000
Response 200:
{
  "messages": [
    {
      "id": "...",
      "direction": "inbound",
      "platform": "telegram",
      "text": "你好",
      "sender": { "id": "user_1", "name": "Alice" },
      "timestamp": 1718000000000
    }
  ],
  "has_more": false
}
```

#### 2.2.7 健康检查

```
GET /health

Response 200:
{
  "status": "healthy",               // healthy | degraded
  "version": "0.0.14",
  "uptime": 86400,
  "adapters": { "total": 5, "connected": 4 },
  "sessions": { "active": 42 }
}
```

> `status` 为 `healthy` 表示至少一个适配器已连接，`degraded` 表示无适配器连接。

#### 2.2.8 配置管理

```
GET /api/v1/config → 当前运行时配置

PUT /api/v1/config
Body: { /* patch 配置 */ }
Response 200: { "ok": true }
```

### 2.3 WebSocket 实时推送

**连接地址：** `ws://<host>:<port>/api/v1/ws`

**认证方式：** 连接成功后发送 JSON 帧 `{"token":"your-api-key"}`（不使用 HTTP Authorization 头，因为部分 WebSocket 客户端不支持自定义 HTTP 头）。

**心跳：** 服务器定期发送 `{"type":"ping"}`，客户端回复 `{"type":"pong"}`。

**服务器推送事件格式：**
```json
{
  "type": "event",
  "event": "message.inbound",
  "data": { /* 事件负载 */ },
  "seq": 42,
  "timestamp": 1718000000000
}
```

**标准事件：**

| 事件名 | 触发时机 |
|--------|----------|
| `message.inbound` | 收到 IM 平台消息 |
| `message.sent` | 消息发送成功 |
| `message.failed` | 消息发送失败 |
| `adapter.connected` | 适配器连接成功 |
| `adapter.disconnected` | 适配器断开 |
| `adapter.reconnecting` | 适配器正在重连 |
| `adapter.reconnected` | 适配器重连成功 |
| `adapter.reconnect_failed` | 适配器重连失败 |
| `adapter.error` | 适配器异常 |
| `callback.received` | 收到按钮回调 |
| `gateway.started` | 网关启动 |
| `gateway.stopping` | 网关关闭 |
| `config.changed` | 配置已热重载 |

### 2.4 统一错误格式

```json
{
  "error": {
    "code": "ADAPTER_NOT_FOUND",
    "message": "Platform 'foobar' is not configured",
    "details": { "availablePlatforms": ["telegram", "discord"] },
    "requestId": "req_abc123"
  }
}
```

**标准错误码：**

| 错误码 | HTTP 状态码 | 含义 |
|--------|------------|------|
| `INVALID_REQUEST` | 400 | 请求参数格式错误 |
| `UNAUTHORIZED` | 401 | API Key 无效 |
| `FORBIDDEN` | 403 | 无权限 |
| `PLATFORM_NOT_FOUND` | 404 | 平台未配置 |
| `CHAT_NOT_FOUND` | 404 | 目标聊天不存在 |
| `ADAPTER_NOT_CONNECTED` | 503 | 适配器未连接 |
| `MESSAGE_TOO_LONG` | 400 | 消息超长 |
| `RATE_LIMITED` | 429 | 平台限流 |
| `CAPABILITY_NOT_SUPPORTED` | 400 | 平台不支持该能力 |
| `INTERNAL_ERROR` | 500 | 内部错误 |

---

## 第三章：南向适配器接口

### 3.1 适配器生命周期

```
┌──────────────────────────────────────────────────────────────┐
│                      Adapter Lifecycle                       │
│                                                              │
│  ┌─────────┐   ┌──────────┐   ┌───────────┐ ┌──────────┐     │
│  │ CREATED │──→│ STARTING │──→│CONNECTING │→│CONNECTED │     │
│  └─────────┘   └──────────┘   └───────────┘ └─────┬────┘     │
│       │                                           │          │
│       │        disconnect / network err           │          │
│       │                                           ▼          │
│       │                                    ┌────────────┐    │
│       │                                    │RECONNECTING│    │
│       │                                    └──────┬─────┘    │
│       │                                           │          │
│       │              max retries                  │          │
│       │                                           ▼          │
│       │                                    ┌────────────┐    │
│       └───────────────────────────────────→│   FAILED   │    │
│                                            └────────────┘    │
│                                                              │
│              any state --stop()--> STOPPED                   │
└──────────────────────────────────────────────────────────────┘
```

### 3.2 适配器接口定义（IDL）

```typescript
/**
 * 平台适配器接口
 * 每个 IM 平台的连接器必须实现此接口。
 */
interface PlatformAdapter {
  // ── 元数据 ──
  readonly platformName: string;
  readonly displayName: string;
  readonly capabilities: Capability[];

  // ── 生命周期 ──
  init(config: AdapterConfig): Promise<InitResult>;
  connect(): Promise<ConnectResult>;
  disconnect(): Promise<void>;
  state(): AdapterState;
  health(): Promise<HealthReport>;

  // ── 消息发送 ──
  send(params: SendTextParams): Promise<SendResult>;
  sendMedia?(params: SendMediaParams): Promise<SendResult>;
  sendInteractive?(params: SendInteractiveParams): Promise<SendResult>;
  sendTyping?(chatId: string): Promise<void>;
  sendDraft?(params: SendDraftParams): Promise<DraftResult>;

  // ── 消息管理 ──
  editMessage?(params: EditMessageParams): Promise<EditResult>;
  deleteMessage?(chatId: string, messageId: string): Promise<DeleteResult>;

  // ── 查询 ──
  getChatInfo(chatId: string): Promise<ChatInfo>;
  listChats?(filter?: ChatFilter): Promise<ChatInfo[]>;

  // ── 配置与状态 ──
  getRuntimeConfig(): AdapterRuntimeConfig;
  statusSummary(): AdapterStatus;
}
```

### 3.3 数据类型定义

```typescript
// ── 能力 ──
interface Capability {
  name: 'Text' | 'Image' | 'Audio' | 'Video' | 'Document'
      | 'Interactive' | 'Streaming' | 'Voice' | 'Markdown' | 'Html'
      | 'CodeBlock' | 'Thread' | 'Topic' | 'Group' | 'ChatList'
      | 'MessageEdit' | 'MessageDelete' | 'TypingIndicator';
  supported: boolean;
  limits?: {
    maxTextLength?: number;
    maxFileSize?: number;
    maxButtons?: number;
  };
}

// ── 适配器状态 ──
type AdapterState = 'Created' | 'Starting' | 'Connecting' | 'Connected'
                  | 'Reconnecting' | 'Failed' | 'Stopped';

type HealthStatus = 'Healthy' | 'Degraded' | 'Down';

interface AdapterStatus {
  platform: string;
  displayName: string;
  state: AdapterState;
  connected: boolean;
  health?: HealthStatus;
  lastError?: string;
  uptime?: number;
  messagesIn: number;
  messagesOut: number;
  errors: number;
}

// ── 消息类型 ──
interface OutboundMessage {
  text: string;
  parseMode?: 'markdown' | 'html' | 'none';
}

interface InboundMessage {
  id: string;
  platform: string;
  text?: string;
  chatId: string;
  chatName?: string;
  chatType: 'Dm' | 'Group' | 'Channel' | 'Thread';
  sender: { id: string; name?: string; username?: string; isBot: boolean };
  mentions?: MentionInfo[];
  mentioned?: boolean;
  threadId?: string;
  media?: MediaAttachment[];
  command?: { name: string; args: string };
  callback?: { data: string; messageId: string };
  replyTo?: { messageId: string; text?: string };
  timestamp: number;
  metadata?: object;
}

// ── 媒体 ──
interface MediaAttachment {
  mediaType: 'Image' | 'Audio' | 'Video' | 'Document' | 'Sticker' | 'Animation';
  url?: string;
  data?: string;        // Base64（小型文件）
  mimeType: string;
  filename?: string;
  caption?: string;
  fileSize?: number;
}

// ── 发送参数 ──
interface SendTextParams {
  chatId: string;
  message: OutboundMessage;
  replyTo?: string;
  metadata?: Record<string, unknown>;
}

interface SendMediaParams {
  chatId: string;
  media: MediaAttachment;
  text?: string;
  replyTo?: string;
}

// ── 结果 ──
type SendResult = {
  success: true; messageId: string; timestamp: number;
} | {
  success: false; error: string; errorCode?: string; retryable: boolean;
};

type EditResult = {
  success: true; updatedAt: number;
} | {
  success: false; error: string;
};

// ── 配置 ──
interface AdapterConfig {
  enabled?: boolean;        // None=auto, true=force on, false=force off
  token?: string;
  apiKey?: string;
  baseUrl?: string;
  extra?: Record<string, unknown>;
}
```

---

## 第四章：核心逻辑层

### 4.1 模块分解

```
┌────────────────────────────────────────────────────────────┐
│                    Gateway Core                            │
│                                                            │
│  ┌───────────────┐  ┌──────────────┐  ┌──────────────────┐ │
│  │  Event Bus    │  │ Session Mgr  │  │  Router          │ │
│  │  publish /    │  │  create/save │  │  inbound routing │ │
│  │  subscribe    │  │  lookup/     │  │  outbound routing│ │
│  │  (tokio bcast)│  │  prune TTL   │  │                  │ │
│  └───────────────┘  └──────┬───────┘  └──────────────────┘ │
│                            │                               │
│  ┌───────────────┐  ┌──────┴───────┐  ┌──────────────────┐ │
│  │  Auth/ACL     │  │  Adapter     │  │  Config Manager  │ │
│  │  API Key      │  │  Manager     │  │  load/save/watch │ │
│  │  (Argon2)     │  │  lifecycle   │  │  hot-reload      │ │
│  │  Rate Limit   │  │  health poll │  │  (60s polling)   │ │
│  │               │  │  reconnect   │  │                  │ │
│  └───────────────┘  └──────────────┘  └──────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

### 4.2 事件总线

基于 tokio broadcast channel（容量 256），每个事件类型有独立通道：

```typescript
interface EventBus {
  publish(event: GatewayEvent): void;
  subscribe(eventType: string): Receiver<GatewayEvent>;
}

interface GatewayEvent {
  event: string;                    // "message.inbound" | "adapter.connected" | ...
  source: string;                   // 适配器名 / "api" / "core"
  timestamp: number;
  data: unknown;
  metadata?: {
    correlationId?: string;
    sessionKey?: string;
  };
}
```

**内部事件订阅关系：**

| 事件 | 订阅者 | 处理逻辑 |
|------|--------|----------|
| `message.inbound` | Session Manager | 创建/查找会话，更新活跃时间 |
| `message.inbound` | MessagePersister | 持久化消息到存储 |
| `message.inbound` | SessionBridge | 自动创建会话 + 异步富化来源信息 |
| `message.inbound` | WebhookDispatcher | POST 到配置的 webhook URL |
| `adapter.disconnected` | Adapter Manager | 触发重连逻辑 |
| `adapter.error` | Adapter Manager | 计数错误，达到阈值时告警 |

### 4.3 入站消息处理流程

```
1. 适配器收到平台消息
   │
2. 适配器构造 InboundMessage
   │
3. 适配器通过 EventBus 发布 message.inbound 事件
   │
4. 核心层处理：
   │
   ├─ Session Manager: 计算 sessionKey = platform:chatId[:threadId]
   │                    创建/查找 Session，更新 updatedAt
   │
   ├─ SessionBridge: 异步富化会话来源信息（用户名、角色等）
   │
   ├─ MessagePersister: 持久化到数据库
   │
   ├─ Router: 通过 WebSocket 推送到外部客户端
   │
   ├─ WebhookDispatcher: POST 到配置的 Webhook URL
   │
   └─ Statistics: 递增消息计数
```

### 4.4 出站消息处理流程

```
1. API Server 收到 POST /api/v1/messages/send
   │
2. 验证请求权限（Bearer token）
   │
3. 解析 target: "telegram:123456" → platform="telegram", chatId="123456"
   │
4. 查找目标适配器 → 不存在/未连接 → 返回 ADAPTER_NOT_CONNECTED
   │
5. 优先级: 存在 media → send_media()
           存在 keyboard → send_interactive()
           否则 → send()
   │
6. 持久化出站消息 → 发布 message.sent 事件
   │
7. 返回结果
```

### 4.5 会话管理器

```typescript
interface SessionManager {
  getOrCreate(key: string, source: SessionSource): Promise<Session>;
  get(key: string): Promise<Session | null>;
  update(key: string, mutation: SessionMutation): Promise<Session>;
  delete(key: string): Promise<void>;
  list(filter?: SessionFilter): Promise<Session[]>;
}

interface Session {
  key: string;              // "telegram:123456"
  platform: string;
  chatId: string;
  threadId?: string;
  source: SessionSource;
  createdAt: number;
  updatedAt: number;
  resetPolicy: ResetPolicy;
  metadata: Record<string, unknown>;
}
```

- 内存中使用 DashMap 存储
- 数据库持久化（SQLite / PostgreSQL）
- 按 TTL 定期清理（默认 session 365 天，message 90 天）
- 支持异步富化（adapter.enrich_source）

### 4.6 适配器管理器

```typescript
interface AdapterManager {
  register(name: string, factory: AdapterFactory, credentialEnvVars: string[]): void;
  getAdapter(platform: string): PlatformAdapter | null;
  listStatuses(): AdapterStatus[];
  startAll(configs: Map<string, AdapterConfig>): Promise<StartResult>;
  stopAll(): Promise<void>;
  startHealthMonitor(interval: Duration): void;
  start(platform: string, config: AdapterConfig): Promise<StartResult>;
  stop(platform: string): Promise<Result>;
  getStatus(platform: string): AdapterStatusSummary | null;
}
```

- 自动检测凭据环境变量启用适配器（`enabled: None` = auto）
- 健康监控每 30 秒检查所有适配器心跳
- 指数退避重连：5s → 10s → 30s → 60s → 120s → 300s（封顶 20 次）

---

## 第五章：插件体系

### 5.1 概述

EasyBot 支持通过动态库加载第三方适配器插件。插件使用 Rust 编写并编译为 cdylib，通过 `libloading` 在运行时动态加载。

每个插件提供两个 C ABI 入口函数：

```c
uint32_t easybot_abi_version();
void* easybot_plugin_create();
```

### 5.2 插件清单

```yaml
# plugin.yaml
name: "my-platform"
display_name: "My Platform"
description: "第三方 IM 平台适配器"
version: "1.0.0"
sdk_version: 1
author: "Your Name"
library: "libmy_adapter.so"
```

### 5.3 加载流程

```
EasyBot 启动
    │
    ▼
1. 扫描 plugins/ 目录
    │
    ▼
2. 每个子目录读取 plugin.yaml
    │
    ▼
3. 加载动态库（.so / .dylib / .dll）
    │
    ▼
4. 调用 easybot_abi_version() 检查兼容性
    │
    ▼
5. 调用 easybot_plugin_create() 创建适配器
    │
    ▼
6. 注册到 AdapterRegistry
    │
    ▼
7. 自动检测凭据，若存在则启动适配器
```

### 5.4 插件 SDK

`easybot-plugin-sdk` crate 为插件开发者提供：

- `PlatformAdapter` trait 完整导出
- `declare_plugin!()` 宏：一行声明入口函数
- 核心类型（`InboundMessage`、`SendResult`、`GatewayError` 等）

详见 `docs/02 plugin-dev.md`。

---

## 第六章：认证与安全

### 6.1 认证模型

```
外部客户端 → API Key (Bearer)  →  API Server

IM 平台 → 平台 Token（Bot Token / App Secret）→ 适配器

管理面 → 管理员密码 (gateway.yaml / EASYBOT_ADMIN_PASSWORD)
```

### 6.2 API 密钥管理

- 使用 Argon2 密码哈希存储
- 支持生成/验证/吊销
- `auth_middleware` 验证 Bearer token
- 内建权限校验：`AdaptersManage`、`MessagesSend`、`ConfigWrite` 等

### 6.3 传输安全

- 外部 API 端点建议使用 HTTPS/WSS（反向代理终止 TLS）
- 内部组件通信使用 HTTP（localhost 绑定）
- Token 敏感信息不在日志中输出全文（掩码模式）
- 速率限制：IP 滑动窗口（默认 60 req/min，突发 10）
- 请求体大小限制：10 MB
- WebSocket 帧大小限制：64 KB / 消息 256 KB
- Content-Security-Policy、X-Frame-Options 等安全头

---

## 第七章：配置与部署

### 7.1 配置文件结构

```yaml
# gateway.yaml（所有 key 使用 camelCase）

server:
  host: "127.0.0.1"
  port: 8080
  adminPassword: "${EASYBOT_ADMIN_PASSWORD}"
  tls:
    enabled: false
    certFile: ""
    keyFile: ""

api:
  basePath: "/api/v1"
  rawPayloadEnabled: false
  websocket:
    enabled: true
    maxClients: 1000
    heartbeatIntervalSecs: 30
  rateLimit:
    enabled: true
    requestsPerMinute: 60
    burstSize: 10
  metrics:
    enabled: true
    path: "/metrics"

storage:
  storageType: "sqlite"              # 也接受 "type"
  path: ""
  connectionString: ""
  poolSize: 10
  retention:
    messageTtlDays: 90
    sessionTtlDays: 365
    cleanupIntervalSecs: 3600

logging:
  level: "info"
  format: "text"
  output: "stdout"

adapters:
  telegram:
    token: "${TELEGRAM_BOT_TOKEN}"
  discord:
    token: "${DISCORD_BOT_TOKEN}"
  feishu:
    token: "${FEISHU_APP_ID}"        # App ID
    apiKey: "${FEISHU_APP_SECRET}"   # App Secret
  qq:
    token: "${QQ_APP_ID}"
    apiKey: "${QQ_CLIENT_SECRET}"

webhooks:
  - name: "my-service"
    url: "https://my-service.com/webhook"
    secret: "${WEBHOOK_SECRET}"
    events: ["message.inbound"]
    platforms: ["telegram"]
```

> **配置优先级**: gateway.yaml ← gateway.local.yaml（递归合并）← `${VAR_NAME}` 替换 ← `.env` 文件 ← 内建默认值

### 7.2 环境变量引用

配置中使用 `${VAR_NAME}` 语法引用环境变量，实现敏感信息与配置分离。

优先级：Shell `export` / Docker `environment:` > `.env` 文件

### 7.3 命令行接口

```bash
easybot                      # 启动（前台）
easybot --dir /etc/easybot   # 指定配置目录
easybot --init               # 初始化配置目录
easybot --config /etc/easybot/gateway.yaml  # 指定配置文件
easybot --debug              # 调试模式
easybot --version            # 查看版本
easybot --help               # 查看帮助
```

### 7.4 部署拓扑

```
Single-node (minimal):
┌──────────────────────────────────┐
│         Single Host              │
│  EasyBot                         │
│  ├── API Server :8080            │
│  ├── Telegram Adapter            │
│  ├── Discord Adapter             │
│  ├── Lark/QQ/WeChat Adapter      │
│  └── SQLite Storage              │
└──────────────────────────────────┘

Production (high-availability):
       ┌──────────────────────────┐
       │   Load Balancer (Nginx)  │
       │   :443 HTTPS / WSS       │
       └──────┬───────────────────┘
              │
      ┌───────┴───────────────┐
      │                       │
 ┌────┴─────┐          ┌──────┴───┐
 │EasyBot #1│          │EasyBot #2│
 └────┬─────┘          └─────┬────┘
      │                      │
      └──────────┬───────────┘
                 │
        ┌────────┴────────┐
        │   PostgreSQL    │
        └─────────────────┘
```

---

## 第八章：技术栈无关的实现要点

### 8.1 语言选择

| 考量点 | 推荐做法 |
|--------|----------|
| **异步 IO** | 网关 IO 密集型，必须选择异步运行时（Rust tokio、Python asyncio、Node.js、Go goroutine） |
| **插件加载** | Rust: libloading + cdylib / Python: importlib / Node: require() / Go: plugin |
| **WebSocket** | 推荐标准化 JSON 帧协议 |
| **存储** | 会话存储推荐 SQL（SQLite/PostgreSQL），消息体可存 JSON 列 |
| **容器化** | Docker 打包，配置通过环境变量注入 |

> **当前实现**: Rust（tokio + axum tower + sqlx）

### 8.2 实施路线图

```
Phase 1 (✅)     Phase 2 (✅)     Phase 3 (✅)     Phase 4 (✅ 95%)    Phase 5 (✅)
─────────        ─────────        ─────────        ─────────            ─────────
REST 单发        WebSocket         Discord           API Key/Argon2      Plugin SDK
Telegram         Webhook           飞书/QQ/微信       速率限制            动态加载
                                   5 平台            热重载
                                                    健康轮询+重连
                                                    HTTPS (⚠️暂缓)
                                                    Prometheus
                                                    Docker
                                                    交互按钮+流式
                                                    PostgreSQL
```

### 8.3 关键数字

| 指标 | 当前 |
|------|------|
| 支持平台数 | **5**（Telegram、Discord、飞书、QQ、微信） |
| 代码行数 | ~30,000+ |
| Rust 文件数 | ~200+ |
| 第三方依赖数 | ~30 |

---

## 附录 A：术语表

| 术语 | 英文 | 说明 |
|------|------|------|
| 网关 | Gateway | 本架构设计的主体服务 |
| 适配器 | Adapter | 连接特定 IM 平台的模块 |
| 事件总线 | Event Bus | 内部事件发布/订阅机制（tokio broadcast） |
| 会话 | Session | 以 `platform:chatId` 为键的对话上下文 |
| 出站 | Outbound | 从网关发往 IM 平台的方向 |
| 入站 | Inbound | 从 IM 平台发往网关的方向 |
| 目标 | Target | 消息投递目的地的描述串 |
| 能力 | Capability | 适配器支持的功能声明 |
| 插件 | Plugin | 独立的适配器/扩展模块 |

## 附录 B：参考来源

本架构设计基于以下开源项目的实践分析：

- **OpenClaw** — TypeScript 实现的 Agent 运行时，提供了严谨的 WebSocket 网关协议设计
- **Hermes-Agent** — Python 实现的通用 AI Agent 框架，提供了完善的多平台 IM 适配器体系
