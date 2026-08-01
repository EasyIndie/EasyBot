#!/usr/bin/env bash
set -euo pipefail
FILE=crates/easybot-api/templates/js/admin.js
[ -s "$FILE" ]
for unsafe in '${m.platform}' '${m.chat_id}' '${m.text' '${s.platform}' '${s.last_message' '${getDisplayName(s)}' '${k.name}'; do
  if grep -Fq "$unsafe" "$FILE"; then
    echo "unescaped platform/customer data interpolation remains: $unsafe" >&2
    exit 1
  fi
done
if grep -Eq 'sessionStorage\.setItem\([^,]*(api|key|token)' "$FILE"; then
  echo "credential is persisted to sessionStorage" >&2
  exit 1
fi
for required in "escapeHtml(String(m.chat_id" "escapeHtml(String(m.text" \
  "escapeHtml(String(s.last_message" "escapeHtml(String(getDisplayName(s)"; do
  grep -Fq "$required" "$FILE" || { echo "missing XSS escaping invariant: $required" >&2; exit 1; }
done
echo "Admin stored-XSS invariant test passed"
