#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 IMAGE@sha256:DIGEST OWNER/REPOSITORY [SIGNER_WORKFLOW]" >&2
}

[ "$#" -ge 2 ] && [ "$#" -le 3 ] || { usage; exit 64; }
IMAGE=$1
REPOSITORY=$2
SIGNER_WORKFLOW=${3:-"$REPOSITORY/.github/workflows/release.yml"}

[[ "$IMAGE" =~ ^[a-z0-9.-]+(/[a-z0-9._-]+)+@sha256:[0-9a-f]{64}$ ]] || {
  echo "Container reference must be a lowercase immutable IMAGE@sha256:DIGEST reference" >&2
  exit 65
}
[[ "$REPOSITORY" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo "Repository must use OWNER/REPOSITORY format" >&2
  exit 65
}
[[ "$SIGNER_WORKFLOW" == "$REPOSITORY/"* ]] || {
  echo "Signer workflow must belong to $REPOSITORY" >&2
  exit 65
}
command -v gh >/dev/null 2>&1 || { echo "gh is required" >&2; exit 69; }

artifact="oci://$IMAGE"
common=("$artifact" --repo "$REPOSITORY" --signer-workflow "$SIGNER_WORKFLOW" --deny-self-hosted-runners)
gh attestation verify "${common[@]}"
gh attestation verify "${common[@]}" --predicate-type https://spdx.dev/Document/v2.3

echo "Container provenance and SBOM attestations verified for $IMAGE"
