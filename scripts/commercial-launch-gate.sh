#!/usr/bin/env bash
set -euo pipefail
usage() { echo "Usage: commercial-launch-gate.sh [--offline] EVIDENCE_ENV [REPORT_JSON]" >&2; }
OFFLINE=false
if [ "${1:-}" = "--offline" ]; then OFFLINE=true; shift; fi
[ "$#" -ge 1 ] && [ "$#" -le 2 ] || { usage; exit 64; }
EVIDENCE_ENV=$1; REPORT_JSON=${2:-}
[ -f "$EVIDENCE_ENV" ] || { echo "Evidence file not found: $EVIDENCE_ENV" >&2; exit 66; }
evidence_mode=$(stat -f '%Lp' "$EVIDENCE_ENV" 2>/dev/null || stat -c '%a' "$EVIDENCE_ENV" 2>/dev/null || echo '')
[[ "$evidence_mode" =~ ^[0-7]{3,4}$ ]] || { echo "Unable to determine evidence file permissions" >&2; exit 65; }
permission_digits=${evidence_mode: -3}
group_digit=${permission_digits:1:1}; other_digit=${permission_digits:2:1}
[ "$group_digit" = 0 ] && [ "$other_digit" = 0 ] || {
  echo "Evidence file must not grant group or other permissions (mode $evidence_mode)" >&2; exit 65;
}
ROOT=$(git rev-parse --show-toplevel)
command -v jq >/dev/null 2>&1 || { echo "commercial launch gate requires jq" >&2; exit 69; }
seen_keys=""
while IFS= read -r line || [ -n "$line" ]; do
  line=${line%$'\r'}
  case "$line" in ''|'#'*) continue;; esac
  [[ "$line" =~ ^[A-Z][A-Z0-9_]*= ]] || { echo "Invalid evidence line: $line" >&2; exit 65; }
  key=${line%%=*}; value=${line#*=}
  case " $seen_keys " in *" $key "*) echo "Duplicate evidence key: $key" >&2; exit 65;; esac
  seen_keys="$seen_keys $key"
done < "$EVIDENCE_ENV"

evidence_get() {
  local wanted=$1 line key
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%$'\r'}
    case "$line" in ''|'#'*) continue;; esac
    key=${line%%=*}
    if [ "$key" = "$wanted" ]; then printf '%s' "${line#*=}"; return; fi
  done < "$EVIDENCE_ENV"
}

required_values=(LEGAL_ENTITY SERVICE_BASE_URL TERMS_URL PRIVACY_URL DPA_URL SUPPORT_EMAIL SECURITY_EMAIL BILLING_MODE STORAGE_BACKEND)
required_files=(LEGAL_APPROVAL_REPORT RELEASE_APPROVAL_REPORT DATA_PROCESSING_INVENTORY INCIDENT_RUNBOOK BACKUP_DRILL_REPORT DATABASE_MIGRATION_REPORT LOAD_TEST_REPORT ALERT_DELIVERY_REPORT ROLLBACK_DRILL_REPORT)
failures=(); checks=0
fail() { failures+=("$1"); }
for key in "${required_values[@]}"; do
  checks=$((checks + 1)); value=$(evidence_get "$key")
  [ -n "$value" ] || fail "missing value: $key"
  case "$value" in *REPLACE*|*example.com*|Example*) fail "placeholder value: $key";; esac
done
for key in SERVICE_BASE_URL TERMS_URL PRIVACY_URL DPA_URL; do
  [[ "$(evidence_get "$key")" == https://* ]] || fail "$key must use HTTPS"
done
for key in SUPPORT_EMAIL SECURITY_EMAIL; do
  [[ "$(evidence_get "$key")" =~ ^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$ ]] || fail "invalid email: $key"
done
case "$(evidence_get BILLING_MODE)" in invoice|provider) ;; *) fail "BILLING_MODE must be invoice or provider";; esac
case "$(evidence_get STORAGE_BACKEND)" in sqlite|postgres) ;; *) fail "STORAGE_BACKEND must be sqlite or postgres";; esac
MAX_AGE=$(evidence_get MAX_EVIDENCE_AGE_DAYS); MAX_AGE=${MAX_AGE:-90}
[[ "$MAX_AGE" =~ ^[1-9][0-9]*$ ]] || { fail "MAX_EVIDENCE_AGE_DAYS must be positive"; MAX_AGE=90; }
now=$(date +%s)
for key in "${required_files[@]}"; do
  checks=$((checks + 1)); path=$(evidence_get "$key")
  [ -n "$path" ] || { fail "missing path: $key"; continue; }
  [[ "$path" = /* ]] || path="$ROOT/$path"
  [ -s "$path" ] || { fail "missing or empty evidence: $key ($path)"; continue; }
  modified=$(stat -f '%m' "$path" 2>/dev/null || stat -c '%Y' "$path" 2>/dev/null || echo 0)
  age_days=$(( (now - modified) / 86400 ))
  [ "$age_days" -le "$MAX_AGE" ] || fail "stale evidence: $key (${age_days}d > ${MAX_AGE}d)"
  if [ "$key" = LOAD_TEST_REPORT ]; then
    jq -e 'type == "object" and .passed == true' "$path" >/dev/null 2>&1 || fail "load test report is invalid or did not pass"
  fi
  if [ "$key" = DATABASE_MIGRATION_REPORT ]; then
    backend=$(evidence_get STORAGE_BACKEND)
    release_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
    jq -e --arg backend "$backend" --arg release "$release_version" '
      type == "object" and
      .passed == true and
      .backend == $backend and
      (.from_version | type == "string" and test("^[0-9]+\\.[0-9]+\\.[0-9]+([+-][0-9A-Za-z.-]+)?$")) and
      .to_version == $release and
      (.duration_ms | type == "number" and . > 0 and floor == .) and
      .row_counts_verified == true and
      .rollback_passed == true
    ' "$path" >/dev/null 2>&1 || fail "database migration report is invalid, mismatched, or did not pass all checks"
  fi
done

if [ "$OFFLINE" = false ] && [ -n "$(evidence_get SERVICE_BASE_URL)" ]; then
  base=$(evidence_get SERVICE_BASE_URL); base=${base%/}
  for endpoint in live ready; do
    checks=$((checks + 1))
    curl --fail --silent --show-error --max-time 15 "$base/api/v1/$endpoint" >/dev/null || fail "online probe failed: $endpoint"
  done
  checks=$((checks + 1))
  headers=$(curl --fail --silent --show-error --max-time 15 --dump-header - --output /dev/null "$base/api/v1/live" 2>/dev/null || true)
  printf '%s' "$headers" | tr -d '\r' | grep -Eqi '^strict-transport-security:' || fail "TLS proxy missing Strict-Transport-Security"
  for key in TERMS_URL PRIVACY_URL DPA_URL; do
    checks=$((checks + 1))
    curl --fail --silent --show-error --location --max-time 15 --output /dev/null "$(evidence_get "$key")" || fail "public legal URL unavailable: $key"
  done
fi

passed=true; [ "${#failures[@]}" -eq 0 ] || passed=false
if [ -n "$REPORT_JSON" ]; then
  umask 077; failure_json=""
  for item in "${failures[@]}"; do
    escaped=${item//\\/\\\\}; escaped=${escaped//\"/\\\"}
    [ -z "$failure_json" ] || failure_json+=","
    failure_json+="\"$escaped\""
  done
  printf '{"timestamp":"%s","offline":%s,"checks":%s,"passed":%s,"failures":[%s]}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$OFFLINE" "$checks" "$passed" "$failure_json" > "$REPORT_JSON"
  chmod 600 "$REPORT_JSON"
fi
if [ "$passed" = true ]; then
  echo "Commercial launch gate passed ($checks checks, offline=$OFFLINE)"
else
  printf 'Commercial launch gate failed:\n' >&2; printf ' - %s\n' "${failures[@]}" >&2; exit 1
fi
