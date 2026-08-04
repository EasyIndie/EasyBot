#!/usr/bin/env bash
set -euo pipefail

usage() { echo "Usage: EASYBOT_IMAGE=... EASYBOT_DOMAIN=... EASYBOT_SECRETS_DIR=... GITHUB_REPOSITORY=OWNER/REPO $0 [--check]" >&2; }
[ "$#" -le 1 ] || { usage; exit 64; }
MODE=${1:-up}
[ "$MODE" = up ] || [ "$MODE" = --check ] || { usage; exit 64; }

: "${EASYBOT_IMAGE:?EASYBOT_IMAGE must be IMAGE@sha256:DIGEST}"
: "${EASYBOT_DOMAIN:?EASYBOT_DOMAIN is required}"
: "${EASYBOT_SECRETS_DIR:?EASYBOT_SECRETS_DIR is required}"
: "${EASYBOT_DATA_DIR:?EASYBOT_DATA_DIR is required}"
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must be OWNER/REPO}"
[[ "$EASYBOT_DOMAIN" =~ ^([A-Za-z0-9-]+\.)+[A-Za-z]{2,63}$ ]] || { echo "Invalid public EASYBOT_DOMAIN" >&2; exit 65; }
[ -d "$EASYBOT_SECRETS_DIR" ] || { echo "Secrets directory does not exist" >&2; exit 66; }

file_mode() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1" 2>/dev/null; }
file_uid() { stat -c '%u' "$1" 2>/dev/null || stat -f '%u' "$1" 2>/dev/null; }
validate_owner_only_path() {
  local path=$1 kind=$2 mode uid
  [ ! -L "$path" ] || { echo "$kind must not be a symbolic link: $path" >&2; exit 65; }
  if [ "$kind" = directory ]; then [ -d "$path" ]; else [ -f "$path" ]; fi || {
    echo "$kind is missing or has the wrong type: $path" >&2; exit 66;
  }
  mode=$(file_mode "$path") || { echo "Unable to inspect permissions: $path" >&2; exit 65; }
  uid=$(file_uid "$path") || { echo "Unable to inspect owner: $path" >&2; exit 65; }
  case "$kind:$mode" in directory:500|directory:700|secret:400|secret:600) ;; *)
    echo "$kind must be owner-only (directory 0500/0700, secret 0400/0600): $path has $mode" >&2; exit 65;;
  esac
  [ "$uid" = 10001 ] || { echo "$kind must be owned by EasyBot UID 10001: $path has UID $uid" >&2; exit 65; }
}
validate_owner_only_path "$EASYBOT_SECRETS_DIR" directory
validate_owner_only_path "$EASYBOT_SECRETS_DIR/admin_password" secret
validate_owner_only_path "$EASYBOT_SECRETS_DIR/database_url" secret
validate_owner_only_path "$EASYBOT_DATA_DIR" directory

# Resolve the repo/kit root relative to this script so the tooling also works
# from the standalone deploy kit (which has no git checkout).
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
"$ROOT/scripts/verify-container-image.sh" "$EASYBOT_IMAGE" "$GITHUB_REPOSITORY"
docker compose -f "$ROOT/deploy/docker-compose.production.yml" config --quiet
[ "$MODE" = --check ] && { echo "Production deployment preflight passed"; exit 0; }
docker compose -f "$ROOT/deploy/docker-compose.production.yml" up -d --pull always
