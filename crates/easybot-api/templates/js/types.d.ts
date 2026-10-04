// EasyBot 管理后台前端类型契约（手写，字段对照后端 OpenAPI / openapi.json）
// 说明：本仓库无 TypeScript 编译链，此文件仅用于 IDE 智能提示与文档化，
// 通过 JSDoc 标注（如 /** @type {AdapterItem} */）在 js/*.js 中获得补全。
// 顶层 interface 无 import/export，故为全局可见。

/** 适配器运行时状态（GET /api/v1/adapters 的 adapters[]） */
interface AdapterItem {
  platform: string;
  display_name: string;
  status: 'Created' | 'Starting' | 'Connecting' | 'Connected' | 'Reconnecting' | 'Disconnecting' | 'Stopping' | 'Failed' | 'Stopped' | 'Degraded';
  connected: boolean;
  health: 'Healthy' | 'Degraded' | 'Down' | null;
  permanent_failure: boolean;
  retry_attempt: number;
  next_retry_in_ms: number | null;
  last_error: string | null;
}

interface AdapterSummary {
  connected: number;
  total: number;
}

interface AdapterListResponse {
  adapters: AdapterItem[];
}

type ChatType = 'Dm' | 'Group' | 'Channel' | 'Thread';

type MessageRole = 'User' | 'Assistant' | 'System';

/** 会话来源信息（Session.source） */
interface SessionSource {
  platform: string;
  chat_id: string;
  chat_name: string | null;
  chat_type: ChatType;
  is_bot: boolean;
  user_id: string | null;
  user_name: string | null;
  user_username: string | null;
  user_role?: string | null;
}

/** 会话（GET /api/v1/sessions 的 sessions[]） */
interface Session {
  key: string;
  platform: string;
  chat_id: string;
  thread_id: string | null;
  custom_name: string | null;
  last_message: string | null;
  last_message_at: number | null;
  source: SessionSource;
  created_at: number;
  updated_at: number;
}

/** 消息历史条目（StoredMessage） */
interface StoredMessage {
  id: string;
  session_key: string;
  platform: string;
  chat_id: string;
  role: MessageRole;
  text: string | null;
  /** 平台原始 payload（仅调试用途）；msg_type 等字段供徽章渲染 */
  raw_data: { msg_type?: string } & Record<string, unknown>;
  /** 原始消息时间戳（毫秒） */
  timestamp: number;
  /** 存储时间戳（毫秒） */
  created_at: number;
}

interface MessageHistoryResponse {
  messages: StoredMessage[];
  has_more: boolean;
}

/** API Key 摘要（GET /api/v1/api-keys） */
interface ApiKeyResponse {
  id: string;
  name: string;
  prefix: string;
  subject_id: string;
  permissions: string[];
  requests_per_minute: number | null;
  revoked: boolean;
  created_at: number;
  expires_at: number | null;
  last_used_at: number | null;
}

/** 创建 / 轮换 API Key 的响应（含仅显示一次的明文 key） */
interface CreateApiKeyResponse extends ApiKeyResponse {
  /** 完整明文密钥，仅此一次返回 */
  key: string;
}

/** 插件市场目录条目（GET /api/v1/plugins/catalog） */
interface CatalogItem {
  name: string;
  publisher: string;
  display_name: string | null;
  description: string | null;
  repo: string;
  tags: string[];
  verified: boolean;
}

interface CatalogResponse {
  plugins: CatalogItem[];
}

/** 已安装插件（GET /api/v1/plugins） */
interface PluginItem {
  name: string;
  display_name: string | null;
  description: string | null;
  version: string;
  publisher: string | null;
  platform: string | null;
  sdk_version: number;
  signed: boolean;
  signature_valid: boolean | null;
  enabled: boolean;
  load_error: string | null;
}

interface PluginsResponse {
  plugins: PluginItem[];
}

/** 会话重命名响应（PUT /api/v1/sessions/{key}） */
interface RenameSessionResponse {
  session: Session;
}

/** Subject 的 Target 授权条目 */
interface TargetGrant {
  id: string;
  platform: string;
  chat_id: string;
  actions: string[];
}
