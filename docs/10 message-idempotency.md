# 消息发送幂等与重试

网络超时不能证明平台没有收到消息。商业客户重试 `POST /api/v1/messages/send` 时必须使用 `Idempotency-Key`，否则 EasyBot 无法阻止重复发送。

```http
POST /api/v1/messages/send
Authorization: Bearer <messagessend-key>
Idempotency-Key: order-20260712-000123
Content-Type: application/json

{"target":"telegram:12345","text":"Your order has shipped"}
```

幂等作用域是“认证 API Key + Idempotency-Key”。Key 必须是 8–128 个安全 ASCII 字符，记录保留 24 小时：

- 首次请求原子取得 `pending` reservation，之后才调用外部平台；
- 成功结果持久化为 `completed`，相同请求重试会返回原 JSON，并带 `Idempotency-Replayed: true`；
- 首次正常响应带 `Idempotency-Replayed: false`；
- 同一幂等键用于不同请求体返回 HTTP 409；
- 并发重复或结果不确定的请求返回 HTTP 409，不会再次调用平台；
- reservation 和响应位于 `auth.db`，重启后仍可重放。

如果适配器调用超时或在外部平台返回结果前连接中断，reservation 会保持 `pending` 直至 24 小时过期。这是有意的 fail-closed：自动换新幂等键可能重复通知终端用户。调用方应保存请求 ID、幂等键和业务订单号，查询平台或运营工单完成对账后再决定是否人工重发。

同一语义也覆盖 `/messages/batch-send`。批量请求会预先拒绝空目标、非法目标、重复目标、超过 100 个目标和超长正文；整体 30 秒超时会保持 reservation 为 `pending`，即使后台平台调用结果不确定也不会允许同键重发。编辑和删除接口尚未提供同等语义，不应直接用于需要财务级“只执行一次”的工作流。用量账本仍会记录每次 HTTP 重放请求，定价规则必须明确重放流量是否计费。

## 出站投递日志

单发与批量发送在调用外部 IM 平台前，都会先向消息数据库的 `outbound_deliveries` 写入 `pending` 投递意图。意图写入失败时不会调用平台。平台返回确定结果后，投递状态和消息历史在同一数据库事务内提交为 `succeeded` 或 `failed`。同一事务还形成待发布事件，后台 outbox 发布器随后广播 `message.sent` 并确认；若进程在提交后、广播前崩溃，新进程会继续发布未确认事件。这避免了“数据库已不可写但平台仍被调用”“投递状态成功、消息历史缺失”以及“提交成功但进程崩溃导致事件永久消失”三类不一致。

Outbox 提供 at-least-once 而不是 exactly-once：广播成功、数据库确认前崩溃会导致同一事件再次出现。启动流程会先同步建立内部事件订阅，再启动 outbox；若 EventBus 报告没有任何活跃订阅者，记录不会被确认，下一轮继续尝试，避免恢复事件在启动竞态中静默消失。所有事件都包含稳定的 `delivery_id`、`platform`、`chat_id` 和 `delivery_state`，Webhook、WebSocket 或内部消费者必须按 `delivery_id` 幂等处理。不能用事件收到次数直接计费；财务计量应以持久化用量账本和唯一投递记录为准。

平台调用超时不是确定失败：对应投递保持 `pending`，批量结果返回 `indeterminate`；单发返回超时错误。系统绝不自动重发这些记录，因为下游可能已经执行。携带 `Idempotency-Key` 的单发或整批请求也会保持幂等 reservation 为 `pending`，后续同键请求返回冲突而不是重复发送。运营人员必须结合平台消息 ID、平台侧查询或客户确认人工对账；在结果确认前不得退款后自动重发。

调用方可使用含 `messagesread` 权限的同一主体 API Key 查询自己的投递记录（接口按服务端稳定主体 ID 强制隔离，不接受任意主体参数）。安全轮换产生的新 Key 继承主体 ID，因此仍能访问轮换前记录：

```http
GET /api/v1/messages/deliveries?limit=50
Authorization: Bearer <customer-key>
```

确认平台侧证据后，使用含 `messagessend` 权限且属于原调用方主体的 Key 对 `pending` 记录结案：

```http
POST /api/v1/messages/deliveries/<delivery-id>/reconcile
Authorization: Bearer <same-customer-key>
Content-Type: application/json

{
  "resolution": "succeeded",
  "evidence": "Telegram history search found platform message tg-12345"
}
```

`resolution` 只能为 `succeeded` 或 `failed`，证据必须为 20–2000 个字符。接口只允许 `pending` 到最终态的一次转换；不存在、属于其他调用方或已经终结的记录统一返回 HTTP 409，避免跨客户枚举。结案不会自动重发，也不会伪造平台消息 ID或消息历史；它会产生新的 outbox 事件，并在防篡改审计链中记录 `message.delivery.reconcile.requested` 与 `message.delivery.reconcile.completed`。证据正文保存在投递库中，不复制到审计元数据，仍应避免写入 token、密码或无关个人信息。

投递日志缩小了崩溃窗口，但跨 EasyBot 数据库与外部平台无法形成分布式原子事务，因此不能宣称 exactly-once。没有 `Idempotency-Key` 的调用方若在超时后自行重试，仍可能重复发送；正式客户 SDK 必须为每个业务操作生成并持久化唯一 Key。

## 客户端重试、超时与请求追踪

每个 HTTP 响应都包含服务端生成的 `X-Request-ID`。客户应把它与自己的业务操作 ID 一起记录，并在支持工单中提供；不得记录 Authorization、管理密码或平台 Token。EasyBot 服务端日志使用同一个 ID 记录方法、URL path（不含 query）、状态码和耗时，便于关联调查。

- `GET` 等只读请求可对网络连接失败、`429`、`502`、`503`、`504` 使用指数退避和随机抖动；存在 `Retry-After` 时至少等待该秒数，并设置最大尝试次数和总时间预算。
- `429` 可能来自 IP/全局限流或客户主体配额，均返回 `Retry-After`；主体配额还返回 `X-RateLimit-Limit`、`Remaining` 和 `Reset`。
- `POST /messages/send` 与 `/messages/batch-send` 只有在首次请求前生成并持久化 `Idempotency-Key`、且每次重试复用同一个 Key 时才可重试。超时表示平台结果不确定，不能换新 Key 重发；应查询投递记录并按对账流程结案（见上文出站投递日志）。
- 配置更新、适配器启停、Key 创建/轮换/吊销/永久删除、隐私删除、账单事件等管理写操作不得盲目自动重试。先使用 `X-Request-ID`、资源查询或审计事件确认前一次结果，再决定补偿操作。
- `500` 不代表安全可重试；客户端必须按操作是否幂等以及是否有持久化幂等键判断。不要把所有 5xx 统一重放。

客户端超时必须小于自身总业务预算，但应大于正常 p95 延迟；连接、读取和总请求超时应分别配置。服务端消息平台调用当前可能等待约 15 秒，批量调用整体最多约 30 秒。反向代理超时不能短于服务端正常处理窗口，否则会制造大量"客户端已超时、平台可能已成功"的不确定投递。
