#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/easybot-release-assets.XXXXXX")
trap 'rm -rf "$TMP_DIR"' EXIT
printf 'verified release artifact' > "$TMP_DIR/easybot-test-target"
(
  cd "$TMP_DIR"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum easybot-test-target > checksums.txt
  else
    shasum -a 256 easybot-test-target > checksums.txt
  fi
)
"$ROOT/scripts/verify-release-assets.sh" "$TMP_DIR" >/dev/null
printf 'tampered' >> "$TMP_DIR/easybot-test-target"
if "$ROOT/scripts/verify-release-assets.sh" "$TMP_DIR" >/dev/null 2>&1; then
  echo "Tampered release artifact unexpectedly passed verification" >&2
  exit 1
fi
echo "Release asset verification test passed"
