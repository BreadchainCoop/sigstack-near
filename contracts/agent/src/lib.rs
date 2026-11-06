use hex;
use near_sdk::{
    AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    env::{self, block_timestamp_ms},
    json_types::U64,
    log, near, require,
    serde::Serialize,
    serde_json,
    store::{IterableMap, IterableSet},
};
use shade_attestation::{
    attestation::{AcceptedDstackAttestation, DstackAttestation},
    measurements::{FullMeasurements, FullMeasurementsHex, create_mock_full_measurements_hex},
    report_data::ReportData,
    tcb_info::HexBytes,
};

pub use internal::events::Event;
pub use internal::helpers::AgentRemovalReason;
pub use views::{AgentValidity, AgentView, ContractInfo};

mod internal;
mod owner;
pub mod views;

pub type Ppid = HexBytes<16>;

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Contract {
    pub requires_tee: bool,
    pub attestation_expiration_time_ms: u64,
    pub owner_id: AccountId,
    pub approved_measurements: IterableSet<FullMeasurementsHex>,
    pub approved_ppids: IterableSet<Ppid>,
    pub agents: IterableMap<AccountId, Agent>,
    pub whitelisted_agents_for_local: IterableSet<AccountId>,
}

#[near(serializers = [borsh])]
pub struct Agent {
    pub measurements: FullMeasurementsHex,
    pub ppid: Ppid,
    pub valid_until_ms: u64,
}

#[derive(BorshStorageKey)]
#[near]
pub enum StorageKey {
    ApprovedMeasurements,
    ApprovedPpids,
    Agents,
    WhitelistedAgentsForLocal,
}

const STORAGE_BYTES_TO_REGISTER: u128 = 486;

#[near]
impl Contract {
    #[init]
    #[private]
    pub fn new(
        requires_tee: bool,
        attestation_expiration_time_ms: U64,
        owner_id: AccountId,
    ) -> Self {
        Self {
            requires_tee,
            attestation_expiration_time_ms: attestation_expiration_time_ms.into(),
            owner_id,
            approved_measurements: IterableSet::new(StorageKey::ApprovedMeasurements),
            approved_ppids: IterableSet::new(StorageKey::ApprovedPpids),
            agents: IterableMap::new(StorageKey::Agents),
            whitelisted_agents_for_local: IterableSet::new(StorageKey::WhitelistedAgentsForLocal),
        }
    }

    // Register an agent, this needs to be called by the agent itself
    #[payable]
    pub fn register_agent(&mut self, attestation: DstackAttestation) -> bool {
        let predecessor = env::predecessor_account_id();
        let already_registered = self.agents.get(&predecessor).is_some();

        // New agents must cover storage; re-registration only updates existing state (no extra storage)
        if !already_registered {
            // You should update the STORAGE_BYTES_TO_REGISTER const if you store more data
            let storage_cost = env::storage_byte_cost()
                .checked_mul(STORAGE_BYTES_TO_REGISTER)
                .unwrap();
            require!(
                env::attached_deposit() >= storage_cost,
                &format!(
                    "Attached deposit must be greater than storage cost {:?}",
                    storage_cost.exact_amount_display()
                )
            );
        }

        // Verify the attestation and get the measurements and PPID for the agent
        let (measurements, ppid, advisory_ids) = self.verify_attestation(attestation);

        let valid_until_ms = block_timestamp_ms() + self.attestation_expiration_time_ms;
        let (advisory_ids_truncated, number_of_advisory_ids) =
            internal::events::summarize_advisory_ids(&advisory_ids);

        Event::AgentRegistered {
            account_id: &predecessor,
            measurements: &measurements,
            ppid: &ppid,
            advisory_ids_truncated,
            number_of_advisory_ids,
            current_time_ms: U64::from(block_timestamp_ms()),
            valid_until_ms: U64::from(valid_until_ms),
        }
        .emit();

        // Register the agent
        self.agents.insert(
            predecessor,
            Agent {
                measurements,
                ppid,
                valid_until_ms,
            },
        );

        true
    }

    // Anyone can remove an agent that is no longer valid, so stale attestations
    // don't linger in state or depend on the owner to clean them up.
    pub fn prune_agent(&mut self, account_id: AccountId) -> Vec<AgentRemovalReason> {
        let agent = self.agents.get(&account_id).expect("Agent not registered");
        let reasons = self.check_invalid_reasons(&account_id, agent);
        require!(!reasons.is_empty(), "Agent is valid");

        self.agents.remove(&account_id);
        Event::AgentRemoved {
            account_id: &account_id,
            reasons: reasons.clone(),
        }
        .emit();
        reasons
    }

    // Target of `update_contract`. State is read as-is, so a release that changes
    // the `Contract` layout must convert from the old layout here.
    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        env::state_read().expect("No contract state")
    }
}
