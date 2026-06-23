#!/usr/bin/env bash
#
# EasyBot 端到端验收脚本
#
# 自动完成：预检 → 编译 → 启动 → 等待适配器就绪 → 检测入站消息 → 发送回复 → 验证 → 生成报告
# 仅需人工：在各平台上向 Bot 发送一条测试消息（首次运行需扫码登录微信）
#
# 用法：
#   bash scripts/e2e-real.sh                 # 交互模式，等待用户发送消息后按 Enter
#   bash scripts/e2e-real.sh --quick         # 快速模式，跳过编译
#   E2E_NON_INTERACTIVE=1 bash scripts/e2e-real.sh  # 非交互模式，自动等待
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
WAIT_TIMEOUT="${E2E_TIMEOUT:-30}"
START_TIMEOUT="${E2E_START_TIMEOUT:-60}"
ADAPTER_CONNECT_TIMEOUT="${E2E_ADAPTER_TIMEOUT:-30}"
POLL_INTERVAL=2
QUICK_MODE=false

# ── 全局状态（跨 Phase 共享） ──

FINAL_ADAPTER_STATUSES=""   # Phase 2 缓存，Phase 5 复用
declare -A PHASE_TIMES       # 各阶段耗时
__CURRENT_PHASE=""
__PHASE_START_TS=0

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

api_put() {
    curl -s -X PUT ${E2E_API_KEY:+-H "Authorization: Bearer $E2E_API_KEY"} \
        -H "Content-Type: application/json" -d "$2" "$1" 2>/dev/null
}

cleanup() {
    if [ -n "${E2E_PID:-}" ]; then
        kill "$E2E_PID" 2>/dev/null || true
        wait "$E2E_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ── 阶段计时 ──

phase_timer_start() {
    __CURRENT_PHASE="$1"
    __PHASE_START_TS=$(date +%s)
}

phase_timer_end() {
    local _elapsed=$(($(date +%s) - __PHASE_START_TS))
    PHASE_TIMES["$__CURRENT_PHASE"]=$_elapsed
}

print_phase_timing_summary() {
    echo ""
    echo -e "${BOLD}${CYAN}═══ 各阶段耗时 ═══${NC}"
    local _total=0
    local _order=(
        "Phase 0: 登录状态预检"
        "Phase 1: 编译与启动"
        "Phase 2: 等待适配器就绪"
        "Phase 3: 等待入站消息"
        "Phase 4: 自动发送回复"
        "Phase 5: 验证与报告"
    )
    for _p in "${_order[@]}"; do
        local _t="${PHASE_TIMES[$_p]:-0}"
        if [ "$_t" -gt 0 ] 2>/dev/null; then
            printf "  %-30s %4ds\n" "$_p" "$_t"
            _total=$((_total + _t))
        fi
    done
    echo "  ──────────────────────────────────────"
    printf "  %-30s %4ds\n" "总计" "$_total"
}

adapter_display_name() {
    case "$1" in
        telegram) echo "Telegram" ;;
        discord)  echo "Discord" ;;
        feishu)   echo "飞书" ;;
        qq)       echo "QQ" ;;
        wechat)   echo "个人微信" ;;
        *)        echo "$1" ;;
    esac
}

# ── Phase 0: 登录状态预检 ──

phase0_preflight() {
    section "Phase 0: 登录状态预检"
    phase_timer_start "Phase 0: 登录状态预检"

    local _cred_file="$HOME/.easybot/.wechat-credentials.json"

    if [ -f "$_cred_file" ]; then
        pass "微信凭据已存在，跳过扫码登录"
        phase_timer_end
        return 0
    fi

    echo ""
    echo -e "  ${BOLD}${YELLOW}╔══════════════════════════════════════════════╗${NC}"
    echo -e "  ${BOLD}${YELLOW}║  微信未登录。服务启动后终端将显示登录链接。     ║${NC}"
    echo -e "  ${BOLD}${YELLOW}║  请复制链接在浏览器中打开，用微信扫码完成登录。  ║${NC}"
    echo -e "  ${BOLD}${YELLOW}║  登录成功后凭据会自动保存，后续无需重复扫码。   ║${NC}"
    echo -e "  ${BOLD}${YELLOW}╚══════════════════════════════════════════════╝${NC}"
    echo ""

    # 如果是非交互模式，不等待
    if [ "${E2E_NON_INTERACTIVE:-}" != "1" ] && [ -t 0 ]; then
        read -r -p "  按 Enter 继续启动服务..." _
    fi

    phase_timer_end
}

# ── Phase 1: 编译与启动 ──

phase1_build_and_start() {
    section "Phase 1: 编译与启动"
    phase_timer_start "Phase 1: 编译与启动"

    if [ "$QUICK_MODE" = true ]; then
        info "快速模式：跳过编译"
    else
        info "编译 easybot (features: full)..."
        cargo build --features full 2>&1 | tail -3
        pass "编译完成"
    fi

    info "启动 easybot --debug（日志: $LOG_FILE）..."
    # 只重定向 stderr 到日志文件，stdout 保留在终端
    # 这样 println! 输出（如微信 QR 码）用户可以直接看到
    cargo run --features full -- --debug 2>"$LOG_FILE" &
    E2E_PID=$!

    # 等待服务就绪
    local _start_ts _elapsed
    _start_ts=$(date +%s)
    while true; do
        if curl -s -o /dev/null -w "%{http_code}" "$BASE_URL/api/v1/health" 2>/dev/null | grep -q 200; then
            break
        fi
        _now=$(date +%s)
        _elapsed=$((_now - _start_ts))
        if [ "$_elapsed" -ge "$START_TIMEOUT" ]; then
            fail "服务启动超时 (${START_TIMEOUT}s)"
            info "查看最后 30 行日志:"
            tail -30 "$LOG_FILE" | sed 's/^/    /'
            info "提示: 设置 E2E_START_TIMEOUT=120 可延长超时时间"
            exit 1
        fi
        if [ "$_elapsed" -lt 5 ]; then
            sleep 0.5
        elif [ "$_elapsed" -lt 20 ]; then
            sleep 1
        else
            sleep 2
        fi
    done
    pass "服务已启动 (PID=$E2E_PID, ${_elapsed:-?}s)"

    # 提取 Dev API Key
    E2E_API_KEY=$(grep -o 'key=eb_[a-f0-9]*' "$LOG_FILE" | head -1 | cut -d= -f2)
    if [ -z "$E2E_API_KEY" ]; then
        fail "未找到 Dev API Key（日志中无 key=eb_...）"
        info "日志最后 10 行:"
        tail -10 "$LOG_FILE" | sed 's/^/    /'
        exit 1
    fi
    pass "API Key: eb_${E2E_API_KEY:3:8}..."

    phase_timer_end
}

# ── Phase 2: 等待适配器就绪 ──

phase2_wait_for_adapters() {
    section "Phase 2: 等待适配器就绪"
    phase_timer_start "Phase 2: 等待适配器就绪"

    info "等待所有适配器连接（超时: ${ADAPTER_CONNECT_TIMEOUT}s）..."

    local _start_ts _elapsed
    _start_ts=$(date +%s)

    while true; do
        sleep "$POLL_INTERVAL"
        _now=$(date +%s)
        _elapsed=$((_now - _start_ts))

        local _adapters_json _total _connected _still_connecting
        _adapters_json=$(api_get "$API_BASE/adapters")

        # 安全解析（jq 失败时用默认值）
        _total=$(echo "$_adapters_json" | jq -r '.adapters | length' 2>/dev/null || echo "0")
        _connected=$(echo "$_adapters_json" | jq -r '[.adapters[] | select(.connected == true)] | length' 2>/dev/null || echo "0")

        # 无适配器注册
        if [ "$_total" -eq 0 ]; then
            warn "没有注册任何适配器"
            FINAL_ADAPTER_STATUSES="$_adapters_json"
            break
        fi

        # 全部就绪
        if [ "$_connected" -ge "$_total" ]; then
            echo ""
            pass "全部 $_connected 个适配器已连接 (${_elapsed}s)"
            echo ""
            # 打印各平台状态
            echo "$_adapters_json" | jq -r '.adapters[] | "    ✅ \(.platform) (\(.display_name)): \(.status)"' 2>/dev/null
            FINAL_ADAPTER_STATUSES="$_adapters_json"
            break
        fi

        # 构建仍在连接的平台列表
        _still_connecting=$(echo "$_adapters_json" | jq -r \
            '[.adapters[] | select(.connected == false) | .display_name] | join(", ")' 2>/dev/null || echo "?")

        # 进度行（每轮一行，不原地刷新以保证终端兼容性）
        echo -e "  ⏳ ${_connected}/${_total} 已连接 (${_still_connecting:-?} 仍在连接) [${_elapsed}s]"

        # 超时检查
        if [ "$_elapsed" -ge "$ADAPTER_CONNECT_TIMEOUT" ]; then
            echo ""
            warn "适配器连接超时 (${ADAPTER_CONNECT_TIMEOUT}s)，以下为最终状态："
            echo ""
            echo "$_adapters_json" | jq -r \
                '.adapters[] | "    \(if .connected then "✅" else "⚠️ " end) \(.platform) (\(.display_name)): \(.status)"' 2>/dev/null
            FINAL_ADAPTER_STATUSES="$_adapters_json"
            break
        fi
    done

    echo ""
    phase_timer_end
}

# ── Phase 3: 等待入站消息 ──

phase3_wait_for_messages() {
    section "Phase 3: 等待入站消息"
    phase_timer_start "Phase 3: 等待入站消息"

    # 记录轮询开始时间戳
    POLL_START_MS=$(($(date +%s) * 1000))

    # 从 Phase 2 结果确定可用平台
    local expected_platforms=()
    local disconnected_platforms=()
    if [ -n "${FINAL_ADAPTER_STATUSES:-}" ]; then
        while IFS= read -r p; do
            [ -n "$p" ] && expected_platforms+=("$p")
        done <<< "$(echo "$FINAL_ADAPTER_STATUSES" | jq -r '.adapters[] | select(.connected == true) | .platform' 2>/dev/null || true)"

        while IFS= read -r p; do
            [ -n "$p" ] && disconnected_platforms+=("$p")
        done <<< "$(echo "$FINAL_ADAPTER_STATUSES" | jq -r '.adapters[] | select(.connected == false) | .platform' 2>/dev/null || true)"
    fi

    # 如果 Phase 2 没有缓存数据，使用默认全部平台
    if [ "${#expected_platforms[@]}" -eq 0 ]; then
        expected_platforms=("telegram" "discord" "feishu" "qq" "wechat")
    fi

    # 提示未连接的平台
    if [ "${#disconnected_platforms[@]}" -gt 0 ]; then
        local _dc_names=()
        for p in "${disconnected_platforms[@]}"; do
            _dc_names+=("$(adapter_display_name "$p")")
        done
        warn "以下平台未连接，将跳过: ${_dc_names[*]}"
    fi

    echo ""
    echo -e "  ${BOLD}📱 请向以下平台 Bot 发送一条测试消息：${NC}"
    echo ""
    for p in "${expected_platforms[@]}"; do
        echo "    $(adapter_display_name "$p") → 向 Bot 发送任意文字"
    done
    echo ""

    # 交互 / 非交互模式
    if [ "${E2E_NON_INTERACTIVE:-}" = "1" ]; then
        local _delay="${E2E_NON_INTERACTIVE_DELAY:-5}"
        info "非交互模式，${_delay}s 后开始检测..."
        sleep "$_delay"
    elif [ -t 0 ]; then
        read -r -p "  完成发送后按 Enter 开始检测..." _
    fi

    info "开始轮询检测入站消息（每 ${POLL_INTERVAL}s 检测一次，最多 ${WAIT_TIMEOUT}s）..."

    local found_platforms=()
    local elapsed=0
    local total_expected=${#expected_platforms[@]}

    while [ "$elapsed" -lt "$WAIT_TIMEOUT" ]; do
        sleep "$POLL_INTERVAL"
        elapsed=$((elapsed + POLL_INTERVAL))

        local resp
        resp=$(api_get "$API_BASE/sessions")
        local cur_platforms
        cur_platforms=$(echo "$resp" | jq -r --argjson since "$POLL_START_MS" \
            '[.sessions[] | select(.updated_at > $since) | .platform] | unique | .[]' 2>/dev/null || true)

        found_platforms=()
        while IFS= read -r p; do
            [ -n "$p" ] && found_platforms+=("$p")
        done <<< "$cur_platforms"

        # 构建逐平台状态行
        local _line_parts=()
        for p in "${expected_platforms[@]}"; do
            local _found=false
            for f in "${found_platforms[@]}"; do
                [ "$f" = "$p" ] && _found=true && break
            done
            local _dname
            _dname=$(adapter_display_name "$p")
            if [ "$_found" = true ]; then
                _line_parts+=("${GREEN}✅${NC} ${_dname}")
            else
                _line_parts+=("${YELLOW}⏳${NC} ${_dname}")
            fi
        done

        local _found_count=${#found_platforms[@]}
        echo -e "  [${elapsed}s] ${_line_parts[*]}  (${_found_count}/${total_expected})"

        if [ "$_found_count" -ge "$total_expected" ]; then
            echo ""
            pass "全部 ${total_expected} 个平台均已收到消息 (${elapsed}s)"
            break
        fi
    done

    if [ "${#found_platforms[@]}" -lt "$total_expected" ]; then
        echo ""
        local missing=()
        for p in "${expected_platforms[@]}"; do
            local _found=false
            for f in "${found_platforms[@]}"; do
                [ "$f" = "$p" ] && _found=true && break
            done
            [ "$_found" = false ] && missing+=("$(adapter_display_name "$p")")
        done
        warn "超时未检测到消息: ${missing[*]}"
    fi
    echo ""

    phase_timer_end
}

# ── Phase 4: 自动发送回复 ──

phase4_auto_reply() {
    section "Phase 4: 自动发送 E2E 回复（覆盖全部消息类型）"
    phase_timer_start "Phase 4: 自动发送回复"

    local ts
    ts=$(date +%H:%M:%S)

    local resp
    resp=$(api_get "$API_BASE/sessions")
    local targets
    targets=$(echo "$resp" | jq -r --argjson since "${POLL_START_MS:-0}" \
        '[.sessions | map(select(.updated_at > $since)) | group_by(.platform) | .[] | "\(.[0].platform):\(.[0].chat_id)"] | .[]' 2>/dev/null)

    if [ -z "$targets" ]; then
        warn "没有检测到本轮新入站消息，跳过自动回复"
        phase_timer_end
        return 0
    fi

    # 安全提取 JSON 字段（兼容 jq 1.5+，不使用 // 操作符）
    _jqr() {
        local val
        val=$(echo "$1" | jq -r "$2" 2>/dev/null) || true
        if [ -z "$val" ] || [ "$val" = "null" ]; then
            echo "${3:-}"
        else
            echo "$val"
        fi
    }

    # 收集所有 target 用于 batch send；保存各平台 message_id 用于 edit
    local all_targets=()
    local -A msg_ids

    # ═══════════════════════════════════════════
    # 4a: 文本消息
    # ═══════════════════════════════════════════
    echo "  ── 文本消息 ──"
    while IFS= read -r target; do
        [ -z "$target" ] && continue
        local plat="${target%%:*}"
        all_targets+=("$target")

        local payload
        payload=$(jq -n --arg t "$target" --arg text "[E2E] $plat text - $ts" \
            '{target: $t, text: $text}')

        local result
        result=$(api_post "$API_BASE/messages/send" "$payload")
        local status
        status=$(_jqr "$result" '.status' 'error')
        local msg_id
        msg_id=$(_jqr "$result" '.messageId' '')
        [ -z "$msg_id" ] && msg_id=$(_jqr "$result" '.id' 'N/A')
        msg_ids["$plat"]="$msg_id"

        if [ "$status" = "sent" ]; then
            pass "$(adapter_display_name "$plat") text → sent (id=$msg_id)"
        else
            fail "$(adapter_display_name "$plat") text → $status ($(_jqr "$result" '.error' 'unknown'))"
        fi
    done <<< "$targets"

    # ═══════════════════════════════════════════
    # 4b: 交互式消息（inline keyboard）
    #   支持: telegram, feishu
    # ═══════════════════════════════════════════
    echo "  ── 交互式消息 ──"
    local interactive_plats=(telegram feishu)
    for plat in "${interactive_plats[@]}"; do
        local target
        target=$(echo "$targets" | { grep "^${plat}:" || true; } | head -1)
        [ -z "$target" ] && { warn "$(adapter_display_name "$plat") 无活跃会话，跳过交互式消息"; continue; }

        local payload
        payload=$(jq -n --arg t "$target" --arg text "[E2E] $plat interactive - $ts" \
            '{target: $t, text: $text, keyboard: {rows: [{buttons: [{text: "确认", callback_data: "e2e_confirm"}, {text: "取消", callback_data: "e2e_cancel"}]}]}}')

        local result
        result=$(api_post "$API_BASE/messages/send" "$payload")
        local status
        status=$(_jqr "$result" '.status' 'error')
        local msg_id
        msg_id=$(_jqr "$result" '.messageId' '')
        [ -z "$msg_id" ] && msg_id=$(_jqr "$result" '.id' 'N/A')

        if [ "$status" = "sent" ]; then
            pass "$(adapter_display_name "$plat") interactive → sent (id=$msg_id)"
        else
            fail "$(adapter_display_name "$plat") interactive → $status ($(_jqr "$result" '.error' 'unknown'))"
        fi
    done

    # ═══════════════════════════════════════════
    # 4c: 编辑消息（基于 4a 的 message_id）
    #   支持: telegram, discord, feishu
    # ═══════════════════════════════════════════
    echo "  ── 编辑消息 ──"
    local edit_plats=(telegram discord feishu)
    for plat in "${edit_plats[@]}"; do
        local msg_id="${msg_ids[$plat]:-}"
        [ -z "$msg_id" ] || [ "$msg_id" = "N/A" ] && { warn "$(adapter_display_name "$plat") 无可用消息 ID，跳过编辑"; continue; }

        local target
        target=$(echo "$targets" | { grep "^${plat}:" || true; } | head -1)
        [ -z "$target" ] && continue

        local payload
        payload=$(jq -n --arg t "$target" --arg text "[E2E] $plat edited - $ts" \
            '{target: $t, text: $text}')

        local result
        result=$(api_put "$API_BASE/messages/$msg_id" "$payload")
        local ok
        ok=$(_jqr "$result" '.ok' 'false')

        if [ "$ok" = "true" ]; then
            pass "$(adapter_display_name "$plat") edit → ok"
        else
            fail "$(adapter_display_name "$plat") edit → $(echo "$result" | $JQ '.error' 2>/dev/null || echo "$result")"
        fi
    done

    # ═══════════════════════════════════════════
    # 4d: 批量发送
    # ═══════════════════════════════════════════
    echo "  ── 批量发送 ──"
    if [ ${#all_targets[@]} -gt 0 ]; then
        local targets_json
        targets_json=$(printf '%s\n' "${all_targets[@]}" | jq -R . | jq -s .)
        local batch_payload
        batch_payload=$(jq -n --argjson targets "$targets_json" --arg text "[E2E] batch - $ts" \
            '{targets: $targets, text: $text}')

        local batch_result
        batch_result=$(api_post "$API_BASE/messages/batch-send" "$batch_payload")
        local total
        total=$(_jqr "$batch_result" '.total' '0')
        local sent_count
        sent_count=$(echo "$batch_result" | jq -r '[.results[] | select(.status=="sent")] | length' 2>/dev/null || echo "0")

        pass "batch send → $sent_count/$total sent"
    fi

    # ═══════════════════════════════════════════
    # 4e: 媒体消息（1x1 透明 PNG base64）
    #   支持: telegram, discord, qq
    # ═══════════════════════════════════════════
    echo "  ── 媒体消息 ──"
    local media_plats=(telegram discord qq)
    for plat in "${media_plats[@]}"; do
        local target
        target=$(echo "$targets" | { grep "^${plat}:" || true; } | head -1)
        [ -z "$target" ] && { warn "$(adapter_display_name "$plat") 无活跃会话，跳过媒体消息"; continue; }

        # 1x1 透明 PNG (最小有效 PNG，~68 bytes base64)
        local png_b64="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=="

        local payload
        if [ "$plat" = "qq" ]; then
            # QQ 适配器仅支持 URL 方式（base64 不适用）
            payload=$(jq -n --arg t "$target" --arg text "[E2E] $plat media - $ts" \
                '{target: $t, text: $text, media: {media_type: "Image", url: "https://via.placeholder.com/150", mime_type: "image/png", filename: "placeholder.png"}}')
        else
            payload=$(jq -n --arg t "$target" --arg text "[E2E] $plat media - $ts" --arg data "$png_b64" \
                '{target: $t, text: $text, media: {media_type: "Image", data: $data, mime_type: "image/png", filename: "e2e-test.png"}}')
        fi

        local result
        result=$(api_post "$API_BASE/messages/send" "$payload")
        local status
        status=$(_jqr "$result" '.status' 'error')
        local msg_id
        msg_id=$(_jqr "$result" '.messageId' '')
        [ -z "$msg_id" ] && msg_id=$(_jqr "$result" '.id' 'N/A')

        if [ "$status" = "sent" ]; then
            pass "$(adapter_display_name "$plat") media → sent (id=$msg_id)"
        else
            fail "$(adapter_display_name "$plat") media → $status ($(_jqr "$result" '.error' 'unknown'))"
        fi
    done

    phase_timer_end
}

# ── Phase 5: 验证与报告 ──

phase5_verify_and_report() {
    section "Phase 5: 验证与报告"
    phase_timer_start "Phase 5: 验证与报告"

    # 一次性获取所有消息（避免逐平台多次 API 调用）
    local _all_messages
    _all_messages=$(api_get "$API_BASE/messages?limit=200")

    local msg_count
    msg_count=$(echo "$_all_messages" | jq -r '.messages | length' 2>/dev/null || echo "0")
    info "消息历史: $msg_count 条"

    local session_count
    session_count=$(api_get "$API_BASE/sessions" | jq -r '.total' 2>/dev/null || echo "0")
    info "活跃会话: $session_count 个"

    echo ""
    echo -e "  ${BOLD}══════ 逐平台验证 ══════${NC}"

    # 表头
    printf "  ${BOLD}%-10s %-8s %-6s %-6s %s${NC}\n" "平台" "Session" "入站" "出站" "状态"
    printf "  %-10s %-8s %-6s %-6s %s\n" "──────────" "───────" "──────" "──────" "───────"

    local all_pass=true
    for p in telegram discord feishu qq wechat; do
        local _dname
        _dname=$(adapter_display_name "$p")

        # Session
        local has_session
        has_session=$(api_get "$API_BASE/sessions" | jq -r "[.sessions[] | select(.platform==\"$p\")] | length")

        # 入站消息（角色为 User）
        local has_inbound
        has_inbound=$(echo "$_all_messages" | jq -r \
            "[.messages[] | select(.platform==\"$p\" and .role==\"User\")] | length")

        # 出站 E2E 消息（角色为 Assistant，文本以 [E2E] 开头）
        local has_outbound
        has_outbound=$(echo "$_all_messages" | jq -r \
            "[.messages[] | select(.platform==\"$p\" and .role==\"Assistant\" and (.text | startswith(\"[E2E]\")))] | length")

        # 判断状态
        local s_icon i_icon o_icon status_text=""
        if [ "$has_session" -eq 0 ] 2>/dev/null; then
            s_icon="❌"; all_pass=false; status_text="无会话"
        else
            s_icon="✅"
        fi

        if [ "$has_inbound" -eq 0 ] 2>/dev/null; then
            i_icon="⚠️ "; status_text="${status_text:-}无入站"
        else
            i_icon="✅"
        fi

        if [ "$has_outbound" -eq 0 ] 2>/dev/null; then
            o_icon="⚠️ "
            [ -z "$status_text" ] && status_text="未回复"
        else
            o_icon="✅"
        fi

        [ -z "$status_text" ] && status_text="✅"

        printf "  %-10s %-8s %-6s %-6s %s\n" \
            "$_dname" "$s_icon" "$i_icon" "$o_icon" "$status_text"
    done

    echo ""

    # 适配器健康（复用 Phase 2 缓存）
    if [ -n "${FINAL_ADAPTER_STATUSES:-}" ]; then
        local _adp_connected
        _adp_connected=$(echo "$FINAL_ADAPTER_STATUSES" | jq -r '[.adapters[] | select(.connected==true)] | length')
        local _adp_total
        _adp_total=$(echo "$FINAL_ADAPTER_STATUSES" | jq -r '.adapters | length')
        echo -e "  ${CYAN}ℹ${NC}  适配器状态: ${_adp_connected}/${_adp_total} 已连接"
    fi

    echo ""
    echo -e "${BOLD}═══════════════════════════════════════════${NC}"

    if [ "$all_pass" = true ]; then
        echo -e "${GREEN}${BOLD}  🎉 ALL PLATFORMS PASSED${NC}"
    else
        echo -e "${YELLOW}${BOLD}  ⚠️  部分平台需要关注${NC}"
    fi

    echo -e "${BOLD}═══════════════════════════════════════════${NC}"
    echo ""
    info "完整日志: $LOG_FILE"

    phase_timer_end
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

    # 预检 jq 是否安装
    if ! command -v jq &>/dev/null; then
        fail "缺少依赖: jq（请安装 jq 后重试）"
        exit 1
    fi

    phase0_preflight
    phase1_build_and_start
    phase2_wait_for_adapters
    phase3_wait_for_messages
    phase4_auto_reply
    phase5_verify_and_report
    print_phase_timing_summary
}

main "$@"
