#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: load-test.sh URL TOKEN_FILE [REQUESTS] [CONCURRENCY]

Runs a bounded authenticated HTTP capacity gate.
Environment:
  MAX_ERROR_RATE   Maximum non-2xx percentage (default: 1)
  MAX_P95_MS       Maximum p95 latency in milliseconds (default: 1000)
  REPORT_FILE      Optional JSON report output path

The token is read from a mode-0600 file and placed in a temporary curl config,
so it is not exposed in the process command line.
EOF
}

[ "$#" -ge 2 ] && [ "$#" -le 4 ] || { usage >&2; exit 64; }
URL=$1
TOKEN_FILE=$2
REQUESTS=${3:-1000}
CONCURRENCY=${4:-20}
MAX_ERROR_RATE=${MAX_ERROR_RATE:-1}
MAX_P95_MS=${MAX_P95_MS:-1000}

case "$URL" in https://*|http://127.0.0.1:*|http://localhost:*) ;; *)
  echo "URL must use HTTPS (HTTP is allowed only for loopback testing)" >&2
  exit 64
esac
for value in "$REQUESTS" "$CONCURRENCY"; do
  case "$value" in ''|*[!0-9]*) echo "requests/concurrency must be positive integers" >&2; exit 64;; esac
  [ "$value" -gt 0 ] || { echo "requests/concurrency must be positive" >&2; exit 64; }
done
[ "$CONCURRENCY" -le "$REQUESTS" ] || { echo "concurrency cannot exceed requests" >&2; exit 64; }
[ -f "$TOKEN_FILE" ] || { echo "token file not found: $TOKEN_FILE" >&2; exit 66; }
mode=$(stat -c '%a' "$TOKEN_FILE" 2>/dev/null || stat -f '%Lp' "$TOKEN_FILE" 2>/dev/null || true)
[ "$mode" = "600" ] || { echo "token file must have mode 0600 (found ${mode:-unknown})" >&2; exit 65; }
token=$(tr -d '\r\n' < "$TOKEN_FILE")
[ -n "$token" ] || { echo "token file is empty" >&2; exit 65; }

tmp=$(mktemp -d "${TMPDIR:-/tmp}/easybot-load.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
config="$tmp/curl.conf"
results="$tmp/results"
umask 077
printf 'silent\nshow-error\noutput = "/dev/null"\nheader = "Authorization: Bearer %s"\n' "$token" > "$config"
unset token

start=$(date +%s)
seq 1 "$REQUESTS" | xargs -P "$CONCURRENCY" -I '{}' sh -c \
  'curl --config "$1" --max-time 30 --write-out "%{http_code} %{time_total}\n" "$2" || printf "000 30.000000\n"' \
  _ "$config" "$URL" > "$results"
elapsed=$(( $(date +%s) - start ))

total=$(wc -l < "$results" | tr -d ' ')
success=$(awk '$1 >= 200 && $1 < 300 {count++} END {print count+0}' "$results")
errors=$((total - success))
error_rate=$(awk -v e="$errors" -v t="$total" 'BEGIN {printf "%.4f", (t ? e*100/t : 100)}')
awk '{printf "%.3f\n", $2*1000}' "$results" | sort -n > "$tmp/latencies"
p95_index=$(( (total * 95 + 99) / 100 ))
p95_ms=$(sed -n "${p95_index}p" "$tmp/latencies")
rps=$(awk -v t="$total" -v s="$elapsed" 'BEGIN {printf "%.2f", t/(s > 0 ? s : 1)}')
passed=$(awk -v e="$error_rate" -v me="$MAX_ERROR_RATE" -v p="$p95_ms" -v mp="$MAX_P95_MS" \
  'BEGIN {print (e <= me && p <= mp) ? "true" : "false"}')

printf 'requests=%s success=%s errors=%s error_rate=%s%% p95_ms=%s rps=%s elapsed_s=%s passed=%s\n' \
  "$total" "$success" "$errors" "$error_rate" "$p95_ms" "$rps" "$elapsed" "$passed"

if [ -n "${REPORT_FILE:-}" ]; then
  printf '{"timestamp":"%s","url":"%s","requests":%s,"concurrency":%s,"success":%s,"errors":%s,"error_rate_percent":%s,"p95_ms":%s,"rps":%s,"elapsed_seconds":%s,"passed":%s}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$URL" "$total" "$CONCURRENCY" "$success" "$errors" \
    "$error_rate" "$p95_ms" "$rps" "$elapsed" "$passed" > "$REPORT_FILE"
  chmod 600 "$REPORT_FILE"
fi

[ "$passed" = "true" ]
