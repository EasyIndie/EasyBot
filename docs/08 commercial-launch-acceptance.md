# 商用上线证据验收

代码测试、发布签名和生产配置门禁都不能证明某个真实服务已经具备对外售卖条件。正式开放域名前，运营方必须为经营主体、法律文本、数据治理、支持渠道、计费方式和生产演练提供当前证据。

## 准备证据

复制 `commercial/evidence.env.example` 到版本控制之外。配置只支持字面量 `KEY=value`，不会执行 shell 展开。以下材料必须非空且在 `MAX_EVIDENCE_AGE_DAYS` 内更新：

- 法务批准记录及经营主体；
- 对应发布版本的 `commercial-release` Environment 审批记录；
- 数据处理清单；
- 安全事件响应手册；
- 真实生产介质的备份恢复演练；
- 与实际 `STORAGE_BACKEND` 一致的已有数据库升级演练 JSON，必须包含 `passed=true`、`backend`、起止版本、迁移耗时、数据校验和回滚结果；
- 与生产规格一致的容量测试 JSON，且 `passed=true`；
- 告警实际送达值班人员的记录；
- 版本回滚演练记录；
- 已公开的服务条款、隐私政策和 DPA；
- 支持、安全联系人及实际计费模式。

证据可能包含内部信息，不应提交 Git。建议放在受访问控制、保留版本历史的合规文档或工单系统中，并在配置文件里引用导出的短期副本。

## 执行门禁

先在隔离环境检查证据结构：

```bash
scripts/commercial-launch-gate.sh --offline /secure/easybot-evidence.env launch-offline.json
```

切流前执行在线验收：

```bash
scripts/commercial-launch-gate.sh /secure/easybot-evidence.env launch-online.json
```

在线模式要求服务域名和法律页面使用 HTTPS，`/api/v1/live`、`/api/v1/ready` 返回成功，并且 TLS 代理发送 `Strict-Transport-Security`。生成报告的权限为 `0600`。只有在线报告 `passed=true` 才允许开放公网流量；每次重大版本、基础设施迁移、经营主体或法律文本变化后都必须重新验收。

门禁要求安装 `jq`，所有 JSON 报告按顶层字段和值类型解析，不使用文本搜索判定通过。`STORAGE_BACKEND` 只能为 `sqlite` 或 `postgres`，且迁移报告中的 `backend` 必须完全匹配。迁移 JSON 必须包含有效语义版本 `from_version`、与当前 Cargo 发布版本完全一致的 `to_version`、正整数 `duration_ms`、布尔值 `row_counts_verified=true` 和 `rollback_passed=true`；字符串 `"true"`、嵌套通过标志、旧版本报告、未核对数据或未通过回滚均不能复用。演练应从当前生产版本的脱敏备份恢复开始，而不是只对空数据库执行建表；完成后至少核对 API Key 主体、配额窗口、用量主体归属、财务事件、投递日志、消息和会话数量，并实际执行回滚。脚本验证证据存在性、时效和服务可达性，不判断合同内容是否符合法律，也不替代真实支付、退款、开票或客户支持流程的人工验收。
