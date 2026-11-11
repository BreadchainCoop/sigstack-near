use crate::*;
use near_sdk::test_utils::{VMContextBuilder, accounts};
use near_sdk::{AccountId, NearToken, testing_env};
use shade_attestation::{
    attestation::create_mock_dstack_attestation,
    measurements::{FullMeasurementsHex, MeasurementsHex, create_mock_full_measurements_hex},
    tcb_info::HexBytes,
};

// Most tests use requires_tee = false, since unit tests cannot produce a real TDX quote.
// The requires_tee = true tests cover the checks a quote can't get past without one, and
// replay a real quote recorded from the demo bot (see "TEE mode with a real quote").

// Deposit constants for tests
const DEPOSIT_005_NEAR: NearToken = NearToken::from_yoctonear(5_000_000_000_000_000_000_000); // 0.005 NEAR
const DEPOSIT_003_NEAR: NearToken = NearToken::from_yoctonear(3_000_000_000_000_000_000_000); // 0.003 NEAR
const DEPOSIT_ZERO: NearToken = NearToken::from_yoctonear(0);

// Helper function to create a mock context
fn get_context(predecessor: AccountId, is_view: bool) -> VMContextBuilder {
    get_context_with_deposit(predecessor, is_view, None)
}

// Helper function to create a mock context with attached deposit
fn get_context_with_deposit(
    predecessor: AccountId,
    is_view: bool,
    deposit: Option<NearToken>,
) -> VMContextBuilder {
    get_context_with_deposit_and_timestamp(predecessor, is_view, deposit, None)
}

// Helper function to create a mock context with attached deposit and block timestamp
fn get_context_with_deposit_and_timestamp(
    predecessor: AccountId,
    is_view: bool,
    deposit: Option<NearToken>,
    block_timestamp_ms: Option<u64>,
) -> VMContextBuilder {
    let mut builder = VMContextBuilder::new();
    builder
        .current_account_id(accounts(0))
        .signer_account_id(predecessor.clone())
        .predecessor_account_id(predecessor)
        .is_view(is_view);
    if let Some(dep) = deposit {
        builder.attached_deposit(dep);
    }
    if let Some(ts) = block_timestamp_ms {
        builder.block_timestamp(ts * 1_000_000); // Convert ms to nanoseconds
    }
    builder
}

/// Returns measurements that differ from default (one byte different in mrtd).
fn non_default_measurements() -> FullMeasurementsHex {
    let mut mrtd = [0u8; 48];
    mrtd[0] = 1;
    FullMeasurementsHex {
        rtmrs: MeasurementsHex {
            mrtd: HexBytes::from(mrtd),
            rtmr0: HexBytes::from([0; 48]),
            rtmr1: HexBytes::from([0; 48]),
            rtmr2: HexBytes::from([0; 48]),
        },
        key_provider_event_digest: HexBytes::from([0; 48]),
        app_compose_hash_payload: HexBytes::from([0; 32]),
    }
}

/// Returns PPID that differs from default (all zeros).
fn non_default_ppid() -> Ppid {
    HexBytes::from([1u8; 16])
}

// Helper function to initialize contract (with default measurements and PPID approved for local mode)
fn setup_contract() -> Contract {
    let owner = accounts(0);
    let context = get_context(owner.clone(), false);
    testing_env!(context.build());
    // Use a short expiration time for tests: 100 seconds = 100000 ms
    let mut contract = Contract::new(
        false,
        U64::from(100000u64), // 100 seconds in milliseconds
        owner,
    );
    contract.approve_measurements(create_mock_full_measurements_hex());
    contract.approve_ppids(vec![Ppid::default()]);
    contract
}

// Test contract initialization with correct owner and empty collections
#[test]
fn test_new() {
    let owner = accounts(0);
    let context = get_context(owner.clone(), false);
    testing_env!(context.build());

    // Use a short expiration time for tests: 100 seconds = 100000 ms
    let attestation_expiration_time_ms = U64::from(100000u64);
    let contract = Contract::new(false, attestation_expiration_time_ms, owner.clone());

    let contract_info = contract.get_contract_info();
    assert_eq!(contract_info.owner_id, owner);
    assert_eq!(contract_info.requires_tee, false);
    assert_eq!(contract_info.attestation_expiration_time_ms.0, 100000u64);
    assert_eq!(contract.get_approved_measurements(&None, &None).len(), 0);
    assert_eq!(contract.get_approved_ppids(&None, &None).len(), 0);
    assert_eq!(contract.get_agents(&None, &None).len(), 0);
    assert_eq!(contract.get_whitelisted_agents_for_local().len(), 0);
}

// Test that owner can approve measurements and it appears in the approved list
#[test]
fn test_approve_measurements() {
    let mut contract = setup_contract();

    let measurements = create_mock_full_measurements_hex();
    contract.approve_measurements(measurements.clone());

    assert!(
        contract
            .get_approved_measurements(&None, &None)
            .contains(&measurements)
    );
    assert_eq!(contract.get_approved_measurements(&None, &None).len(), 1);
}

// Test that non-owner cannot approve measurements
#[test]
#[should_panic(expected = "Caller is not the owner")]
fn test_approve_measurements_not_owner() {
    let mut contract = setup_contract();
    let non_owner = accounts(2);
    let context = get_context(non_owner, false);
    testing_env!(context.build());

    contract.approve_measurements(create_mock_full_measurements_hex());
}

// Test that owner can remove measurements from the approved list
#[test]
fn test_remove_measurements() {
    let mut contract = setup_contract();

    let extra = create_mock_full_measurements_hex();
    contract.approve_measurements(extra.clone());
    let count_before = contract.get_approved_measurements(&None, &None).len();
    assert_eq!(count_before, 1);

    contract.remove_measurements(extra.clone());
    assert!(
        !contract
            .get_approved_measurements(&None, &None)
            .contains(&extra)
    );
}

// Test that non-owner cannot remove measurements from the approved list
#[test]
#[should_panic(expected = "Caller is not the owner")]
fn test_remove_measurements_not_owner() {
    let mut contract = setup_contract();
    let non_owner = accounts(2);
    contract.approve_measurements(create_mock_full_measurements_hex());

    let context = get_context(non_owner, false);
    testing_env!(context.build());
    contract.remove_measurements(create_mock_full_measurements_hex());
}

// Test that remove_measurements panics when measurements are not in the approved list
#[test]
#[should_panic(expected = "Measurements not in approved list")]
fn test_remove_measurements_not_found() {
    let mut contract = setup_contract();
    // Try to remove measurements that were never approved
    contract.remove_measurements(non_default_measurements());
}

// Test that remove_ppids panics when PPID is not in the approved list
#[test]
#[should_panic(expected = "PPID not in approved list")]
fn test_remove_ppids_not_found() {
    let mut contract = setup_contract();
    // Try to remove PPID that was never approved
    contract.remove_ppids(vec![non_default_ppid()]);
}

// Test that owner can whitelist an agent for local and agent appears in whitelist (not yet registered)
#[test]
fn test_whitelist_agent() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());

    let whitelisted = contract.get_whitelisted_agents_for_local();
    assert!(whitelisted.contains(&agent));
    // Not registered yet, so get_agent returns None
    assert!(contract.get_agent(agent.clone()).is_none());
}

// Test that whitelisting an agent twice doesn't create duplicates
#[test]
fn test_whitelist_agent_twice() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());
    contract.whitelist_agent_for_local(agent.clone()); // Should not panic

    let whitelisted = contract.get_whitelisted_agents_for_local();
    assert_eq!(whitelisted.len(), 1);
}

// Test that whitelisting an already registered agent doesn't unregister it
#[test]
fn test_whitelist_agent_after_registration() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    // Whitelist agent
    contract.whitelist_agent_for_local(agent.clone());

    // Register agent (default measurements and PPID already approved in setup)
    let context = get_context_with_deposit(agent.clone(), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    // Verify agent is registered and valid
    let agent_info = contract.get_agent(agent.clone()).unwrap();
    assert!(matches!(agent_info.validity, AgentValidity::Valid));

    // Whitelist agent again (should not unregister)
    let context = get_context(accounts(0), false);
    testing_env!(context.build());
    contract.whitelist_agent_for_local(agent.clone());

    // Verify agent is still registered and valid
    let agent_info = contract.get_agent(agent.clone()).unwrap();
    assert!(matches!(agent_info.validity, AgentValidity::Valid));
}

// Test that non-owner cannot whitelist an agent
#[test]
#[should_panic(expected = "Caller is not the owner")]
fn test_whitelist_agent_not_owner() {
    let mut contract = setup_contract();
    let non_owner = accounts(2);
    let agent = accounts(3);
    let context = get_context(non_owner, false);
    testing_env!(context.build());

    contract.whitelist_agent_for_local(agent);
}

// Test that remove_agent_for_local panics when agent is not in the whitelist
#[test]
#[should_panic(expected = "Agent not in whitelist for local")]
fn test_remove_agent_for_local_not_found() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    // Agent was never whitelisted
    contract.remove_agent_from_whitelist_for_local(agent);
}

// Test that owner can remove an agent from the whitelist (local)
#[test]
fn test_remove_agent_from_whitelist() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());
    assert!(contract.get_whitelisted_agents_for_local().contains(&agent));

    contract.remove_agent_from_whitelist_for_local(agent.clone());
    assert!(!contract.get_whitelisted_agents_for_local().contains(&agent));
    assert!(contract.get_agent(agent).is_none());
}

// Test that non-owner cannot remove an agent from the whitelist
#[test]
#[should_panic(expected = "Caller is not the owner")]
fn test_remove_agent_for_local_not_owner() {
    let mut contract = setup_contract();
    let non_owner = accounts(2);
    let agent = accounts(3);
    contract.whitelist_agent_for_local(agent.clone());

    let context = get_context(non_owner, false);
    testing_env!(context.build());
    contract.remove_agent_from_whitelist_for_local(agent);
}

// Test that owner can remove a registered agent from the agents map
#[test]
fn test_remove_agent() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());
    let context = get_context_with_deposit(agent.clone(), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());
    assert!(contract.get_agent(agent.clone()).is_some());

    let context = get_context(accounts(0), false);
    testing_env!(context.build());
    contract.remove_agent(agent.clone());
    assert!(contract.get_agent(agent).is_none());
}

// Test that remove_agent panics when agent is not registered
#[test]
#[should_panic(expected = "Agent not registered")]
fn test_remove_agent_not_found() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    // Agent was never registered
    contract.remove_agent(agent);
}

// Test that non-owner cannot remove a registered agent
#[test]
#[should_panic(expected = "Caller is not the owner")]
fn test_remove_agent_not_owner() {
    let mut contract = setup_contract();
    let non_owner = accounts(2);
    let agent = accounts(3);
    contract.whitelist_agent_for_local(agent.clone());
    let context = get_context_with_deposit(agent.clone(), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    let context = get_context(non_owner, false);
    testing_env!(context.build());
    contract.remove_agent(agent);
}

// Happy path: first-time registration must attach sufficient storage deposit
#[test]
fn test_register_agent_happy_first_registration_with_storage_deposit() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());

    let context = get_context_with_deposit(agent.clone(), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());

    let result = contract.register_agent(create_mock_dstack_attestation());
    assert!(result);

    let agent_info = contract.get_agent(agent.clone()).unwrap();
    assert!(matches!(agent_info.validity, AgentValidity::Valid));
}

// Happy path: first registration with deposit, then re-register with zero (no extra storage)
#[test]
fn test_register_agent_happy_reregister_without_additional_deposit() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());

    let context = get_context_with_deposit(agent.clone(), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());
    assert!(contract.register_agent(create_mock_dstack_attestation()));
    assert!(matches!(
        contract.get_agent(agent.clone()).unwrap().validity,
        AgentValidity::Valid
    ));

    let context = get_context_with_deposit(agent.clone(), false, Some(DEPOSIT_ZERO));
    testing_env!(context.build());
    assert!(contract.register_agent(create_mock_dstack_attestation()));
    assert!(matches!(
        contract.get_agent(agent.clone()).unwrap().validity,
        AgentValidity::Valid
    ));
}

// Test that an agent cannot register if not whitelisted for local
#[test]
#[should_panic(expected = "Agent needs to be whitelisted for local mode")]
fn test_register_agent_not_whitelisted() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    let context = get_context_with_deposit(agent, false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());

    contract.register_agent(create_mock_dstack_attestation());
}

// First-time registration requires storage stake: zero attached deposit must fail
#[test]
#[should_panic(expected = "Attached deposit must be greater than storage cost")]
fn test_register_agent_errors_when_storage_deposit_required_but_zero_attached() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());

    // Try to register with 0 NEAR deposit
    let context = get_context_with_deposit(agent, false, Some(DEPOSIT_ZERO));
    testing_env!(context.build());

    contract.register_agent(create_mock_dstack_attestation());
}

// First-time registration: attached deposit below storage cost must fail
#[test]
#[should_panic(expected = "Attached deposit must be greater than storage cost")]
fn test_register_agent_errors_when_storage_deposit_insufficient() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());

    // Try to register with 0.003 NEAR deposit (less than storage cost)
    let context = get_context_with_deposit(agent, false, Some(DEPOSIT_003_NEAR));
    testing_env!(context.build());

    contract.register_agent(create_mock_dstack_attestation());
}

// Test that owner can update the owner ID
#[test]
fn test_update_owner_id() {
    let mut contract = setup_contract();
    let new_owner = accounts(3);

    contract.update_owner_id(new_owner.clone());
    let contract_info = contract.get_contract_info();
    assert_eq!(contract_info.owner_id, new_owner);
}

// Test that non-owner cannot update the owner ID
#[test]
#[should_panic(expected = "Caller is not the owner")]
fn test_update_owner_id_not_owner() {
    let mut contract = setup_contract();
    let non_owner = accounts(2);
    let new_owner = accounts(3);
    let context = get_context(non_owner, false);
    testing_env!(context.build());

    contract.update_owner_id(new_owner);
}

// Test that owner can update the MPC contract ID
#[test]
fn test_update_attestation_expiration_time() {
    let mut contract = setup_contract();
    let new_expiration_time = U64::from(200000u64); // 200 seconds

    contract.update_attestation_expiration_time(new_expiration_time.clone());
    let contract_info = contract.get_contract_info();
    assert_eq!(contract_info.attestation_expiration_time_ms.0, 200000u64);
}

// Test that non-owner cannot update the attestation expiration time
#[test]
#[should_panic(expected = "Caller is not the owner")]
fn test_update_attestation_expiration_time_not_owner() {
    let mut contract = setup_contract();
    let non_owner = accounts(2);
    let new_expiration_time = U64::from(200000u64);
    let context = get_context(non_owner, false);
    testing_env!(context.build());

    contract.update_attestation_expiration_time(new_expiration_time);
}

// Test that get_contract_info returns the correct values
#[test]
fn test_get_contract_info() {
    let contract = setup_contract();
    let contract_info = contract.get_contract_info();
    assert_eq!(contract_info.requires_tee, false);
    assert_eq!(contract_info.attestation_expiration_time_ms.0, 100000u64);
}

// Test that get_approved_measurements returns approved measurements and pagination works
#[test]
fn test_get_approved_measurements() {
    let mut contract = setup_contract();

    let default = create_mock_full_measurements_hex();
    contract.approve_measurements(default.clone());

    let all = contract.get_approved_measurements(&None, &None);
    assert_eq!(all.len(), 1);
    assert!(all.contains(&default));

    // Test pagination
    let first_two = contract.get_approved_measurements(&Some(0), &Some(2));
    assert_eq!(first_two.len(), 1);
}

// Test that get_agents returns only registered agents, pagination works, and validity reflects current approvals
#[test]
fn test_get_agents() {
    let mut contract = setup_contract();
    let agent1 = accounts(2);
    let agent2 = accounts(3);
    let agent3 = accounts(4);

    contract.whitelist_agent_for_local(agent1.clone());
    contract.whitelist_agent_for_local(agent2.clone());
    contract.whitelist_agent_for_local(agent3.clone());

    // None registered yet
    assert_eq!(contract.get_agents(&None, &None).len(), 0);

    // Register agent1 and agent2; agent3 remains unregistered
    let context = get_context_with_deposit(agent1.clone(), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    let context = get_context_with_deposit(agent2.clone(), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    assert!(contract.get_agent(agent3.clone()).is_none());

    let agents = contract.get_agents(&None, &None);
    assert_eq!(agents.len(), 2);

    let agent1_info = agents.iter().find(|a| a.account_id == agent1).unwrap();
    assert!(matches!(agent1_info.validity, AgentValidity::Valid));

    let agent2_info = agents.iter().find(|a| a.account_id == agent2).unwrap();
    assert!(matches!(agent2_info.validity, AgentValidity::Valid));

    // Pagination
    assert_eq!(contract.get_agents(&Some(0), &Some(1)).len(), 1);

    // Remove default measurements; approval flags should update
    let context = get_context(accounts(0), false);
    testing_env!(context.build());
    contract.remove_measurements(create_mock_full_measurements_hex());

    let agents = contract.get_agents(&None, &None);
    let agent1_info = agents.iter().find(|a| a.account_id == agent1).unwrap();
    assert!(
        matches!(agent1_info.validity, AgentValidity::Invalid(ref r) if r.contains(&AgentRemovalReason::InvalidMeasurements))
    );

    let agent2_info = agents.iter().find(|a| a.account_id == agent2).unwrap();
    assert!(
        matches!(agent2_info.validity, AgentValidity::Invalid(ref r) if r.contains(&AgentRemovalReason::InvalidMeasurements))
    );
}

// Test that get_agent returns correct agent information (None when not registered, Some when registered)
#[test]
fn test_get_agent() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    // Agent not whitelisted / not registered
    assert!(contract.get_agent(agent.clone()).is_none());

    // Whitelist agent but don't register
    contract.whitelist_agent_for_local(agent.clone());
    assert!(contract.get_agent(agent.clone()).is_none());

    // Register agent
    let context = get_context_with_deposit(agent.clone(), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    let agent_info = contract.get_agent(agent.clone()).unwrap();
    assert_eq!(agent_info.account_id, agent);
    assert!(matches!(agent_info.validity, AgentValidity::Valid));
}

// Whitelists and registers `agent` at 1000 ms, then switches the context to `caller`.
fn register_and_switch(contract: &mut Contract, agent: &AccountId, caller: AccountId) {
    contract.whitelist_agent_for_local(agent.clone());
    let context = get_context_with_deposit_and_timestamp(
        agent.clone(),
        false,
        Some(DEPOSIT_005_NEAR),
        Some(1000u64),
    );
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());
    let context = get_context_with_deposit_and_timestamp(caller, false, None, Some(1000u64));
    testing_env!(context.build());
}

// Anyone, not only the owner, can prune an agent that is no longer whitelisted
#[test]
fn test_prune_agent_not_whitelisted() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    register_and_switch(&mut contract, &agent, accounts(0));
    contract.remove_agent_from_whitelist_for_local(agent.clone());

    let context = get_context(accounts(3), false);
    testing_env!(context.build());
    let reasons = contract.prune_agent(agent.clone());

    assert_eq!(reasons, vec![AgentRemovalReason::NotWhitelistedForLocal]);
    assert!(contract.get_agent(agent).is_none());
}

#[test]
#[should_panic(expected = "Agent not registered")]
fn test_prune_agent_not_registered() {
    let mut contract = setup_contract();
    contract.prune_agent(accounts(2));
}

#[test]
#[should_panic(expected = "Agent is valid")]
fn test_prune_agent_valid() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    register_and_switch(&mut contract, &agent, accounts(3));
    contract.prune_agent(agent);
}

#[test]
fn test_prune_agent_invalid_measurements() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    register_and_switch(&mut contract, &agent, accounts(0));
    contract.remove_measurements(create_mock_full_measurements_hex());

    let reasons = contract.prune_agent(agent.clone());

    assert_eq!(reasons, vec![AgentRemovalReason::InvalidMeasurements]);
    assert!(contract.get_agent(agent).is_none());
}

#[test]
fn test_prune_agent_invalid_ppid() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    register_and_switch(&mut contract, &agent, accounts(0));
    contract.remove_ppids(vec![Ppid::default()]);

    let reasons = contract.prune_agent(agent.clone());

    assert_eq!(reasons, vec![AgentRemovalReason::InvalidPpid]);
    assert!(contract.get_agent(agent).is_none());
}

#[test]
fn test_prune_agent_multiple_reasons() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    register_and_switch(&mut contract, &agent, accounts(0));
    contract.remove_measurements(create_mock_full_measurements_hex());
    contract.remove_ppids(vec![Ppid::default()]);
    contract.remove_agent_from_whitelist_for_local(agent.clone());

    let reasons = contract.prune_agent(agent.clone());

    assert_eq!(
        reasons,
        vec![
            AgentRemovalReason::InvalidMeasurements,
            AgentRemovalReason::InvalidPpid,
            AgentRemovalReason::NotWhitelistedForLocal,
        ]
    );
    assert!(contract.get_agent(agent).is_none());
}

#[test]
fn test_prune_agent_expired_attestation() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    register_and_switch(&mut contract, &agent, accounts(3));

    // valid_until_ms = 1000 + 100000
    let context = get_context_with_deposit_and_timestamp(accounts(3), false, None, Some(101001u64));
    testing_env!(context.build());
    let reasons = contract.prune_agent(agent.clone());

    assert_eq!(reasons, vec![AgentRemovalReason::ExpiredAttestation]);
    assert!(contract.get_agent(agent).is_none());
}

// A pruned agent must register a fresh attestation to come back
#[test]
fn test_prune_agent_then_reregister() {
    let mut contract = setup_contract();
    let agent = accounts(2);
    register_and_switch(&mut contract, &agent, accounts(3));
    let context = get_context_with_deposit_and_timestamp(accounts(3), false, None, Some(101001u64));
    testing_env!(context.build());
    contract.prune_agent(agent.clone());

    let context = get_context_with_deposit_and_timestamp(
        agent.clone(),
        false,
        Some(DEPOSIT_005_NEAR),
        Some(101001u64),
    );
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    let agent_info = contract.get_agent(agent).unwrap();
    assert!(matches!(agent_info.validity, AgentValidity::Valid));
    assert_eq!(agent_info.valid_until_ms.0, 201001u64);
}

// Test that get_agent shows validity correctly (Valid vs Invalid with ExpiredAttestation)
#[test]
fn test_get_agent_expiration_fields() {
    let mut contract = setup_contract();
    let agent = accounts(2);

    contract.whitelist_agent_for_local(agent.clone());

    // Register agent at timestamp 1000 ms
    let context = get_context_with_deposit_and_timestamp(
        agent.clone(),
        false,
        Some(DEPOSIT_005_NEAR),
        Some(1000u64),
    );
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    // Check agent info - should be valid (not expired)
    let agent_info = contract.get_agent(agent.clone()).unwrap();
    assert!(matches!(agent_info.validity, AgentValidity::Valid));
    assert_eq!(agent_info.valid_until_ms.0, 101000u64); // 1000 + 100000

    // Fast forward time past expiration
    // Note: We use is_view: false because contract drop needs to flush storage
    // View methods can still be called with is_view: false
    let context =
        get_context_with_deposit_and_timestamp(agent.clone(), false, None, Some(101001u64));
    testing_env!(context.build());

    // Check agent info - should be invalid (expired)
    let agent_info = contract.get_agent(agent.clone()).unwrap();
    assert!(
        matches!(agent_info.validity, AgentValidity::Invalid(ref r) if r.contains(&AgentRemovalReason::ExpiredAttestation))
    );
    assert_eq!(agent_info.valid_until_ms.0, 101000u64);
}

// Test that get_agents shows validity correctly
#[test]
fn test_get_agents_expiration_fields() {
    let mut contract = setup_contract();
    let agent1 = accounts(2);
    let agent2 = accounts(3);

    contract.whitelist_agent_for_local(agent1.clone());
    contract.whitelist_agent_for_local(agent2.clone());

    // Register agent1 at timestamp 1000 ms
    let context = get_context_with_deposit_and_timestamp(
        agent1.clone(),
        false,
        Some(DEPOSIT_005_NEAR),
        Some(1000u64),
    );
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    // Register agent2 at timestamp 2000 ms
    let context = get_context_with_deposit_and_timestamp(
        agent2.clone(),
        false,
        Some(DEPOSIT_005_NEAR),
        Some(2000u64),
    );
    testing_env!(context.build());
    contract.register_agent(create_mock_dstack_attestation());

    // Check at timestamp 1001 ms - both should be valid
    // Note: We use is_view: false because contract drop needs to flush storage
    // View methods can still be called with is_view: false
    let context = get_context_with_deposit_and_timestamp(accounts(0), false, None, Some(1001u64));
    testing_env!(context.build());

    let agents = contract.get_agents(&None, &None);
    assert_eq!(agents.len(), 2);
    for agent_info in &agents {
        assert!(matches!(agent_info.validity, AgentValidity::Valid));
    }

    // Fast forward past agent1 expiration but not agent2
    // agent1 expires at 101000, agent2 expires at 102000
    // Note: We use is_view: false because contract drop needs to flush storage
    let context = get_context_with_deposit_and_timestamp(
        accounts(0),
        false,
        None,
        Some(101500u64), // Between the two expirations
    );
    testing_env!(context.build());

    let agents = contract.get_agents(&None, &None);
    assert_eq!(agents.len(), 2);
    let agent1_info = agents.iter().find(|a| a.account_id == agent1).unwrap();
    let agent2_info = agents.iter().find(|a| a.account_id == agent2).unwrap();
    assert!(
        matches!(agent1_info.validity, AgentValidity::Invalid(ref r) if r.contains(&AgentRemovalReason::ExpiredAttestation))
    );
    assert!(matches!(agent2_info.validity, AgentValidity::Valid));
}

// -------- summarize_advisory_ids (AgentRegistered event payload) --------

use super::events::{MAX_ADVISORY_IDS, summarize_advisory_ids};

// Builds `n` distinct advisory IDs in a stable order.
fn advisory_ids(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("INTEL-DOC-{i}")).collect()
}

// No advisories: empty list and a zero count.
#[test]
fn summarize_advisory_ids_empty() {
    let (shown, total) = summarize_advisory_ids(&[]);
    assert!(shown.is_empty());
    assert_eq!(total, 0);
}

// Fewer than the cap: every ID is returned, in order, with the exact count.
#[test]
fn summarize_advisory_ids_below_cap() {
    let ids = advisory_ids(3);
    let (shown, total) = summarize_advisory_ids(&ids);
    assert_eq!(shown, ids);
    assert_eq!(total, 3);
}

// Exactly at the cap: every ID is returned and none are dropped.
#[test]
fn summarize_advisory_ids_at_cap() {
    let ids = advisory_ids(MAX_ADVISORY_IDS);
    let (shown, total) = summarize_advisory_ids(&ids);
    assert_eq!(shown, ids);
    assert_eq!(total as usize, MAX_ADVISORY_IDS);
}

// More than the cap: the shown list is truncated to the first `MAX_ADVISORY_IDS`,
// but the reported total still reflects the true number of advisories.
#[test]
fn summarize_advisory_ids_above_cap_truncates_but_counts_all() {
    let ids = advisory_ids(MAX_ADVISORY_IDS + 4);
    let (shown, total) = summarize_advisory_ids(&ids);
    assert_eq!(shown.as_slice(), &ids[..MAX_ADVISORY_IDS]);
    assert_eq!(total as usize, MAX_ADVISORY_IDS + 4);
}

// TEE mode, with the mock measurements and PPID approved.
fn setup_tee_contract() -> Contract {
    let owner = accounts(0);
    let context = get_context(owner.clone(), false);
    testing_env!(context.build());
    let mut contract = Contract::new(true, U64::from(100000u64), owner);
    contract.approve_measurements(create_mock_full_measurements_hex());
    contract.approve_ppids(vec![Ppid::default()]);
    contract
}

// An implicit account ID is the agent's public key, which the quote's report data binds.
// A named account has no such binding, so TEE mode rejects it before reading the quote.
#[test]
#[should_panic(expected = "Account ID must be implicit account")]
fn test_tee_register_rejects_named_account() {
    let mut contract = setup_tee_contract();
    let context = get_context_with_deposit(accounts(2), false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());

    contract.register_agent(create_mock_dstack_attestation());
}

// The mock attestation local mode accepts fails quote verification in TEE mode, even with
// the mock measurements and PPID approved.
#[test]
#[should_panic(expected = "Attestation verification failed")]
fn test_tee_register_rejects_mock_attestation() {
    let mut contract = setup_tee_contract();
    let agent: AccountId = "ab".repeat(32).parse().unwrap();
    let context = get_context_with_deposit(agent, false, Some(DEPOSIT_005_NEAR));
    testing_env!(context.build());

    contract.register_agent(create_mock_dstack_attestation());
}

// Local-mode whitelisting can't be used to skip attestation in TEE mode.
#[test]
#[should_panic(expected = "Whitelisting agents is not supported for TEE")]
fn test_tee_whitelist_is_rejected() {
    let mut contract = setup_tee_contract();
    contract.whitelist_agent_for_local(accounts(2));
}

// -------- TEE mode with a real quote --------
//
// `tests/fixtures/demo_register_agent.json` holds the exact arguments the demo bot sent to
// `register_agent` on agent.sigstack.testnet, in transaction
// 4zqtSnqZu3t8UDoJonqtzrrhdPk96KeFC8BJCWe3KRRW: a real TDX quote from Phala Cloud, the PCCS
// collateral the bot fetched, and dstack's tcb_info. The quote verifies only against Intel's
// root of trust, so these tests run the full verification path without a TEE. The block
// timestamp is set to when that transaction executed, which keeps the collateral in date.

const DEMO_REGISTER_ARGS: &str = include_str!("../../tests/fixtures/demo_register_agent.json");

// The implicit account whose ID the quote's report data binds.
const DEMO_AGENT: &str = "b0c2f7e3b2e49476e99d8219703fa414bfb65d3b0077cf3a048662029ef46476";

// Block timestamp of the receipt that executed `register_agent`, in ms.
const DEMO_REGISTERED_AT_MS: u64 = 1_794_424_965_344;

// What the owner approved for the demo build (logged by the bot, checked against Phala's
// attestation before approving).
const DEMO_MEASUREMENTS: &str = r#"{
    "rtmrs": {
        "mrtd": "7fe60787222bd394cb516abca2435f22f035ab2cc9c0a4b4b3b148e46297f3d931a237b72f359052c6e657d8c1409173",
        "rtmr0": "027b610ab0555482f8a3868524093bdf3cdbe2539f81dfeeb886864654cb2fe3422f7fc36d4bab6fa46683aad11d1ba7",
        "rtmr1": "4ae609807c765916a31d724786ed74bcb97e168a9d937b04936aeef2015c907eba5ac352c6bcd749c246fd62ffb659ec",
        "rtmr2": "80e945df96ec417b68d574dba18d199a7448d66c3d0e56f073167a9513d2381601f5d614853646d2c49c5d8d58a7daf5"
    },
    "key_provider_event_digest": "1de9cfa982eab350c1c6719fed6f35f9be554d202c26d354e498b0b9e6ad2e6e1e02df3abfa85bd8113c9ea219c16deb",
    "app_compose_hash_payload": "a949c1ffaf5aede815321e6a3fc7bb4190637522f38bb841ff13fe50051b507d"
}"#;
const DEMO_PPID: &str = "\"3e306e133b7fdaa7bb79f67634cb520c\"";

const SEVEN_DAYS_MS: u64 = 7 * 24 * 60 * 60 * 1000;

fn demo_attestation() -> DstackAttestation {
    let args: near_sdk::serde_json::Value =
        near_sdk::serde_json::from_str(DEMO_REGISTER_ARGS).unwrap();
    near_sdk::serde_json::from_value(args["attestation"].clone()).unwrap()
}

fn demo_measurements() -> FullMeasurementsHex {
    near_sdk::serde_json::from_str(DEMO_MEASUREMENTS).unwrap()
}

fn demo_ppid() -> Ppid {
    near_sdk::serde_json::from_str(DEMO_PPID).unwrap()
}

// TEE mode with nothing approved yet; each test approves what it needs.
fn setup_demo_contract() -> Contract {
    let owner = accounts(0);
    testing_env!(get_context(owner.clone(), false).build());
    Contract::new(true, U64::from(SEVEN_DAYS_MS), owner)
}

// Calls register_agent with the demo attestation as `caller`, at `at_ms`.
fn register_demo(contract: &mut Contract, caller: &str, at_ms: u64) -> bool {
    let context = get_context_with_deposit_and_timestamp(
        caller.parse().unwrap(),
        false,
        Some(DEPOSIT_005_NEAR),
        Some(at_ms),
    );
    testing_env!(context.build());
    contract.register_agent(demo_attestation())
}

// The demo's registration replays: with its build and PPID approved, the real quote verifies
// and the agent is valid for 7 days with the measurements it attested.
#[test]
fn test_tee_register_demo_quote() {
    let mut contract = setup_demo_contract();
    contract.approve_measurements(demo_measurements());
    contract.approve_ppids(vec![demo_ppid()]);

    assert!(register_demo(&mut contract, DEMO_AGENT, DEMO_REGISTERED_AT_MS));

    let agent = contract.get_agent(DEMO_AGENT.parse().unwrap()).unwrap();
    assert!(matches!(agent.validity, AgentValidity::Valid));
    assert_eq!(agent.measurements, demo_measurements());
    assert_eq!(agent.ppid, demo_ppid());
    assert_eq!(
        agent.valid_until_ms,
        U64::from(DEMO_REGISTERED_AT_MS + SEVEN_DAYS_MS)
    );
}

// The quote's report data names the demo agent's account, so anyone else replaying it is
// rejected, even with the build and PPID approved.
#[test]
#[should_panic(expected = "wrong report_data hash")]
fn test_tee_demo_quote_rejects_other_account() {
    let mut contract = setup_demo_contract();
    contract.approve_measurements(demo_measurements());
    contract.approve_ppids(vec![demo_ppid()]);

    let other = "ab".repeat(32);
    register_demo(&mut contract, &other, DEMO_REGISTERED_AT_MS);
}

// A genuine quote from a build the owner hasn't approved is rejected.
#[test]
#[should_panic(expected = "wrong expected_measurements hash")]
fn test_tee_demo_quote_rejects_unapproved_build() {
    let mut contract = setup_demo_contract();
    contract.approve_measurements(non_default_measurements());
    contract.approve_ppids(vec![demo_ppid()]);

    register_demo(&mut contract, DEMO_AGENT, DEMO_REGISTERED_AT_MS);
}

// A genuine quote from hardware whose PPID isn't approved is rejected.
#[test]
#[should_panic(expected = "is not in the allowed PPIDs list")]
fn test_tee_demo_quote_rejects_unapproved_ppid() {
    let mut contract = setup_demo_contract();
    contract.approve_measurements(demo_measurements());
    contract.approve_ppids(vec![non_default_ppid()]);

    register_demo(&mut contract, DEMO_AGENT, DEMO_REGISTERED_AT_MS);
}

// Collateral has a validity window, so an old quote and collateral can't be replayed
// indefinitely: 90 days later the same arguments fail DCAP verification.
#[test]
#[should_panic(expected = "dcap verification failed")]
fn test_tee_demo_quote_rejects_stale_collateral() {
    let mut contract = setup_demo_contract();
    contract.approve_measurements(demo_measurements());
    contract.approve_ppids(vec![demo_ppid()]);

    let ninety_days_later = DEMO_REGISTERED_AT_MS + 90 * 24 * 60 * 60 * 1000;
    register_demo(&mut contract, DEMO_AGENT, ninety_days_later);
}
