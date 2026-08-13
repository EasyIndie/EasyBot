# EasyBot 部署工具包

> 随 EasyBot Release 发布，供下载后离线部署使用。包含**部署说明**与**部署工具**，
> 无需克隆源码即可完成 Docker / Docker Compose / 生产加固 / 原生二进制部署。

## 目录结构

```
easybot-deploy-kit/
├── README.md                 # 本说明（部署总入口）
├── deploy.sh                 # 一键部署脚本（Docker，最简路径）
├── compose.quickstart.yml    # 单容器快速部署（已发布镜像；与 .env 同目录）
├── VERSION                   # 对应 EasyBot 版本
├── gateway.yaml              # 基础配置（生产 compose 以 ../gateway.yaml 挂载）
├── .env.example              # 环境变量模板（适配器令牌）
├── deploy/
│   ├── docker-compose.production.yml   # 生产加固栈（Caddy TLS + 只读容器）
│   ├── Caddyfile                       # Caddy 反向代理配置（HTTPS/HSTS/WS）
│   ├── gateway.container.yaml          # 容器内置默认配置（可挂载覆盖）
│   └── gateway.production.local.yaml   # 生产 local 覆盖
└── scripts/
    ├── production-up.sh                 # 生产部署入口（预检 + compose up）
    ├── production-backup.sh             # 配对备份（PostgreSQL + auth.db）
    ├── production-restore.sh            # 配对恢复 + 校验
    ├── production-backup-retention.sh   # 备份保留策略
    ├── verify-container-image.sh        # 镜像 digest / SBOM 校验
    └── easybot-backup.sh                # 底层备份工具
```

---

## 方式一：Docker 一键部署（最快）

> 🔒 **版本锁定**：本工具包随 EasyBot `<VERSION>` 一起发布，`deploy.sh` 与
> `compose.quickstart.yml` 默认拉取 **`ghcr.io/easyindie/easybot:<VERSION>`** 同版本
> 镜像（不跟随 `latest` 漂移），保证部署的镜像与工具包说明/脚本一致。如需其他版本，
> 用 `EASYBOT_IMAGE=ghcr.io/easyindie/easybot:<其他版本>` 显式覆盖。

```bash
# 1. 从环境变量模板填入令牌（对应适配器自动启用，未设令牌的平台自动跳过）
cp .env.example .env && vim .env

# 2. 一键部署（基于已发布 GHCR 镜像，无需构建；默认部署与工具包同版本镜像）
./deploy.sh

# 3. 验证
curl http://127.0.0.1:8080/api/v1/live   # → 200 OK
# 管理后台: http://127.0.0.1:8080/admin
```

`deploy.sh` 等价于（`<VERSION>` 见工具包内 `VERSION` 文件）：

```bash
docker pull ghcr.io/easyindie/easybot:<VERSION>
docker run -d --name easybot \
  -p 127.0.0.1:8080:8080 \
  -v easybot-data:/var/lib/easybot \
  -e EASYBOT_HOME=/var/lib/easybot \
  -e EASYBOT_ALLOW_PLAINTEXT=true \
  -e EASYBOT_ADMIN_PASSWORD='更换为强密码' \
  ghcr.io/easyindie/easybot:<VERSION>
```

> ⚠️ `EASYBOT_ALLOW_PLAINTEXT=true` 是发布镜像的硬性启动要求：应用监听器仅支持 HTTP，
> 必须置于可信 TLS 反向代理后或受信主机上。宿主机侧默认映射 `127.0.0.1`。

**自定义访问端点**：默认 `127.0.0.1:8080`，需改端口/绑定地址时：

```bash
EASYBOT_PORT=9090 EASYBOT_BIND_ADDRESS=0.0.0.0 ./deploy.sh   # → http://0.0.0.0:9090/admin
# compose 方式同理：
EASYBOT_PORT=9090 docker compose -f compose.quickstart.yml up -d
```

## 方式二：Docker Compose 部署

```bash
cp .env.example .env && vim .env
docker compose -f compose.quickstart.yml up -d
```

工具包内的 `compose.quickstart.yml` 默认镜像已锁定为 **`ghcr.io/easyindie/easybot:<VERSION>`**
（与工具包同版本），挂载命名卷持久化 `EASYBOT_HOME`，支持通过环境变量或 `*_FILE` 注入令牌。
需覆盖镜像时：
`EASYBOT_IMAGE=ghcr.io/easyindie/easybot:<其他版本> docker compose -f compose.quickstart.yml up -d`。

## 方式三：生产加固部署（推荐线上）

生产栈 = Caddy（TLS 终止 / HSTS / WebSocket 代理）+ EasyBot（只读 root、非 root UID、
cap_drop、tmpfs）+ 外部 PostgreSQL + SQLite auth.db：

```bash
# 前置：secret 目录（属主 UID 10001，权限 0600/0700）
#   admin_password / database_url（postgresql://user:pass@host/db）
# 及 EASYBOT_DOMAIN、EASYBOT_IMAGE（镜像 digest）、GITHUB_REPOSITORY

bash scripts/production-up.sh            # 预检 + 部署
bash scripts/production-backup.sh        # 每日配对备份
bash scripts/production-restore.sh       # 恢复演练
```

生产要求（由 production-up.sh 强制校验）：镜像必须是不可变 digest（`IMAGE@sha256:...`），
secrets/data 目录属主 `UID 10001` 且仅属主可读。

## 方式四：原生二进制部署（裸机 / systemd / launchd）

下载对应平台二进制（`easybot-<target>`），直接运行：

```bash
./easybot --init            # 初始化 ~/.easybot 配置目录
./easybot --debug           # 前台运行
```

安装为系统服务（自动检测 systemd / launchd / Windows 服务）：

```bash
./easybot service install
./easybot service start
```

> 原生二进制升级：`./easybot check-update` → `./easybot update`（自动备份 + 原子替换 +
> 失败回滚），重启服务后生效。详见各版本 README。

---

## 配置参考

| 项 | 说明 |
|---|---|
| `EASYBOT_HOME` | 配置目录（默认 `~/.easybot`） |
| `EASYBOT_ADMIN_PASSWORD` | 管理后台密码（必填；生产用 `_FILE` 注入） |
| `EASYBOT_ALLOW_PLAINTEXT` | 发布镜像启动必须 `true`（仅 HTTP） |
| `EASYBOT_PORT` | 宿主端口（默认 `8080`；`deploy.sh` / `compose.quickstart.yml`） |
| `EASYBOT_BIND_ADDRESS` | 宿主绑定地址（默认 `127.0.0.1` 仅本机；公网访问改 `0.0.0.0` 或置于 TLS 反代后） |
| `TELEGRAM_BOT_TOKEN` / `DISCORD_BOT_TOKEN` / `FEISHU_APP_ID`+`FEISHU_APP_SECRET` / `QQ_APP_ID`+`QQ_CLIENT_SECRET` | 平台令牌，设置即自动启用对应适配器 |
| `DATABASE_URL` | PostgreSQL 连接串（storageType=postgres 时） |

消息保留：默认 90 天；会话 365 天（`gateway.yaml` 中 `storage.retention` 可调）。

## 控制适配器启用（尤其微信）

启用规则：**设置了凭据的平台自动启用；个人微信无需凭据、默认自动启用并等待扫码登录**。
容器部署时若不想启用微信（或要强制开关某平台），在挂载的 `gateway.yaml` 中显式声明
（覆盖镜像内置 `/etc/easybot/gateway.yaml`，需继承容器基础配置）：

```yaml
server:
  host: "0.0.0.0"
  port: 8080
adapters:
  wechat:
    enabled: false        # 显式禁用（默认 None=自动；微信无凭据会自动启用）
  telegram:
    enabled: true         # 显式启用（可选；设置了 token 也会自动启用）
```

```bash
docker run -d --name easybot \
  -p 127.0.0.1:8080:8080 \
  -v easybot-data:/var/lib/easybot \
  -v $PWD/gateway.yaml:/etc/easybot/gateway.yaml:ro \
  ghcr.io/easyindie/easybot:<VERSION>
```

> **微信扫码登录**：微信适配器启动时会把登录二维码打印到 `docker logs -f easybot`，
> 用手机微信扫码即可；登录态保存在数据卷 `easybot-data`，容器重启无需重扫。
> 不启用则如上 `enabled: false`（已在镜像实测：日志出现
> `Skipping adapter 'wechat' — explicitly disabled in config`）。

**不改 YAML、用环境变量切换**：在 `gateway.yaml` 里写 `enabled: ${EASYBOT_WECHAT_ENABLED}`，
运行加 `-e EASYBOT_WECHAT_ENABLED=false` 即禁用（该值未设置时视为自动）。其余平台同理。

## 升级与回滚

- **Docker**：`docker compose pull && docker compose up -d`（或 `deploy.sh` 重跑）
- **原生**：`easybot update`（自动备份 + SHA256 校验 + 原子替换 + 验证失败自动回滚）；
  `easybot rollback` 回滚到备份版本
- 相邻版本（post-G0）二进制回滚已验证兼容同一数据库（schema v2），见项目
  `commercial/evidence/rollback-drill-0029.txt`

## 备份与恢复

生产环境使用配对备份（PostgreSQL dump + auth.db 归档）：

```bash
bash scripts/production-backup.sh     # 产出带 sha256 校验的配对批次
bash scripts/production-restore.sh    # 校验 + 恢复（自动保留恢复前备份）
```

恢复目标：RPO 24h / RTO 4h（G1 演练验证，见项目 `commercial/evidence/`）。

---

## 完整文档

工具包是离线部署的最小集。完整用户指南 / API / 插件开发文档见仓库：
`docs/01 user-guide.md`、`docs/04 architecture.md`、`docs/15 plugin-quickstart.md`，
或在线浏览 https://github.com/EasyIndie/EasyBot 。
