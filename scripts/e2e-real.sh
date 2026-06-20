#!/usr/bin/env bash
#
# EasyBot 半自动化端到端验收脚本
#
# 自动完成：编译 → 启动 → API 检查 → 检测入站消息 → 发送回复 → 验证 → 生成报告
# 仅需人工：在各平台上向 Bot 发送一条测试消息
#
# 用法：
#   bash scripts/e2e-real.sh          # 交互模式，等待用户确认
#   bash scripts/e2e-real.sh --quick  # 快速模式，跳过编译
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── 配置 ──

BASE_URL="${E2E_BASE_URL:-http://127.0.0.1:8080}"
API_BASE="$BASE_URL/api/v1"
LOG_FILE="/tmp/easybot-e2e.log"
SUMMARY_FILE="/tmp/easybot-e2e-summary.txt"
WAIT_TIMEOUT="${E2E_TIMEOUT:-120}"
QUICK_MODE=false

# ── 工具函数 ──

section() { echo -e "\n${BOLD}${CYAN}═══ $1 ═══${NC}"; }
pass()  { echo -e "  ${GREEN}✅${NC} $1"; }
fail()  { echo -e "  ${RED}❌${NC} $1"; }
warn()  { echo -e "  ${YELLOW}⚠️${NC}  $1"; }
info()  { echo -e "  ${CYAN}ℹ${NC}  $1"; }

api_get() {
    curl -s ${E2E_API_KEY:+-H "Authorization: Bearer $E2E_API_KEY"} "$1" 2>/dev/null
}

api_post() {
    curl -s -X POST ${E2E_API_KEY:+-H "Authorization: Bearer $E2E_API_KEY"} \
        -H "Content-Type: application/json" -d "$2" "$1" 2>/dev/null
}

cleanup() {
    if [ -n "${E2E_PID:-}" ]; then
        kill "$E2E_PID" 2>/dev/null || true
        wait "$E2E_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ── Phase 1: 编译与启动 ──

phase1_build_and_start() {
    section "Phase 1: 编译与启动"

    if [ "$QUICK_MODE" = true ]; then
        info "快速模式：跳过编译"
    else
        info "编译 easybot (features: full)..."
        cargo build --features full 2>&1 | tail -3
        pass "编译完成"
    fi

    info "启动 easybot --debug..."
    cargo run --features full -- --debug > "$LOG_FILE" 2>&1 &
    E2E_PID=$!

    # 等待服务就绪
    for i in $(seq 1 15); do
        sleep 1
        if curl -s "$BASE_URL/api/v1/health" > /dev/null 2>&1; then
            break
        fi
        [ "$i" -eq 15 ] && { fail "服务启动超时"; exit 1; }
    done
    pass "服务已启动 (PID=$E2E_PID)"

    # 提取 Dev API Key
    E2E_API_KEY=$(grep -o 'key=eb_[a-f0-9]*' "$LOG_FILE" | head -1 | cut -d= -f2)
    if [ -z "$E2E_API_KEY" ]; then
        fail "未找到 Dev API Key（日志中无 key=eb_...）"
        exit 1
    fi
    pass "API Key: eb_${E2E_API_KEY:3:8}..."
}

# ── Phase 2: 自动 API 检查 ──

phase2_auto_checks() {
    section "Phase 2: 自动 API 检查"

    # Health
    local health
    health=$(api_get "$API_BASE/health")
    local total adapters=$(echo "$health" | jq -r '.adapters.total // 0')
    local connected=$(echo "$health" | jq -r '.adapters.connected // 0')
    if [ "$connected" -ge 5 ]; then
        pass "Health: $connected/$total adapters connected"
    else
        fail "Health: $connected/$total (expected 5)"
    fi

    # Adapters
    echo ""
    info "适配器状态:"
    api_get "$API_BASE/adapters" | jq -r '.adapters[] | "    \(.platform) (\(.display_name)): \(.status)"' 2>/dev/null

    # Session 预检查
    local session_count
    session_count=$(api_get "$API_BASE/sessions" | jq -r '.total // 0')
    info "当前活跃会话: $session_count"
}

# ── Phase 3: 等待入站消息 ──

phase3_wait_for_messages() {
    section "Phase 3: 等待入站消息"

    echo ""
    echo -e "  ${BOLD}📱 请在各平台客户端向 Bot 发送一条测试消息：${NC}"
    echo ""
    echo "    Telegram → 向 Bot 发送任意文字"
    echo "    Discord  → 在已添加 Bot 的频道/私信发送消息"
    echo "    飞书     → 向 Bot 发送任意文字"
    echo "    QQ       → 在 Bot 所在群发送消息"
    echo "    微信     → 向 Bot 发送任意文字"
    echo ""

    if [ -t 0 ]; then
        read -r -p "  准备好后按 Enter 开始检测（或等待 ${WAIT_TIMEOUT}s 自动继续）..." _
    fi

    info "轮询检测入站消息（最多 ${WAIT_TIMEOUT}s）..."

    local expected_platforms=("telegram" "discord" "feishu" "qq" "wechat")
    local found_platforms=()
    local prev_count=0

    for i in $(seq 1 "$WAIT_TIMEOUT"); do
        sleep 1

        # 检查新会话
        local resp
        resp=$(api_get "$API_BASE/sessions")
        local cur_count
        cur_count=$(echo "$resp" | jq -r '.total // 0')

        # 提取平台列表
        local cur_platforms
        cur_platforms=$(echo "$resp" | jq -r '[.sessions[].platform] | unique | .[]' 2>/dev/null || true)

        found_platforms=()
        while IFS= read -r p; do
            [ -n "$p" ] && found_platforms+=("$p")
        done <<< "$cur_platforms"

        if [ "$cur_count" -gt "$prev_count" ]; then
            echo ""
            info "检测到新会话 (${cur_count} total): ${found_platforms[*]}"
            prev_count="$cur_count"
        fi

        # 5 个全部出现则提前退出
        if [ "${#found_platforms[@]}" -ge 5 ]; then
            echo ""
            pass "全部 5 个平台均已收到消息!"
            break
        fi

        # 进度指示
        if [ $((i % 5)) -eq 0 ]; then
            echo -n "  [${i}s] 已检测: ${found_platforms[*]:-无}  "
        fi
    done
    echo ""
}

# ── Phase 4: 自动发送回复 ──

phase4_auto_reply() {
    section "Phase 4: 自动发送 E2E 回复"

    local ts
    ts=$(date +%H:%M:%S)
    local results=()

    local resp
    resp=$(api_get "$API_BASE/sessions")
    local sessions
    sessions=$(echo "$resp" | jq -r '.sessions[] | "\(.platform):\(.chat_id)"')

    while IFS= read -r target; do
        [ -z "$target" ] && continue
        local plat="${target%%:*}"
        local chat="${target#*:}"

        local payload
        payload=$(jq -n --arg t "$plat:$chat" --arg text "[E2E] $plat test - $ts" \
            '{target: $t, text: $text}')

        local result
        result=$(api_post "$API_BASE/messages/send" "$payload")
        local status
        status=$(echo "$result" | jq -r '.status // "error"')
        local msg_id
        msg_id=$(echo "$result" | jq -r '.messageId // .id // "N/A"')

        if [ "$status" = "sent" ]; then
            pass "$plat → $status (id=$msg_id)"
        else
            fail "$plat → $status ($(echo "$result" | jq -r '.error // "unknown"'))"
        fi
        results+=("$plat:$status:$msg_id")
    done <<< "$sessions"

    # 写入汇总
    echo "send_results=${results[*]}" > "$SUMMARY_FILE"
}

# ── Phase 5: 验证与报告 ──

phase5_verify_and_report() {
    section "Phase 5: 验证与报告"

    # 消息历史
    local msg_count
    msg_count=$(api_get "$API_BASE/messages?limit=50" | jq -r '.messages | length // 0')
    info "消息历史: $msg_count 条"

    local session_count
    session_count=$(api_get "$API_BASE/sessions" | jq -r '.total // 0')
    info "活跃会话: $session_count 个"

    echo ""

    # 按平台统计
    local all_pass=true
    for p in telegram discord feishu qq wechat; do
        local has_session has_inbound has_outbound
        has_session=$(api_get "$API_BASE/sessions" | jq -r "[.sessions[] | select(.platform==\"$p\")] | length")

        # 检查入站消息 (User role)
        has_inbound=$(api_get "$API_BASE/messages?platform=$p&limit=50" | jq -r "[.messages[] | select(.role==\"User\")] | length")

        # 检查出站消息 (Assistant role, E2E prefix)
        has_outbound=$(api_get "$API_BASE/messages?platform=$p&limit=50" | jq -r "[.messages[] | select(.role==\"Assistant\" and (.text | startswith(\"[E2E]\")))] | length")

        local s_icon="✅" i_icon="✅" o_icon="✅"
        [ "$has_session" -eq 0 ] && { s_icon="❌"; all_pass=false; }
        [ "$has_inbound" -eq 0 ] && { i_icon="⚠️"; }
        [ "$has_outbound" -eq 0 ] && { o_icon="⚠️"; all_pass=false; }

        printf "  %-10s Session: %s  Inbound: %s  Outbound: %s\n" \
            "$p" "$s_icon" "$i_icon" "$o_icon"
    done

    echo ""
    echo -e "${BOLD}═══════════════════════════════════════════${NC}"

    if [ "$all_pass" = true ]; then
        echo -e "${GREEN}${BOLD}  🎉 ALL 5 PLATFORMS PASSED${NC}"
    else
        echo -e "${YELLOW}${BOLD}  ⚠️  Some platforms need attention${NC}"
    fi

    echo -e "${BOLD}═══════════════════════════════════════════${NC}"
    echo ""
    info "完整日志: $LOG_FILE"
}

# ── 主流程 ──

main() {
    # 参数解析
    if [[ "${1:-}" == "--quick" ]]; then
        QUICK_MODE=true
    fi

    echo -e "${BOLD}${CYAN}"
    echo "╔══════════════════════════════════════════╗"
    echo "║       EasyBot E2E Real Test Suite       ║"
    echo "╚══════════════════════════════════════════╝"
    echo -e "${NC}"

    phase1_build_and_start
    phase2_auto_checks
    phase3_wait_for_messages
    phase4_auto_reply
    phase5_verify_and_report
}

main "$@"
