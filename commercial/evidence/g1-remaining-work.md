# G1 剩余推进项（本地不可执行，后续推进）

日期：2026-08-03 · 候选：v0.0.28

本地已完成并验证：
- 生产 Compose 部署（PostgreSQL + auth.db 配对结构）✅
- 备份/恢复演练（配对备份 + 全量丢失恢复 + 全链路验证）✅
- 回滚边界定义（二进制回滚验证推迟 v0.0.29）✅
- 容量测试（500@c10 p95 397ms / 1000@c20 p95 977ms，0 错误）✅
- 告警检测演练（审计链篡改 → 规则 FIRING）✅

## 剩余项与阻塞

| # | 项 | G1 退出证据 | 阻塞 | 前置条件 |
|---|---|---|---|---|
| 1 | 首发平台真实烟测 | 平台烟测记录 | 需要真实账号凭据 | 提供 Telegram/Discord/飞书/QQ 正式 Bot Token（`e2e-real.sh`）|
| 2 | 7 天稳定性观察 | 连续 7 天无未解释 P0/P1 | 需要时间 | 预生产实例持续运行 + 值班监控 |
| 3 | HTTPS/HSTS 端到端验证 | 部署验收 | 需要真实域名 + DNS + 证书 | 生产主机 + 域名 A/AAAA 指向 + Caddy ACME |
| 4 | 告警送达真人 | ALERT_DELIVERY_REPORT | 需要 Alertmanager + 接收端 | 决定告警接收渠道（webhook/邮箱/工单）并部署 |
| 5 | 数据库迁移演练 | DATABASE_MIGRATION_REPORT | v0.0.28 是首个基线，无存量生产数据可迁移 | 真实预生产部署时从干净 bootstrap 演练，生成符合门禁 schema 的迁移 JSON |
| 6 | 二进制回滚验证 | ROLLBACK_DRILL_REPORT | v0.0.28 有意移除向后兼容 | **v0.0.29 发布时**验证 v0.0.28↔v0.0.29 二进制回滚 |
| 7 | 生产主机部署门禁 | production-up.sh 全量预检 | 开发机无 UID 10001 宿主属主 | 生产 Linux 主机上执行完整 `production-up.sh` |

## 备注

- 告警**检测**已本地验证（Prometheus 规则 FIRING）；"送达真人"需 #4。
- 直接对运行中 DB 做外部写入会破坏应用连接池（SQLITE_NOTADB）——生产只能通过应用或备份工具访问数据库（详见 alert-detection-drill.txt 的 finding）。
