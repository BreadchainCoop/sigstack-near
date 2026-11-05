#!/bin/bash
# Pin docker/phala-demo.yaml's sigstack-near images to the digests CI built for a commit.
#
#   ./scripts/pin_demo_images.sh          # commit at origin/main
#   ./scripts/pin_demo_images.sh <sha>    # a specific commit
#
# Reads the `sha-<commit>` tags the "images" workflow pushes, so the pinned digest is
# traceable to the commit it was built from. The packages must be public.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE="${COMPOSE:-$ROOT/docker/phala-demo.yaml}"
OWNER=breadchaincoop
IMAGES=(sigstack-near-bot-tee sigstack-near-registration-proxy)
ACCEPT="application/vnd.oci.image.index.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json"

SHA="${1:-$(git -C "$ROOT" rev-parse origin/main)}"

for name in "${IMAGES[@]}"; do
  token=$(curl -fsS "https://ghcr.io/token?scope=repository:$OWNER/$name:pull" |
    sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
  digest=$(curl -fsSI -H "Authorization: Bearer $token" -H "Accept: $ACCEPT" \
    "https://ghcr.io/v2/$OWNER/$name/manifests/sha-$SHA" |
    tr -d '\r' | awk 'tolower($1) == "docker-content-digest:" { print $2 }')
  if [ -z "$digest" ]; then
    echo "No $name image tagged sha-$SHA: not built yet, or the package is not public." >&2
    exit 1
  fi
  sed -i.bak -E "s#(ghcr\.io/$OWNER/$name)@sha256:[0-9a-f]{64}#\1@$digest#" "$COMPOSE"
  rm -f "$COMPOSE.bak"
  echo "$name@$digest"
done

echo "Pinned to $SHA in ${COMPOSE#"$ROOT"/}"
