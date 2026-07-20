#!/usr/bin/env bash
set -euo pipefail

[ "$#" -ge 1 ] || { echo "Usage: $0 RELEASE_DIRECTORY [OWNER/REPOSITORY]" >&2; exit 2; }
DIR=$1
REPOSITORY=${2:-}
[ -f "$DIR/checksums.txt" ] || { echo "checksums.txt not found" >&2; exit 1; }

cd "$DIR"
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum --check checksums.txt
else
  shasum -a 256 -c checksums.txt
fi

if [ -n "$REPOSITORY" ]; then
  command -v gh >/dev/null 2>&1 || { echo "gh is required for attestation verification" >&2; exit 1; }
  find . -type f -name 'easybot-*' ! -name '*.spdx.json' -print0 | while IFS= read -r -d '' artifact; do
    gh attestation verify "$artifact" -R "$REPOSITORY"
  done
fi

echo "Release assets verified"
