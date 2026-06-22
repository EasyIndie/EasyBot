#!/usr/bin/env bash
#
# EasyBot 一键验收脚本
# 与 CI 执行相同的检验逻辑，开发者在本地（macOS / Linux）运行即可。
#
# 用法：
#   bash scripts/verify.sh          # 跑全部检查
#   bash scripts/verify.sh --fast   # 只跑测试，跳过 clippy / fmt
#   bash scripts/verify.sh --help   # 查看帮助
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

PASS=0
FAIL=0
TIMING=""

# ── helpers ──────────────────────────────────────────────────────

section() {
  local label="$1"
  echo ""
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
  echo -e "${CYAN}$label${NC}"
  echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
}

pass() {
  local elapsed="$1"
  PASS=$((PASS + 1))
  TIMING="${TIMING}${GREEN}✓${NC} ${2:-} (${elapsed}s)\n"
}

fail() {
  local elapsed="$1"
  FAIL=$((FAIL + 1))
  TIMING="${TIMING}${RED}✗${NC} ${2:-}  (${elapsed}s)\n"
}

run_step() {
  local name="$1"
  shift
  section "$name"
  local start
  start=$(date +%s)
  if "$@"; then
    local elapsed=$(( $(date +%s) - start ))
    pass "$elapsed" "$name"
  else
    local elapsed=$(( $(date +%s) - start ))
    fail "$elapsed" "$name"
    echo -e "${RED}❌ $name 失败，退出验收${NC}"
    exit 1
  fi
}

# ── main ─────────────────────────────────────────────────────────

FAST=false
for arg in "$@"; do
  case "$arg" in
    --fast) FAST=true ;;
    --help)
      echo "EasyBot 一键验收脚本"
      echo ""
      echo "  bash scripts/verify.sh         完整检查（编译 + 测试 + lint + fmt）"
      echo "  bash scripts/verify.sh --fast  只跑测试，跳过 clippy 和 fmt"
      exit 0
      ;;
  esac
done

echo ""
echo "╔══════════════════════════════════════════╗"
echo "║    EasyBot Verification Suite           ║"
echo "╚══════════════════════════════════════════╝"
echo "  工作目录: $PROJECT_DIR"
echo "  模式: $([ "$FAST" = true ] && echo 'fast (跳过 lint/fmt)' || echo '完整')"
echo "  日期: $(date '+%Y-%m-%d %H:%M:%S')"
echo ""

# ── 1. 编译检查 ──────────────────────────────────────────────────
run_step "cargo check (workspace + full features)" \
  cargo check --workspace --features "full,plugin-system"

# ── 2. 格式化检查（全量提交时必做）───────────────────────────────
if [ "$FAST" = false ]; then
  run_step "cargo fmt --check" \
    cargo fmt --all --check
fi

# ── 3. Clippy lint（全量提交时必做）───────────────────────────────
if [ "$FAST" = false ]; then
  run_step "cargo clippy (all targets + warnings as errors)" \
    cargo clippy --workspace --features "full,plugin-system" --all-targets -- -D warnings
fi

# ── 4. 构建全部（确保 mock-adapter 可用）──────────────────────────
run_step "cargo build --workspace" \
  cargo build --workspace

# ── 5. 默认特性下的测试 ──────────────────────────────────────────
run_step "cargo test (default features)" \
  cargo test --workspace

# ── 6. 编译 mock-adapter（插件集成测试前置条件）───────────────────
run_step "cargo build -p mock-adapter" \
  cargo build -p mock-adapter

# ── 7. 全特性测试（验证所有适配器 + 插件系统 + E2E）─────────────
run_step "cargo test (full features + plugin-system)" \
  cargo test --workspace --features "full,plugin-system"

# ── 汇总报告 ─────────────────────────────────────────────────────
echo ""
echo "╔══════════════════════════════════════════╗"
echo "║  验收结果                                ║"
echo "╚══════════════════════════════════════════╝"
echo ""
echo -e "$TIMING"
echo -e "总计: $((PASS + FAIL)) 步 | ${GREEN}通过: $PASS${NC} | ${RED}失败: $FAIL${NC}"
echo ""

if [ "$FAIL" -gt 0 ]; then
  echo -e "${RED}❌ 验收未通过，请检查上述失败步骤。${NC}"
  exit 1
else
  echo -e "${GREEN}✅ 验收全部通过！${NC}"
fi
