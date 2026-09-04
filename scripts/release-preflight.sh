#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
cd "$ROOT"

VERSION=${1:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)}
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.-]+)?$ ]] || {
  echo "Invalid semantic version: $VERSION" >&2; exit 1;
}

CURRENT=$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
[ "$CURRENT" = "$VERSION" ] || { echo "Cargo.toml version mismatch" >&2; exit 1; }
grep -q "^## \[$VERSION\]" CHANGELOG.md || { echo "CHANGELOG entry [$VERSION] missing" >&2; exit 1; }

for file in LICENSE SECURITY.md README.md deny.toml Dockerfile.release deploy/Caddyfile \
  deploy/docker-compose.production.yml scripts/production-up.sh \
  scripts/production-backup.sh \
  scripts/production-restore.sh \
  scripts/production-backup-retention.sh \
  crates/easybot-api/src/snapshots/easybot_api__openapi__tests__openapi_v1_contract.snap \
  "docs/05 commercial-readiness.md" "docs/06 privacy-and-data-rights.md" \
  commercial/evidence.env.example \
  "docs/07 commercial-release.md" "docs/13 commercialization-roadmap.md" \
  deploy/gateway.production.local.yaml \
  "docs/09 billing-integration.md" \
  "docs/10 message-idempotency.md" \
  scripts/easybot-backup.sh scripts/test-backup.sh easybot-alerts.yml; do
  [ -s "$file" ] || { echo "Required commercial release file missing: $file" >&2; exit 1; }
done
if find crates/easybot-api/src/snapshots -name '*.snap.new' -type f | grep -q .; then
  echo "Unreviewed OpenAPI snapshot update is pending" >&2
  exit 1
fi

PREFLIGHT_DIR="${TMPDIR:-/tmp}/easybot-preflight-$$"
trap 'rm -rf "$PREFLIGHT_DIR"' EXIT
cargo run -q -p easybot-bin -- --init --dir "$PREFLIGHT_DIR" >/dev/null 2>&1
grep -q '^# EASYBOT_ALLOW_PLAINTEXT=true$' "$PREFLIGHT_DIR/.env" || {
  echo "Generated environment must keep plaintext opt-in commented" >&2; exit 1;
}

bash -n scripts/easybot-backup.sh
bash -n scripts/test-backup.sh
bash -n scripts/commercial-launch-gate.sh scripts/test-commercial-launch-gate.sh
bash -n scripts/test-admin-xss.sh
bash -n scripts/test-container-contract.sh
bash -n scripts/verify-container-image.sh scripts/test-container-image-verification.sh
bash -n scripts/test-actions-pinning.sh
bash -n scripts/production-up.sh scripts/test-production-up.sh
bash -n scripts/production-backup.sh
bash -n scripts/test-production-backup.sh
bash -n scripts/production-restore.sh scripts/test-production-restore.sh
bash -n scripts/production-backup-retention.sh scripts/test-production-backup-retention.sh
bash -n scripts/test-release-workflow-security.sh
bash scripts/test-backup.sh >/dev/null
bash scripts/test-release-assets.sh >/dev/null
bash scripts/test-commercial-launch-gate.sh >/dev/null
bash scripts/test-admin-xss.sh >/dev/null
bash scripts/test-container-contract.sh >/dev/null
bash scripts/test-container-image-verification.sh >/dev/null
bash scripts/test-actions-pinning.sh >/dev/null
bash scripts/test-production-up.sh >/dev/null
bash scripts/test-production-backup.sh >/dev/null
bash scripts/test-production-restore.sh >/dev/null
bash scripts/test-production-backup-retention.sh >/dev/null
bash scripts/test-release-workflow-security.sh >/dev/null
cargo test -q -p easybot-api openapi_v1_contract_snapshot -- --test-threads=1
echo "Commercial release preflight passed for v$VERSION"
