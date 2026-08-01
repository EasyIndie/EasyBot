#!/usr/bin/env bash
set -euo pipefail

ROOT=$(git rev-parse --show-toplevel)
cd "$ROOT"

failures=0
while IFS= read -r action; do
  case "$action" in
    ./*) continue ;;
  esac
  if [[ ! "$action" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(/[^@[:space:]]+)?@[0-9a-f]{40}$ ]]; then
    echo "External GitHub Action is not pinned to a 40-character commit SHA: $action" >&2
    failures=$((failures + 1))
  fi
done < <(grep -RhoE --include='*.yml' --include='*.yaml' 'uses:[[:space:]]*[^[:space:]#]+' .github | sed -E 's/^uses:[[:space:]]*//' | sort -u)

[ "$failures" -eq 0 ] || exit 1

for workflow in .github/workflows/*.yml .github/workflows/*.yaml; do
  [ -e "$workflow" ] || continue
  grep -Eq '^permissions:[[:space:]]*($|\{)' "$workflow" || {
    echo "$workflow must declare top-level token permissions" >&2
    exit 1
  }
done

release_workflow=.github/workflows/release.yml
grep -Eq '^  approve-commercial-release:$' "$release_workflow"
grep -Eq '^    environment: commercial-release$' "$release_workflow"
for job in create-release docker; do
  awk -v job="$job" '
    $0 == "  " job ":" { inside=1; next }
    inside && /^  [A-Za-z0-9_-]+:/ { exit found ? 0 : 1 }
    inside && /needs:.*approve-commercial-release/ { found=1 }
    END { if (inside) exit found ? 0 : 1 }
  ' "$release_workflow" || {
    echo "release job $job must depend on protected commercial approval" >&2
    exit 1
  }
done
echo "GitHub Actions pinning policy passed"
