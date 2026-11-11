#!/bin/bash
# Deploy and initialize the Sigstack agent contract on NEAR testnet.
#
#   ./scripts/deploy_agent_contract.sh <contract-account> <owner-account> [wasm]
#
# Without [wasm], downloads the wasm the "contract" workflow built from the newest
# green main commit, so the deployed code_hash traces to a commit and CI run.
# Both accounts must exist with keys in the near CLI keychain. Measurements and
# PPIDs are approved separately, once the demo CVM's values are known.
set -euo pipefail

if [ $# -lt 2 ]; then
  sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
fi

CONTRACT_ID="$1"
OWNER_ID="$2"
NETWORK="${NETWORK:-testnet}"
REPO=BreadchainCoop/sigstack-near
# Agents re-register weekly with a fresh quote, so a revoked build or TCB drops off within a week.
EXPIRATION_MS=604800000

if [ $# -ge 3 ]; then
  WASM="$3"
else
  read -r RUN_ID HEAD_SHA < <(gh run list -R "$REPO" --workflow contract.yml --branch main \
    --status success --limit 1 --json databaseId,headSha -q '.[0] | "\(.databaseId) \(.headSha)"')
  if [ -z "${RUN_ID:-}" ]; then
    echo "No successful contract workflow run on main." >&2
    exit 1
  fi
  DIR=$(mktemp -d)
  gh run download "$RUN_ID" -R "$REPO" -n agent-contract-wasm -D "$DIR"
  WASM="$DIR/sigstack_agent.wasm"
  echo "Using wasm from run $RUN_ID (commit $HEAD_SHA)"
fi

echo "wasm sha256: $(shasum -a 256 "$WASM" | cut -d' ' -f1)"

near contract deploy "$CONTRACT_ID" use-file "$WASM" \
  with-init-call new json-args "$(printf '{"requires_tee":true,"attestation_expiration_time_ms":"%s","owner_id":"%s"}' "$EXPIRATION_MS" "$OWNER_ID")" \
  prepaid-gas '100.0 Tgas' attached-deposit '0 NEAR' \
  network-config "$NETWORK" sign-with-keychain send

near contract call-function as-read-only "$CONTRACT_ID" get_contract_info json-args '{}' \
  network-config "$NETWORK" now
