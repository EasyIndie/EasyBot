#!/usr/bin/env bash
set -euo pipefail

usage() { echo "Usage: $0 BACKUP_ROOT RETENTION_DAYS MIN_BACKUPS [--dry-run|--apply]" >&2; }
[ "$#" -eq 4 ] || { usage; exit 64; }
ROOT_INPUT=$1
RETENTION_DAYS=$2
MIN_BACKUPS=$3
MODE=$4
[[ "$RETENTION_DAYS" =~ ^[1-9][0-9]*$ ]] || { echo "RETENTION_DAYS must be positive" >&2; exit 65; }
[[ "$MIN_BACKUPS" =~ ^[1-9][0-9]*$ ]] || { echo "MIN_BACKUPS must be positive" >&2; exit 65; }
[ "$MODE" = --dry-run ] || [ "$MODE" = --apply ] || { usage; exit 64; }
[ -d "$ROOT_INPUT" ] && [ ! -L "$ROOT_INPUT" ] || { echo "backup root is missing or is a symlink" >&2; exit 66; }
BACKUP_ROOT=$(cd "$ROOT_INPUT" && pwd -P)
if [ "$MODE" = --apply ] && [ "${EASYBOT_CONFIRM_BACKUP_DELETE:-}" != yes ]; then
  echo "Set EASYBOT_CONFIRM_BACKUP_DELETE=yes to apply retention" >&2
  exit 65
fi

mtime() { stat -c '%Y' "$1" 2>/dev/null || stat -f '%m' "$1" 2>/dev/null; }
batches=()
while IFS= read -r batch; do batches+=("$batch"); done < <(
  find "$BACKUP_ROOT" -mindepth 1 -maxdepth 1 -type d -name 'easybot-????????T??????Z' -print | sort -r
)
now=$(date +%s)
max_age_seconds=$((RETENTION_DAYS * 86400))
eligible=0
deleted=0
for index in "${!batches[@]}"; do
  batch=${batches[$index]}
  [ "$index" -ge "$MIN_BACKUPS" ] || continue
  [ ! -L "$batch" ] && [ -f "$batch/COMPLETE" ] && [ ! -L "$batch/COMPLETE" ] || continue
  modified=$(mtime "$batch") || { echo "Unable to inspect backup age: $batch" >&2; exit 65; }
  age=$((now - modified))
  [ "$age" -gt "$max_age_seconds" ] || continue
  case "$batch" in "$BACKUP_ROOT"/easybot-????????T??????Z) ;; *) echo "Unsafe backup path: $batch" >&2; exit 65;; esac
  eligible=$((eligible + 1))
  if [ "$MODE" = --apply ]; then
    rm -rf "$batch"
    deleted=$((deleted + 1))
    echo "deleted=$batch"
  else
    echo "would_delete=$batch"
  fi
done
echo "retention_mode=${MODE#--} complete_batches=${#batches[@]} eligible=$eligible deleted=$deleted minimum_preserved=$MIN_BACKUPS"
