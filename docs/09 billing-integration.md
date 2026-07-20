# 财务事件与支付集成

EasyBot 不直接接收某一家支付平台的公网 Webhook。正式部署应由独立支付桥接器验证供应商签名、时间戳、证书和重放窗口，再用仅含 `billingwrite` 权限的短期 API Key 提交规范化事件。不要把供应商签名密钥交给 EasyBot，也不要把原始支付 payload 写入消息数据库或审计元数据。

## 规范化事件

```http
POST /api/v1/billing/events
Authorization: Bearer <billingwrite-only-key>
Content-Type: application/json

{
  "provider": "stripe",
  "event_id": "evt_123",
  "event_type": "payment_succeeded",
  "object_id": "pi_123",
  "customer_ref": "customer-42",
  "amount_minor": 9900,
  "currency": "USD",
  "occurred_at": 1783814400000
}
```

金额使用非负最小货币单位整数，币种是三个大写 ASCII 字母。支持 `invoice_paid`、`invoice_voided`、`payment_succeeded`、`payment_failed`、`refund_succeeded`、`chargeback_opened`、`chargeback_closed`。退款、拒付和撤销必须作为新事件提交，禁止修改历史成功事件。

`provider + event_id` 是幂等键。首次写入返回 `created`；内容完全相同的重放返回 `duplicate`；相同幂等键携带不同不可变内容返回 HTTP 409。数据库只保存规范化字段及 SHA-256 内容哈希，不保存银行卡、身份证、供应商原始 payload 或签名头。

## 财务查询与对账

仅含 `billingread` 权限的 Key 可以查询最长 366 天的事件区间：

```http
GET /api/v1/billing/events?from=1782864000000&to=1785542400000&customer_ref=customer-42&provider=stripe&limit=500&offset=0
Authorization: Bearer <billingread-only-key>
```

可选筛选包括 `customer_ref`、`provider` 和受支持的 `event_type`。结果按 `occurred_at DESC, provider, event_id` 稳定排序；单页 `limit` 为 1–1,000（默认 100）。当 `truncated=true` 时继续使用 `next_offset`，直到 false；`total_events` 是完整筛选范围的独立数据库计数，不是当前页长度。对账程序必须保存所有页面，不能仅凭第一页或把每页 `total_events` 相加。

每次导出前都会流式扫描完整财务事件账本，重新计算所有规范化事件的内容哈希，并与独立、事务性递增的事件总数锚点比较；因此字段篡改和整行删除都会令接口以 5xx 失败关闭，并写入 `billing.events.integrity_failed` 审计事件。升级旧数据库时锚点从当时账本初始化一次，之后启动和迁移不会覆盖它。校验通过后仍会再次验证当前分页，任何失败都不能用于出账。正常响应包含 `integrity_valid=true`。

每次写入尝试和每页财务读取都会进入防篡改审计链，包含筛选、offset、limit、当前页记录数和总记录数。财务事件表与用量账本都位于 `auth.db`，必须纳入同一备份批次。出账系统应把用量、定价版本、财务事件、税费和发票状态汇总到独立财务系统；EasyBot 的事件账本不替代支付平台余额、银行流水、总账或税务系统。

## 支付桥接器上线门禁

- 只接受供应商官方 HTTPS 入口并按官方文档验证签名；
- 在验证前保留原始字节，不要先反序列化再验签；
- 检查事件时间与重放窗口，使用供应商事件 ID 作为 `event_id`；
- 未识别事件类型进入隔离队列，不要静默映射为成功；
- 429/5xx 使用有上限的指数退避，409 进入人工调查，200 duplicate 视为成功；
- 演练重复通知、乱序退款、拒付、超时和密钥轮换；
- 将供应商控制台、EasyBot 财务事件、用量账本和银行/发票记录做周期性四方对账。
