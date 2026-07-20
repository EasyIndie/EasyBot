#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/easybot-production-up.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/bin" "$TMP/secrets" "$TMP/data"
printf 'strong-admin-password\n' > "$TMP/secrets/admin_password"
printf 'postgresql://easybot:secret@db/easybot\n' > "$TMP/secrets/database_url"
chmod 600 "$TMP/secrets/admin_password" "$TMP/secrets/database_url"
cat > "$TMP/bin/gh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat > "$TMP/bin/docker" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$DOCKER_CALLS"
exit 0
EOF
cat > "$TMP/bin/stat" <<'EOF'
#!/usr/bin/env bash
if [ -n "${STAT_BAD:-}" ] && [ "${@: -1}" = "$STAT_BAD" ] && [[ "$2" != *%u* ]]; then echo 644; exit; fi
case "$2" in
  *%u*) echo 10001 ;;
  *) [ -d "${@: -1}" ] && echo 700 || echo 600 ;;
esac
EOF
chmod +x "$TMP/bin/gh" "$TMP/bin/docker" "$TMP/bin/stat"
export DOCKER_CALLS="$TMP/docker-calls"
export EASYBOT_IMAGE="ghcr.io/easyindie/easybot@sha256:$(printf 'b%.0s' {1..64})"
export EASYBOT_DOMAIN=gateway.acme.test
export EASYBOT_SECRETS_DIR="$TMP/secrets"
export EASYBOT_DATA_DIR="$TMP/data"
export GITHUB_REPOSITORY=EasyIndie/EasyBot
PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-up.sh" --check >/dev/null
grep -Fq 'compose -f' "$DOCKER_CALLS"
grep -Fq 'config --quiet' "$DOCKER_CALLS"
if EASYBOT_IMAGE=ghcr.io/easyindie/easybot:latest PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-up.sh" --check >/dev/null 2>&1; then
  echo "production preflight accepted a mutable image tag" >&2
  exit 1
fi
if STAT_BAD="$TMP/secrets/admin_password" PATH="$TMP/bin:$PATH" "$ROOT/scripts/production-up.sh" --check >/dev/null 2>&1; then
  echo "production preflight accepted a group-readable secret" >&2
  exit 1
fi
echo "Production deployment preflight test passed"
