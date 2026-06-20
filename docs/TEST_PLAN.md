# EasyBot 全面验收测试计划

## 项目总览

EasyBot 是一个 **IM Gateway** 服务，连接多个 IM 平台并对外提供统一的 REST API + WebSocket。
开发阶段 P1-P5 已全部完成，以下是全功能测试计划。

---

## T1: 构建与编译验证

| # | 测试项 | 命令 | 预期结果 |
|---|--------|------|----------|
| T1.1 | 默认构建 | `cargo build` | 编译成功，0 warnings |
| T1.2 | 全特性构建 | `cargo build --features full` | 编译成功，包含所有 5 个内置适配器 |
| T1.3 | 插件系统构建 | `cargo build --features "full,plugin-system"` | 编译成功，包含插件系统 |
| T1.4 | 最小构建 | `cargo build --no-default-features --features adapter-telegram` | 编译成功，仅含 Telegram 适配器 |
| T1.5 | 无适配器构建 | `cargo build --no-default-features` | 编译成功，启动时提示无适配器 |
| T1.6 | release 构建 | `cargo build --release --features full` | 编译成功，优化模式 |
| T1.7 | Clippy | `cargo clippy --all-targets --features full` | 无严重 lint 错误 |
| T1.8 | 单元测试 | `cargo test --workspace --lib --features "full,plugin-system"` | **39 tests passed** |
| T1.9 | 集成测试 | `cargo build -p mock-adapter && cargo test -p integration-tests` | **1 test passed** (Plugin system E2E) |

---

## T2: CLI 与启动路径

### T2.1 命令行标志

| # | 测试项 | 命令 | 预期结果 |
|---|--------|------|----------|
| T2.1.1 | 帮助 | `cargo run -- --help` | 显示所有 CLI 选项说明 |
| T2.1.2 | 版本 | `cargo run -- --version` | 显示版本号 |
| T2.1.3 | --init | `cargo run -- --init --dir /tmp/easybot-test` | 创建 `gateway.yaml` 和目录结构 |
| T2.1.4 | --init 重复 | `cargo run -- --init --dir /tmp/easybot-test` (再次) | 提示 "EasyBot is already initialized"，不覆盖 |
| T2.1.5 | --dir | `EASYBOT_HOME= cargo run -- --dir /tmp/easybot-test --debug` | 使用指定目录启动 |
| T2.1.6 | --config | `cargo run -- --config /tmp/easybot-test/gateway.yaml --debug` | 使用指定配置文件 |
| T2.1.7 | --debug | `cargo run -- --debug` | 日志级别 debug，创建 dev API key |
| T2.1.8 | 无 config 启动 | `cargo run -- --dir /tmp/easybot-empty` (空目录) | 使用默认配置启动，打印警告 |

### T2.2 路径解析

| # | 测试项 | 操作 | 预期结果 |
|---|--------|------|----------|
| T2.2.1 | 默认路径 | 不设环境变量，不传 --dir | 使用 `~/.easybot/` 或平台标准目录 |
| T2.2.2 | EASYBOT_HOME | `EASYBOT_HOME=/custom/path cargo run -- --debug` | 使用 `/custom/path/` 作为 home |
| T2.2.3 | --dir 优先级 | `EASYBOT_HOME=/ignored cargo run -- --dir /my-dir --debug` | 使用 `/my-dir/` |

### T2.3 优雅关闭

| # | 测试项 | 操作 | 预期结果 |
|---|--------|------|----------|
| T2.3.1 | Ctrl+C | 启动后按 Ctrl+C | 打印 "Shutting down..."，发布 `gateway.stopping` 事件，停止适配器 |
| T2.3.2 | 发布事件验证 | 检查日志 | 日志中出现 "gateway.started" 和 "gateway.stopping" |

---

## T3: API 端点验证

> 说明：以下测试依赖 `--debug` 模式启动（自动创建 dev API key）。
> 使用 `curl` 或 `httpie` 测试。

### T3.1 公共端点

| # | 测试项 | 请求 | 预期结果 |
|---|--------|------|----------|
| T3.1.1 | 健康检查 | `GET /api/v1/health` | `200`, `{"status":"healthy"/"degraded", "version":"...", "adapters":{...}}` |
| T3.1.2 | 指标端点 | `GET /api/v1/metrics` | `200`, Prometheus 文本格式（需配置 metrics.enabled=true） |
| T3.1.3 | Swagger UI | `GET /swagger` | `200`, HTML 页面 |
| T3.1.4 | OpenAPI JSON | `GET /openapi.json` | `200`, OpenAPI 3.1 JSON |

### T3.2 认证验证

| # | 测试项 | 请求 | 预期结果 |
|---|--------|------|----------|
| T3.2.1 | 无 token | `GET /api/v1/adapters` | `401 Missing or invalid Authorization header` |
| T3.2.2 | 错误 token | `GET /api/v1/adapters` + `Authorization: Bearer fake_key` | `401 Invalid API key` |
| T3.2.3 | 有效 token | 使用 dev key | `200` 正常响应 |
| T3.2.4 | 吊销后验证 | 创建 key → 吊销 → 使用 | `401` |

### T3.3 速率限制

| # | 测试项 | 操作 | 预期结果 |
|---|--------|------|----------|
| T3.3.1 | 正常请求 | 10 个请求/秒 | 全部 `200` |
| T3.3.2 | 超限请求 | 连续 70+ 请求/分钟 | 部分请求返回 `429 RateLimited` |
| T3.3.3 | 不同 IP | 从不同 IP 模拟 | 各自独立计数 |
| T3.3.4 | 关闭限流 | 设置 `api.rate_limit.enabled=false` | 不限流 |

### T3.4 适配器管理

| # | 测试项 | 请求 | 预期结果 |
|---|--------|------|----------|
| T3.4.1 | 列出适配器 | `GET /api/v1/adapters` | 返回注册的适配器列表及状态 |
| T3.4.2 | 获取状态 | `GET /api/v1/adapters/telegram/status` | 返回 `AdapterStatusSummary` |
| T3.4.3 | 获取不存在的适配器 | `GET /api/v1/adapters/nonexistent/status` | `404` |
| T3.4.4 | 启动适配器 | `POST /api/v1/adapters/telegram/start` | `{"ok":..., "platform":"telegram"}` |
| T3.4.5 | 停止适配器 | `POST /api/v1/adapters/telegram/stop` | `{"ok":true, "platform":"telegram"}` |

### T3.5 消息发送

| # | 测试项 | 请求 | 预期结果 |
|---|--------|------|----------|
| T3.5.1 | 发送消息 | `POST /api/v1/messages/send` `{"target":"telegram:123","text":"hello"}` | `200` 含消息 ID |
| T3.5.2 | 非法 target 格式 | 同上，`target:"invalid"` | `400` |
| T3.5.3 | 批量发送 | `POST /api/v1/messages/batch-send` `{"targets":["tg:1","tg:2"],"text":"batch"}` | `200` 含每个 target 的结果 |
| T3.5.4 | 批量发送超限 | 100 个 target | 并发 5，15s 超时，错误展示在结果中 |

### T3.6 消息管理

| # | 测试项 | 请求 | 预期结果 |
|---|--------|------|----------|
| T3.6.1 | 编辑消息 | `PUT /api/v1/messages/{id}` `{"target":"tg:1","text":"edited"}` | `200` |
| T3.6.2 | 删除消息 | `DELETE /api/v1/messages/{id}` `{"target":"tg:1"}` | `200` |
| T3.6.3 | 消息历史 | `GET /api/v1/messages?session_key=telegram:123&limit=10` | `200` 含消息列表 + `has_more` |
| T3.6.4 | 游标分页 | `GET /api/v1/messages?session_key=telegram:123&before=TIMESTAMP` | 返回该时间戳之前的消息 |

### T3.7 会话管理

| # | 测试项 | 请求 | 预期结果 |
|---|--------|------|----------|
| T3.7.1 | 列出会话 | `GET /api/v1/sessions` | `200` 含会话列表 |
| T3.7.2 | 获取会话 | `GET /api/v1/sessions/telegram:123` | `200` 含会话详情 |
| T3.7.3 | 获取不存在会话 | `GET /api/v1/sessions/nonexistent` | `404` |
| T3.7.4 | 删除会话 | `DELETE /api/v1/sessions/telegram:123` | `200` |

### T3.8 聊天查询

| # | 测试项 | 请求 | 预期结果 |
|---|--------|------|----------|
| T3.8.1 | 列出聊天 | `GET /api/v1/chats/telegram` | 返回该平台的聊天列表 |
| T3.8.2 | 获取聊天 | `GET /api/v1/chats/telegram/123` | 返回聊天信息 |

### T3.9 配置管理

| # | 测试项 | 请求 | 预期结果 |
|---|--------|------|----------|
| T3.9.1 | 获取配置 | `GET /api/v1/config` | `200`, 完整配置 JSON |
| T3.9.2 | 更新配置 | `PUT /api/v1/config` `{"api":{"rate_limit":{"enabled":false}}}` | 合并更新，返回 `{"ok":true}` |
| T3.9.3 | 更新后读取 | `GET /api/v1/config` | 反映刚才的变更 |
| T3.9.4 | 非法配置 | `PUT /api/v1/config` `{"port":"string"}` | 返回 `PARSE_ERROR`，配置不变 |
| T3.9.5 | 断言不变 | `GET /api/v1/config` | 与更新前一致 |

### T3.10 WebSocket 推送

| # | 测试项 | 操作 | 预期结果 |
|---|--------|------|----------|
| T3.10.1 | 连接 | `ws://host:port/api/v1/ws` | 升级成功 |
| T3.10.2 | 认证 | 发送 `{"token":"dev-key"}` | 收到 `{"type":"auth_ok"}` |
| T3.10.3 | 错误 token | 发送 `{"token":"bad"}` | 收到 `{"type":"auth_failed"}` 后断开 |
| T3.10.4 | 事件推送 | 认证后触发消息事件 | 收到 JSON 事件帧 |
| T3.10.5 | 背压断开 | 慢速消费者 | 丢弃 50 个事件后断开 |

---

## T4: Core 子系统

### T4.1 API Key 管理

| # | 测试项 | 位置 | 验证方式 |
|---|--------|------|----------|
| T4.1.1 | 创建 key | `ApiKeyManager::create_key()` | 返回 `(id, raw_key)`，raw_key 以 `eb_` 开头 |
| T4.1.2 | 验证 key | `authenticate()` | 返回 `AuthInfo` 含 name/permissions |
| T4.1.3 | 吊销 key | `revoke_key()` + `authenticate()` | 吊销后验证失败 |
| T4.1.4 | 过期 key | `create_key(expires_at=1)` + `authenticate()` | 返回 expired 错误 |
| T4.1.5 | 无效 key | `authenticate("invalid")` | 返回 Invalid API key |
| T4.1.6 | Argon2 哈希 | 内部逻辑 | 使用 argon2id, PHC 格式存储，spawn_blocking 执行 |

### T4.2 事件总线

| # | 测试项 | 位置 | 验证方式 |
|---|--------|------|----------|
| T4.2.1 | 发布/订阅 | `EventBus::publish()` + `subscribe()` | 订阅者收到事件 |
| T4.2.2 | 多订阅者 | 多个 `subscribe()` 同一类型 | 每个订阅者都收到 |
| T4.2.3 | 无订阅者 | 发布到无订阅者的类型 | 安全无操作 |
| T4.2.4 | subscribe_many | 订阅多个类型 | 合并流收到所有类型 |
| T4.2.5 | 信道容量 | 256 事件满 | 旧事件被丢弃，Lagged 通知 |
| T4.2.6 | 事件类型 | `event_types::all()` | 包含所有 10 个预定义类型 |

### T4.3 SessionManager

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.3.1 | get_or_create 新会话 | 返回新 Session，含默认值 |
| T4.3.2 | get_or_create 重复 | 返回同一 Session，updated_at 更新 |
| T4.3.3 | get 存在 | 返回 Session |
| T4.3.4 | get 不存在 | 返回 None |
| T4.3.5 | delete 存在 | 返回 true |
| T4.3.6 | delete 不存在 | 返回 false |
| T4.3.7 | list 过滤 | 按 platform/active time 过滤 |
| T4.3.8 | list 排序 | 按 updated_at DESC |
| T4.3.9 | update | 修改 reset_policy/metadata |
| T4.3.10 | store_ref | 返回 Option<Arc<dyn SessionStore>> |
| T4.3.11 | load_from_store | 从持久化存储加载会话 |

### T4.4 SessionBridge

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.4.1 | 入站消息事件触发 | 订阅 `message.inbound` → 自动创建会话 |
| T4.4.2 | 重复消息 | 相同 chat_id → 复用已有会话 |
| T4.4.3 | 非法事件数据 | 反序列化失败 → 静默跳过 |

### T4.5 MessagePersister

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.5.1 | 入站消息持久化 | 订阅 `message.inbound` → 存储到 MessageStore |
| T4.5.2 | 非法事件数据 | 反序列化失败 → 静默跳过（无日志） |

### T4.6 AdapterManager

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.6.1 | start → init → connect | 适配器从 Created → Starting → Connected |
| T4.6.2 | start 失败（init 返回 error） | 不存储适配器，不调用 connect |
| T4.6.3 | start 失败（connect 返回 error） | 适配器以 Failed 状态存储 |
| T4.6.4 | stop → disconnect | 适配器从映射移除，调用 disconnect |
| T4.6.5 | 重复 stop | 安全（Ok(())，无副作用） |
| T4.6.6 | send_message 到未连接适配器 | 返回 `AdapterNotConnected` |
| T4.6.7 | start_all 跳过 disabled | enabled:false 的适配器不启动 |
| T4.6.8 | start_all 部分失败 | 返回 succeeded + failed |
| T4.6.9 | stop_all | 所有适配器 disconnect |

### T4.7 AdapterRegistry

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.7.1 | register | 平台可被 `has_platform` 检测 |
| T4.7.2 | create | 调用 factory，返回 Box<dyn PlatformAdapter> |
| T4.7.3 | create 未注册平台 | 返回 `"no factory registered"` |
| T4.7.4 | 覆盖注册 | 同名平台覆盖，无警告 |
| T4.7.5 | list_platforms | 返回 `[(name, display_name)]` |

### T4.8 WebhookDispatcher

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.8.1 | 事件转发 | 匹配事件 → HTTP POST 到 webhook URL |
| T4.8.2 | 事件过滤 | `wh.events` 列表过滤 |
| T4.8.3 | 平台过滤 | `wh.platforms` 列表过滤 |
| T4.8.4 | 通配符 | `"*"` 订阅所有事件类型 |
| T4.8.5 | HMAC 签名 | 配置 `secret` → 发送 `X-Signature-256` 头 |
| T4.8.6 | 无 webhook | `webhooks: []` → 不启动 |
| T4.8.7 | HTTP 错误 | 非 2xx → warn 日志，不重试 |
| T4.8.8 | 超时 | 10s 超时 → warn 日志 |

### T4.9 配置管理器

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.9.1 | get 正确 | 返回当前配置 Arc 引用 |
| T4.9.2 | swap 原子替换 | 返回旧配置，新配置可读 |
| T4.9.3 | 文件轮询 | 配置文件变更后 60s 内自动重新加载 |
| T4.9.4 | 文件不存在 | 安全，不报错 |
| T4.9.5 | PUT /config + 轮询 | API 热更新后文件变更不覆盖 |

### T4.10 TTL 清理

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.10.1 | 过期消息清理 | 插入 100 天前的消息 → 运行清理 → 被删除 |
| T4.10.2 | 最近消息保留 | 插入今天的消息 → 运行清理 → 保留 |
| T4.10.3 | 过期会话清理 | 插入 400 天前的会话 → 清理 → 被删除 |
| T4.10.4 | 配置为 0 | `cleanup_interval_secs=0` → 不启动 worker |
| T4.10.5 | 错误容忍 | 数据库错误 → warn 日志，循环继续 |

### T4.11 速率限制器

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.11.1 | under_limit | 60 请求/分钟 → 全部通过 |
| T4.11.2 | over_limit | 61+ 请求 → 部分拒绝 |
| T4.11.3 | burst | 1 秒内 11 请求 → 第 11 个拒绝 |
| T4.11.4 | 不同 IP | 各自独立计数 |
| T4.11.5 | disabled | `enabled=false` → 全部通过 |
| T4.11.6 | 窗口滑动 | 60s 窗口过去后计数重置 |
| T4.11.7 | X-Forwarded-For | 优先使用该头提取 IP |

### T4.12 Prometheus 指标

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.12.1 | /metrics 端点 | 返回 Prometheus 文本格式 |
| T4.12.2 | http_requests_total | 请求后计数器递增 |
| T4.12.3 | disabled | `metrics.enabled=false` → /metrics 不注册 |

---

## T5: 存储后端

### T5.1 SQLite

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T5.1.1 | 创建连接池 | `create_pool` → WAL 模式设置 |
| T5.1.2 | 迁移 | `run_migrations` → 所有表创建 |
| T5.1.3 | 幂等迁移 | 重复运行 → 不报错 |
| T5.1.4 | 会话 CRUD | upsert → get → delete → 验证 |
| T5.1.5 | 消息 CRUD | store → list → delete → 验证 |
| T5.1.6 | 会话过滤 | list_sessions 按 platform |
| T5.1.7 | 消息过滤 | list_messages 按 session_key/platform/chat_id |
| T5.1.8 | 并发写入 | 多适配器同时写入 → 无丢失 |
| T5.1.9 | 数据库文件权限 | 运行时创建 600/644 权限 |

### T5.2 PostgreSQL（需要 `--features integration-test` + 运行中的 PG）

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T5.2.1 | 创建连接池 | `create_pg_pool` |
| T5.2.2 | 迁移 | `run_pg_migrations` |
| T5.2.3 | 会话 CRUD | 与 SQLite 相同测试覆盖 |
| T5.2.4 | 消息 CRUD | 与 SQLite 相同测试覆盖 |
| T5.2.5 | JSONB 字段 | metadata/raw_data 使用 JSONB |

### T5.3 TTL 实现

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T5.3.1 | SQLite 删除过期 | `delete_expired_messages/sessions` |
| T5.3.2 | PG 删除过期 | 同上（PG 实现） |

---

## T6: 插件系统

### T6.1 加载流程

| # | 测试项 | 操作 | 预期结果 |
|---|--------|------|----------|
| T6.1.1 | 完整加载 | 构建 mock-adapter，创建 plugin.yaml + .so 到目录 | 插件成功加载 |
| T6.1.2 | 目录不存在 | `plugins/` 目录缺失 | 优雅跳过，info 日志 |
| T6.1.3 | 缺少 plugin.yaml | 目录存在但无 manifest | `ManifestNotFound` 跳过 |
| T6.1.4 | 非法 plugin.yaml | YAML 格式错误 | `ManifestParseError` 跳过 |
| T6.1.5 | 缺少 .so | plugin.yaml 指向不存在的文件 | `LibraryNotFound` 跳过 |
| T6.1.6 | 损坏的 .so | 文件不是有效动态库 | `LibraryLoadError` 跳过 |
| T6.1.7 | ABI 版本不匹配 | 返回不同版本 | `AbiVersionMismatch` 跳过 |
| T6.1.8 | 平台名冲突 | 两个插件同 platform_name | 后者 `PlatformConflict` 跳过 |

### T6.2 生命周期

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T6.2.1 | 工厂创建 | `get_factory` → 闭包 → create_adapter |
| T6.2.2 | init | 工厂自动调用 init |
| T6.2.3 | connect | 适配器方法可调用 |
| T6.2.4 | send | 适配器方法可调用 |
| T6.2.5 | disconnect | 适配器方法可调用 |
| T6.2.6 | set_event_bus | 事件总线注入可用 |

### T6.3 SDK

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T6.3.1 | declare_plugin! | 生成三个 extern C 符号 |
| T6.3.2 | prelude | `use prelude::*` 导入所有类型 |
| T6.3.3 | 示例编译 | `cd plugins/example-slack-plugin && cargo build` | 编译通过 |

---

## T7: Docker 部署

| # | 测试项 | 命令 | 预期结果 |
|---|--------|------|----------|
| T7.1 | Docker 构建 | `docker build -t easybot .` | 构建成功 |
| T7.2 | 容器启动 | `docker run -d easybot` | 容器运行 |
| T7.3 | 健康检查 | `curl localhost:8080/health` | `200` |
| T7.4 | 配置挂载 | `-v ./gateway.yaml:/etc/easybot/gateway.yaml` | 使用挂载的配置 |
| T7.5 | 数据持久化 | `-v easybot_data:/var/lib/easybot/data` | 数据在容器重启后保留 |
| T7.6 | docker-compose | `docker-compose up -d` | 所有服务启动 |
| T7.7 | 环境变量 | `TELEGRAM_BOT_TOKEN=xxx docker-compose up -d` | 环境变量传递给容器 |
| T7.8 | Prometheus（可选） | `docker-compose --profile monitoring up -d` | Prometheus 抓取指标 |
| T7.9 | PostgreSQL（可选） | `docker-compose --profile postgres up -d` | PG 服务健康 |

---

## T8: 配置与环境变量

| # | 测试项 | 操作 | 预期结果 |
|---|--------|------|----------|
| T8.1 | 环境变量替换 | config: `${MY_VAR}`, `MY_VAR=hello` | 解析为 "hello" |
| T8.2 | 变量未设置 | config: `${MISSING_VAR}` | 替换为空字符串，warn 日志 |
| T8.3 | 默认配置生成 | `--init` | 生成含注释的完整 YAML |
| T8.4 | 所有字段默认值 | 空配置启动 | 所有字段使用默认值 |
| T8.5 | storage_type=sqlite | 默认 | 使用 SQLite |
| T8.6 | storage_type=postgres | 配置 connection_string | 使用 PostgreSQL |
| T8.7 | storage_type=unsupported | 任意其他字符串 | 回退到内存模式 + warn 日志 |
| T8.8 | adpater.enabled=false | 适配器配置 | 不启动对应适配器 |
| T8.9 | TLS 配置 | tls.enabled=true | 启动时提示使用反向代理 |
| T8.10 | 指标路径 | api.metrics.path="/custom-metrics" | `/custom-metrics` 响应指标 |

---

## T9: 边界情况与错误处理

| # | 测试项 | 场景 | 预期行为 |
|---|--------|------|----------|
| T9.1 | 速率限制 + 认证 | 先触发限流 → 再发有效 token 请求 | 限流优先，返回 429 |
| T9.2 | 并发消息发送 | 100 个同时发送请求 | 全部返回，无 panic |
| T9.3 | WebSocket 大量事件 | 快速发布 1000 个事件 | 慢速消费者丢帧，不断连（除非 50+ 连续丢） |
| T9.4 | 空消息历史 | 查无数据的 session_key | 空列表 `{messages:[], has_more:false}` |
| T9.5 | 分页边界 | limit=0 | 默认 50 |
| T9.6 | 超长消息 | 1MB 文本 | API 拒收或截断 |
| T9.7 | 加载空插件目录 | `plugins/` 为空 | 优雅跳过 |
| T9.8 | 数据库连接丢失 | 运行中 PG 断开 | 错误日志，适配器标记为 Failed |
| T9.9 | 2 个相同 IP | 速率限制 | 共享计数器 |
| T9.10 | 配置热更新无效值 | `PUT /config` 非法值 | 保持原配置，返回 error |

---

## T10: 集成测试流水线

```bash
#!/bin/bash
set -e

echo "=== T1: 构建 ==="
cargo build --features "full,plugin-system"
cargo clippy --all-targets --features "full,plugin-system" || true

echo "=== T1.8: 单元测试 ==="
cargo test --workspace --lib --features "full,plugin-system"

echo "=== T1.9: 插件集成测试 ==="
cargo build -p mock-adapter
cargo test -p integration-tests

echo "=== T4.1: API Key 测试 ==="
cargo test -p easybot-core auth::api_key::tests

echo "=== T4.3 + T5.1: 存储测试 ==="
cargo test -p easybot-core storage::sqlite::tests
cargo test -p easybot-core storage::retention::tests

echo "=== T4.11: 速率限制测试 ==="
cargo test -p easybot-api middleware::rate_limit::tests

echo "=== T4.9: 配置加载测试 ==="
cargo test -p easybot-core config::tests

echo "=== T6: 插件加载测试 ==="
cargo test -p easybot-core plugin::

echo "=== T7: Docker 构建 ==="
docker build -t easybot .

echo "=== 全部测试完成 ==="
```

---

## 测试覆盖率总结

| 领域 | 单元测试 | 集成测试 | 手动/e2e |
|------|----------|----------|----------|
| API Key Auth | ✅ 4 tests | — | T3.2 |
| SQLite Storage | ✅ 12 tests | — | — |
| PostgreSQL | ✅ (feature-gated) | — | T5.2 |
| TTL Retention | ✅ 3 tests | — | — |
| 速率限制 | ✅ 6 tests | — | T3.3 |
| 配置加载 | ✅ 2 tests | — | T8 |
| SessionManager | ✅ 3 tests | — | T3.7 |
| 插件 Manifest | ✅ 5 tests | — | — |
| 插件加载 | ✅ 2 tests | ✅ 1 test | T6.1 |
| API Endpoints | — | — | T3.1-T3.10 |
| Docker | — | — | T7 |
| WebSocket | — | — | T3.10 |
| Webhook | — | — | T4.8 |
| 优雅关闭 | — | — | T2.3 |
| TLS | — | — | T8.9 |

**总计：39 单元测试 + 1 集成测试 + ~60 手动/e2e 验收点**

---

## 已知限制（已修复清单）

以下 6 个问题已修复 ✅：

1. ✅ **路由错误响应**：`adapter_status`/`get_session`/`delete_session` 现在返回正确的 HTTP 404 而非 200 + body error
2. ✅ **`--config` 路径处理**：ConfigManager 路径优先使用 `--config` 指定路径，否则使用默认路径
3. ✅ **gateway.local.yaml 合并**：main.rs 启动时加载基础配置后合并 local 覆盖
4. ✅ **YAML key 命名**：所有配置结构体添加 `#[serde(rename_all = "camelCase")]`，默认 YAML 可回读
5. ✅ **SQLite 回退 panic**：SQLite 连接失败时使用 `:memory:` 内存模式代替 panic
6. ✅ **plugin display_name**：`register_all` 使用真实显示名而非平台名注册

以下 2 项为功能缺失（非 bug，暂时跳过）：

7. ⬜ **事件类型未发布**：`MESSAGE_SENT`/`MESSAGE_FAILED`/`CALLBACK_RECEIVED` 等事件类型已定义但未被适配器发布
8. ⬜ **WebSocket 心跳未实现**：配置中的 `heartbeatInterval` 字段存在但 handler 未使用
