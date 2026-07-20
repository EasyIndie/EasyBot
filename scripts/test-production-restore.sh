#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/easybot-production-restore.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/data" "$TMP/secrets" "$TMP/backups" "$TMP/safety"
sqlite3 "$TMP/data/auth.db" 'CREATE TABLE proof(value TEXT); INSERT INTO proof VALUES("before");'
printf 'postgresql://easybot:secret@db/easybot\n' > "$TMP/secrets/database_url"
cat > "$TMP/bin/pg_dump" <<'EOF'
#!/usr/bin/env bash
for arg in "$@"; do case "$arg" in --file=*) file=${arg#--file=};; esac; done
printf 'mock postgres custom archive' > "$file"
EOF
cat > "$TMP/bin/pg_restore" <<'EOF'
#!/usr/bin/env bash
[ "${1:-}" = --list ] && exit 0
touch "$PG_RESTORE_MARKER"
EOF
cat > "$TMP/bin/psql" <<'EOF'
#!/usr/bin/env bash
echo 1
EOF
cat > "$TMP/bin/chown" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$TMP/bin/"*
export EASYBOT_DATA_DIR="$TMP/data"
export EASYBOT_SECRETS_DIR="$TMP/secrets"
export PG_RESTORE_MARKER="$TMP/postgres-restored"
BATCH=$(PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-backup.sh" "$TMP/backups")
sqlite3 "$TMP/data/auth.db" 'UPDATE proof SET value="after";'
export EASYBOT_CONFIRM_STOPPED=yes
cp "$BATCH/manifest.txt" "$TMP/manifest.original"
cp "$BATCH/manifest.txt.sha256" "$TMP/manifest-checksum.original"
rm "$BATCH/manifest.txt.sha256"
if PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-restore.sh" "$BATCH" "$TMP/safety" --force >/dev/null 2>&1; then
  echo "restore accepted a batch without a manifest checksum" >&2
  exit 1
fi
[ ! -f "$PG_RESTORE_MARKER" ]
[ "$(find "$TMP/safety" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" = 0 ]
cp "$TMP/manifest-checksum.original" "$BATCH/manifest.txt.sha256"
sed -i.bak 's/^auth_schema_version=.*/auth_schema_version=999/' "$BATCH/manifest.txt"
if PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-restore.sh" "$BATCH" "$TMP/safety" --force >/dev/null 2>&1; then
  echo "restore accepted manifest with a mismatched checksum" >&2
  exit 1
fi
[ ! -f "$PG_RESTORE_MARKER" ]
[ "$(find "$TMP/safety" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" = 0 ]
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$BATCH/manifest.txt" | awk '{ print $1 }' > "$BATCH/manifest.txt.sha256"
else
  shasum -a 256 "$BATCH/manifest.txt" | awk '{ print $1 }' > "$BATCH/manifest.txt.sha256"
fi
if PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-restore.sh" "$BATCH" "$TMP/safety" --force >/dev/null 2>&1; then
  echo "restore accepted mismatched auth schema version" >&2
  exit 1
fi
[ ! -f "$PG_RESTORE_MARKER" ]
[ "$(find "$TMP/safety" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" = 0 ]
cp "$TMP/manifest.original" "$BATCH/manifest.txt"
cp "$TMP/manifest-checksum.original" "$BATCH/manifest.txt.sha256"
PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-restore.sh" "$BATCH" "$TMP/safety" --force >/dev/null
[ -f "$PG_RESTORE_MARKER" ]
[ "$(sqlite3 "$TMP/data/auth.db" 'SELECT value FROM proof;')" = before ]
find "$TMP/safety" -name COMPLETE -type f | grep -q .
if EASYBOT_CONFIRM_STOPPED=no PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-restore.sh" "$BATCH" "$TMP/safety" --force >/dev/null 2>&1; then
  echo "restore ran without stopped-service confirmation" >&2
  exit 1
fi
echo "Production paired restore test passed"
