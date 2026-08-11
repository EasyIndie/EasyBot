#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
ACTION="${1:-report}"
CONFIRM="${2:-}"

usage() {
    cat <<'EOF'
Usage:
  scripts/dev-disk.sh                         report only (default)
  scripts/dev-disk.sh --clean-target --yes    clean this worktree's target only
  scripts/dev-disk.sh --prune-worktrees --yes remove stale worktree metadata
  scripts/dev-disk.sh --prune-docker --yes    prune BuildKit cache older than 7 days
  scripts/dev-disk.sh --clean-cargo --yes     run cargo-cache autoclean (if installed)

No command deletes Docker images/volumes or active worktrees.
EOF
}

report() {
    bash scripts/dev-diagnostics.sh
    local target_kb=0
    local target_budget_gib="${EASYBOT_TARGET_BUDGET_GIB:-8}"
    [ -d target ] && target_kb=$(du -sk target | awk '{print $1}')
    if [ "$target_kb" -ge $((target_budget_gib * 1024 * 1024)) ]; then
        echo "WARNING: target reached the ${target_budget_gib} GiB budget; consider --clean-target --yes after active work."
    fi
}

case "$ACTION" in
    report) report ;;
    --help|-h) usage ;;
    --clean-target)
        [ "$CONFIRM" = "--yes" ] || { echo "Preview: cargo clean in $ROOT"; exit 0; }
        cargo clean
        ;;
    --prune-worktrees)
        git worktree prune --dry-run --verbose
        if [ "$CONFIRM" = "--yes" ]; then
            git worktree prune --verbose
        fi
        ;;
    --prune-docker)
        command -v docker >/dev/null || { echo "ERROR: docker unavailable" >&2; exit 1; }
        [ "$CONFIRM" = "--yes" ] || { echo "Preview: docker builder prune --filter until=168h"; exit 0; }
        docker builder prune --filter until=168h --force
        ;;
    --clean-cargo)
        [ "$CONFIRM" = "--yes" ] || { echo "Preview: cargo cache --autoclean"; exit 0; }
        command -v cargo-cache >/dev/null || { echo "ERROR: install cargo-cache before using this explicit cleanup" >&2; exit 1; }
        cargo cache --autoclean
        ;;
    *) usage; exit 2 ;;
esac
