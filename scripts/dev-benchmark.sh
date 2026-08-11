#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
COLD=false
[ "${1:-}" = "--cold" ] && COLD=true

if [ "$COLD" = true ]; then
    echo ">>> cold baseline: removing only $ROOT/target via cargo clean"
    cargo clean
fi

time_step() {
    local name="$1"
    shift
    local start end
    start=$(date +%s)
    "$@"
    end=$(date +%s)
    printf '%-36s %6ss\n' "$name" "$((end - start))"
}

echo ">>> commit $(git rev-parse --short HEAD), mode=$([ "$COLD" = true ] && echo cold || echo warm)"
time_step "workspace check" cargo check --workspace --features "default,plugin-system" --locked
time_step "incremental core tests" cargo test -p easybot-core --locked
time_step "pre-push gate" bash scripts/verify-push.sh
echo ">>> target size: $(du -sh target 2>/dev/null | awk '{print $1}' || echo 0B)"
echo ">>> run 'bash scripts/verify.sh --locked' separately for the full 12-step baseline"
