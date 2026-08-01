#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/easybot-backup-test.XXXXXX")
trap 'rm -rf "$TMP_DIR"' EXIT

SOURCE="$TMP_DIR/source.db"
RESTORED="$TMP_DIR/restored.db"
BACKUPS="$TMP_DIR/backups"

sqlite3 "$SOURCE" "CREATE TABLE proof(value TEXT); INSERT INTO proof VALUES('commercial-ready'); PRAGMA journal_mode=WAL;" >/dev/null
ARCHIVE=$("$ROOT/scripts/easybot-backup.sh" backup sqlite "$SOURCE" "$BACKUPS")
"$ROOT/scripts/easybot-backup.sh" verify "$ARCHIVE" >/dev/null
"$ROOT/scripts/easybot-backup.sh" restore sqlite "$ARCHIVE" "$RESTORED" >/dev/null
[ "$(sqlite3 "$RESTORED" 'SELECT value FROM proof;')" = "commercial-ready" ]

# Existing databases require explicit confirmation and are preserved on replace.
sqlite3 "$RESTORED" "UPDATE proof SET value='modified';"
sqlite3 "$RESTORED" "PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;" >/dev/null
if "$ROOT/scripts/easybot-backup.sh" restore sqlite "$ARCHIVE" "$RESTORED" >/dev/null 2>&1; then
  echo "restore unexpectedly overwrote an existing database without --force" >&2
  exit 1
fi
"$ROOT/scripts/easybot-backup.sh" restore sqlite "$ARCHIVE" "$RESTORED" --force >/dev/null
[ "$(sqlite3 "$RESTORED" 'SELECT value FROM proof;')" = "commercial-ready" ]
find "$TMP_DIR" -name 'restored.db.pre-restore.*' -type f | grep -q .

printf 'tampered' >> "$ARCHIVE"
if "$ROOT/scripts/easybot-backup.sh" verify "$ARCHIVE" >/dev/null 2>&1; then
  echo "tampered backup unexpectedly passed verification" >&2
  exit 1
fi

echo "Backup/restore test passed"
