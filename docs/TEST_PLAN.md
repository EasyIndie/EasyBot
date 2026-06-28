# EasyBot 全面验收测试计划

## 项目总览

EasyBot 是一个 **IM Gateway** 服务，连接多个 IM 平台并对外提供统一的 REST API + WebSocket。
开发阶段 P1-P5 已全部完成，以下是全功能测试计划。

> **维护说明**: 本文档为测试策略指引。各命令应当通过，预期输出可能因版本微调而略有差异。实际测试计数请运行 `cargo test --workspace` 获取。

---

## T1: 构建与编译验证

| # | 测试项 | 命令 | 预期 |
|---|--------|------|------|
| T1.1 | 默认构建 | `cargo build` | ✅ 通过 |
| T1.2 | 全特性构建 | `cargo build --features full` | ✅ 通过 |
| T1.3 | 插件系统构建 | `cargo build --features "full,plugin-system"` | ✅ 通过 |
| T1.4 | 最小构建 | `cargo build --no-default-features --features adapter-telegram` | ✅ 通过 |
| T1.5 | 无适配器构建 | `cargo build --no-default-features` | ✅ 通过 |
| T1.6 | release 构建 | `cargo build --release --features full` | ✅ 通过 |
| T1.7 | Clippy | `cargo clippy --all-targets --features full` | ✅ 无严重 lint |
| T1.8 | 单元测试 | `cargo test --workspace --lib --features "full,plugin-system"` | ✅ 全部通过 |
| T1.9 | 集成测试 | `cargo build -p mock-adapter && cargo test -p integration-tests` | ✅ 全部通过 |

---

## T2: CLI 与启动路径

### T2.1 命令行标志

| # | 测试项 | 命令 | 预期 |
|---|--------|------|------|
| T2.1.1 | 帮助 | `cargo run -- --help` | ✅ 显示选项 |
| T2.1.2 | 版本 | `cargo run -- --version` | ✅ 显示版本号 |
| T2.1.3 | --init | `cargo run -- --init --dir /tmp/easybot-test` | ✅ 创建配置目录 |
| T2.1.4 | --init 重复 | 再次执行同上 | ✅ 提示已初始化 |
| T2.1.5 | --dir | `EASYBOT_HOME= cargo run -- --dir /tmp/easybot-test --debug` | ✅ 使用指定目录 |
| T2.1.6 | --config | `cargo run -- --config <path> --debug` | ✅ 使用指定配置 |
| T2.1.7 | --debug | `cargo run -- --debug` | ✅ debug 日志 |
| T2.1.8 | 无 config | `cargo run -- --dir /tmp/easybot-empty` | ✅ 默认配置启动 |

### T2.2 路径解析

| # | 测试项 | 操作 | 预期 |
|---|--------|------|------|
| T2.2.1 | 默认路径 | 不设环境变量 | ✅ 平台标准目录 |
| T2.2.2 | EASYBOT_HOME | `EASYBOT_HOME=/custom/path cargo run -- --debug` | ✅ 使用 env 路径 |
| T2.2.3 | --dir 优先级 | `EASYBOT_HOME=/ignored cargo run -- --dir /my-dir --debug` | ✅ CLI 优先 |

### T2.3 优雅关闭

| # | 测试项 | 操作 | 预期 |
|---|--------|------|------|
| T2.3.1 | Ctrl+C | 启动后按 Ctrl+C | ✅ 优雅关闭 |
| T2.3.2 | 事件验证 | 检查日志 | ✅ `gateway.started`/`stopping` |

---

## T3: API 端点验证

> 以下测试依赖 `--debug` 模式启动（自动创建 dev API key）。使用 `curl` 或 `httpie` 测试。

### T3.1 公共端点

| # | 测试项 | 请求 | 预期 |
|---|--------|------|------|
| T3.1.1 | 健康检查 | `GET /api/v1/health` | ✅ 200 |
| T3.1.2 | 指标端点 | `GET /api/v1/metrics` | ✅ Prometheus 格式 |
| T3.1.3 | Swagger UI | `GET /swagger` | ✅ HTML 页面 |
| T3.1.4 | OpenAPI JSON | `GET /openapi.json` | ✅ JSON |

### T3.2 认证验证

| # | 测试项 | 请求 | 预期 |
|---|--------|------|------|
| T3.2.1 | 无 token | `GET /api/v1/adapters` | ✅ 401 |
| T3.2.2 | 错误 token | `Authorization: Bearer fake_key` | ✅ 401 |
| T3.2.3 | 有效 token | 使用 dev key | ✅ 200 |
| T3.2.4 | 吊销后 | 创建 → 吊销 → 使用 | ✅ 401 |

### T3.3 速率限制

| # | 测试项 | 操作 | 预期 |
|---|--------|------|------|
| T3.3.1 | 正常请求 | 10 req/s | ✅ 全部 200 |
| T3.3.2 | 超限请求 | 连续 70+ req/min | ✅ 部分 429 |
| T3.3.3 | 不同 IP | 模拟不同 IP | ✅ 各自计数 |
| T3.3.4 | 关闭限流 | `rateLimit.enabled=false` | ✅ 不限流 |

### T3.4 适配器管理

| # | 测试项 | 请求 | 预期 |
|---|--------|------|------|
| T3.4.1 | 列适配器 | `GET /api/v1/adapters` | ✅ 列表+状态 |
| T3.4.2 | 获取状态 | `GET /api/v1/adapters/telegram/status` | ✅ 状态摘要 |
| T3.4.3 | 不存在适配器 | `GET /api/v1/adapters/nonexistent/status` | ✅ 404 |
| T3.4.4 | 启动 | `POST /api/v1/adapters/telegram/start` | ✅ ok |
| T3.4.5 | 停止 | `POST /api/v1/adapters/telegram/stop` | ✅ ok |

### T3.5 消息发送

| # | 测试项 | 请求 | 预期 |
|---|--------|------|------|
| T3.5.1 | 发送消息 | `POST /api/v1/messages/send` | ✅ 200 |
| T3.5.2 | 非法 target | `target:"invalid"` | ✅ 400 |
| T3.5.3 | 批量发送 | `POST /api/v1/messages/batch-send` | ✅ 200 |
| T3.5.4 | 批量超限 | 100 targets | ✅ 并发 5 |
| T3.5.5 | 媒体发送 | `media: {url, type}` | ✅ 200 |
| T3.5.6 | 非法 media | `media.url: "invalid"` | ✅ 返回错误 |

### T3.6 消息管理

| # | 测试项 | 请求 | 预期 |
|---|--------|------|------|
| T3.6.1 | 编辑消息 | `PUT /api/v1/messages/{id}` | ✅ 200 |
| T3.6.2 | 删除消息 | `DELETE /api/v1/messages/{id}` | ✅ 200 |
| T3.6.3 | 消息历史 | `GET /api/v1/messages?session_key=...` | ✅ 列表+分页 |
| T3.6.4 | 游标分页 | `?before=TIMESTAMP` | ✅ 正确分页 |

### T3.7 会话管理

| # | 测试项 | 请求 | 预期 |
|---|--------|------|------|
| T3.7.1 | 列会话 | `GET /api/v1/sessions` | ✅ 200 |
| T3.7.2 | 获取会话 | `GET /api/v1/sessions/telegram:123` | ✅ 200 |
| T3.7.3 | 不存在 | `GET /api/v1/sessions/nonexistent` | ✅ 404 |
| T3.7.4 | 删除会话 | `DELETE /api/v1/sessions/telegram:123` | ✅ 200 |

### T3.8 聊天查询

| # | 测试项 | 请求 | 预期 |
|---|--------|------|------|
| T3.8.1 | 列聊天 | `GET /api/v1/chats/telegram` | ✅ 列表 |
| T3.8.2 | 获聊天信息 | `GET /api/v1/chats/telegram/123` | ✅ 信息 |

### T3.9 配置管理

| # | 测试项 | 请求 | 预期 |
|---|--------|------|------|
| T3.9.1 | 获取配置 | `GET /api/v1/config` | ✅ 200 |
| T3.9.2 | 更新配置 | `PUT /api/v1/config` | ✅ 合并更新 |
| T3.9.3 | 验证变更 | `GET /api/v1/config` | ✅ 反映变更 |
| T3.9.4 | 非法配置 | `PUT {"port":"string"}` | ✅ 400 |
| T3.9.5 | 断言不变 | `GET /api/v1/config` | ✅ 与原一致 |

### T3.10 WebSocket 推送

| # | 测试项 | 操作 | 预期 |
|---|--------|------|------|
| T3.10.1 | 连接 | `ws://host:port/api/v1/ws` | ✅ 升级成功 |
| T3.10.2 | 认证 | 发送 `{"token":"dev-key"}` | ✅ auth_ok |
| T3.10.3 | 错误 token | 发送 `{"token":"bad"}` | ✅ auth_failed |
| T3.10.4 | 事件推送 | 触发消息事件 | ✅ 收到事件帧 |
| T3.10.5 | 背压断开 | 慢速消费者 | ✅ 50 丢帧后断开 |

---

## T4: Core 子系统

### T4.1 API Key 管理

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.1.1 | 创建 key | 返回 `(id, raw_key)` |
| T4.1.2 | 验证 key | 返回 `AuthInfo` |
| T4.1.3 | 吊销 | 吊销后验证失败 |
| T4.1.4 | 过期 key | 返回 expired 错误 |
| T4.1.5 | 无效 key | 返回 Invalid |
| T4.1.6 | Argon2 | PHC 格式，spawn_blocking |

### T4.2 事件总线

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.2.1 | 发布/订阅 | 订阅者收到事件 |
| T4.2.2 | 多订阅者 | 全部收到 |
| T4.2.3 | 无订阅者 | 安全无操作 |
| T4.2.4 | subscribe_many | 合并流 |
| T4.2.5 | 信道容量 | 256 满丢弃旧事件 |
| T4.2.6 | 事件类型 | 12 个预定义类型 |

### T4.3 SessionManager

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.3.1 | get_or_create 新 | 返回新 Session |
| T4.3.2 | get_or_create 重复 | 返回同一 Session |
| T4.3.3 | get 存在 | 返回 Session |
| T4.3.4 | get 不存在 | 返回 None |
| T4.3.5 | delete 存在 | 返回 true |
| T4.3.6 | delete 不存在 | 返回 false |
| T4.3.7-11 | list/update/store | 过滤/排序/存储正常 |

### T4.4 SessionBridge + T4.5 MessagePersister

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.4.1 | 入站→自动创建会话 | 订阅 message.inbound |
| T4.4.2 | 重复→复用 | 相同 chat_id |
| T4.4.3 | 非法数据→跳过 | 静默处理 |

### T4.6 AdapterManager

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.6.1 | start → Connected | 生命周期流转 |
| T4.6.2-3 | 失败路径 | Failed 状态 |
| T4.6.4-5 | stop | 断开+移除 |
| T4.6.6-7 | 未连接错误 | `AdapterNotConnected` |
| T4.6.8-10 | start/stop_all | 批量正确 |

### T4.7 AdapterRegistry

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.7.1-2 | register/create | 正常创建 |
| T4.7.3 | 未注册 | 返回错误 |
| T4.7.4 | 覆盖 | 无警告 |
| T4.7.5 | list | 返回注册列表 |

### T4.8 WebhookDispatcher

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T4.8.1 | 事件转发 | HTTP POST |
| T4.8.2-3 | 过滤 | events/platforms 过滤 |
| T4.8.4 | 通配符 | `"*"` 全订阅 |
| T4.8.5 | HMAC | X-Signature-256 |
| T4.8.6-8 | 错误/超时 | warn 日志 |

### T4.9 配置管理器 + T4.10 TTL 清理 + T4.11 速率限制 + T4.12 指标

全部涵盖 CRUD 和边界条件测试（详见 `cargo test -p easybot-core`）。

---

## T5: 存储后端

### T5.1 SQLite

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T5.1.1 | 连接池 | WAL 模式 |
| T5.1.2-3 | 迁移 | 幂等 |
| T5.1.4-7 | CRUD | 会话/消息读写 |
| T5.1.8 | 并发写入 | 无丢失 |
| T5.1.9 | 文件权限 | 600/644 |

### T5.2 PostgreSQL（需 `--features integration-test` + 运行中 PG）

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T5.2.1-2 | 连接+迁移 | 同 SQLite |
| T5.2.3-4 | 会话/消息 CRUD | 同 SQLite |
| T5.2.5 | JSONB | metadata 列 |

### T5.3 TTL

| # | 测试项 | 验证方式 |
|---|--------|----------|
| T5.3.1 | SQLite | 过期清理 |
| T5.3.2 | PG | 同上 |

---

## T6: 插件系统

### T6.1 加载流程

| # | 测试项 | 操作 | 预期 |
|---|--------|------|------|
| T6.1.1 | 完整加载 | mock-adapter → plugin.yaml + .so | ✅ 加载成功 |
| T6.1.2 | 目录不存在 | plugins/ 缺失 | ✅ 优雅跳过 |
| T6.1.3 | 缺 plugin.yaml | 目录存在无 manifest | ✅ 跳过 |
| T6.1.4 | 非法 YAML | 格式错误 | ✅ 跳过 |
| T6.1.5 | 缺 .so | manifest 指向不存在文件 | ✅ 跳过 |
| T6.1.6 | 损坏 .so | 无效动态库 | ✅ 跳过 |
| T6.1.7 | ABI 版本 | 版本不匹配 | ✅ 跳过 |
| T6.1.8 | 平台冲突 | 同名冲突 | ✅ 后者跳过 |

### T6.2 生命周期 + T6.3 SDK

工厂创建 → init → connect → send → disconnect 全部可调用。`declare_plugin!` 宏生成三个 extern C 符号。

---

## T7: Docker 部署

| # | 测试项 | 命令 | 预期 |
|---|--------|------|------|
| T7.1 | 构建 | `docker build -t easybot .` | ✅ 通过 |
| T7.2 | 容器启动 | `docker run -d easybot` | ✅ 运行 |
| T7.3 | 健康检查 | `curl localhost:8080/health` | ✅ 200 |
| T7.4 | 配置挂载 | `-v ./gateway.yaml:/etc/easybot/...` | ✅ 生效 |
| T7.5 | 数据持久化 | `-v easybot_data:/var/lib/easybot/data` | ✅ 持久化 |
| T7.6 | docker-compose | `docker-compose up -d` | ✅ 启动 |
| T7.7 | 环境变量 | `TELEGRAM_BOT_TOKEN=xxx docker-compose up` | ✅ 传递 |
| T7.8 | Prometheus | `--profile monitoring up` | ✅ 抓取 |
| T7.9 | PostgreSQL | `--profile postgres up` | ✅ PG 健康 |

---

## T8: 配置与环境变量

| # | 测试项 | 操作 | 预期 |
|---|--------|------|------|
| T8.1 | 变量替换 | `${MY_VAR}` → "hello" | ✅ 替换 |
| T8.2 | 变量未设 | `${MISSING}` | ✅ 空+ warning |
| T8.3 | --init 生成 | 完整 YAML | ✅ 生成 |
| T8.4 | 默认值 | 空配置启动 | ✅ 全默认 |
| T8.5-7 | storage type | sqlite/postgres/unsupported | ✅ 正确 |
| T8.8 | adapter.enabled | false | ✅ 不启动 |
| T8.9 | TLS | tls.enabled=true | ✅ 提示反向代理 |
| T8.10 | 指标路径 | api.metrics.path | ✅ 正确 |

---

## T9: 边界情况与错误处理

| # | 测试项 | 场景 | 预期 |
|---|--------|------|------|
| T9.1 | 限流+认证 | 限流中发有效请求 | ✅ 429 优先 |
| T9.2 | 并发发送 | 100 同时请求 | ✅ 无 panic |
| T9.3 | WS 大事件 | 快速 1000 事件 | ✅ 丢帧不断连 |
| T9.4 | 空历史 | 无数据 session_key | ✅ 空列表 |
| T9.5 | 分页边界 | limit=0 | ✅ 默认 50 |
| T9.6 | 超长消息 | 1MB 文本 | ✅ 拒收/截断 |
| T9.7 | 空插件目录 | plugins/ 空 | ✅ 优雅跳过 |
| T9.8 | 数据库断连 | 运行中断开 | ✅ 错误日志 |
| T9.9 | 相同 IP | 速率限制 | ✅ 共享计数 |
| T9.10 | 配置无效 | `PUT` 非法值 | ✅ 保持原值 |

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

echo "=== T4.x: 核心测试 ==="
cargo test -p easybot-core auth::api_key::tests
cargo test -p easybot-core storage::sqlite::tests
cargo test -p easybot-core storage::retention::tests
cargo test -p easybot-api middleware::rate_limit::tests
cargo test -p easybot-core config::

echo "=== T6: 插件加载测试 ==="
cargo test --workspace --lib --features "full,plugin-system" plugin:: ffi::

echo "=== T7: Docker 构建 ==="
docker build -t easybot .

echo "=== 全部测试完成 ==="
```

---

## 测试覆盖率总结

| 领域 | 单元测试 | 集成测试 | 手动/e2e |
|------|----------|----------|----------|
| API Key Auth | ✅ | — | T3.2 |
| SQLite Storage | ✅ | — | — |
| PostgreSQL | ✅ (feature-gated) | — | T5.2 |
| TTL Retention | ✅ | — | — |
| 速率限制 | ✅ | — | T3.3 |
| 配置加载 | ✅ | — | T8 |
| SessionManager | ✅ | — | T3.7 |
| 插件 Manifest | ✅ | ✅ | T6.1 |
| API Endpoints | — | — | T3.1-T3.10 |
| Docker | — | — | T7 |
| WebSocket | — | — | T3.10 |
| Webhook | — | — | T4.8 |
| 优雅关闭 | — | — | T2.3 |
| TLS | — | — | T8.9 |
