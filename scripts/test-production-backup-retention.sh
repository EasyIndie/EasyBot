#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/easybot-retention.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
for n in 01 02 03 04 05 06 07 08 09 10; do
  dir="$TMP/easybot-202601${n}T000000Z"
  mkdir "$dir"
  touch "$dir/COMPLETE"
  touch -t 202601010000 "$dir"
done
OUTPUT=$("$ROOT/scripts/production-backup-retention.sh" "$TMP" 30 7 --dry-run)
[ "$(printf '%s\n' "$OUTPUT" | grep -c '^would_delete=')" = 3 ]
if EASYBOT_CONFIRM_BACKUP_DELETE=no "$ROOT/scripts/production-backup-retention.sh" "$TMP" 30 7 --apply >/dev/null 2>&1; then
  echo "retention applied without explicit confirmation" >&2
  exit 1
fi
EASYBOT_CONFIRM_BACKUP_DELETE=yes "$ROOT/scripts/production-backup-retention.sh" "$TMP" 30 7 --apply >/dev/null
[ "$(find "$TMP" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" = 7 ]
echo "Production backup retention test passed"
