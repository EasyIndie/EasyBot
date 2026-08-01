#!/usr/bin/env bash
# EasyBot database backup, verification and restore utility.

set -euo pipefail
umask 077

die() { echo "ERROR: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"; }

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

usage() {
  cat <<'EOF'
Usage:
  easybot-backup.sh backup sqlite SOURCE_DB OUTPUT_DIR
  easybot-backup.sh backup postgres OUTPUT_DIR
  easybot-backup.sh verify ARCHIVE
  easybot-backup.sh restore sqlite ARCHIVE DESTINATION_DB [--force]
  easybot-backup.sh restore postgres ARCHIVE [--force]

PostgreSQL commands read DATABASE_URL from the environment. Restore refuses to
overwrite a non-empty database unless --force is supplied.
EOF
}

write_checksum() {
  local archive="$1"
  sha256_file "$archive" > "${archive}.sha256"
}

verify_archive() {
  local archive="$1" checksum expected actual
  [ -f "$archive" ] || die "archive not found: $archive"
  checksum="${archive}.sha256"
  [ -f "$checksum" ] || die "checksum not found: $checksum"
  expected=$(awk 'NR == 1 {print $1}' "$checksum")
  actual=$(sha256_file "$archive")
  [ -n "$expected" ] && [ "$expected" = "$actual" ] || die "checksum mismatch: $archive"
  case "$archive" in
    *.sqlite3)
      need sqlite3
      [ "$(sqlite3 "$archive" 'PRAGMA integrity_check;')" = "ok" ] || die "SQLite integrity check failed"
      ;;
    *.dump) need pg_restore; pg_restore --list "$archive" >/dev/null ;;
    *) die "unsupported archive extension: $archive" ;;
  esac
  echo "Verified: $archive"
}

backup_sqlite() {
  local source="$1" output_dir="$2" timestamp archive
  need sqlite3
  [ -f "$source" ] || die "SQLite database not found: $source"
  [ ! -L "$source" ] || die "refusing to back up a symlink: $source"
  mkdir -p "$output_dir"
  timestamp=$(date -u '+%Y%m%dT%H%M%SZ')
  archive="${output_dir%/}/easybot-${timestamp}.sqlite3"
  [[ "$archive" != *"'"* && "$archive" != *$'\n'* ]] || die "output path contains unsupported characters"
  [ ! -e "$archive" ] || die "archive already exists: $archive"
  sqlite3 "$source" ".timeout 10000" ".backup '$archive'"
  [ "$(sqlite3 "$archive" 'PRAGMA integrity_check;')" = "ok" ] || die "backup integrity check failed"
  write_checksum "$archive"
  echo "$archive"
}

backup_postgres() {
  local output_dir="$1" timestamp archive
  need pg_dump
  [ -n "${DATABASE_URL:-}" ] || die "DATABASE_URL is required"
  mkdir -p "$output_dir"
  timestamp=$(date -u '+%Y%m%dT%H%M%SZ')
  archive="${output_dir%/}/easybot-${timestamp}.dump"
  [ ! -e "$archive" ] || die "archive already exists: $archive"
  pg_dump --format=custom --no-owner --no-privileges --file="$archive" "$DATABASE_URL"
  write_checksum "$archive"
  echo "$archive"
}

restore_sqlite() {
  local archive="$1" destination="$2" force="${3:-}" parent tmp previous
  verify_archive "$archive" >/dev/null
  [ ! -L "$destination" ] || die "refusing to replace a symlink: $destination"
  if { [ -e "${destination}-wal" ] || [ -e "${destination}-shm" ]; } && [ "$force" != "--force" ]; then
    die "WAL/SHM files exist; stop EasyBot cleanly and pass --force to restore"
  fi
  if [ -e "$destination" ] && [ "$force" != "--force" ]; then
    die "destination exists; stop EasyBot and pass --force to replace it"
  fi
  parent=$(dirname "$destination")
  mkdir -p "$parent"
  tmp="${destination}.restore.$$"
  cp "$archive" "$tmp"
  chmod 600 "$tmp"
  [ "$(sqlite3 "$tmp" 'PRAGMA integrity_check;')" = "ok" ] || die "restored copy integrity check failed"
  if [ -e "$destination" ]; then
    previous="${destination}.pre-restore.$(date -u '+%Y%m%dT%H%M%SZ')"
    mv "$destination" "$previous"
    [ ! -e "${destination}-wal" ] || mv "${destination}-wal" "${previous}-wal"
    [ ! -e "${destination}-shm" ] || mv "${destination}-shm" "${previous}-shm"
    echo "Previous database preserved at: $previous" >&2
  fi
  mv "$tmp" "$destination"
  echo "Restored: $destination"
}

restore_postgres() {
  local archive="$1" force="${2:-}" count
  need pg_restore
  need psql
  [ -n "${DATABASE_URL:-}" ] || die "DATABASE_URL is required"
  verify_archive "$archive" >/dev/null
  count=$(psql "$DATABASE_URL" -Atqc \
    "SELECT count(*) FROM pg_tables WHERE schemaname NOT IN ('pg_catalog','information_schema')")
  if [ "${count:-0}" != "0" ] && [ "$force" != "--force" ]; then
    die "target database is not empty; pass --force to clean and restore it"
  fi
  if [ "$force" = "--force" ]; then
    pg_restore --clean --if-exists --no-owner --no-privileges --dbname="$DATABASE_URL" "$archive"
  else
    pg_restore --no-owner --no-privileges --dbname="$DATABASE_URL" "$archive"
  fi
  echo "PostgreSQL restore completed"
}

[ "$#" -ge 1 ] || { usage; exit 2; }
command_name="$1"; shift
case "$command_name" in
  backup)
    [ "$#" -ge 1 ] || die "backup type is required"
    type="$1"; shift
    case "$type" in
      sqlite) [ "$#" -eq 2 ] || die "sqlite backup requires SOURCE_DB OUTPUT_DIR"; backup_sqlite "$1" "$2" ;;
      postgres) [ "$#" -eq 1 ] || die "postgres backup requires OUTPUT_DIR"; backup_postgres "$1" ;;
      *) die "unsupported backup type: $type" ;;
    esac ;;
  verify) [ "$#" -eq 1 ] || die "verify requires ARCHIVE"; verify_archive "$1" ;;
  restore)
    [ "$#" -ge 1 ] || die "restore type is required"
    type="$1"; shift
    case "$type" in
      sqlite) [ "$#" -ge 2 ] && [ "$#" -le 3 ] || die "sqlite restore requires ARCHIVE DESTINATION_DB [--force]"; restore_sqlite "$1" "$2" "${3:-}" ;;
      postgres) [ "$#" -ge 1 ] && [ "$#" -le 2 ] || die "postgres restore requires ARCHIVE [--force]"; restore_postgres "$1" "${2:-}" ;;
      *) die "unsupported restore type: $type" ;;
    esac ;;
  -h|--help|help) usage ;;
  *) die "unknown command: $command_name" ;;
esac
