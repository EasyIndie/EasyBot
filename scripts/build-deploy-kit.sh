#!/usr/bin/env bash
# Assemble the EasyBot deployment toolkit release asset.
#
# The kit mirrors the repo root layout (deploy/, scripts/, gateway.yaml,
# .env.example) so the production scripts resolve their own location via
# BASH_SOURCE and work standalone without a git checkout.
#
# Usage: bash scripts/build-deploy-kit.sh [VERSION]
#   VERSION defaults to the version in Cargo.toml.
#   Output: dist/easybot-deploy-kit-<VERSION>.tar.gz
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

VERSION=${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)}
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]] || {
  echo "Invalid version: $VERSION" >&2; exit 1;
}

KIT="easybot-deploy-kit"
STAGE=$(mktemp -d "${TMPDIR:-/tmp}/easybot-deploy-kit.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT
OUT_DIR="$STAGE/$KIT"

# The kit mirrors the repo root layout exactly (deploy/, scripts/, gateway.yaml,
# .env.example) so the production scripts and compose relative mounts
# (e.g. docker-compose.production.yml mounts ../gateway.yaml) work unchanged.
mkdir -p "$OUT_DIR/deploy" "$OUT_DIR/scripts"

# ── 部署说明 + 一键部署入口 ──
cp deploy-kit/README.md "$OUT_DIR/README.md"
cp deploy-kit/deploy.sh "$OUT_DIR/deploy.sh"
chmod +x "$OUT_DIR/deploy.sh"
printf '%s\n' "$VERSION" > "$OUT_DIR/VERSION"

# ── 配置模板（放 kit 根：docker-compose.production.yml 以 ../gateway.yaml 挂载）──
cp gateway.yaml "$OUT_DIR/gateway.yaml"
cp .env.example "$OUT_DIR/.env.example"

# ── Docker 部署资产（快速开始 + 生产栈，镜像仓库 deploy/ 布局）──
# quickstart 默认镜像锁定为工具包同版本 tag（保留 ${EASYBOT_IMAGE:-...} 覆盖），
# 避免 latest 漂移导致 kit 内 compose 部署的版本与工具包不一致。
sed "s|ghcr.io/easyindie/easybot:latest}|ghcr.io/easyindie/easybot:${VERSION}}|" \
  compose.quickstart.yml > "$OUT_DIR/deploy/compose.quickstart.yml"
cp deploy/docker-compose.production.yml "$OUT_DIR/deploy/"
cp deploy/Caddyfile "$OUT_DIR/deploy/"
cp deploy/gateway.container.yaml "$OUT_DIR/deploy/"
cp deploy/gateway.production.local.yaml "$OUT_DIR/deploy/"

# ── 生产工具脚本 ──
for s in production-up.sh production-backup.sh production-restore.sh \
         production-backup-retention.sh verify-container-image.sh easybot-backup.sh; do
  [ -f "scripts/$s" ] || { echo "missing required script: scripts/$s" >&2; exit 1; }
  cp "scripts/$s" "$OUT_DIR/scripts/"
done
chmod +x "$OUT_DIR/scripts/"*.sh

# ── 打包 ──
mkdir -p dist
ARCHIVE="dist/$KIT-$VERSION.tar.gz"
tar -C "$STAGE" -czf "$ARCHIVE" "$KIT"

echo "✓ Deploy kit assembled: $ARCHIVE"
echo "  size: $(du -h "$ARCHIVE" | cut -f1)"
echo "  files:"
tar -tzf "$ARCHIVE" | sed 's/^/    /'
