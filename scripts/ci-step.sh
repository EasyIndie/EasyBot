#!/usr/bin/env bash
set -euo pipefail

NAME="${1:?step name is required}"
shift
START=$(date +%s)
if "$@"; then
    STATUS=0
else
    STATUS=$?
fi
END=$(date +%s)
DURATION=$((END - START))
TARGET_SIZE="n/a"
[ -d target ] && TARGET_SIZE=$(du -sh target 2>/dev/null | awk '{print $1}')

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    if ! grep -q '^## Build performance' "$GITHUB_STEP_SUMMARY" 2>/dev/null; then
        printf '## Build performance\n\n| Step | Duration | target size |\n|---|---:|---:|\n' >> "$GITHUB_STEP_SUMMARY"
    fi
    printf '| %s | %ss | %s |\n' "$NAME" "$DURATION" "$TARGET_SIZE" >> "$GITHUB_STEP_SUMMARY"
fi
exit "$STATUS"
