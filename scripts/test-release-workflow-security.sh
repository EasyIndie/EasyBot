#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
WORKFLOW="$ROOT/.github/workflows/release.yml"

scan_line=$(grep -n 'name: Block critical and high container vulnerabilities' "$WORKFLOW" | cut -d: -f1)
provenance_line=$(grep -n 'name: Attest container build provenance' "$WORKFLOW" | cut -d: -f1)
[ -n "$scan_line" ] && [ -n "$provenance_line" ] && [ "$scan_line" -lt "$provenance_line" ] || {
  echo "container scan must run before provenance attestation" >&2
  exit 1
}
for policy in \
  'severity: CRITICAL,HIGH' \
  'ignore-unfixed: false' \
  'exit-code: 1' \
  'vuln-type: os,library'; do
  grep -Fq "$policy" "$WORKFLOW" || { echo "release scan policy missing: $policy" >&2; exit 1; }
done
AUDIT_WORKFLOW="$ROOT/.github/workflows/container-audit.yml"
grep -Fq 'cron: "30 2 * * *"' "$AUDIT_WORKFLOW"
grep -Fq 'severity: CRITICAL,HIGH' "$AUDIT_WORKFLOW"
grep -Fq 'ignore-unfixed: false' "$AUDIT_WORKFLOW"
grep -Fq 'exit-code: 1' "$AUDIT_WORKFLOW"
grep -Fq 'name: Validate production Caddy configuration' "$WORKFLOW"
grep -Fq 'validate --config /etc/caddy/Caddyfile --adapter caddyfile' "$WORKFLOW"
# Container tags must be derived from the verified release version, not the
# triggering ref, so the workflow_dispatch fallback (run on a branch ref) works.
if grep -Fq 'github.ref_type' "$WORKFLOW"; then
  echo "release container tags must not depend on github.ref_type" >&2
  exit 1
fi
grep -Fq 'name: Compute container tags' "$WORKFLOW" || {
  echo "release container tag derivation step is missing" >&2
  exit 1
}
echo "Release vulnerability policy test passed"
