#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/easybot-container-verify.XXXXXX")
trap 'rm -rf "$TMP"' EXIT

cat > "$TMP/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_CALLS"
EOF
chmod +x "$TMP/gh"
export GH_CALLS="$TMP/calls"
IMAGE="ghcr.io/easyindie/easybot@sha256:$(printf 'a%.0s' {1..64})"
PATH="$TMP:$PATH" "$ROOT/scripts/verify-container-image.sh" "$IMAGE" EasyIndie/EasyBot >/dev/null
[ "$(wc -l < "$GH_CALLS" | tr -d ' ')" = 2 ] || { echo "expected two attestation checks" >&2; exit 1; }
grep -Fq -- '--signer-workflow EasyIndie/EasyBot/.github/workflows/release.yml' "$GH_CALLS"
grep -Fq -- '--deny-self-hosted-runners' "$GH_CALLS"
grep -Fq -- '--predicate-type https://spdx.dev/Document/v2.3' "$GH_CALLS"

if PATH="$TMP:$PATH" "$ROOT/scripts/verify-container-image.sh" ghcr.io/easyindie/easybot:latest EasyIndie/EasyBot >/dev/null 2>&1; then
  echo "mutable container tag was accepted" >&2
  exit 1
fi
if PATH="$TMP:$PATH" "$ROOT/scripts/verify-container-image.sh" "$IMAGE" EasyIndie/EasyBot Other/Repo/.github/workflows/release.yml >/dev/null 2>&1; then
  echo "foreign signer workflow was accepted" >&2
  exit 1
fi
echo "Container image verification policy test passed"
