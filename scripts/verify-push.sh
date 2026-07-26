#!/usr/bin/env bash
#
# EasyBot push 前门禁
#
# 目标：覆盖最常见的 CI 失败点，同时避免每次 push 都跑完整 feature matrix。
# 完整 CI 等价验收仍使用: bash scripts/verify.sh --locked

set -euo pipefail

PROJECT_DIR="$(git rev-parse --show-toplevel 2>/dev/null || echo '')"
if [ -z "$PROJECT_DIR" ]; then
    echo "ERROR: 不在 git 仓库中" >&2
    exit 1
fi
cd "$PROJECT_DIR"

if command -v cargo >/dev/null 2>&1; then
    CARGO="cargo"
elif command -v wsl >/dev/null 2>&1; then
    CARGO="wsl cargo"
else
    echo "ERROR: cargo not found (tried 'cargo' and 'wsl cargo')" >&2
    exit 1
fi

if $CARGO nextest --version >/dev/null 2>&1; then
    TEST_RUNNER="$CARGO nextest run"
else
    TEST_RUNNER="$CARGO test"
fi

run_step() {
    name="$1"
    shift
    echo ""
    echo ">>> $name"
    "$@"
}

run_step "cargo fmt --check" \
    $CARGO fmt --all --check

run_step "cargo clippy (workspace + plugin-system)" \
    $CARGO clippy --workspace --features "default,plugin-system" --all-targets --locked -- -D warnings

run_step "cargo build -p mock-adapter" \
    $CARGO build -p mock-adapter --locked

run_step "tests (workspace + plugin-system)" \
    $TEST_RUNNER --workspace --features "default,plugin-system" --locked

echo ""
echo ">>> push 门禁通过"
