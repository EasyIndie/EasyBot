# 即时通信网关架构设计说明书

> **摘要：** 一份语言无关的即时通信网关（EasyBot）架构设计。该网关作为独立服务运行，连接多种即时通信平台，对外暴露统一的 API 供第三方客户端调用，支持消息发送与接收的双向通信。设计遵循契约驱动、分层隔离、插件扩展的原则，可方便地翻译为任意编程语言的具体实现。
>
> **当前实现**: EasyBot 已用 Rust (tokio + axum stack) 实现，详见 `CLAUDE.md` 和源码。

---

## 第一章：总体架构

### 1.1 系统定位

EasyBot 是连接**IM 平台**与**业务系统**之间的独立中间层服务，承担以下职责：

- **南向（Southbound）**：接入 Telegram、Discord、飞书（Lark）、QQ、微信等即时通信平台
- **北向（Northbound）**：对外暴露 RESTful API + WebSocket，供第三方业务系统或客户端调用
- **双向转发**：将接收到的 IM 消息转发给业务系统，将业务系统产生的消息发送到 IM 平台

### 1.2 架构分层

```
                      ┌────────────────────────────────────┐
          ┌─────────  │        第三方客户端 / 业务系统       │  ──────────┐
          │           └────────────────────────────────────┘            │
          │                          ↕                                  │
          │               REST API / WebSocket                          │
          │                          ↕                                  │
          │           ┌────────────────────────────────┐                │
          │           │         EasyBot (独立服务)      │               │
          │           │                                  │  北向 API 层  │
          │           │  ┌──────────────────────────┐   │               │
          │           │  │    API Server (REST/WS)  │   │               │
          │           │  └──────────┬───────────────┘   │               │
          │           │             │                    │               │
          │           │  ┌──────────┴───────────────┐   │               │
          │           │  │     Gateway Core         │   │  核心逻辑层  │
          │           │  │  ┌───┐ ┌───┐ ┌───┐ ┌──┐ │   │               │
          │           │  │  │Msg│ │Ses│ │Rtr│ │Au│ │   │               │
          │           │  │  │Mgr│ │Mgr│ │   │ │th│ │   │               │
          │           │  │  └───┘ └───┘ └───┘ └──┘ │   │               │
          │           │  └──────────┬───────────────┘   │               │
          │           │             │                    │               │
          │           │  ┌──────────┴───────────────┐   │               │
          │           │  │  Adapter Manager          │   │  适配器层   │
          │           │  └──┬──┬──┬──┬──┬──┬──┬──┬──┘   │               │
          ▼           │     │  │  │  │  │  │  │  │      │               │
                      └─────┼──┼──┼──┼──┼──┼──┼──┼──────┘               │
                            │  │  │  │  │  │  │  │                        │
              南向适配器      ▼  ▼  ▼  ▼  ▼  ▼  ▼  ▼                       │
              ┌─────────────────────────────────────┐                     │
              │  Telegram | Discord | 飞书 | QQ | 微信 │  IM 平台          │
              └─────────────────────────────────────┘                     │
                                                                          │
              ┌─────────────────────────────────────┐                     │
              │      你的最终用户（在 IM 上聊天）      │                     │
              └─────────────────────────────────────┘                     │
```

### 1.3 核心概念

| 概念 | 说明 |
|------|------|
| **IM 平台（Platform）** | 即时通信服务商，如 Telegram、Discord、微信 |
| **平台适配器（Adapter）** | 连接特定 IM 平台的软件模块，处理协议差异 |
| **账号（Account）** | 一个 IM 平台上的机器人/应用账号（一个适配器管理一个账号） |
| **聊天（Chat）** | IM 平台上的一个对话（私聊/群聊/频道） |
| **消息（Message）** | 在 IM 聊天中传递的内容单元 |
| **会话（Session）** | 以 `platform:chatId` 标识的持久化对话上下文 |
| **外部客户端（Client）** | 调用网关 API 的第三方业务系统 |
| **目标（Target）** | 消息投递的目的地描述（platform + chatId） |

---

## 第二章：API 接口设计

### 2.1 总览

外部客户端通过 **REST API** 进行命令式交互，通过 **WebSocket** 接收实时推送。

**基础 URL 模式：** `http(s)://<gateway-host>:<port>/api/v1`

### 2.2 REST API

完整路由列表（实际实现）：

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/health` | 健康检查 |
| GET | `/adapters` | 适配器状态列表 |
| POST | `/adapters/{platform}/start` | 启动适配器 |
| POST | `/adapters/{platform}/stop` | 停止适配器 |
| GET | `/adapters/{platform}/status` | 适配器运行状态 |
| POST | `/messages/send` | 发送消息 |
| POST | `/messages/batch-send` | 批量发送 |
| PUT | `/messages/{message_id}` | 编辑消息 |
| DELETE | `/messages/{message_id}` | 删除消息 |
| GET | `/messages` | 消息历史 |
| GET | `/sessions` | 会话列表 |
| GET | `/sessions/{key}` | 会话详情 |
| DELETE | `/sessions/{key}` | 删除会话 |
| GET | `/chats/{platform}` | 聊天列表 |
| GET | `/chats/{platform}/{chat_id}` | 聊天信息 |
| GET | `/config` | 获取配置 |
| PUT | `/config` | 更新配置（热重载） |
| GET | `/ws` | WebSocket 实时事件流 |
| GET | `/metrics` | Prometheus 指标 |
| GET | `/swagger` | Swagger UI |
| GET | `/openapi.json` | OpenAPI 3.1 规范 |

#### 2.2.1 消息发送

```
POST /api/v1/messages/send
Content-Type: application/json

{
  "target": "telegram:123456789",     // "platform:chatId" 格式
  "text": "Hello from API!",
  "parseMode": "markdown",            // 可选: markdown / html / none
  "media": [                          // 可选: 媒体附件
    {
      "type": "image",
      "url": "https://example.com/photo.jpg",
      "caption": "照片说明"
    }
  ],
  "replyTo": "98765",                 // 可选: 被回复消息 ID
  "metadata": {                       // 可选: 平台特有参数
    "disable_web_page_preview": true
  }
}

Response 200:
{
  "id": "msg_abc123",
  "status": "sent",
  "platform": "telegram",
  "chatId": "123456789",
  "messageId": "msg_telegram_98765",
  "timestamp": 1718000000000
}
```

```
POST /api/v1/messages/batch-send
Content-Type: application/json

{
  "targets": [
    "telegram:123456",
    "discord:789012",
    "qq:987654"
  ],
  "text": "群发公告：系统维护通知...",
  "parseMode": "markdown"
}

Response 200:
{
  "total": 3,
  "succeeded": 2,
  "failed": 1,
  "results": {
    "telegram:123456": { "id": "msg_1", "status": "sent", "messageId": "..." },
    "discord:789012": { "id": "msg_2", "status": "sent", "messageId": "..." },
    "qq:987654": { "id": "msg_3", "status": "failed", "error": "chat not found" }
  }
}
```

#### 2.2.2 消息编辑与删除

```
PUT /api/v1/messages/{message_id}
{
  "text": "修改后的消息",
  "parseMode": "markdown"
}

Response 200: { "id": "...", "status": "edited", "updatedAt": 1718000100000 }

DELETE /api/v1/messages/{message_id}

Response 200: { "id": "...", "status": "deleted" }
```

> 编辑/删除能力因平台而异（微信 ❌ 平台不支持，QQ 仅频道消息支持）。

#### 2.2.3 适配器管理

```
GET /api/v1/adapters
Response 200:
{
  "adapters": [
    {
      "platform": "telegram",
      "displayName": "Telegram",
      "status": "connected"
    },
    {
      "platform": "discord",
      "displayName": "Discord",
      "status": "disconnected",
      "error": "token not configured"
    }
  ]
}

POST /api/v1/adapters/{platform}/start
Response 200: { "platform": "telegram", "status": "starting" }

POST /api/v1/adapters/{platform}/stop
Response 200: { "platform": "telegram", "status": "stopped" }

GET /api/v1/adapters/{platform}/status
Response 200: {
  "platform": "telegram",
  "connected": true,
  "uptime": 3600,
  "messagesIn": 150,
  "messagesOut": 200,
  "errors": 2,
  "lastError": "rate limited, retrying in 30s"
}
```

#### 2.2.4 聊天管理

```
GET /api/v1/chats/telegram
Response 200:
{
  "chats": [
    { "id": "123456", "name": "Alice", "type": "dm" },
    { "id": "-789012", "name": "Team Chat", "type": "group", "memberCount": 15 }
  ]
}

GET /api/v1/chats/telegram/123456
Response 200:
{
  "id": "telegram:123456",
  "platform": "telegram",
  "chatId": "123456",
  "name": "Alice",
  "type": "dm",
  "available": true
}
```

> 注意：聊天路由使用路径参数 `/chats/{platform}`，而非查询参数。`list_chats` 能力因平台而异（Discord/QQ 支持，Telegram/飞书因 API 限制返回空列表，微信不支持）。

#### 2.2.5 消息历史查询

```
GET /api/v1/messages?sessionKey=telegram:123456&limit=50&before=1717000000000
Response 200:
{
  "messages": [
    {
      "id": "...",
      "direction": "inbound",          // inbound | outbound
      "platform": "telegram",
      "chatId": "123456",
      "text": "你好",
      "author": { "id": "user_1", "name": "Alice" },
      "timestamp": 1718000000000
    },
    {
      "id": "...",
      "direction": "outbound",
      "platform": "telegram",
      "chatId": "123456",
      "text": "你好！有什么可以帮助你的？",
      "timestamp": 1718000001000
    }
  ],
  "hasMore": false
}
```

#### 2.2.6 网关健康检查

```
GET /api/v1/health
Response 200:
{
  "status": "healthy",                // healthy | degraded | down
  "uptime": 86400,
  "version": "0.0.5",
  "adapters": {
    "total": 5,
    "connected": 4
  }
}
```

#### 2.2.7 配置管理

```
GET /api/v1/config
Response 200: { /* 网关当前配置 */ }

PUT /api/v1/config
Body: { /* 更新配置 */ }
Response 200: { "ok": true, "requiresRestart": true }
```

### 2.3 WebSocket 实时推送

客户端通过 WebSocket 连接接收实时事件推送。

**连接地址：** `ws(s)://<gateway-host>:<port>/api/v1/ws`

**连接认证（双层）：**
- HTTP 升级时需 `Authorization: Bearer <key>` 头
- 连接后可发送 `{"token":"..."}` 二次认证

**服务器推送事件格式：**
```json
{
  "type": "event",
  "event": "message.inbound",
  "data": {
    "id": "msg_789",
    "platform": "telegram",
    "chatId": "123456",
    "chatName": "Alice",
    "author": { "id": "user_1", "name": "Alice" },
    "text": "你好",
    "timestamp": 1718000000000
  },
  "seq": 42
}
```

**标准事件列表：**

| 事件名 | 触发时机 | 数据 |
|--------|----------|------|
| `message.inbound` | 收到 IM 平台消息 | InboundMessage |
| `message.sent` | 消息发送成功 | { id, platform, chatId, messageId } |
| `message.failed` | 消息发送失败 | { id, platform, chatId, error } |
| `message.edited` | 消息被编辑 | InboundMessage |
| `message.deleted` | 消息被删除 | { messageId, chatId } |
| `adapter.connected` | 适配器连接成功 | { platform } |
| `adapter.disconnected` | 适配器断开 | { platform, reason } |
| `adapter.error` | 适配器异常 | { platform, error } |
| `callback.received` | 收到按钮回调 | { id, platform, chatId, data, messageId } |
| `gateway.started` | 网关启动 | { timestamp, version } |
| `gateway.stopping` | 网关关闭 | { reason } |

### 2.4 API 公共约定

**所有响应中错误时的统一格式：**
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
| `MESSAGE_TOO_LONG` | 400 | 消息超过平台长度限制 |
| `RATE_LIMITED` | 429 | 平台限流 |
| `CAPABILITY_NOT_SUPPORTED` | 400 | 平台不支持该能力 |
| `INTERNAL_ERROR` | 500 | 内部错误 |

---

## 第三章：南向适配器接口

### 3.1 适配器生命周期

```
┌──────────────────────────────────────────────────────────────┐
│                     Adapter Lifecycle                        │
│                                                              │
│  ┌─────────┐    ┌──────────┐    ┌───────────┐  ┌──────────┐ │
│  │ CREATED │───→│ STARTING │───→│CONNECTING │─→│CONNECTED │ │
│  └─────────┘    └──────────┘    └───────────┘  └────┬─────┘ │
│       │                                              │       │
│       │                    (disconnect /              │       │
│       │                     rate limited /            │       │
│       │                     network error)            │       │
│       │                                              ▼       │
│       │                                       ┌────────────┐ │
│       │                                       │RECONNECTING│ │
│       │                                       └──────┬─────┘ │
│       │                                              │       │
│       │                              (max retries    │       │
│       │                               exceeded)      │       │
│       │                                              ▼       │
│       │                                       ┌────────────┐ │
│       └──────────────────────────────────────→│   FAILED   │ │
│                                                └────────────┘ │
│                                                              │
│  任何时候调用 stop() → STOPPED                               │
└──────────────────────────────────────────────────────────────┘
```

### 3.2 适配器接口定义（IDL）

以下为语言无关的接口定义，用类似 TypeScript 语法的 IDL 书写。

```typescript
/**
 * 平台适配器接口
 * 
 * 每个 IM 平台的连接器必须实现此接口。
 * 适配器由 Adapter Plugin 工厂方法创建。
 */
interface PlatformAdapter {
  // ── 元数据 ───────────────────────────────────────────
  
  /** 平台唯一标识，如 "telegram"、"discord" */
  readonly platformName: string;
  
  /** 人类可读的平台显示名 */
  readonly displayName: string;
  
  /** 本适配器支持的能力列表 */
  readonly capabilities: Capability[];
  
  // ── 生命周期 ─────────────────────────────────────────
  
  /**
   * 初始化适配器，但不连接。
   * 在此阶段加载配置、检查依赖、但不建立网络连接。
   */
  init(config: AdapterConfig): Promise<InitResult>;
  
  /**
   * 连接到 IM 平台并开始接收消息。
   * 对于长连接平台（如 Telegram polling）：建立连接并注册事件监听器。
   * 对于 Webhook 平台（如飞书）：注册 Webhook 端点。
   */
  connect(): Promise<ConnectResult>;
  
  /**
   * 断开与 IM 平台的连接。
   * 清理资源、关闭网络连接、取消定时任务。
   */
  disconnect(): Promise<void>;
  
  /** 当前连接状态 */
  state(): AdapterState;
  
  /**
   * 健康检查。
   * 返回当前适配器的健康度量，核心层根据返回值决策是否触发重连。
   */
  health(): Promise<HealthReport>;
  
  // ── 消息发送 ─────────────────────────────────────────
  
  /**
   * 发送文本消息到指定聊天。
   * 这是最基本的发送方法，所有适配器都必须实现。
   */
  send(params: SendTextParams): Promise<SendResult>;
  
  /**
   * 发送媒体消息（图片/音频/视频/文档）。
   * 平台不支持时返回 CapabilityError。
   */
  sendMedia?(params: SendMediaParams): Promise<SendResult>;
  
  /**
   * 发送交互式消息（带按钮的内联键盘）。
   * 平台不支持时返回 CapabilityError。
   */
  sendInteractive?(params: SendInteractiveParams): Promise<SendResult>;
  
  /**
   * 发送输入状态指示器（"正在输入..."）。
   */
  sendTyping?(chatId: string): Promise<void>;
  
  // ── 消息管理 ─────────────────────────────────────────
  
  /**
   * 编辑已发送的消息内容。
   * 平台不支持时返回 CapabilityError。
   */
  editMessage?(params: EditMessageParams): Promise<EditResult>;
  
  /**
   * 删除已发送的消息。
   * 平台不支持时返回 CapabilityError。
   */
  deleteMessage?(chatId: string, messageId: string): Promise<DeleteResult>;
  
  // ── 流式发送 ─────────────────────────────────────────
  
  /**
   * 发送流式更新草稿（如 AI 回复的逐步生成效果）。
   * 同一个 draftId 的多次调用会更新同一草稿。
   * 平台不支持时返回 CapabilityError。
   */
  sendDraft?(chatId: string, draftId: string, content: string,
             metadata?: Record<string, unknown>): Promise<DraftResult>;
  
  // ── 查询 ─────────────────────────────────────────────
  
  /**
   * 获取聊天室的基本信息。
   */
  getChatInfo(chatId: string): Promise<ChatInfo>;
  
  /**
   * 列出可用的聊天列表（可选，部分平台不支持枚举）。
   */
  listChats?(filter?: ChatFilter): Promise<ChatInfo[]>;
  
  // ── 配置 ─────────────────────────────────────────────

  /**
   * 返回适配器的运行时配置状态。
   */
  getRuntimeConfig(): AdapterRuntimeConfig;

  /**
   * 返回适配器状态摘要（用于快速查询）。
   */
  statusSummary(): AdapterStatus;
}
```

### 3.3 数据类型定义

```typescript
// ── 能力 ──
interface Capability {
  name: 'text' | 'image' | 'audio' | 'video' | 'document'
      | 'interactive' | 'streaming' | 'voice' | 'markdown' | 'html'
      | 'code_block' | 'thread' | 'topic' | 'group' | 'chat_list'
      | 'message_edit' | 'message_delete' | 'typing_indicator';
  supported: boolean;
  limits?: {
    maxTextLength?: number;
    maxFileSize?: number;
    maxButtons?: number;
    maxInlineButtons?: number;
  };
}

// ── 消息类型 ──
interface OutboundMessage {
  text: string;
  parseMode?: 'markdown' | 'html' | 'none';     // 默认自动检测
}

interface InboundMessage {
  id: string;                                    // 平台消息 ID
  platform: string;                              // 平台标识
  chatId: string;                                // 来源聊天 ID
  chatName?: string;                             // 聊天名称
  chatType: 'dm' | 'group' | 'channel' | 'thread';
  text?: string;                                 // 文本内容
  author: {
    id: string;
    name?: string;
    isBot: boolean;
  };
  timestamp: number;                             // 毫秒时间戳
  media?: MediaAttachment[];
  command?: { name: string; args: string };      // 斜杠命令
  callback?: { data: string; messageId: string };// 按钮回调
  replyTo?: { messageId: string; text?: string };// 回复引用
  threadId?: string;
  metadata?: Record<string, unknown>;            // 平台特有元数据
}

// ── 媒体 ──
interface MediaAttachment {
  type: 'image' | 'audio' | 'video' | 'document' | 'sticker' | 'animation';
  url?: string;                                  // 远程地址
  data?: string;                                 // Base64 数据（小型文件）
  mimeType: string;
  filename?: string;
  caption?: string;
  thumbnailUrl?: string;
  fileSize?: number;
  duration?: number;                             // 音频/视频时长（秒）
}

// ── 交互式按键 ──
interface InlineKeyboard {
  rows: { buttons: Button[] }[];
}
interface Button {
  text: string;
  callbackData?: string;                         // 回调负载
  url?: string;                                  // 超链接
  style?: 'default' | 'primary' | 'danger';
}

// ── 发送参数 ──
interface SendTextParams {
  chatId: string;
  message: OutboundMessage;
  replyTo?: string;                              // 回复消息 ID
  metadata?: Record<string, unknown>;
}

interface SendMediaParams {
  chatId: string;
  media: MediaAttachment;
  text?: string;
  replyTo?: string;
}

interface SendInteractiveParams {
  chatId: string;
  text: string;
  keyboard: InlineKeyboard;
  replyTo?: string;
}

interface EditMessageParams {
  chatId: string;
  messageId: string;
  message: OutboundMessage;
  keyboard?: InlineKeyboard;                     // 更新按钮
}

// ── 结果 ──
interface SendResult {
  success: true;
  messageId: string;
  timestamp: number;
} | {
  success: false;
  error: string;
  errorCode?: string;                            // 标准错误码
  retryable: boolean;
}

interface EditResult {
  success: true;
  updatedAt: number;
} | {
  success: false;
  error: string;
  errorCode?: string;
}

interface DeleteResult {
  success: true;
} | {
  success: false;
  error: string;
}

// ── 适配器状态 ──
type AdapterState = 'created' | 'starting' | 'connecting' | 'connected'
                  | 'reconnecting' | 'failed' | 'stopped';

interface AdapterStatus {
  platform: string;
  displayName: string;
  state: AdapterState;
  connected: boolean;
  health?: 'healthy' | 'degraded' | 'down';
  lastError?: string;
  uptime?: number;
}

// ── 生命周期 ──
interface InitResult {
  ok: boolean;
  error?: string;
  configErrors?: string[];                       // 配置校验错误
}

interface ConnectResult {
  ok: boolean;
  error?: string;
  botInfo?: {
    name: string;
    username?: string;
    id: string;
  };
}

interface HealthReport {
  status: 'healthy' | 'degraded' | 'down';
  connected: boolean;
  lastConnectedAt?: number;
  lastErrorAt?: number;
  lastError?: string;
  messagesIn: number;
  messagesOut: number;
  errors: number;
  uptime?: number;
  details?: Record<string, unknown>;
}

// ── 聊天信息 ──
interface ChatInfo {
  chatId: string;
  name: string;
  type: 'dm' | 'group' | 'channel' | 'thread';
  memberCount?: number;
  isBotAdmin?: boolean;
  icon?: string;
}

interface ChatFilter {
  type?: 'dm' | 'group' | 'channel';
  query?: string;                                // 搜索关键字
}

// ── 配置 ──
interface AdapterConfig {
  token?: string;                                // Bot Token
  apiKey?: string;                               // API Key
  appSecret?: string;                            // App Secret
  webhookUrl?: string;
  proxyUrl?: string;
  extra: Record<string, unknown>;                // 平台特有配置
}

interface AdapterRuntimeConfig {
  enabled: boolean;
  tokenConfigured: boolean;
  extra: Record<string, unknown>;
}
```

---

## 第四章：核心逻辑层

### 4.1 模块分解

核心逻辑层是网关的"大脑"，由以下模块构成：

```
┌────────────────────────────────────────────────────────────┐
│                    Gateway Core                            │
│                                                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐ │
│  │  Event Bus    │  │ Session Mgr  │  │  Router          │ │
│  │              │  │              │  │                  │ │
│  │  Publish /   │  │  create/save │  │  inbound routing │ │
│  │  Subscribe   │  │  lookup/     │  │  outbound routing│ │
│  │              │  │  resetPolicy │  │  multi-target     │ │
│  └──────────────┘  └──────┬───────┘  └──────────────────┘ │
│                            │                                │
│  ┌──────────────┐  ┌──────┴───────┐  ┌──────────────────┐ │
│  │  Auth/ACL     │  │  Adapter     │  │  Config Manager  │ │
│  │              │  │  Manager     │  │                  │ │
│  │  API Key     │  │  lifecycle   │  │  load/save/watch │ │
│  │  Rate Limit  │  │  health poll │  │  hot-reload      │ │
│  │              │  │  reconnect   │  │  (60s 轮询)       │ │
│  └──────────────┘  └──────────────┘  └──────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

### 4.2 事件总线

事件总线使用 tokio broadcast channel 实现一对多分发。

```typescript
interface EventBus {
  /**
   * 发布事件到总线
   */
  publish(event: GatewayEvent): void;
  
  /**
   * 订阅特定事件
   */
  subscribe(eventType: string): Receiver<GatewayEvent>;
}

interface GatewayEvent {
  type: string;                     // "message.inbound" | "adapter.connected" | ...
  source: string;                   // 事件源（适配器名 / "api" / "core"）
  timestamp: number;
  data: unknown;
  metadata?: {
    correlationId?: string;         // 关联 ID，用于追踪消息链路
    sessionKey?: string;
  };
}
```

**内部事件订阅关系：**

| 事件 | 订阅者 | 处理逻辑 |
|------|--------|----------|
| `message.inbound` | Session Manager | 创建/查找会话，更新活跃时间 |
| `message.inbound` | Router | 路由到外部客户端的 WebSocket |
| `message.inbound` | Webhook | POST 到配置的 webhook URL |
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
4. 核心层收到 InboundMessage：
   │
   ├─ 4a. 发布 message.inbound 事件到 EventBus
   │
   ├─ 4b. Session Manager:
   │      ├─ 计算 sessionKey = platform:chatId[:threadId]
   │      ├─ 查找或创建 Session
   │      ├─ 检查 resetPolicy（是否需要重置上下文）
   │      └─ 更新会话的 updatedAt
   │
   ├─ 4c. Router:
   │      ├─ 检查是否有绑定的外部 WebSocket 客户端
   │      ├─ 如果有：通过 WebSocket 推送消息到客户端
   │      ├─ 检查是否有配置的 Webhook URL
   │      └─ 如果有：POST 消息到 Webhook URL
   │
   ├─ 4d. 如果消息是斜杠命令:
   │      └─ 解析命令
   │
   └─ 4e. Statistics:
          └─ 递增计数
```

### 4.4 出站消息处理流程

```
1. API Server 收到 POST /api/v1/messages/send
   或 WebSocket 客户端发送 { type: "send", ... }
   │
2. 验证请求权限
   │
3. 解析 target：
   │  "telegram:123456" → platform="telegram", chatId="123456"
   │
4. 查找目标适配器：
   ├─ 适配器存在且已连接 → 继续
   └─ 适配器不存在/未连接 → 返回错误 ADAPTER_NOT_CONNECTED
   │
5. 构造 OutboundMessage，进行平台适配：
   ├─ 文本裁剪（控制长度）
   ├─ 媒体 URL 验证
   └─ 格式转换（按目标平台能力）
   │
6. 调用 adapter.send(params)
   │
7. 返回 SendResult
   │
8. 如果成功，发布 message.sent 事件
   如果失败，发布 message.failed 事件
```

### 4.5 会话管理器

```typescript
interface SessionManager {
  /**
   * 获取或创建会话
   * sessionKey = platform:chatId[:threadId]
   */
  getOrCreate(key: string, source: SessionSource): Promise<Session>;
  
  /**
   * 获取会话（不存在返回 null）
   */
  get(key: string): Promise<Session | null>;
  
  /**
   * 更新会话
   */
  update(key: string, mutation: SessionMutation): Promise<Session>;
  
  /**
   * 删除会话
   */
  delete(key: string): Promise<void>;
  
  /**
   * 列表查询
   */
  list(filter?: SessionFilter): Promise<Session[]>;
}

interface Session {
  key: string;                      // "telegram:123456"
  platform: string;
  chatId: string;
  threadId?: string;
  source: SessionSource;
  createdAt: number;
  updatedAt: number;
  resetPolicy: ResetPolicy;
  metadata: Record<string, unknown>;
}

interface SessionSource {
  platform: string;
  chatId: string;
  chatName?: string;
  chatType: 'dm' | 'group' | 'channel' | 'thread';
  userId?: string;
  userName?: string;
  isBot?: boolean;
}

type ResetPolicy = 'never' | 'after_1h' | 'after_24h' | 'after_50_msgs' | 'daily' | 'manual';
```

### 4.6 适配器管理器

```typescript
interface AdapterManager {
  /**
   * 注册适配器工厂
   */
  register(name: string, factory: AdapterFactory): void;
  
  /**
   * 获取适配器实例
   */
  getAdapter(platform: string): PlatformAdapter | null;
  
  /**
   * 列出所有适配器状态
   */
  listStatuses(): AdapterStatus[];
  
  /**
   * 启动所有已注册的适配器（自动检测凭据）
   */
  startAll(): Promise<{ succeeded: string[]; failed: { platform: string; error: string }[] }>;
  
  /**
   * 停止所有适配器
   */
  stopAll(): Promise<void>;
  
  /**
   * 启动健康轮询 + 自动重连
   */
  startHealthMonitor(): void;
}

interface AdapterStatus {
  platform: string;
  displayName: string;
  state: 'created' | 'starting' | 'connecting' | 'connected' | 'reconnecting' | 'failed' | 'stopped';
  connected: boolean;
  health?: 'healthy' | 'degraded' | 'down';
  lastError?: string;
  uptime?: number;
}
```

---

## 第五章：插件体系

### 5.1 概述

EasyBot 支持通过动态库加载第三方适配器插件。插件使用 Rust 编写并编译为 cdylib，通过 `libloading` 在运行时动态加载。

每个插件必须提供以下两个 C ABI 入口函数：

```c
// 返回 ABI 版本，用于兼容性检查
uint32_t easybot_abi_version();

// 创建适配器实例（由加载器传入 platform_name 和配置）
void* easybot_plugin_create();
```

### 5.2 插件清单文件

每个插件目录包含一个 `plugin.yaml` 清单文件：

```yaml
# plugin.yaml
name: my-custom-adapter
version: "1.0.0"
platform_name: my_platform
display_name: "My Platform"
description: "第三方 IM 平台适配器"

# 插件自动启用的凭据环境变量名
credential_env_vars:
  - MY_PLATFORM_TOKEN

# ABI 兼容性
easybot_abi_version: 1
```

### 5.3 插件加载流程

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
3. 加载 .so/.dylib/.dll 动态库
    │
    ▼
4. 调用 easybot_abi_version() 检查兼容性
    │
    ▼
5. 调用 easybot_plugin_create() 创建适配器
    │
    ▼
6. 注册到 AdapterManager
    │
    ▼
7. 自动检测凭据，若存在则启动适配器
```

### 5.4 插件 SDK

`easybot-plugin-sdk` crate 为第三方插件开发者提供：

- `PlatformAdapter` trait 的完整导出
- `declare_plugin!()` 宏：一行声明插件入口函数
- 核心类型（`InboundMessage`, `SendResult`, `GatewayError` 等）的完整导出

详见 `docs/PLUGIN_DEV.md`。

---

## 第六章：认证与安全

### 6.1 认证模型

```
┌─────────────────────────────────────────────────────────┐
│                    认证层次                              │
│                                                         │
│  外部客户端 → API Key (Bearer)  →  API Server          │
│                                                         │
│  IM 平台 → 平台 Token（Bot Token / App Secret）→ 适配器  │
│                                                         │
│  管理面 → 管理员密码 (gateway.yaml) →  管理后台          │
└─────────────────────────────────────────────────────────┘
```

### 6.2 API 密钥管理

API Key 使用 Argon2 密码哈希存储，支持生成/验证/吊销。所有受保护路由通过 auth_middleware 验证 Bearer token，并注入请求上下文。

```typescript
interface ApiKeyManager {
  /**
   * 创建新的 API Key
   */
  createKey(params: {
    name: string;
    expiresAt?: number;
  }): Promise<{ key: string; id: string }>;
  
  /**
   * 验证 API Key，返回认证信息
   */
  authenticate(authorizationHeader: string): Promise<AuthInfo | null>;
  
  /**
   * 吊销 API Key
   */
  revokeKey(keyId: string): Promise<void>;
  
  /**
   * 列出所有 API Key
   */
  listKeys(): Promise<ApiKeyInfo[]>;
}

interface AuthInfo {
  id: string;
  name: string;
  permissions: string[];
  expiresAt?: number;
}
```

### 6.3 传输安全

```
- 所有外部 API 端点建议使用 HTTPS / WSS
- 内部组件间通信可使用 HTTP（localhost 绑定）
- Token 敏感信息不在日志中输出全文（掩码模式）
- 支持速率限制（token bucket）防止滥用
- 支持 HTTP 请求体大小限制
- WebSocket 帧大小限制
```

> RBAC 权限模型（`message:send`、`adapter:control` 等细粒度权限检查）在单用户部署场景中暂缓实现。

---

## 第七章：配置与部署

### 7.1 配置文件结构

```yaml
# gateway.yaml

# 服务配置
server:
  host: "0.0.0.0"
  port: 8080

# 外部 API 配置
api:
  basePath: "/api/v1"
  rateLimit: 100                     # 每秒请求数
  adminPassword: "${ADMIN_PASSWORD}" # 管理后台密码

# 数据存储
storage:
  type: "sqlite"                     # sqlite | postgres
  path: "./data/gateway.db"          # sqlite 路径
  # postgres 时:
  # connectionString: "postgres://user:pass@localhost:5432/gateway"

# 日志
logging:
  level: "info"
  format: "json"                     # json | text
  output: "stdout"                   # stdout | file:./logs/gateway.log

# 适配器配置（通过凭据环境变量自动启用）
adapters:
  telegram:
    token: "${TELEGRAM_BOT_TOKEN}"
  discord:
    token: "${DISCORD_BOT_TOKEN}"
  feishu:
    appId: "${FEISHU_APP_ID}"
    appSecret: "${FEISHU_APP_SECRET}"
  qq:
    botAppId: "${QQ_BOT_APP_ID}"
    botToken: "${QQ_BOT_TOKEN}"
  wechat:
    apiUrl: "${WECHAT_API_URL}"

# 外部客户端 Webhook
webhooks:
  - name: "my-service"
    url: "https://my-service.com/im-gateway-webhook"
    secret: "${WEBHOOK_SECRET}"       # 用于 HMAC 签名验证
    events: ["message.inbound"]
    platforms: ["telegram"]
    retry:
      maxRetries: 3
      backoffMs: 1000
```

> **配置优先级**: `gateway.yaml` ← `gateway.local.yaml`（递归覆盖）← `${VAR_NAME}` 环境变量替换。适配器凭据检测通过环境变量自动启用（无需手动设置 `enabled: true`）。

### 7.2 环境变量引用

配置文件中使用 `${VAR_NAME}` 语法引用环境变量，实现敏感信息与配置分离。环境变量来源优先级：`export` / Docker `environment:` > `.env` 文件。

### 7.3 命令行接口

```bash
# 启动服务（前台）
easybot

# 指定配置目录启动
easybot --dir /etc/easybot --debug

# 初始化配置目录
easybot --init

# 指定配置文件
easybot --config /etc/easybot/gateway.yaml

# 查看版本
easybot --version

# 服务管理（Linux systemd / macOS launchd）
easybot service install
easybot service uninstall
easybot service status
easybot service start
easybot service stop
```

### 7.4 部署拓扑示例

```
最小化部署（单机）:
┌──────────────────────────────────┐
│         单一主机                  │
│                                  │
│  EasyBot（进程）              │
│  ├── API Server :8080            │
│  ├── Telegram Adapter            │
│  ├── Discord Adapter             │
│  ├── 飞书/QQ/WeChat Adapter      │
│  └── SQLite 存储                 │
└──────────────────────────────────┘
        ↕                    ↕
  Telegram Bot API    Discord Bot API

生产部署（高可用）:
         ┌──────────────────────┐
         │  负载均衡器 (Nginx)   │
         │  :443 HTTPS / :443 WSS│
         └──────┬───────────────┘
                │
        ┌───────┴───────────────┐
        │                       │
  ┌─────┴─────┐          ┌─────┴─────┐
  │ EasyBot #1│          │ EasyBot #2│
  │ :8080     │          │ :8080     │
  └─────┬─────┘          └─────┬─────┘
        │                      │
        └──────────┬───────────┘
                   │
          ┌────────┴────────┐
          │   PostgreSQL    │
          │   (会话存储)     │
          └─────────────────┘
```

---

## 第八章：技术栈无关的实现要点

### 8.1 选择编程语言时的考量

| 考量点 | 推荐做法 |
|--------|----------|
| **异步 IO** | 网关的核心是 IO 密集型，必须选择或构建异步运行时（如 Python asyncio、Node.js、Go goroutine、Java Netty） |
| **插件加载** | Rust：libloading + cdylib / Python：动态 import / Node.js：require() / Go：plugin 或 WASM |
| **WebSocket** | 各语言主流 WebSocket 库均可，推荐使用标准化的 JSON 帧协议 |
| **存储** | 会话存储建议 SQL（SQLite/PostgreSQL），消息体可存 JSON 列 |
| **容器化** | 建议 Docker 打包，配置文件通过环境变量注入 |

> **当前实现**: EasyBot 使用 Rust (tokio + axum tower + sqlx) 实现。详见 `CLAUDE.md` 和源码目录。

### 8.2 实施路线图（当前状态）

```
                 Phase 1              Phase 2              Phase 3              Phase 4               Phase 5
                ─────────            ─────────            ─────────            ─────────              ─────────
                 ✅ 完成              ✅ 完成              ✅ 100%               ✅ 95%                 ✅ 完成

REST 单发        ██
Telegram         ██        ██
WebSocket                    ██
Webhook                      ██
Discord                                ██
飞书/QQ/微信                            ██
5 平台                                   ██
API Key / Argon2                                      ██  (RBAC ⚠️暂缓)
速率限制                                                 ██
热重载                                                    ██
健康轮询 + 自动重连                                        ██
HTTPS/WSS                                                  ██ (⚠️暂缓)
Prometheus                                                  ██
Docker                                                       ██
交互式按钮 + 流式                                              ██
PostgreSQL                                                     ██
插件 SDK                                                                   ██
动态加载                                                                     ██
```

### 8.3 关键数字

| 指标 | 当前 |
|------|------|
| 支持平台数 | **5** (Telegram, Discord, 飞书, QQ, 微信) |
| 代码行数 | ~30,000+ |
| Rust 文件数 | ~200+ |
| 第三方依赖数 | ~30 |

---

## 附录 A：术语表

| 术语 | 英文 | 说明 |
|------|------|------|
| 网关 | Gateway | 本架构设计的主体服务 |
| 适配器 | Adapter | 连接特定 IM 平台的模块 |
| 事件总线 | Event Bus | 内部事件发布/订阅机制 |
| 会话 | Session | 以 chatId 为键的对话上下文 |
| 出站 | Outbound | 从网关发往 IM 平台的方向 |
| 入站 | Inbound | 从 IM 平台发往网关的方向 |
| 目标 | Target | 消息投递目的地的描述串 |
| 能力 | Capability | 适配器支持的功能声明 |
| 插件 | Plugin | 独立的适配器/扩展模块 |

## 附录 B：参考来源

本架构设计基于以下开源项目的实践分析：

- **OpenClaw** — TypeScript 实现的 Agent 运行时，提供了严谨的 WebSocket 网关协议设计（Gateway Protocol）和设备认证机制
- **Hermes-Agent** — Python 实现的通用 AI Agent 框架，提供了完善的多平台 IM 适配器体系（BasePlatformAdapter + 插件系统），支持 15+ 即时通信平台
