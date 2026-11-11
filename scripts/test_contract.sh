#!/bin/bash
# Run the agent contract's tests: unit tests, then the wasm build and the near-sandbox
# integration tests. The "contract" CI workflow runs this same script.
#
#   ./scripts/test_contract.sh
#
# Needs rustup and Docker. No TEE, NEAR account, or secrets. The wasm is built in the
# pinned cargo-near image, so it builds the same on Linux and macOS.
set -euo pipefail

cd "$(dirname "$0")/../contracts/agent"

# Same image and digest as Cargo.toml's reproducible_build metadata.
IMAGE=sourcescan/cargo-near@sha256:772638e343baeeea24e49062c7d424274f3441452cc06ce97fc4e5695b19fecc

echo "== Unit tests"
cargo test --lib --locked

echo "== Building the contract wasm in $IMAGE"
# The image runs as its own user: open target/ to it, then hand it back.
mkdir -p target
chmod -R a+rwX target
docker run --rm -v "$PWD":/workspace -w /workspace "$IMAGE" \
  cargo near build non-reproducible-wasm --locked --no-abi
docker run --rm -v "$PWD":/workspace -w /workspace --user root --entrypoint chown "$IMAGE" \
  -R "$(id -u):$(id -g)" target
sha256sum target/near/*.wasm 2>/dev/null || shasum -a 256 target/near/*.wasm

echo "== Integration tests (near-sandbox, loads the wasm above)"
cargo test --tests --locked
