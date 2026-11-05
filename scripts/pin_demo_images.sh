#!/bin/bash
# Pin the demo composes' sigstack-near images to the digests CI built for a commit.
#
#   ./scripts/pin_demo_images.sh          # newest build matching origin/main
#   ./scripts/pin_demo_images.sh <sha>    # a specific commit
#
# Reads the `sha-<commit>` tags the "images" workflow pushes, so the pinned digest is
# traceable to the commit it was built from. The packages must be public.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSES=("$ROOT/docker/phala-demo.yaml" "$ROOT/docker/phala-demo-register.yaml")
OWNER=breadchaincoop
IMAGES=(sigstack-near-bot-tee sigstack-near-registration-proxy)
# Keep in sync with the push paths in .github/workflows/images.yml.
IMAGE_INPUTS=(crates Cargo.toml Cargo.lock 'docker/Dockerfile*' .github/workflows/images.yml)
ACCEPT="application/vnd.oci.image.index.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json"

# Prints the digest of $1:sha-$2, or nothing if that tag doesn't exist.
image_digest() {
  local token
  token=$(curl -fsS "https://ghcr.io/token?scope=repository:$OWNER/$1:pull" |
    sed -n 's/.*"token":"\([^"]*\)".*/\1/p') || return 0
  curl -fsSI -H "Authorization: Bearer $token" -H "Accept: $ACCEPT" \
    "https://ghcr.io/v2/$OWNER/$1/manifests/sha-$2" 2>/dev/null |
    tr -d '\r' | awk 'tolower($1) == "docker-content-digest:" { print $2 }' || true
}

# The images workflow only runs when image inputs change, so most main commits have no
# tag. Any commit whose inputs match main's builds identical images; take the newest
# such commit that was actually built.
newest_built_commit() {
  local head commit
  head=$(git -C "$ROOT" rev-parse origin/main)
  for commit in $(git -C "$ROOT" rev-list --max-count=200 origin/main); do
    git -C "$ROOT" diff --quiet "$commit" "$head" -- "${IMAGE_INPUTS[@]}" || break
    if [ -n "$(image_digest "${IMAGES[0]}" "$commit")" ]; then
      echo "$commit"
      return
    fi
  done
  echo "No published images match origin/main ($head). Run the images workflow on main" \
    "(gh workflow run images.yml --ref main), then retry." >&2
  exit 1
}

if [ $# -ge 1 ]; then
  SHA="$1"
else
  git -C "$ROOT" fetch --quiet origin main
  SHA=$(newest_built_commit)
fi

for name in "${IMAGES[@]}"; do
  digest=$(image_digest "$name" "$SHA")
  if [ -z "$digest" ]; then
    echo "No $name image tagged sha-$SHA. Build that commit with the images workflow, or check the package is public." >&2
    exit 1
  fi
  for compose in "${COMPOSES[@]}"; do
    sed -i.bak -E "s#(ghcr\.io/$OWNER/$name)@sha256:[0-9a-f]{64}#\1@$digest#" "$compose"
    rm -f "$compose.bak"
  done
  echo "$name@$digest"
done

echo "Pinned to $SHA in docker/phala-demo.yaml and docker/phala-demo-register.yaml"
