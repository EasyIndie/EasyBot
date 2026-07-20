#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/easybot-production-backup.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/data" "$TMP/secrets" "$TMP/backups"
sqlite3 "$TMP/data/auth.db" 'CREATE TABLE proof(value TEXT); INSERT INTO proof VALUES("paired");'
sqlite3 "$TMP/data/auth.db" 'PRAGMA user_version=1;'
printf 'postgresql://easybot:secret@db/easybot\n' > "$TMP/secrets/database_url"
cat > "$TMP/bin/pg_dump" <<'EOF'
#!/usr/bin/env bash
for arg in "$@"; do case "$arg" in --file=*) file=${arg#--file=};; esac; done
printf 'mock postgres custom archive' > "$file"
EOF
cat > "$TMP/bin/pg_restore" <<'EOF'
#!/usr/bin/env bash
[ "${1:-}" = --list ]
EOF
chmod +x "$TMP/bin/pg_dump" "$TMP/bin/pg_restore"
export EASYBOT_DATA_DIR="$TMP/data"
export EASYBOT_SECRETS_DIR="$TMP/secrets"
BATCH=$(PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-backup.sh" "$TMP/backups")
[ -f "$BATCH/COMPLETE" ]
[ -f "$BATCH/manifest.txt" ]
[ -f "$BATCH/manifest.txt.sha256" ]
grep -Fxq 'auth_schema_version=1' "$BATCH/manifest.txt"
[ "$(find "$BATCH" -name '*.dump' | wc -l | tr -d ' ')" = 1 ]
[ "$(find "$BATCH" -name '*.sqlite3' | wc -l | tr -d ' ')" = 1 ]
[ "$(find "$BATCH" -name '*.sha256' | wc -l | tr -d ' ')" = 3 ]
expected_manifest_checksum=$(awk 'NR == 1 { print $1 }' "$BATCH/manifest.txt.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  actual_manifest_checksum=$(sha256sum "$BATCH/manifest.txt" | awk '{ print $1 }')
else
  actual_manifest_checksum=$(shasum -a 256 "$BATCH/manifest.txt" | awk '{ print $1 }')
fi
[ "$expected_manifest_checksum" = "$actual_manifest_checksum" ]
sqlite_archive=$(find "$BATCH" -name '*.sqlite3')
[ "$(sqlite3 "$sqlite_archive" 'SELECT value FROM proof;')" = paired ]
echo "Production paired backup test passed"
