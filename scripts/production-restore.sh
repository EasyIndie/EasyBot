#!/usr/bin/env bash
set -euo pipefail
umask 077

usage() {
  echo "Usage: EASYBOT_DATA_DIR=... EASYBOT_SECRETS_DIR=... EASYBOT_CONFIRM_STOPPED=yes $0 BATCH_DIR PRE_RESTORE_BACKUP_ROOT --force" >&2
}
[ "$#" -eq 3 ] && [ "$3" = --force ] || { usage; exit 64; }
: "${EASYBOT_DATA_DIR:?EASYBOT_DATA_DIR is required}"
: "${EASYBOT_SECRETS_DIR:?EASYBOT_SECRETS_DIR is required}"
[ "${EASYBOT_CONFIRM_STOPPED:-}" = yes ] || { echo "Stop EasyBot and set EASYBOT_CONFIRM_STOPPED=yes" >&2; exit 65; }
BATCH=$1
SAFETY_ROOT=$2
[ -d "$BATCH" ] && [ ! -L "$BATCH" ] || { echo "backup batch is missing or is a symlink" >&2; exit 66; }
[ -f "$BATCH/COMPLETE" ] && [ ! -L "$BATCH/COMPLETE" ] || { echo "backup batch is incomplete" >&2; exit 65; }
[ -f "$BATCH/manifest.txt" ] && [ ! -L "$BATCH/manifest.txt" ] || { echo "backup manifest is missing" >&2; exit 66; }
[ -f "$BATCH/manifest.txt.sha256" ] && [ ! -L "$BATCH/manifest.txt.sha256" ] || { echo "backup manifest checksum is missing" >&2; exit 66; }

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

manifest_checksum=$(awk 'NR == 1 && NF == 1 { value = $1; next } { invalid = 1 } END { if (invalid || NR != 1 || value == "") exit 1; print value }' "$BATCH/manifest.txt.sha256") || {
  echo "backup manifest checksum file is invalid" >&2; exit 65;
}
[[ "$manifest_checksum" =~ ^[[:xdigit:]]{64}$ ]] || { echo "backup manifest checksum is invalid" >&2; exit 65; }
actual_manifest_checksum=$(sha256_file "$BATCH/manifest.txt")
[ "${manifest_checksum,,}" = "${actual_manifest_checksum,,}" ] || { echo "backup manifest checksum mismatch" >&2; exit 65; }

manifest_get() {
  local key=$1 count
  count=$(awk -F= -v key="$key" '$1 == key { count++ } END { print count+0 }' "$BATCH/manifest.txt")
  [ "$count" = 1 ] || { echo "manifest must contain exactly one $key" >&2; exit 65; }
  awk -F= -v key="$key" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' "$BATCH/manifest.txt"
}
postgres_name=$(manifest_get postgres_archive)
auth_name=$(manifest_get auth_archive)
auth_schema_version=$(manifest_get auth_schema_version)
[[ "$auth_schema_version" =~ ^[0-9]+$ ]] || { echo "manifest auth schema version is invalid" >&2; exit 65; }
for name in "$postgres_name" "$auth_name"; do
  [ "$name" = "$(basename "$name")" ] && [[ "$name" != *$'\n'* ]] || {
    echo "manifest archive name is unsafe" >&2; exit 65;
  }
done
[[ "$postgres_name" == *.dump ]] || { echo "manifest PostgreSQL archive has wrong extension" >&2; exit 65; }
[[ "$auth_name" == *.sqlite3 ]] || { echo "manifest auth archive has wrong extension" >&2; exit 65; }

# Resolve the repo/kit root relative to this script so the tooling also works
# from the standalone deploy kit (which has no git checkout).
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
POSTGRES_ARCHIVE="$BATCH/$postgres_name"
AUTH_ARCHIVE="$BATCH/$auth_name"
"$ROOT/scripts/easybot-backup.sh" verify "$POSTGRES_ARCHIVE" >/dev/null
"$ROOT/scripts/easybot-backup.sh" verify "$AUTH_ARCHIVE" >/dev/null
archive_schema_version=$(sqlite3 "$AUTH_ARCHIVE" 'SELECT COALESCE(MAX(version), 0) FROM _schema_version;')
[ "$archive_schema_version" = "$auth_schema_version" ] || {
  echo "auth archive schema version does not match manifest" >&2; exit 65;
}

# Preserve both current stores before the first destructive restore step.
"$ROOT/scripts/production-backup.sh" "$SAFETY_ROOT" >/dev/null
DATABASE_URL=$(tr -d '\r\n' < "$EASYBOT_SECRETS_DIR/database_url")
[ -n "$DATABASE_URL" ] || { echo "database_url secret is empty" >&2; exit 65; }
export DATABASE_URL
"$ROOT/scripts/easybot-backup.sh" restore postgres "$POSTGRES_ARCHIVE" --force
unset DATABASE_URL
"$ROOT/scripts/easybot-backup.sh" restore sqlite "$AUTH_ARCHIVE" "$EASYBOT_DATA_DIR/auth.db" --force
chown 10001:10001 "$EASYBOT_DATA_DIR/auth.db"
chmod 600 "$EASYBOT_DATA_DIR/auth.db"
echo "Paired production restore completed; keep EasyBot stopped until validation finishes"
