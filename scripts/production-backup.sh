#!/usr/bin/env bash
set -euo pipefail
umask 077

usage() { echo "Usage: EASYBOT_DATA_DIR=... EASYBOT_SECRETS_DIR=... $0 BACKUP_ROOT" >&2; }
[ "$#" -eq 1 ] || { usage; exit 64; }
: "${EASYBOT_DATA_DIR:?EASYBOT_DATA_DIR is required}"
: "${EASYBOT_SECRETS_DIR:?EASYBOT_SECRETS_DIR is required}"
BACKUP_ROOT=$1
AUTH_DB="$EASYBOT_DATA_DIR/auth.db"
DATABASE_URL_FILE="$EASYBOT_SECRETS_DIR/database_url"
[ -f "$AUTH_DB" ] && [ ! -L "$AUTH_DB" ] || { echo "auth.db is missing or is a symlink" >&2; exit 66; }
[ -f "$DATABASE_URL_FILE" ] && [ ! -L "$DATABASE_URL_FILE" ] || { echo "database_url secret is missing or is a symlink" >&2; exit 66; }

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# Resolve the repo/kit root relative to this script so the tooling also works
# from the standalone deploy kit (which has no git checkout).
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p "$BACKUP_ROOT"
timestamp=$(date -u '+%Y%m%dT%H%M%SZ')
partial="$BACKUP_ROOT/.easybot-$timestamp.partial"
final="$BACKUP_ROOT/easybot-$timestamp"
[ ! -e "$partial" ] && [ ! -e "$final" ] || { echo "backup batch already exists" >&2; exit 73; }
mkdir "$partial"
cleanup() { [ ! -d "$partial" ] || rm -rf "$partial"; }
trap cleanup EXIT

DATABASE_URL=$(tr -d '\r\n' < "$DATABASE_URL_FILE")
[ -n "$DATABASE_URL" ] || { echo "database_url secret is empty" >&2; exit 65; }
export DATABASE_URL
postgres_archive=$("$ROOT/scripts/easybot-backup.sh" backup postgres "$partial")
unset DATABASE_URL
auth_archive=$("$ROOT/scripts/easybot-backup.sh" backup sqlite "$AUTH_DB" "$partial")
"$ROOT/scripts/easybot-backup.sh" verify "$postgres_archive" >/dev/null
"$ROOT/scripts/easybot-backup.sh" verify "$auth_archive" >/dev/null
auth_schema_version=$(sqlite3 "$auth_archive" 'SELECT COALESCE(MAX(version), 0) FROM _schema_version;')
[[ "$auth_schema_version" =~ ^[0-9]+$ ]] || { echo "auth.db schema version is invalid" >&2; exit 65; }

{
  echo "batch_id=$timestamp"
  echo "created_at_utc=$timestamp"
  echo "postgres_archive=$(basename "$postgres_archive")"
  echo "auth_archive=$(basename "$auth_archive")"
  echo "auth_schema_version=$auth_schema_version"
} > "$partial/manifest.txt"
chmod 600 "$partial/manifest.txt"
sha256_file "$partial/manifest.txt" > "$partial/manifest.txt.sha256"
chmod 600 "$partial/manifest.txt.sha256"
touch "$partial/COMPLETE"
chmod 600 "$partial/COMPLETE"
mv "$partial" "$final"
trap - EXIT
echo "$final"
