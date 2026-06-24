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

PROJECT_DIR="$(git rev-parse --show-toplevel 2>/dev/null || echo '')"
if [ -z "$PROJECT_DIR" ]; then
    echo "ERROR: 不在 git 仓库中" >&2
    exit 1
fi
cd "$PROJECT_DIR"

# 将所有编译警告视为错误（与 CI 一致）
export RUSTFLAGS="-D warnings"

# 自动检测 cargo 路径：优先本地 cargo，fallback 到 wsl cargo
if command -v cargo >/dev/null 2>&1; then
    CARGO="cargo"
elif command -v wsl >/dev/null 2>&1; then
    CARGO="wsl cargo"
else
    echo "ERROR: cargo not found (tried 'cargo' and 'wsl cargo')" >&2
    exit 1
fi

# 自动检测测试运行器：优先 cargo nextest（与 CI 一致），fallback 到 cargo test
if $CARGO nextest --version >/dev/null 2>&1; then
    TEST_RUNNER="$CARGO nextest run"
    TEST_LABEL="nextest"
else
    TEST_RUNNER="$CARGO test"
    TEST_LABEL="test"
fi
echo "  测试运行器: $TEST_LABEL"

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
      echo "EasyBot 一键验收脚本（与 CI workflow 一致）"
      echo ""
      echo "  bash scripts/verify.sh         完整检查（8 步：fmt + clippy + check + matrix + build + test）"
      echo "  bash scripts/verify.sh --fast  快速检查，跳过 clippy 和 fmt"
      echo ""
      echo "  步骤:"
      echo "    1. cargo check    (full + plugin-system)"
      echo "    2. cargo fmt --check"
      echo "    3. cargo clippy"
      echo "    4. Feature Matrix (7 种适配器组合的 cargo check)"
      echo "    5. cargo build --workspace"
      echo "    6. 测试 (default features) — 自动选择 nextest 或 cargo test"
      echo "    7. cargo build -p mock-adapter"
      echo "    8. 测试 (full + plugin-system)"
      echo ""
      echo "  安装 cargo-nextest 可加速测试: cargo install cargo-nextest"
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
  $CARGO check --workspace --features "full,plugin-system"

# ── 2. 格式化检查（全量提交时必做）───────────────────────────────
if [ "$FAST" = false ]; then
  run_step "cargo fmt --check" \
    $CARGO fmt --all --check
fi

# ── 3. Clippy lint（全量提交时必做）───────────────────────────────
if [ "$FAST" = false ]; then
  run_step "cargo clippy (all targets + warnings as errors)" \
    $CARGO clippy --workspace --features "full,plugin-system" --all-targets -- -D warnings
fi

# ── 4. Feature Matrix 检查（与 CI test-feature-matrix 一致）─────
# 验证各适配器独立 + 组合编译，防止非条件编译导入导致单适配器 check 失败
section "Feature Matrix (7 种组合)"
echo "  验证各适配器独立/组合编译..."
FEATURE_COMBOS=(
    "--no-default-features                                               # 无适配器"
    "--no-default-features --features adapter-telegram                   # 仅 Telegram"
    "--no-default-features --features adapter-discord                    # 仅 Discord"
    "--features adapter-telegram,adapter-discord                         # Telegram + Discord"
    "--no-default-features --features adapter-feishu                     # 仅飞书"
    "--no-default-features --features adapter-qq                         # 仅 QQ"
    "--no-default-features --features adapter-wechat                     # 仅微信"
)

MATRIX_START=$(date +%s)
MATRIX_FAIL=0
for entry in "${FEATURE_COMBOS[@]}"; do
    features="${entry%%#*}"                     # 去掉行内注释
    features=$(echo "$features" | xargs)        # trim whitespace
    label="${entry#*#}"                          # 提取注释作为标签
    label=$(echo "$label" | xargs)

    echo -ne "  ⏳ ${label}..."
    if $CARGO check --workspace $features >/dev/null 2>&1; then
        echo -e "\r  ${GREEN}✅${NC} ${label}"
    else
        echo -e "\r  ${RED}❌${NC} ${label} — cargo check 失败！"
        MATRIX_FAIL=$((MATRIX_FAIL + 1))
    fi
done

MATRIX_ELAPSED=$(( $(date +%s) - MATRIX_START ))
echo ""
if [ "$MATRIX_FAIL" -eq 0 ]; then
    pass "$MATRIX_ELAPSED" "Feature Matrix (7/7 通过)"
else
    fail "$MATRIX_ELAPSED" "Feature Matrix ($MATRIX_FAIL 项失败)"
    echo -e "${RED}❌ Feature Matrix 未通过 (${MATRIX_FAIL}/7 失败)，退出验收${NC}"
    exit 1
fi

# ── 5. 构建全部（确保 mock-adapter 可用）──────────────────────────
run_step "cargo build --workspace" \
  $CARGO build --workspace

# ── 6. 默认特性下的测试 ──────────────────────────────────────────
run_step "$TEST_LABEL (default features)" \
  $TEST_RUNNER --workspace

# ── 7. 编译 mock-adapter（插件集成测试前置条件）───────────────────
run_step "cargo build -p mock-adapter" \
  $CARGO build -p mock-adapter

# ── 8. 全特性测试（验证所有适配器 + 插件系统 + E2E）─────────────
run_step "$TEST_LABEL (full features + plugin-system)" \
  $TEST_RUNNER --workspace --features "full,plugin-system"

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
