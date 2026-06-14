# 即时通信网关架构设计说明书

> **摘要：** 一份语言无关的即时通信网关（EasyBot）架构设计。该网关作为独立服务运行，连接多种即时通信平台，对外暴露统一的 API 供第三方客户端调用，支持消息发送与接收的双向通信。设计遵循契约驱动、分层隔离、插件扩展的原则，可方便地翻译为任意编程语言的具体实现。

---

## 第一章：总体架构

### 1.1 系统定位

EasyBot 是连接**IM 平台**与**业务系统**之间的独立中间层服务，承担以下职责：

- **南向（Southbound）**：接入 Telegram、WhatsApp、Discord、微信、飞书等即时通信平台
- **北向（Northbound）**：对外暴露 RESTful API + WebSocket，供第三方业务系统或客户端调用
- **双向转发**：将接收到的 IM 消息转发给业务系统，将业务系统产生的消息发送到 IM 平台

### 1.2 架构分层

```
                      ┌────────────────────────────────────┐
          ┌─────────  │        第三方客户端 / 业务系统       │  ──────────┐
          │           └────────────────────────────────────┘            │
          │                          ↕                                  │
          │               REST API / WebSocket / gRPC                   │
          │                          ↕                                  │
          │           ┌────────────────────────────────┐                │
          │           │      EasyBot (独立服务)       │               │
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
              │ Telegram | Discord | WhatsApp | ...  │  IM 平台           │
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
| **账号（Account）** | 一个 IM 平台上的机器人/应用账号（一个适配器可管理多个账号） |
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
    "slack:C0123ABC"
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
    "slack:C0123ABC": { "id": "msg_3", "status": "failed", "error": "chat not found" }
  }
}
```

#### 2.2.2 消息编辑与删除

```
PUT /api/v1/messages/{messageId}
{
  "text": "修改后的消息",
  "parseMode": "markdown"
}

Response 200: { "id": "...", "status": "edited", "updatedAt": 1718000100000 }

DELETE /api/v1/messages/{messageId}

Response 200: { "id": "...", "status": "deleted" }
```

#### 2.2.3 媒体发送

```
POST /api/v1/messages/send-media
Content-Type: multipart/form-data

fields:
  target: "telegram:123456"
  caption: "这是图片说明"
  file: <binary>

Response 200:
{
  "id": "msg_456",
  "status": "sent",
  "messageId": "...",
  "mediaType": "image"
}
```

#### 2.2.4 适配器管理

```
GET /api/v1/adapters
Response 200:
{
  "adapters": [
    {
      "platform": "telegram",
      "displayName": "Telegram",
      "status": "connected",
      "accounts": [
        {
          "accountId": "bot_123",
          "name": "My Bot",
          "enabled": true,
          "connected": true,
          "lastActivityAt": 1718000000000
        }
      ]
    },
    {
      "platform": "discord",
      "displayName": "Discord",
      "status": "disconnected",
      "accounts": [],
      "error": "token not configured"
    }
  ]
}

POST /api/v1/adapters/{platform}/start
Body: { "accountId": "..." }
Response 200: { "platform": "telegram", "status": "starting" }

POST /api/v1/adapters/{platform}/stop
Body: { "accountId": "..." }
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

#### 2.2.5 聊天管理

```
GET /api/v1/chats?platform=telegram
Response 200:
{
  "chats": [
    { "id": "123456", "name": "Alice", "type": "dm" },
    { "id": "-789012", "name": "Team Chat", "type": "group", "memberCount": 15 }
  ]
}

GET /api/v1/chats/{platform}:{chatId}
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

#### 2.2.6 消息历史查询

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

#### 2.2.7 消息接收（轮询模式）

```
GET /api/v1/messages/inbox?platform=telegram&since=1717000000000&limit=20

Response 200:
{
  "messages": [ /* inbound messages */ ],
  "nextSince": 1718000100000
}
```

#### 2.2.8 网关健康检查

```
GET /api/v1/health
Response 200:
{
  "status": "healthy",                // healthy | degraded | down
  "uptime": 86400,
  "version": "1.0.0",
  "adapters": {
    "total": 5,
    "connected": 4,
    "disconnected": 1
  },
  "sessions": { "active": 128, "total": 1024 },
  "metrics": {
    "messagesInPerMin": 45,
    "messagesOutPerMin": 60,
    "errorRate": 0.02
  }
}
```

#### 2.2.9 配置管理

```
GET /api/v1/config
Response 200: { /* 网关当前配置 */ }

PUT /api/v1/config
Body: { /* 更新配置 */ }
Response 200: { "ok": true, "requiresRestart": true }

PATCH /api/v1/config/adapters/telegram
Body: { "token": "new_token_here" }
Response 200: { "ok": true, "requiresRestart": false }
```

### 2.3 WebSocket 实时推送

客户端通过 WebSocket 连接接收实时事件推送。

**连接地址：** `ws(s)://<gateway-host>:<port>/api/v1/ws`

**连接认证：**
```
// 客户端连接后发送认证帧
{
  "type": "auth",
  "token": "client_api_key_here"
}
```

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
| `adapter.connected` | 适配器连接成功 | { platform, accountId } |
| `adapter.disconnected` | 适配器断开 | { platform, accountId, reason } |
| `adapter.error` | 适配器异常 | { platform, accountId, error } |
| `callback.received` | 收到按钮回调 | { id, platform, chatId, data, messageId } |
| `gateway.started` | 网关启动 | { timestamp, version } |
| `gateway.stopping` | 网关关闭 | { reason } |

**客户端订阅过滤：**
```json
// 客户端可发送订阅帧来过滤事件
{
  "type": "subscribe",
  "events": ["message.inbound", "adapter.*"],
  "platforms": ["telegram", "discord"]
}
```

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
| `INTERNAL_ERROR` | 500 | 内部错误 |

---

## 第三章：南向适配器接口

### 3.1 适配器生命周期

```
┌──────────────────────────────────────────────────────────────┐
│                     Adapter Lifecycle                        │
│                                                              │
│  ┌─────────┐    ┌──────────┐    ┌────────────┐              │
│  │ CREATED │───→│ STARTING │───→│ CONNECTED  │              │
│  └─────────┘    └──────────┘    └─────┬──────┘              │
│       │                                │                     │
│       │                                │ (disconnect /       │
│       │                                │  rate limited /     │
│       │                                │  network error)     │
│       │                                ▼                     │
│       │                         ┌────────────┐              │
│       │                         │ RECONNECTING│─────────────→│
│       │                         └────────────┘              │
│       │                                │                     │
│       │                                │ (max retries        │
│       │                                │  exceeded)          │
│       │                                ▼                     │
│       │                         ┌────────────┐              │
│       └────────────────────────→│   FAILED   │              │
│                                 └────────────┘              │
│                                                              │
│  任何时候调用 stop() → DISCONNECTED                         │
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
   * 必须在此方法中设置入站消息的处理链条。
   */
  connect(): Promise<ConnectResult>;
  
  /**
   * 断开与 IM 平台的连接。
   * 清理资源、关闭网络连接、取消定时任务。
   * 调用后 isConnected() 应返回 false。
   */
  disconnect(): Promise<void>;
  
  /** 当前连接状态 */
  isConnected(): boolean;
  
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
             metadata?: Record<string, unknown>): Promise<void>;
  
  /** 是否支持流式草稿 */
  supportsDraftStreaming?(): boolean;
  
  // ── 查询 ─────────────────────────────────────────────
  
  /**
   * 获取聊天室的基本信息。
   */
  getChatInfo(chatId: string): Promise<ChatInfo>;
  
  /**
   * 列出可用的聊天列表（可选，部分平台不支持枚举）。
   */
  listChats?(filter?: ChatFilter): Promise<ChatInfo[]>;
  
  // ── 入站消息事件 ────────────────────────────────────
  
  /**
   * 注册入站消息处理器。
   * 适配器在收到平台消息时调用此处理器。
   * 由核心层在 connect 之前设置。
   */
  onMessage(handler: MessageHandler): void;
  
  /**
   * 注册按钮回调处理器。
   * 适配器在收到按钮点击回传时调用此处理器。
   */
  onCallback?(handler: CallbackHandler): void;

  // ── 配置 ─────────────────────────────────────────────

  /**
   * 返回适配器的运行时配置状态。
   * 用于外部 API 查询适配器当前配置。
   */
  getRuntimeConfig(): AdapterRuntimeConfig;
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

// ── 处理器 ──
type MessageHandler = (message: InboundMessage) => Promise<void>;
type CallbackHandler = (callback: {
  id: string;
  platform: string;
  chatId: string;
  userId: string;
  data: string;
  messageId: string;
  metadata?: Record<string, unknown>;
}) => Promise<void>;
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
│  │  Message Bus  │  │ Session Mgr  │  │  Router          │ │
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
│  │  User ACL    │  │  health poll │  │  hot-reload      │ │
│  │  Rate Limit  │  │  reconnect   │  │                  │ │
│  └──────────────┘  └──────────────┘  └──────────────────┘ │
└────────────────────────────────────────────────────────────┘
```

### 4.2 消息总线

消息总线是内部的事件枢纽，负责模块间解耦。

```typescript
interface MessageBus {
  /**
   * 发布事件到总线
   */
  publish(event: GatewayEvent): Promise<void>;
  
  /**
   * 订阅特定事件
   */
  subscribe(eventType: string, handler: EventHandler): Subscription;
  
  /**
   * 取消订阅
   */
  unsubscribe(subscription: Subscription): void;
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

type EventHandler = (event: GatewayEvent) => Promise<void>;

type Subscription = {
  id: string;
  eventType: string;
  cancel(): void;
};
```

**内部事件订阅关系：**

| 事件 | 订阅者 | 处理逻辑 |
|------|--------|----------|
| `message.inbound` | Session Manager | 创建/查找会话，更新活跃时间 |
| `message.inbound` | Router | 路由到外部客户端的 WebSocket |
| `message.inbound` | Statistics | 更新统计计数 |
| `adapter.disconnected` | Adapter Manager | 触发重连逻辑 |
| `adapter.error` | Adapter Manager | 计数错误，达到阈值时告警 |

### 4.3 入站消息处理流程

```
1. 适配器收到平台消息
   │
2. 适配器构造 InboundMessage
   │
3. 适配器调用 onMessage(handler)
   │
4. 核心层收到 InboundMessage：
   │
   ├─ 4a. 发布 message.inbound 事件到 MessageBus
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
   │      └─ 解析命令，执行内置命令逻辑
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
3. Router 解析 target：
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
  
  /**
   * 检查是否需要重置会话
   * 根据 resetPolicy 判断
   */
  shouldReset(session: Session): boolean;
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

type SessionMutation = Partial<Pick<Session, 'metadata' | 'resetPolicy'>>;

interface SessionFilter {
  platform?: string;
  activeWithin?: number;            // N 分钟内的活跃会话
  limit?: number;
  offset?: number;
}
```

### 4.6 适配器管理器

```typescript
interface AdapterManager {
  /**
   * 注册适配器工厂（由插件在加载时调用）
   */
  register(name: string, factory: AdapterFactory): void;
  
  /**
   * 根据配置创建并启动适配器
   */
  startAdapter(platform: string, config: AdapterConfig): Promise<StartResult>;
  
  /**
   * 停止适配器
   */
  stopAdapter(platform: string): Promise<void>;
  
  /**
   * 获取适配器实例
   */
  getAdapter(platform: string): PlatformAdapter | null;
  
  /**
   * 列出所有适配器状态
   */
  listAdapters(): AdapterStatus[];
  
  /**
   * 启动所有已配置的适配器
   */
  startAll(): Promise<{ succeeded: string[]; failed: { platform: string; error: string }[] }>;
  
  /**
   * 停止所有适配器
   */
  stopAll(): Promise<void>;
  
  /**
   * 对所有适配器进行健康轮询
   */
  pollHealth(): Promise<void>;
}

interface AdapterStatus {
  platform: string;
  displayName: string;
  state: 'created' | 'starting' | 'connected' | 'reconnecting' | 'failed' | 'stopped';
  connected: boolean;
  health?: 'healthy' | 'degraded' | 'down';
  lastError?: string;
  uptime?: number;
  accounts?: AccountInfo[];
}

interface AccountInfo {
  accountId: string;
  name: string;
  connected: boolean;
  lastActivityAt?: number;
}

type AdapterFactory = (config: AdapterConfig) => Promise<PlatformAdapter>;

interface StartResult {
  ok: boolean;
  platform: string;
  error?: string;
  botInfo?: { name: string; username?: string; id: string };
}
```

---

## 第五章：插件体系

### 5.1 插件定义

每个平台适配器作为一个独立插件，由核心动态加载。

```typescript
/**
 * 适配器插件描述文件 format
 * 可定义为 JSON / YAML / TOML
 */
{
  "name": "im-gateway-adapter-telegram",
  "version": "1.0.0",
  "displayName": "Telegram",
  "description": "Telegram Bot API 适配器",
  "entrypoint": "adapter.js",                  // 主入口文件
  "dependencies": {
    "sdk": ["python-telegram-bot@>=20.0"],
    "optionalSdk": ["aiohttp-socks"]           // 可选依赖
  },
  "configSchema": {
    "token": {
      "type": "password",
      "label": "Bot Token",
      "description": "从 @BotFather 获取的 Telegram Bot Token",
      "required": true,
      "envVar": "TELEGRAM_BOT_TOKEN"
    },
    "proxyUrl": {
      "type": "string",
      "label": "代理 URL",
      "description": "SOCKS5/HTTP 代理（可选）",
      "required": false,
      "envVar": "TELEGRAM_PROXY"
    }
  },
  "capabilities": [
    "text", "image", "audio", "video", "document",
    "interactive", "streaming", "markdown", "code_block",
    "group", "typing_indicator", "message_edit", "message_delete"
  ],
  "promptHints": "你正在 Telegram 上与用户对话。支持 Markdown 格式..."
}
```

### 5.2 插件加载流程

```
Gateway 启动
    │
    ▼
1. 扫描插件目录（默认 ./plugins/）
    │
    ▼
2. 读取每个插件的描述文件
    │
    ▼
3. 验证插件的依赖是否满足
    ├─ 满足 → 继续
    └─ 不满足 → 日志警告，跳过该插件
    │
    ▼
4. 调用插件的工厂方法，传入配置
    │
    ▼
5. 插件返回 PlatformAdapter 实例
    │
    ▼
6. 注册到 AdapterManager
    │
    ▼
7. 调用 adapter.init(config)，然后 adapter.connect()
```

### 5.3 插件注册表接口

```typescript
interface PluginRegistry {
  /**
   * 注册一个适配器插件
   */
  register(plugin: PluginDescriptor): void;
  
  /**
   * 发现插件目录中的所有插件
   * 加载描述文件，验证结构完整性
   */
  discover(pluginDir: string): PluginDescriptor[];
  
  /**
   * 为指定平台创建适配器实例
   */
  createAdapter(platform: string, config: AdapterConfig): Promise<PlatformAdapter | null>;
  
  /**
   * 检查指定平台的依赖是否满足
   */
  checkRequirements(platform: string): boolean;
  
  /**
   * 列出所有已注册的插件
   */
  listPlugins(): PluginInfo[];
}

interface PluginDescriptor {
  name: string;
  version: string;
  displayName: string;
  description: string;
  entrypoint: string;
  dependencies: {
    sdk: string[];                    // 必需的 SDK/包
    optionalSdk: string[];            // 可选的 SDK/包
  };
  configSchema: Record<string, ConfigField>;
  capabilities: string[];
}

interface PluginInfo {
  name: string;
  displayName: string;
  version: string;
  loaded: boolean;
  requirementsMet: boolean;
  error?: string;
}
```

---

## 第六章：认证与安全

### 6.1 认证模型

```
┌─────────────────────────────────────────────────────────┐
│                    认证层次                              │
│                                                         │
│  外部客户端 → API Key / JWT Token  →  API Server        │
│                                                         │
│  IM 平台 → 平台 Token（Bot Token / App Secret）→ 适配器  │
│                                                         │
│  管理面 → 用户名密码 / mTLS  →  管理 API                 │
└─────────────────────────────────────────────────────────┘
```

### 6.2 API 密钥管理

```typescript
interface ApiKeyManager {
  /**
   * 创建新的 API Key
   */
  createKey(params: {
    name: string;
    permissions: string[];            // "message:send" | "message:read" | "adapter:admin" | ...
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

interface ApiKeyInfo {
  id: string;
  name: string;
  prefix: string;                     // Key 前缀（"sk_abc..."）
  createdAt: number;
  expiresAt?: number;
  lastUsedAt?: number;
  revoked: boolean;
}
```

### 6.3 权限模型

```typescript
// 权限字符串
type Permission =
  // 消息
  | 'message:send'          // 发送消息
  | 'message:read'          // 读取消息（WebSocket 订阅）
  | 'message:edit'          // 编辑消息
  | 'message:delete'        // 删除消息
  | 'message:history'       // 查看历史
  // 适配器
  | 'adapter:list'          // 查看适配器列表
  | 'adapter:control'       // 启停适配器
  | 'adapter:configure'     // 修改适配器配置
  // 系统
  | 'system:health'         // 查看健康状态
  | 'system:config'         // 查看/修改系统配置
  | 'system:admin';         // 完全控制

// 角色
type Role = 'admin' | 'operator' | 'developer' | 'guest';

const ROLE_PERMISSIONS: Record<Role, Permission[]> = {
  admin: ['*'],
  operator: ['message:send', 'message:read', 'message:edit', 'message:delete',
             'message:history', 'adapter:list', 'adapter:control', 'system:health'],
  developer: ['message:send', 'message:read', 'message:history',
              'adapter:list', 'system:health'],
  guest: ['message:send'],
};
```

### 6.4 传输安全

```
- 所有外部 API 端点建议使用 HTTPS / WSS
- 内部组件间通信可使用 HTTP（localhost 绑定）
- 支持 mTLS 用于管理接口
- Token 敏感信息不在日志中输出全文（"tok_***" 模式）
- 建议支持速率限制（rate limiting）防止滥用
```

---

## 第七章：配置与部署

### 7.1 配置文件结构

```yaml
# gateway.yaml

# 服务配置
server:
  host: "0.0.0.0"
  port: 8080
  tls:
    enabled: false
    certFile: ""
    keyFile: ""

# 外部 API 配置
api:
  basePath: "/api/v1"
  rateLimit: 100  # 每秒请求数
  websocket:
    enabled: true
    maxClients: 1000
    heartbeatInterval: 30  # 秒

# 数据存储
storage:
  type: "sqlite"             # sqlite | postgres | mysql
  path: "./data/gateway.db"  # sqlite 路径
  # postgres 时:
  # connectionString: "postgres://user:pass@localhost:5432/gateway"

# 日志
logging:
  level: "info"
  format: "json"             # json | text
  output: "stdout"           # stdout | file:./logs/gateway.log

# 插件目录
plugins:
  dir: "./plugins"
  autoDiscover: true

# 适配器配置
adapters:
  telegram:
    enabled: true
    token: "${TELEGRAM_BOT_TOKEN}"    # 支持环境变量引用
    proxyUrl: ""
  
  discord:
    enabled: true
    token: "${DISCORD_BOT_TOKEN}"
  
  whatsapp_cloud:
    enabled: false
    phoneNumberId: ""
    accessToken: ""
    webhookVerifyToken: ""

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

### 7.2 环境变量引用

配置文件中使用 `${VAR_NAME}` 语法引用环境变量，实现敏感信息与配置分离。

### 7.3 命令行接口

```bash
# 启动网关（前台）
im-gateway start --config ./gateway.yaml

# 启动网关（守护进程）
im-gateway start --config ./gateway.yaml --daemon

# 测试适配器连接
im-gateway test telegram

# 查看状态
im-gateway status

# 重新加载配置（热重载）
im-gateway reload

# 优雅关闭
im-gateway stop

# 列出可用插件
im-gateway plugins list

# 安装一个插件
im-gateway plugins install ./plugin-telegram.zip
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
  │ IM GW #1  │          │ IM GW #2  │
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
| **插件加载** | Python：动态 import / Node.js：require() / Go：plugin 或 WASM / Java：SPI |
| **WebSocket** | 各语言主流 WebSocket 库均可，推荐使用标准化的 WAMP 或自定 JSON 帧协议 |
| **存储** | 会话存储建议 SQL（SQLite/PostgreSQL），消息体可存 JSON 列 |
| **容器化** | 建议 Docker 打包，配置文件通过环境变量注入 |

### 8.2 实现优先级

```
Phase 1 — 最小可用（核心链路打通）:
  ├── API Server (REST)
  ├── Adapter Manager（生命周期管理）
  ├── Telegram Adapter（首个参考实现）
  └── 配置文件加载

Phase 2 — 双向通信：
  ├── WebSocket 实时推送
  ├── 入站消息路由（→ WebSocket / Webhook）
  ├── 会话管理
  └── Discord Adapter（验证接口通用性）

Phase 3 — 完善：
  ├── 插件系统
  ├── API Key 管理
  ├── 权限模型
  ├── 媒体发送
  ├── 交互式按钮
  └── 流式草稿

Phase 4 — 生产级：
  ├── 速率限制
  ├── 指标收集（Prometheus）
  ├── 健康检查 API
  ├── 配置热重载
  ├── 多账号支持
  ├── HTTPS / WSS
  └── Docker 镜像
```

### 8.3 数据结构存储建议

```sql
-- 适配器表
CREATE TABLE adapters (
    platform    TEXT PRIMARY KEY,
    enabled     INTEGER NOT NULL DEFAULT 1,
    config_json TEXT NOT NULL,       -- JSON 序列化配置
    state       TEXT NOT NULL DEFAULT 'stopped',  -- created/starting/connected/reconnecting/failed/stopped
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- 聊天室缓存表
CREATE TABLE chats (
    id          TEXT PRIMARY KEY,    -- "platform:chatId"
    platform    TEXT NOT NULL,
    chat_id     TEXT NOT NULL,
    name        TEXT,
    type        TEXT NOT NULL,       -- dm/group/channel/thread
    metadata    TEXT,                -- JSON
    last_seen   INTEGER,
    UNIQUE(platform, chat_id)
);

-- 会话表
CREATE TABLE sessions (
    key         TEXT PRIMARY KEY,    -- "platform:chatId"
    platform    TEXT NOT NULL,
    chat_id     TEXT NOT NULL,
    thread_id   TEXT,
    source_json TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    reset_policy TEXT NOT NULL DEFAULT 'never',
    metadata    TEXT                 -- JSON
);
CREATE INDEX idx_sessions_platform ON sessions(platform);
CREATE INDEX idx_sessions_updated ON sessions(updated_at);

-- API Key 表
CREATE TABLE api_keys (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    key_hash    TEXT NOT NULL UNIQUE, -- 存储 hash 而非明文
    permissions TEXT NOT NULL,        -- JSON 数组
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER,
    last_used   INTEGER,
    revoked     INTEGER NOT NULL DEFAULT 0
);

-- 消息历史表
CREATE TABLE message_history (
    id          TEXT PRIMARY KEY,
    session_key TEXT NOT NULL,
    direction   TEXT NOT NULL,       -- inbound / outbound
    platform    TEXT NOT NULL,
    chat_id     TEXT NOT NULL,
    text        TEXT,
    author      TEXT,                -- JSON { id, name, isBot }
    media       TEXT,                -- JSON [MediaAttachment]
    platform_message_id TEXT,        -- 平台侧的消息 ID
    timestamp   INTEGER NOT NULL,
    metadata    TEXT
);
CREATE INDEX idx_msgs_session ON message_history(session_key, timestamp);
CREATE INDEX idx_msgs_platform ON message_history(platform, timestamp);
```

---

## 第九章：接口一致性承诺

### 9.1 版本兼容性策略

- API 采用 URL 前缀版本化：`/api/v1/`, `/api/v2/`
- 新增 API 字段必须 optional，默认值与旧行为一致
- 适配器接口采用鸭子类型（Duck Typing），新增方法不应破坏现有实现

### 9.2 跨实现的一致性要求

无论选择哪种编程语言实现，以下约定必须保持一致：

1. **API 路径和请求/响应格式**完全遵循第二章定义
2. **适配器接口方法签名**完全遵循第三章定义
3. **事件类型和事件数据格式**完全遵循第二章定义
4. **错误码和错误响应格式**完全遵循第二章定义
5. **配置文件结构**完全遵循第七章定义

### 9.3 测试套件一致性

推荐为所有实现共享一套行为测试（Behavioral Tests）：

```yaml
# 测试场景（语言无关）
scenarios:
  - name: "发送文本消息到 Telegram"
    api: "POST /api/v1/messages/send"
    request:
      target: "telegram:123456"
      text: "Hello"
    expected:
      status: 200
      body.status: "sent"
      
  - name: "断开的适配器返回 503"
    api: "POST /api/v1/messages/send"
    request:
      target: "discord:123"
      text: "Hi"
    expected:
      status: 503
      body.error.code: "ADAPTER_NOT_CONNECTED"
      
  - name: "未知平台返回 404"
    api: "POST /api/v1/messages/send"
    request:
      target: "unknown_platform:123"
      text: "Hi"
    expected:
      status: 404
      body.error.code: "PLATFORM_NOT_FOUND"
```

---

## 附录 A：术语表

| 术语 | 英文 | 说明 |
|------|------|------|
| 网关 | Gateway | 本架构设计的主体服务 |
| 适配器 | Adapter | 连接特定 IM 平台的模块 |
| 消息总线 | Message Bus | 内部事件发布/订阅机制 |
| 会话 | Session | 以 chatId 为键的对话上下文 |
| 出站 | Outbound | 从网关发往 IM 平台的方向 |
| 入站 | Inbound | 从 IM 平台发往网关的方向 |
| 目标 | Target | 消息投递目的地的描述串 |
| 能力 | Capability | 适配器支持的功能声明 |
| 挂件 | Plugin | 独立的适配器/扩展模块 |
| 投递路由 | Delivery Router | 路由消息到目标的组件 |

## 附录 B：参考来源

本架构设计基于以下两个开源项目的实践分析：

- **OpenClaw** — TypeScript 实现的 Agent 运行时，提供了严谨的 WebSocket 网关协议设计（Gateway Protocol）和设备认证机制
- **Hermes-Agent** — Python 实现的通用 AI Agent 框架，提供了完善的多平台 IM 适配器体系（BasePlatformAdapter + 插件系统），支持 15+ 即时通信平台
