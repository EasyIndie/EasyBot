#!/usr/bin/env bash
set -euo pipefail
# 前端源码采用「垂直切片」多文件（templates/js/*.js），由 build.rs 按文件名排序
# 拼接进 templates/gen/admin.html。因此本测试在源文件集合上校验 XSS 不变量，
# 而不是单个 admin.js（该文件已在切片重构中移除）。
JS_DIR=crates/easybot-api/templates/js
mapfile -t FILES < <(find "$JS_DIR" -maxdepth 1 -name '*.js' -type f | sort)
[ "${#FILES[@]}" -gt 0 ] || { echo "no frontend js sources found under $JS_DIR" >&2; exit 1; }

for unsafe in '${m.platform}' '${m.chat_id}' '${m.text' '${s.platform}' '${s.last_message' '${getDisplayName(s)}' '${k.name}'; do
  if grep -Fq "$unsafe" "${FILES[@]}"; then
    echo "unescaped platform/customer data interpolation remains: $unsafe" >&2
    exit 1
  fi
done
if grep -Eq 'sessionStorage\.setItem\([^,]*(api|key|token)' "${FILES[@]}"; then
  echo "credential is persisted to sessionStorage" >&2
  exit 1
fi
for required in "escapeHtml(String(m.chat_id" "escapeHtml(String(m.text" \
  "escapeHtml(String(s.last_message" "escapeHtml(String(getDisplayName(s)"; do
  grep -Fq "$required" "${FILES[@]}" || { echo "missing XSS escaping invariant: $required" >&2; exit 1; }
done
echo "Admin stored-XSS invariant test passed"
