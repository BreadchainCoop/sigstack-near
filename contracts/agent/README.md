> **Origin:** imported from [NearDeFi/shade-agent-framework](https://github.com/NearDeFi/shade-agent-framework/tree/35758f354ad9862fb590daf9843fea4974d52549/shade-contract-template) `shade-contract-template` at `35758f3` (Apache-2.0). Changes from upstream are in this repo's git history.

# sigstack-agent

NEAR contract that records which Sigstack bots are running an approved build in a TEE.
It uses the shade-attestation crate from the framework repo, pinned at `35758f3`.

## How it works

1. The bot generates a fresh ed25519 key inside the TEE on every boot. Its NEAR account is
   the implicit account of that key.
2. The bot asks dstack for a TDX quote whose report data is that account ID, and calls
   `register_agent` with the quote, its collateral, and the TCB info.
3. The contract verifies the quote (TCB status must be up to date), checks that the report
   data matches the caller, and checks the measurements (MRTD, RTMR0-2, the key-provider
   digest and the compose hash) and PPID against the approved lists.
4. Anyone can read `get_agent` / `get_agents` to see which bot accounts are valid right now,
   and which measurements they registered with.

The compose hash binds the build: [`docker/phala-demo.yaml`](../../docker/phala-demo.yaml)
pins every image by digest, so each code change is a new hash that the owner must approve
on-chain before a bot running it can register.

A registered agent stays valid only while its measurements and PPID are still approved and
its attestation is younger than `attestation_expiration_time_ms` (7 days on testnet). Once
any of those fails, **anyone** can call `prune_agent` to remove it; the reasons are returned
and emitted in an `agent_removed` event.

### Trust in the owner

The owner approves measurements and PPIDs, can remove agents, and can replace the contract
code with `update_contract`. Until ownership moves to Bread's DAO (sigstack-near#14), the
owner is a single account, and each of these actions is a public transaction.

### Changes from the template

- Chain signatures removed (`request_signature`, the MPC contract, `mpc_contract_id`): the
  bot does not sign transactions for other chains.
- `prune_agent` added, so expired or revoked agents can be removed by anyone.
- `migrate` added, so `update_contract` can upgrade to a new build of this crate.
- Event standard renamed to `sigstack-agent`.

## Deploying

```bash
near account create-account sponsor-by-faucet-service <contract>.testnet \
  autogenerate-new-keypair save-to-keychain network-config testnet create
../../scripts/deploy_agent_contract.sh <contract>.testnet <owner>.testnet
```

The script deploys the wasm CI built from main with `requires_tee = true` and a 7-day
expiration. Measurements and the PPID are approved once the demo CVM is running.

## Building the contract

You currently need to build this contract without an ABI. CI builds it with
[`.github/workflows/contract.yml`](../../.github/workflows/contract.yml).

### Linux

From this directory:

```bash
cargo near build non-reproducible-wasm --locked --no-abi
```

### Mac

dcap-qvl's ring dependency cannot be built on Mac, so build in the pinned image CI uses. From this directory:

```bash
docker run --rm -v "$(pwd)":/workspace -w /workspace \
  sourcescan/cargo-near@sha256:772638e343baeeea24e49062c7d424274f3441452cc06ce97fc4e5695b19fecc \
  cargo near build non-reproducible-wasm --locked --no-abi
```

## Testing

You need to build the contract first

```bash
cargo test
```

Tests cover only `requires_tee = false` (can't produce valid TEE attestation outside of TEE).

### Unit tests

Contract init; owner-only methods (approve_measurements, remove_measurements, approve_ppids, remove_ppids, whitelist_agent_for_local, remove_agent_from_whitelist_for_local, remove_agent, update_owner_id, update_attestation_expiration_time) and panics when non-owner calls; agent registration (happy first registration with storage deposit, happy re-register without extra deposit, not whitelisted, insufficient / missing deposit on first registration); views (get_contract_info, get_agent, get_agents, pagination, expiration fields); prune_agent (callable by anyone, not registered, still valid, removal on invalid measurements/PPID/expired/not whitelisted/multiple reasons, re-registration after pruning).

### Integration tests

| Test                                                       | What it tests                                                                                                                                                                                                                                                                                                                                     |
| ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `test_measurements_and_ppid_lifecycle`                     | Exercises the full measurements and PPID approval lifecycle. Verifies that `prune_agent` removes agents with InvalidMeasurements, InvalidPpid, or both when approvals are revoked, and emits the right events; that re-approving measurements and PPID makes the remaining agent valid again, so it cannot be pruned; and that a removed agent cannot re-register. |
| `test_register_fails_without_default_measurements_or_ppid` | Confirms that registration fails in local mode when the default measurements or the default PPID have not been approved by the owner.                                                                                                                                                                                                             |
| `test_attestation_expiration`                              | Fast-forwards the sandbox past the attestation expiration time, then checks that `prune_agent` removes the agent with ExpiredAttestation, that a second prune returns "Agent not registered", and that the agent can re-register afterward.                                                                                                  |
| `test_large_dataset_pagination_real_contract`              | Registers 20 agents and checks that `get_agents` pagination works as expected using `from_index` and `limit`.                                                                                                                                                                                                                                     |
| `test_owner_transfer_and_new_owner_operations`             | Transfers contract ownership and verifies that the new owner can approve measurements while the old owner can no longer do so.                                                                                                                                                                                                                    |
| `test_update_contract`                                     | Upgrades the contract to this build's wasm with `update_contract` and checks that `migrate` keeps the owner, settings and approved measurements.                                                                                                                                                                                                  |
| `test_register_agent_new_agent_requires_storage_deposit_integration` | First `register_agent` with no attached deposit fails; with `0.005 NEAR` succeeds; `get_agent` shows a valid agent. |
| `test_register_agent_reregister_without_storage_deposit_integration` | After a successful first registration, `register_agent` again with no deposit succeeds.|
