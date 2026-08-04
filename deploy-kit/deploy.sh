#!/usr/bin/env bash
# EasyBot 一键部署（基于已发布 GHCR 镜像，无需构建）
#
# 用法:
#   ./deploy.sh                        # 用默认值部署 latest
#   EASYBOT_IMAGE=ghcr.io/easyindie/easybot:0.0.29 ./deploy.sh   # 指定版本
#   EASYBOT_BIND_ADDRESS=0.0.0.0 ./deploy.sh                     # 绑定到非回环地址
#
# 可覆盖环境变量:
#   EASYBOT_IMAGE        镜像引用（默认 ghcr.io/easyindie/easybot:latest）
#   EASYBOT_CONTAINER_NAME  容器名（默认 easybot）
#   EASYBOT_PORT        宿主端口（默认 8080）
#   EASYBOT_BIND_ADDRESS 宿主绑定地址（默认 127.0.0.1，仅本机可访问）
#   EASYBOT_ADMIN_PASSWORD 管理后台密码（默认 strong-admin-password，务必覆盖！）
#
# 令牌通过 .env 或环境变量传入（未设令牌的平台自动跳过）。
set -euo pipefail

IMAGE="${EASYBOT_IMAGE:-ghcr.io/easyindie/easybot:latest}"
NAME="${EASYBOT_CONTAINER_NAME:-easybot}"
PORT="${EASYBOT_PORT:-8080}"
BIND="${EASYBOT_BIND_ADDRESS:-127.0.0.1}"
ADMIN_PASSWORD="${EASYBOT_ADMIN_PASSWORD:-strong-admin-password}"

command -v docker >/dev/null 2>&1 || { echo "✗ docker 未安装" >&2; exit 1; }

echo "▶ 拉取镜像 $IMAGE ..."
docker pull "$IMAGE"

# 幂等：若存在同名容器则先删除（保留卷数据）
if docker ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
  echo "▶ 移除已存在容器 $NAME（命名卷 easybot-data 保留）..."
  docker rm -f "$NAME" >/dev/null
fi

echo "▶ 启动 $NAME ..."
ARGS=(
  -d --name "$NAME"
  -p "$BIND:$PORT:8080"
  -v easybot-data:/var/lib/easybot
  -e EASYBOT_HOME=/var/lib/easybot
  -e EASYBOT_ALLOW_PLAINTEXT=true
  -e EASYBOT_ADMIN_PASSWORD="$ADMIN_PASSWORD"
)
# 透传 .env 中已定义的平台令牌（存在才传，避免空值注入）
if [ -f .env ]; then
  set -a; # shellcheck disable=SC1091
  . ./.env; set +a
fi
for var in TELEGRAM_BOT_TOKEN DISCORD_BOT_TOKEN FEISHU_APP_ID FEISHU_APP_SECRET \
           QQ_APP_ID QQ_CLIENT_SECRET; do
  if [ -n "${!var:-}" ]; then ARGS+=(-e "$var=${!var}"); fi
done
ARGS+=("$IMAGE")

docker run "${ARGS[@]}"

echo
echo "✓ 部署完成:"
echo "  - 健康检查:  curl http://$BIND:$PORT/api/v1/live"
echo "  - 管理后台:  http://$BIND:$PORT/admin"
echo "  - 日志:      docker logs -f $NAME"
echo
echo "⚠ 若宿主机有防火墙/仅本机使用，保持 $BIND 不回环绑定即可；"
echo "  公网/多机访问请置于可信 TLS 反向代理后（见 deploy/docker-compose.production.yml）。"
