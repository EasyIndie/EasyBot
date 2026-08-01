#!/usr/bin/env bash
set -euo pipefail
ROOT=$(git rev-parse --show-toplevel)
TMP=$(mktemp -d "${TMPDIR:-/tmp}/easybot-launch-gate.XXXXXX")
trap 'rm -rf "$TMP"' EXIT
VERSION=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
for name in legal release-approval inventory incident backup alert rollback; do printf 'evidence: %s\n' "$name" > "$TMP/$name.txt"; done
printf '{"passed":true,"p95_ms":10}\n' > "$TMP/load.json"
printf '{"passed":true,"backend":"postgres","from_version":"0.0.14","to_version":"%s","duration_ms":1234,"row_counts_verified":true,"rollback_passed":true}\n' "$VERSION" > "$TMP/migration.json"
printf '%s\n' 'LEGAL_ENTITY=Acme Technology Ltd.' 'SERVICE_BASE_URL=https://gateway.acme.test' \
  'TERMS_URL=https://acme.test/terms' 'PRIVACY_URL=https://acme.test/privacy' 'DPA_URL=https://acme.test/dpa' \
  'SUPPORT_EMAIL=support@acme.test' 'SECURITY_EMAIL=security@acme.test' 'BILLING_MODE=invoice' \
  'STORAGE_BACKEND=postgres' \
  "LEGAL_APPROVAL_REPORT=$TMP/legal.txt" "RELEASE_APPROVAL_REPORT=$TMP/release-approval.txt" \
  "DATA_PROCESSING_INVENTORY=$TMP/inventory.txt" \
  "INCIDENT_RUNBOOK=$TMP/incident.txt" "BACKUP_DRILL_REPORT=$TMP/backup.txt" \
  "DATABASE_MIGRATION_REPORT=$TMP/migration.json" \
  "LOAD_TEST_REPORT=$TMP/load.json" "ALERT_DELIVERY_REPORT=$TMP/alert.txt" \
  "ROLLBACK_DRILL_REPORT=$TMP/rollback.txt" > "$TMP/evidence.env"
chmod 600 "$TMP/evidence.env"
"$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" "$TMP/report.json" >/dev/null
grep -Eq '"passed":true' "$TMP/report.json"
chmod 644 "$TMP/evidence.env"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted world-readable evidence file" >&2; exit 1; fi
chmod 600 "$TMP/evidence.env"
printf '{"passed":false}\n' > "$TMP/load.json"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted failed load evidence" >&2; exit 1; fi
printf '{"passed":true,"backend":"sqlite","from_version":"0.0.14","to_version":"%s","duration_ms":1234,"row_counts_verified":true,"rollback_passed":true}\n' "$VERSION" > "$TMP/migration.json"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted migration evidence for wrong backend" >&2; exit 1; fi
printf '{"passed":true,"backend":"postgres","from_version":"0.0.14","to_version":"%s","duration_ms":1234,"row_counts_verified":true,"rollback_passed":false}\n' "$VERSION" > "$TMP/migration.json"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted failed migration rollback" >&2; exit 1; fi
printf '{"passed":true,"backend":"postgres","from_version":"0.0.14","to_version":"%s","duration_ms":1234,"row_counts_verified":false,"rollback_passed":true}\n' "$VERSION" > "$TMP/migration.json"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted unverified migration row counts" >&2; exit 1; fi
printf '{"metadata":{"passed":true},"backend":"postgres","from_version":"0.0.14","to_version":"%s","duration_ms":1234,"row_counts_verified":true,"rollback_passed":true}\n' "$VERSION" > "$TMP/migration.json"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted nested migration pass flag" >&2; exit 1; fi
printf '{"passed":"true","backend":"postgres","from_version":"0.0.14","to_version":"%s","duration_ms":"1234","row_counts_verified":true,"rollback_passed":true}\n' "$VERSION" > "$TMP/migration.json"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted wrong migration field types" >&2; exit 1; fi
printf '{"passed":true,"backend":"postgres","from_version":"0.0.13","to_version":"0.0.14","duration_ms":1234,"row_counts_verified":true,"rollback_passed":true}\n' > "$TMP/migration.json"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted migration report for another release" >&2; exit 1; fi
printf '{"passed":true,"backend":"postgres","from_version":"0.0.14","to_version":"%s","duration_ms":1234,"row_counts_verified":true,"rollback_passed":true}\n' "$VERSION" > "$TMP/migration.json"
printf 'SERVICE_BASE_URL=http://gateway.acme.test\n' >> "$TMP/evidence.env"
if "$ROOT/scripts/commercial-launch-gate.sh" --offline "$TMP/evidence.env" >/dev/null 2>&1; then echo "accepted duplicate evidence" >&2; exit 1; fi
echo "Commercial launch gate self-test passed"
