#!/usr/bin/env bash
set -euo pipefail

COUNT="${1:-10}"
case "$COUNT" in
    ''|*[!0-9]*) echo "ERROR: count must be a positive integer" >&2; exit 2 ;;
esac
[ "$COUNT" -gt 0 ] || { echo "ERROR: count must be greater than zero" >&2; exit 2; }
command -v gh >/dev/null || { echo "ERROR: gh is required" >&2; exit 1; }
command -v jq >/dev/null || { echo "ERROR: jq is required" >&2; exit 1; }

TMP_FILE=$(mktemp "${TMPDIR:-/tmp}/easybot-ci-history.XXXXXX")
STATS_FILE=$(mktemp "${TMPDIR:-/tmp}/easybot-ci-stats.XXXXXX")
trap 'rm -f "$TMP_FILE" "$STATS_FILE"' EXIT

gh run list --workflow CI --event pull_request --status completed --limit "$COUNT" \
    --json databaseId,createdAt,updatedAt,conclusion,url > "$TMP_FILE"

echo "Recent PR CI runs (seconds)"
echo "run_id       max-job-wait   wall   conclusion"
while IFS=$'\t' read -r run_id created_at updated_at conclusion; do
    jobs_json=$(gh run view "$run_id" --json jobs)
    max_wait=$(jq -n --arg created "$created_at" --argjson data "$jobs_json" \
        '$data.jobs | map((.startedAt|fromdateiso8601)-($created|fromdateiso8601)) | max // 0')
    wall=$(jq -n --arg created "$created_at" --arg updated "$updated_at" \
        '($updated|fromdateiso8601)-($created|fromdateiso8601)')
    printf '%-12s %12s %6s   %s\n' "$run_id" "$max_wait" "$wall" "$conclusion"
    printf '%s\t%s\n' "$max_wait" "$wall" >> "$STATS_FILE"
done < <(jq -r '.[] | [(.databaseId|tostring), .createdAt, .updatedAt, .conclusion] | @tsv' "$TMP_FILE")

percentile() {
    local column="$1"
    local pct="$2"
    awk -v column="$column" '{ print $column }' "$STATS_FILE" \
        | sort -n \
        | awk -v p="$pct" '{ values[NR]=$1 } END { if (NR == 0) { print "n/a"; exit } rank=int((NR*p+99)/100); if (rank < 1) rank=1; if (rank > NR) rank=NR; print values[rank] }'
}

echo ""
echo "max job wait P50/P95: $(percentile 1 50)s / $(percentile 1 95)s"
echo "wall         P50/P95: $(percentile 2 50)s / $(percentile 2 95)s"
