#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

size_of() {
    if [ -e "$1" ]; then du -sh "$1" 2>/dev/null | awk '{print $1}'; else echo "0B"; fi
}

echo "EasyBot development diagnostics"
echo "commit: $(git rev-parse --short HEAD)"
echo "platform: $(uname -sm)"
echo "rustc: $(rustc --version 2>/dev/null || echo unavailable)"
echo "cargo: $(cargo --version 2>/dev/null || echo unavailable)"
echo "target: $(size_of "$ROOT/target")"
echo "cargo-home: $(size_of "${CARGO_HOME:-$HOME/.cargo}")"
echo "rustup-home: $(size_of "${RUSTUP_HOME:-$HOME/.rustup}")"
echo "repository-including-builds: $(size_of "$ROOT")"
echo ""
echo "Worktrees:"
git worktree list
echo ""
echo "Prunable worktrees (preview; Git may print this line on stderr):"
git worktree prune --dry-run --verbose 2>&1 || true
echo ""
if command -v docker >/dev/null 2>&1; then
    echo "Docker reclaimable space:"
    docker system df 2>/dev/null || echo "Docker daemon unavailable"
else
    echo "Docker: unavailable"
fi
