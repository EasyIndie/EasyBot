# EasyBot 商用上线门禁

EasyBot 的生产模式用于阻止开发配置意外上线。启动方式：

```bash
easybot --production --dir /var/lib/easybot
# 或
EASYBOT_ENV=production easybot --dir /var/lib/easybot
```

生产模式会一次性检查并报告所有错误：

- 必须在可信 TLS 反向代理后运行，并显式设置 `EASYBOT_ALLOW_PLAINTEXT=true` 确认代理到应用的私网 HTTP 跳转；
- 管理密码不少于 12 个字符；
- API 限流已启用且配额大于零；
- 禁止透传平台原始 payload；
- CORS 必须使用明确的 origin，禁止 `*`、`null` 和非本地 HTTP origin；
- 所有 Webhook 必须使用 HTTPS；
- 拒绝无实际终止能力的 `server.tls.enabled` 配置；
- 禁止 `EASYBOT_DEBUG_CORS`、`--debug` 和非空动态插件目录。

## 推荐生产拓扑

公网流量必须先进入支持自动证书续期的反向代理或云负载均衡，再转发到仅在私网监听的 EasyBot。`server.tls.enabled` 当前不实现 TLS 终止，不能替代反向代理，也不得据此把 EasyBot 端口直接暴露公网。PostgreSQL 和 Prometheus 不应直接暴露公网。每个客户应使用独立 EasyBot 实例与独立数据库；当前版本尚未提供进程内租户级数据隔离，不能把多个互不信任客户放在同一实例中。

参考 `docker-compose.yml` 默认将 EasyBot 和 Prometheus 绑定到宿主机 `127.0.0.1`。宿主机反向代理可转发到 `127.0.0.1:8080`；若反向代理也是 Compose 服务，应加入 `frontend` 网络并直接转发到 `http://easybot:8080`，同时删除 EasyBot 的 `ports` 映射。不要把 `EASYBOT_BIND_ADDRESS` 改为 `0.0.0.0` 来代替正确的代理或防火墙配置。

仓库提供固定 digest 的 Caddy 生产入口模板。先让域名 A/AAAA 记录指向主机，开放 TCP 80/443 与 UDP 443，并设置生产门禁所需变量，再启动：

```bash
EASYBOT_DOMAIN=gateway.example.com \
EASYBOT_ENV=production \
EASYBOT_ALLOW_PLAINTEXT=true \
docker compose --profile production-tls up -d
```

Caddy 自动申请和续期证书，通过 `frontend` 私网转发到 `easybot:8080`，并发送 HSTS、`nosniff` 和 Referrer-Policy。`EASYBOT_DOMAIN` 不得保留默认的 `gateway.invalid`；首次签发前应检查 DNS、80/443 入站规则和 CA 速率限制。若使用云负载均衡替代 Caddy，也必须保留相同的 HTTPS、HSTS、WebSocket 转发和 `/api/v1/live` 上游探测契约。

Caddy 访问日志删除完整 URI 和请求头，并把 IPv4/IPv6 客户地址分别掩码到 `/24`、`/48`，避免未知 query、Cookie 或凭据进入代理日志。EasyBot 应用日志只记录 URL path（不含 query）、状态、耗时和服务端请求 ID，足以进行接口级排障。若运营方为流量分析重新增加 URI、请求头或完整 IP，必须先完成隐私影响评估、字段白名单、访问控制和短期保留策略，不能直接恢复默认全量日志。

真正发布生产服务时不要使用开发 Compose 的 `build: .`。EasyBot 镜像固定以 UID/GID `10001` 运行；准备由 UID `10001` 持有、权限为 `0400` 或 `0600` 的 `admin_password`、`database_url` 文件，并让密钥目录同样由 UID `10001` 持有且权限为 `0500` 或 `0700`。目录和文件均不得为符号链接。先按发布记录取得并验证镜像 digest，再使用独立生产清单：

```bash
export EASYBOT_IMAGE='ghcr.io/easyindie/easybot@sha256:<64位digest>'
export GITHUB_REPOSITORY='EasyIndie/EasyBot'
export EASYBOT_DOMAIN='gateway.example.com'
export EASYBOT_SECRETS_DIR='/secure/easybot-secrets'
export EASYBOT_DATA_DIR='/var/lib/easybot/data'
scripts/production-up.sh --check
scripts/production-up.sh
```

脚本先验证镜像 provenance、SPDX SBOM、发布工作流身份和不可变 digest，再校验生产 Compose，最后才执行拉取和启动。生产清单没有 `build` 路径，因此无法在部署机静默回退到未经证明的本地镜像。

生产清单同时限制 EasyBot/Caddy 的 CPU、内存和 PID 数量，JSON 容器日志按大小轮转，并给 EasyBot 45 秒优雅停机窗口。这些值是安全起点，不是容量承诺；压力测试若证明需要调整，应在受审阅的配置变更中同时更新宿主机容量、告警阈值和演练报告，不能临时取消资源上限。

生产数据使用显式的 `EASYBOT_DATA_DIR` 宿主机路径，便于受控备份而不是依赖 Docker 内部卷名。`production-backup.sh` 在同一个临时批次目录内备份 PostgreSQL 与本地 `auth.db`，逐一验证后把归档内的 `auth_schema_version` 写入清单，为清单生成独立 SHA-256 摘要，再写 `COMPLETE` 标志并原子重命名；恢复会先验证清单摘要，随后在触碰 PostgreSQL 前重新读取 SQLite 归档版本并与清单比对。失败批次会被清理，异地同步任务只能复制带 `COMPLETE` 的最终目录：

本地 SQLite 连接池对每一条文件连接强制启用 WAL、5 秒 busy timeout、外键约束和 `synchronous=FULL`，不能依赖只作用于随机池连接的启动后 PRAGMA。数据库路径拒绝符号链接和非普通文件；Unix 上新主库在连接前以原子 `create_new` 和 `0600` 创建，已有主库也先收紧权限，因此随后产生的 WAL/SHM 从第一笔事务起继承私有权限。任一连接或权限配置失败都会阻止启动，而不是静默降级。`FULL` 会增加写入延迟，容量测试必须覆盖认证、配额和用量计量的目标并发，不能为了跑分把财务/凭据数据库改回 `NORMAL`。

增量迁移在单个 `BEGIN IMMEDIATE` 事务内完成：先通过 `pragma_table_info` 检查目标列，仅在缺失时执行 `ALTER TABLE`，回填旧数据后再创建依赖新列的完整 schema；任一步失败会回滚此前的 DDL 和数据变更。不得用忽略全部错误的方式容忍重复列；只读介质、磁盘故障、锁超时或非法 schema 都必须使迁移失败并阻止服务启动。迁移可重复执行，升级演练需要同时验证旧数据回填、失败回滚和第二次运行幂等。

成功迁移把 SQLite `PRAGMA user_version` 设置为当前 schema 版本 `1`。启动时若数据库版本高于二进制支持版本，会在任何 DDL 前失败关闭，防止版本回滚时旧程序静默修改未来 schema。应用语义版本与 schema 版本是不同维度：迁移演练报告仍记录发布版本，备份/恢复验收还应记录 `user_version`；降级前必须使用与目标二进制兼容的数据库备份，不能只回滚容器镜像。

```bash
EASYBOT_DATA_DIR=/var/lib/easybot/data \
EASYBOT_SECRETS_DIR=/secure/easybot-secrets \
scripts/production-backup.sh /secure/easybot-backups
```

成对恢复会先验证 `COMPLETE`、清单摘要、清单字段、两份归档校验和及归档结构，并在任何覆盖前自动把当前 PostgreSQL 与 `auth.db` 写入另一个安全备份根目录。摘要用于发现损坏或未同步完整的批次；具备备份目录写权限的攻击者仍可重算摘要，因此异地备份存储还必须启用不可变保留或对象锁。必须先停止 EasyBot，再显式确认并执行：

```bash
export EASYBOT_DATA_DIR=/var/lib/easybot/data
export EASYBOT_SECRETS_DIR=/secure/easybot-secrets
export EASYBOT_CONFIRM_STOPPED=yes
scripts/production-restore.sh \
  /secure/easybot-backups/easybot-20260713T120000Z \
  /secure/easybot-pre-restore-backups \
  --force
```

脚本完成后仍保持服务停止状态；先核对数据库版本、主体/Key 数量、审计链、账本汇总和待对账投递，再由变更负责人启动服务。若 PostgreSQL 恢复后后续步骤失败，使用自动创建的恢复前批次回退两部分数据，禁止只启动其中一份已恢复的数据。

保留策略只识别带 `COMPLETE` 的标准批次目录，并始终保留最近指定数量，即使它们已超过时间窗口。先审阅 dry-run，再显式确认删除；以下示例保留至少 7 个完整批次，并清理其余超过 90 天的批次：

```bash
scripts/production-backup-retention.sh /secure/easybot-backups 90 7 --dry-run
EASYBOT_CONFIRM_BACKUP_DELETE=yes \
  scripts/production-backup-retention.sh /secure/easybot-backups 90 7 --apply
```

保留清理只能在异地复制成功且对象存储侧已验证后运行。异地目标必须启用服务端加密、版本控制或对象锁、独立删除权限和生命周期策略；本地脚本不会把“已生成备份”冒充为“已完成异地灾备”。

动态插件当前通过进程内 `dlopen` 执行，尚无插件签名、发布者信任或沙箱。生产门禁会拒绝非空 `plugins/`；需要扩展时，应先把扩展部署为独立隔离服务，或建立完整插件签名供应链后再修改门禁，不能用环境变量直接跳过。

生产凭据优先使用文件注入。管理密码、数据库连接串以及 Telegram、Discord、飞书、QQ 的凭据均支持对应的 `<变量名>_FILE`，例如 `EASYBOT_ADMIN_PASSWORD_FILE=/run/secrets/admin_password`。文件必须是普通文件、不能是符号链接、不能授予 group/other 任何权限，内容上限为 64 KiB；同一个凭据不得同时设置值变量和 `_FILE`，否则启动失败。这样可避免原始密钥出现在 `docker inspect` 的容器环境中。

## 健康探针与优雅停机

- `GET /api/v1/live` 只表示进程事件循环仍可响应，始终不依赖外部 IM 平台；容器 liveness 使用该端点。
- `GET /api/v1/ready` 分别检查认证/计费存储（`storage`）、消息历史存储（`message_storage`）、持久化配额（`quota`）、Key 轮换过渡查询（`key_rotations`）和投递日志查询（`delivery_journal`），并返回运行中 `auth_schema_version`；版本无法读取时 readiness 返回 503。负载均衡器使用该端点，HTTP 503 时停止转发新流量。最近一次配额事务失败会令 `quota=unavailable`，后续成功消费会自动恢复。存在未完成轮换时 `key_rotations=attention_required` 并返回 `pending_key_rotations`，不摘除仍可服务的实例，但值班人员必须按 `api_key_rotation_transitions` 的 `created`/`prepared` 状态和审计记录完成恢复；过渡表不可查询则 readiness 返回 503。投递日志可查询但存在超过五分钟的 pending 或未发布事件时，`delivery_journal=attention_required` 并返回积压数量，但不会仅因此摘除健康实例；由告警触发人工对账。消息存储探针验证查询可用性；磁盘写满仍须通过告警和故障演练覆盖。
- `GET /api/v1/health` 是面向人工诊断的摘要，不应作为自动重启依据。

EasyBot 同时处理交互式 `SIGINT` 与容器/systemd 常用的 `SIGTERM`。收到信号后先停止接收新 HTTP 连接并等待在途请求，再断开适配器。编排平台的 termination grace period 应覆盖最长 API 请求时间和下游平台超时；建议从 30 秒开始，通过故障演练调整。

入站消息批量写库每轮最多重试三次；若数据库仍不可用，失败批次会保留在内存队列头部并在下一轮继续尝试，不会因一次短暂故障直接丢弃，数据库恢复后仍保持接收顺序写入。该队列不是磁盘 WAL：进程崩溃或主机掉电仍可能丢失尚未写库的消息，持续数据库故障也会增加内存占用。因此正式部署必须让 `message_storage=unavailable` 触发停止流量与最高优先级告警，并结合平台侧 webhook 重投/事件补拉能力制定恢复流程；不能把内存重试当作零数据丢失承诺。

## 容量与 SLA 验收

不要直接把开发机结果写入合同。先使用与生产一致的 CPU、内存、数据库、代理和网络，在隔离环境签发最小权限测试 Key，再运行受保护 API 容量门禁：

```bash
install -m 600 /dev/null ./secrets/load-test-token
printf '%s' 'eb_REPLACE_ME' > ./secrets/load-test-token
MAX_ERROR_RATE=0.1 MAX_P95_MS=500 REPORT_FILE=load-report.json \
  scripts/load-test.sh https://gateway.example.com/api/v1/adapters \
  ./secrets/load-test-token 10000 100
```

脚本拒绝公网 HTTP、要求 token 文件权限为 `0600`，并按非 2xx 错误率和 p95 延迟返回成功或失败。每次正式发布至少保存请求数、并发数、实例规格、配置、数据规模、版本、时间窗口和 JSON 报告。压测会写入真实用量账本，应使用专用 Key 并在出账时排除。SLA 还必须单独演练 SIGTERM 滚动更新、数据库不可用、平台 API 超时、磁盘写满、备份恢复和版本回滚；单次吞吐测试不能证明可用性承诺。

## 备份与恢复

仓库提供 `scripts/easybot-backup.sh`，所有归档均使用 `0600` 权限并生成同名 `.sha256` 校验文件。备份必须复制到与服务主机故障域不同、启用静态加密和版本保留的存储。

SQLite 支持服务运行期间的一致性在线备份；不要直接复制 WAL 数据库文件：

```bash
scripts/easybot-backup.sh backup sqlite /var/lib/easybot/data/gateway.db /secure/backups
scripts/easybot-backup.sh verify /secure/backups/easybot-20260711T120000Z.sqlite3

# 恢复前必须先停止 EasyBot；--force 会一并保留旧 WAL/SHM 文件
scripts/easybot-backup.sh restore sqlite BACKUP.sqlite3 /var/lib/easybot/data/gateway.db --force
```

覆盖恢复会先将现有数据库保留为带时间戳的 `.pre-restore.*` 文件，并通过临时文件原子替换。确认业务数据后再按数据保留策略删除旧文件。

PostgreSQL 使用标准 custom-format 归档。凭据只从 `DATABASE_URL` 环境变量读取：

```bash
export DATABASE_URL='postgresql://easybot:***@db/easybot'
scripts/easybot-backup.sh backup postgres /secure/backups
scripts/easybot-backup.sh verify /secure/backups/easybot-20260711T120000Z.dump
scripts/easybot-backup.sh restore postgres BACKUP.dump --force
```

使用 PostgreSQL 作为消息/会话存储时，API Key 哈希、权限和套餐配额保存在 EasyBot 数据目录的 `auth.db`，以保证服务重启后不会丢失调用方凭据。灾备任务必须同时备份两部分：

```bash
scripts/easybot-backup.sh backup postgres /secure/backups
scripts/easybot-backup.sh backup sqlite /var/lib/easybot/data/auth.db /secure/backups
```

恢复 PostgreSQL 服务时也必须恢复配套的 `auth.db`，且两份备份应来自同一备份批次。不要在多个 EasyBot 副本之间通过共享文件系统并发挂载同一个 `auth.db`。

生产环境至少每日自动备份，并定期在隔离数据库进行恢复演练。`scripts/verify.sh` 已包含 SQLite 备份、篡改检测和恢复测试，但它不能替代真实备份介质与 PostgreSQL 的定期演练。

## 用量计量与告警

API Key 固定使用 `eb_` 加 32 位十六进制随机值。服务重启后不会持有原始密钥或其快速 SHA-256 索引；认证先严格检查格式，再使用公开随机前缀缩小持久化候选集，最终仍必须通过 Argon2 验证。这样历史 Key 数量增长不会让每次请求对所有哈希执行 Argon2；前缀不是秘密，也不能单独用于认证。

生产启动只把未吊销且未过期的持久化 Key 加入认证候选内存；历史 Key 仍保留在 SQLite，并由管理列表直接查询。`GET /api/v1/api-keys` 按 `created_at DESC, id` 稳定排序，`limit` 必须为 1–1,000（默认 100），`offset` 最大 10,000,000；客户端按页读取，不能假定一次响应包含全部历史。100 个 Key 的签发上限只计算当前有效凭据，计数与插入在 `BEGIN IMMEDIATE` 写事务中完成，因此并发签发不会超卖。满额时保留唯一的第 101 个轮换过渡槽，使“先持久化替换 Key、再写 prepared 审计、最后吊销旧 Key”的安全顺序仍可执行；普通创建不能使用该槽，槽被占用时其他轮换失败并重试，旧 Key 吊销后有效数量恢复为 100。替换 Key 与 `api_key_rotation_transitions(created)` 在同一事务写入，prepared 审计成功后推进为 `prepared`，旧 Key 吊销后清理；进程在任一窗口崩溃时，恢复工具都能用 source/replacement ID 和状态判断是撤销未准备的替换 Key，还是继续已准备的轮换，而不必猜测第 101 个 Key 的来源。正常轮换也不会因保留审计历史而最终耗尽名额。创建后的回读和轮换使用主键单条查询；列表、轮换、吊销或永久删除读取数据库失败时以 5xx 失败关闭，不能把不完整的内存视图当作权威状态。

值班人员使用 `GET /api/v1/api-keys/rotations` 查看遗留过渡。对 `created` 状态向 `/api/v1/api-keys/rotations/{source_id}/{replacement_id}/reconcile` 提交 `{"action":"cancel"}`，系统在单个写事务中吊销替换 Key并删除过渡；对 `prepared` 提交 `{"action":"complete"}`，系统原子吊销原 Key并删除过渡。状态与动作不匹配、目标 Key 已非 active 或记录不存在都会失败关闭。恢复请求和完成结果均写入管理审计，调用方必须持有 `apikeysmanage` 权限。

每次 API Key 认证成功后，EasyBot 会更新该 Key 的 `last_used_at`；为避免高流量下每次请求都写数据库，持久化时间最多延迟一分钟。Prometheus 提供以下运营指标：

- `api_key_requests_total{key_id,status_class}`：按内部 Key ID 和 HTTP 状态类别统计请求；
- `api_auth_failures_total{reason}`：按固定原因统计认证失败；
- `http_requests_total`、`http_request_duration_seconds`：全局请求量和延迟；
- `messages_inbound_total`、`messages_outbound_total`：按平台统计消息；
- `adapter_status`：适配器连接状态。
- `outbound_delivery_backlog{state}`：`pending`、超过五分钟的 `stale_pending` 和未确认的 `unpublished_event` 持久化积压。
- `audit_chain_integrity`：管理审计链完整性，`1` 为有效、`0` 为损坏或无法验证；
- `ledger_integrity{ledger="usage|billing_events"}`：用量及支付事件账本完整性，首次抓取立即检查，之后最多每五分钟执行一次全账本校验。
- `pending_key_rotation_transitions`：等待值班人员取消或完成的持久化 Key 轮换数量；`-1` 表示过渡表不可查询，同样触发告警。

示例 PromQL：

```promql
# 各调用方过去 30 天请求量
sum by (key_id) (increase(api_key_requests_total[30d]))

# 各调用方过去一小时 4xx/5xx 数量
sum by (key_id, status_class) (
  increase(api_key_requests_total{status_class=~"4xx|5xx"}[1h])
)
```

指标只使用服务端生成的 Key ID，不包含原始 API Key 或调用方可控名称。Prometheus 指标会随进程重启重置，必须由具备持久化和保留策略的外部 Prometheus 抓取；这些指标适合运营、配额预警和账单对账，不应单独作为不可抵赖的财务账本。

创建仅包含 `metricsread` 权限的 API Key，将原始 token 写入权限为 `0600` 的文件，再设置 `EASYBOT_METRICS_TOKEN_FILE` 后启动 monitoring profile：

```bash
install -m 600 /dev/null ./secrets/easybot-metrics-token
printf '%s' 'eb_REPLACE_ME' > ./secrets/easybot-metrics-token
EASYBOT_METRICS_TOKEN_FILE=./secrets/easybot-metrics-token \
  docker compose --profile monitoring up -d
```

`easybot-alerts.yml` 默认覆盖服务不可用、5xx 比例过高、认证失败激增、适配器断连、审计链损坏、用量/支付账本损坏、Key 轮换遗留超过五分钟、长期不确定投递和事件 outbox 堵塞。账本完整性校验为全局扫描，指标端点以五分钟缓存限制数据库开销；大型账本仍应在容量验收中测量校验耗时。Prometheus 只负责计算告警；正式上线时还必须连接 Alertmanager 或云监控通知渠道，并验证值班人员实际收到测试告警。

## 调用方配额

创建 API Key 时可设置 `requests_per_minute`，范围为 1–1,000,000。该配额独立于全局/IP 限流，按服务端生成的稳定主体 ID 隔离；凭据轮换不会创建新的额度窗口：

```json
POST /api/v1/api-keys
{
  "name": "starter-customer",
  "permissions": ["messagesread", "messagessend"],
  "event_filters": [],
  "requests_per_minute": 600
}
```

成功响应包含：

- `X-RateLimit-Limit`：该调用方主体的分钟配额；
- `X-RateLimit-Remaining`：当前滑动窗口剩余请求；
- `X-RateLimit-Reset`：窗口重置估算秒数。

超额返回 HTTP 429、`Retry-After` 及同一组配额响应头。省略或传入 `null` 表示不设置主体级配额，但仍受全局/IP 限流约束。配额配置和最近 60 秒消费事件均持久化在 `auth.db`；每次判断使用 `BEGIN IMMEDIATE` 事务完成过期清理、计数和写入，因此并发请求不会超卖，Key 轮换和服务重启也不会清空当前窗口。配额存储失败时请求以 503 失败关闭，不能绕过额度继续进入业务处理器。

客户 Key 应设置 `expires_at`（UTC Unix 毫秒），允许范围为签发后 5 分钟至 366 天。到期前使用管理 Key 轮换；轮换会复制权限、事件过滤器和分钟配额，持久化新 Key 后立即吊销旧 Key，原始新 token 同样只返回一次。持久化 Key 的 Argon2 可能与吊销并发执行；验证结果进入快速缓存前会在生命周期锁内重新读取当前状态，已吊销或已删除的凭据不能被旧验证结果重新激活：

```http
POST /api/v1/api-keys/<old-key-id>/rotate
Authorization: Bearer <apikeysmanage-key>
Content-Type: application/json

{"expires_at":1817078400000}
```

禁止用某个 Key 自己轮换自己，避免响应传输失败时锁死管理面。轮换先持久化新 Key，再写入 `api_key.rotation.prepared` 审计门禁，之后才吊销旧 Key；准备审计失败时旧 Key 保持有效并清理新 Key。旧 Key 吊销后会补写 `api_key.rotated`，该完成事件写入失败时仍返回新 Key，因为 prepared 事件与数据库中的两代 Key 状态足以重建结果，不能在旧 Key 已失效后再吊销唯一可用的新 Key。值班系统必须对 completion 审计错误日志告警并补充事件说明。客户端应先安全保存新 token，再验证一次最小权限请求，最后删除旧 token。生产管理后台不再读取或写入 `.dev_api_key`：每次密码登录签发一小时、仅存在内存的管理会话 Key；升级自旧版本后应人工删除遗留文件。

每个首次签发的 Key 同时获得服务端稳定的调用方主体 ID；轮换只更换凭据 ID 和密钥，主体 ID 保持不变。调用方配额、投递日志归属和消息发送幂等命名空间使用主体 ID，因此替换 Key 不能重置分钟额度，仍可查询、结案轮换前的不确定投递，并且相同 `Idempotency-Key` 不会因轮换而失去保护。用量账本和管理审计仍记录实际凭据 ID，以便追踪是哪一代 Key 发起调用。旧数据库升级时，现有 Key 的主体 ID 回填为其当前 Key ID。

配置、日志和系统信息使用不同的最小权限：`configread`、`logsread`、`systemread`。日志可能包含客户标识和平台错误上下文，不应授予普通调用方；EasyBot 会对常见 Bearer token、`eb_` Key、凭据赋值和 URL Basic Auth 做二次脱敏，但这不能替代限制日志访问与上游错误内容治理。配置 API 会递归隐藏 token、secret、password、API Key、数据库连接字符串等字段，并从 URL 中移除用户信息、查询参数和 fragment。

管理后台不会把管理员或新创建的客户 Key 写入 localStorage/sessionStorage，页面刷新后必须重新登录；API、管理后台、Swagger/OpenAPI 响应均发送 `Cache-Control: no-store`。所有响应包含服务端生成的 `X-Request-ID`，客户支持工单应保存该值但不得保存 Authorization 头。后台渲染平台消息、会话和适配器字段前必须进行 HTML 转义，发布门禁中的 `test-admin-xss.sh` 会阻止已知未转义插值和浏览器凭据持久化模式重新出现。

当前窗口保存在单实例的 `auth.db`，适用于文档推荐的单客户独立实例部署。不要让多个 EasyBot 副本通过共享文件系统并发挂载同一 SQLite 文件；若同一客户需要横向运行多个副本，必须在外部 API Gateway/Redis 中实施共享配额，不能把各副本的独立窗口相加后当作严格套餐上限。

## 持久化用量账本

Prometheus 指标不是财务账本。EasyBot 会把每个已认证 HTTP 请求按 API Key、UTC 小时和最终 HTTP 状态类别原子累计到 `api_usage_hourly`；同一事务还会更新独立完整性镜像和累计请求总数锚点。该表位于保存 API Key 的 `auth.db`，会随其备份和恢复。配额拒绝产生的 429 也会计入账本，未通过认证的请求不会归属任何客户。

每次 WebSocket 应用层认证成功也会以 HTTP 状态类别 `1` 记录一次，计量单位是“已认证连接”，不是推送帧数。套餐和合同必须明确 HTTP 请求、WebSocket 连接以及消息发送是否分别计费，不能在出账时临时改变口径。

使用仅含 `billingread` 权限的 Key 导出最长 366 天的对账区间。按稳定主体 ID 筛选可一次覆盖轮换前后的所有凭据：

```http
GET /api/v1/usage?from=1782864000000&to=1785542400000&subject_id=<stable-subject-id>&limit=500&offset=0
Authorization: Bearer <billingread-only-key>
```

`from` 为包含边界、`to` 为排除边界，均为 UTC Unix 毫秒。`subject_id` 与 `key_id` 互斥；前者用于客户账单汇总，后者用于调查某一代凭据。明细必须分页读取，`limit` 范围 1–1,000（默认 500）；当 `truncated=true` 时使用 `next_offset` 继续，直到 false。`page_requests` 仅为当前页合计，`total_requests` 始终由数据库对完整筛选范围单独聚合，不能把各页重复返回的 `total_requests` 相加。每条记录同时返回稳定 `subject_id` 和实际 `key_id`，并按小时与状态类别提供 `request_count`。主体 ID 在写入用量时直接固化到记录中，即使旧 Key 后续被清理也不会丢失轮换归属。每次导出都会先比较完整聚合表、完整性镜像和累计请求锚点；任一行被删除、字段或计数被修改时以 5xx 失败关闭，并写入 `billing.usage.integrity_failed` 审计事件。校验通过后每页导出都会写入 `billing.usage.exported`。账单系统应保存导出区间、所有分页响应、主体 ID、纳入的 Key ID、定价版本、税费、抵扣、生成时间和不可变发票编号；EasyBot 只提供可复核用量，不负责支付、退款或税务计算。

账本写入失败会产生 `failed to persist authenticated API usage` 错误日志，并把 metering readiness 标记为不可用。后续已认证 HTTP 请求会在进入业务处理器前失败，WebSocket 不会返回 `auth_ok`，`/api/v1/ready` 返回 503；readiness 通过一次真实的事务性账本写入/删除探针确认恢复，而不是只执行 `SELECT 1`。正式环境必须对该状态建立最高优先级告警，并在故障期间暂停自动出账，使用访问日志与客户侧记录人工对账。同一客户多副本部署时，各实例账本必须汇总到外部计费系统后再出账。

## 管理审计账本

EasyBot 将关键管理操作写入 `audit_events`，当前覆盖管理员登录成功/失败、API Key 创建/吊销/永久删除、配置热更新，以及适配器启动/停止请求。事件包含内部操作者 ID、动作、资源、时间和经过筛选的元数据，不保存管理密码或原始 API Key。

审计事件按顺序组成 SHA-256 哈希链。SQLite 另存一个事务性更新的链头与事件总数锚点，因此中间记录、最新记录或整张事件表被删除都会令校验失败；升级旧数据库时锚点会从当时的现有账本初始化一次，之后迁移不会覆盖它。读取需要独立 `auditread` 权限：

```http
GET /api/v1/audit-events?limit=100
Authorization: Bearer <auditread-only-key>
```

`limit` 必须在 1–1,000 之间。成功响应中的 `integrity_valid=true` 表示从创世记录到当前记录的整条链连续且内容未变化；完整性检查会从 SQLite 流式扫描完整账本，生产进程只常驻保存当前链头，不会让内存随审计历史无限增长。链校验或数据库查询失败时接口以 5xx 失败关闭并记录错误，审计导出任务必须停止，不能消费部分事件或把损坏账本标记为成功。

哈希链能检测数据库内事件被修改、删除或重排，但无法阻止拥有数据库写权限的攻击者重算整条链。正式商用时应持续把审计事件和应用日志转发到限制删除权限的外部日志平台/WORM 存储，并对 `integrity_valid=false` 建立最高级别告警。

配置变更、适配器启停、Key 吊销/永久删除以及隐私导出/删除采用双阶段审计：系统必须先持久化 `*.requested` 意图事件，才执行副作用，随后写入结果事件。外部平台操作无法与本地 SQLite 放入同一事务；若结果事件缺失，意图事件仍会保留，运营方必须把“长时间没有对应完成事件”视为需要人工核查的不完整操作。API Key 的创建、吊销和永久删除还会对数据库持久化错误采取 fail-closed 策略，不能以内存成功冒充持久化成功。

## 上线前人工门禁

- 使用 PostgreSQL，并完成异地加密备份、恢复演练和迁移回滚演练；
- 为健康检查、错误率、延迟、队列积压和磁盘容量配置告警；
- 为每个调用方签发最小权限 API Key，禁止共享通配符 Key；
- 明确消息数据保留期、删除流程、隐私政策和平台授权边界；
- 核对 GPLv3 分发义务；若提供修改后程序下载或交付给客户，应让法律顾问确认源码提供方式；
- 为依赖漏洞扫描、镜像签名、版本发布和紧急回滚建立持续流程；
- 在承诺 SLA 前完成目标并发量的压力测试和至少一次故障演练。

通过生产配置门禁只代表配置满足最低安全条件，不等同于多租户 SaaS 认证或合规认证。
