//! Creates the agent account and keeps it registered with the agent contract.
//!
//! The agent signs `register_agent` as a NEP-366 delegate action and the sponsor relays
//! it, paying the gas and the storage deposit. The agent account only has to exist, so
//! each boot's account holds a few thousandths of a NEAR, not the ~0.31 NEAR of prepaid
//! gas a transaction it signed itself would need.

use crate::{
    attestation::{approval_args, build_attestation},
    base58,
    rpc::{AccessKeyInfo, NearRpc},
    tx::{Action, DelegateAction, Transaction},
    AgentConfig, AgentError, SigningKey, Tee,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// What shade-agent-js attaches; attestation verification is gas-heavy.
pub const REGISTER_GAS: u64 = 300_000_000_000_000;
/// 0.005 NEAR covers the contract's storage for a new agent (486 bytes).
pub const REGISTER_DEPOSIT: u128 = 5_000_000_000_000_000_000_000;
/// How many blocks (about a second each) a signed delegate action stays valid.
const DELEGATE_TTL_BLOCKS: u64 = 100;
/// Warn below this sponsor balance (1 NEAR). Each registration holds about 0.31 NEAR of
/// prepaid gas while in flight, so agents stop registering not long after.
const SPONSOR_LOW_BALANCE: u128 = 1_000_000_000_000_000_000_000_000;
/// Smallest excess (0.001 NEAR) worth returning to the sponsor; below it the relay's gas
/// isn't worth spending.
const MIN_RETURN: u128 = 1_000_000_000_000_000_000_000;
/// Kept back from a returned excess (0.0001 NEAR) for the transfer's own gas, which the
/// agent pays: a transfer burns about 0.00005 NEAR.
const RETURN_GAS_RESERVE: u128 = 100_000_000_000_000_000_000;

/// Re-register at most this long before the registration expires.
const MAX_REFRESH_MARGIN: Duration = Duration::from_secs(24 * 60 * 60);
const MIN_REFRESH_DELAY: Duration = Duration::from_secs(10 * 60);
const MIN_RETRY_DELAY: Duration = Duration::from_secs(30);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub account_id: String,
    /// base58 hash of the `register_agent` transaction.
    pub tx_hash: String,
    pub valid_until_ms: u64,
}

/// What the registrar has managed so far, for the bot to report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistrationState {
    /// The latest successful registration this boot.
    pub registration: Option<Registration>,
    /// Why the latest attempt failed, if it did.
    pub last_error: Option<String>,
}

/// Shared view of this boot's agent registration. Clone the `Arc` from
/// [`Registrar::status`] before spawning [`Registrar::run`].
#[derive(Debug)]
pub struct AgentStatus {
    pub account_id: String,
    pub contract_id: String,
    state: Mutex<RegistrationState>,
}

impl AgentStatus {
    pub fn new(account_id: impl Into<String>, contract_id: impl Into<String>) -> Self {
        Self {
            account_id: account_id.into(),
            contract_id: contract_id.into(),
            state: Mutex::default(),
        }
    }

    pub fn state(&self) -> RegistrationState {
        self.lock().clone()
    }

    /// Records an attempt: a success replaces the registration and clears the error;
    /// a failure keeps the last registration, which stays valid until it expires.
    pub fn record(&self, attempt: &Result<Registration, AgentError>) {
        let mut state = self.lock();
        match attempt {
            Ok(registration) => {
                state.registration = Some(registration.clone());
                state.last_error = None;
            }
            Err(e) => state.last_error = Some(e.to_string()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RegistrationState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub struct Registrar {
    tee: Arc<dyn Tee>,
    rpc: NearRpc,
    http: reqwest::Client,
    agent: SigningKey,
    sponsor: SigningKey,
    sponsor_id: String,
    contract_id: String,
    min_balance: u128,
    top_up_balance: u128,
    pccs_endpoints: Vec<String>,
    /// Approval args are logged on the first attempt of each boot.
    approval_logged: AtomicBool,
    status: Arc<AgentStatus>,
}

impl Registrar {
    /// Validates the config and generates this boot's random agent key.
    pub fn new(config: &AgentConfig, tee: Arc<dyn Tee>) -> Result<Self, AgentError> {
        Self::with_key(config, tee, SigningKey::generate()?)
    }

    pub(crate) fn with_key(
        config: &AgentConfig,
        tee: Arc<dyn Tee>,
        agent: SigningKey,
    ) -> Result<Self, AgentError> {
        config.validate()?;
        let (min_balance, top_up_balance) = config.balances()?;
        let contract_id = config.contract_id.trim().to_string();
        let status = Arc::new(AgentStatus::new(
            agent.implicit_account_id(),
            contract_id.clone(),
        ));
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            tee,
            rpc: NearRpc::new(config.rpc_url.trim())?,
            http,
            agent,
            sponsor: SigningKey::from_near_secret(&config.sponsor_private_key)?,
            sponsor_id: config.sponsor_account_id.trim().to_string(),
            contract_id,
            min_balance,
            top_up_balance,
            pccs_endpoints: config.pccs_endpoints(),
            approval_logged: AtomicBool::new(false),
            status,
        })
    }

    /// This boot's registration status, updated by [`Registrar::run`].
    pub fn status(&self) -> Arc<AgentStatus> {
        self.status.clone()
    }

    /// This boot's agent account: the implicit account of the random key.
    pub fn account_id(&self) -> String {
        self.agent.implicit_account_id()
    }

    async fn access(
        &self,
        key: &SigningKey,
        account_id: &str,
    ) -> Result<AccessKeyInfo, AgentError> {
        let public_key = key.public_key_string();
        self.rpc
            .access_key(account_id, &public_key)
            .await?
            .ok_or_else(|| AgentError::Rpc(format!("{account_id} has no access key {public_key}")))
    }

    /// Signs and sends a one-action transaction; returns its base58 hash.
    async fn send(
        &self,
        key: &SigningKey,
        signer_id: &str,
        receiver_id: &str,
        action: Action,
    ) -> Result<String, AgentError> {
        let access = self.access(key, signer_id).await?;
        let signed = Transaction {
            signer_id: signer_id.to_string(),
            public_key: key.public_key(),
            nonce: access.nonce + 1,
            receiver_id: receiver_id.to_string(),
            block_hash: access.block_hash,
            actions: vec![action],
        }
        .sign(key);
        self.rpc.send_tx(&signed).await?;
        Ok(base58::encode(&signed.hash))
    }

    /// Creates the agent's implicit account with a transfer from the sponsor, or tops it
    /// up if it has dropped below `min_balance`. The agent pays for nothing itself, so
    /// this only has to cover the account's storage.
    ///
    /// A failed relayed call refunds its deposit to the agent, not the sponsor, so anything
    /// above `top_up_balance` is sent back first. That's best effort: a failure is logged.
    pub async fn ensure_funded(&self) -> Result<(), AgentError> {
        let account_id = self.account_id();
        let balance = self.rpc.balance(&account_id).await?.unwrap_or(0);
        if balance >= self.min_balance {
            let excess = balance.saturating_sub(self.top_up_balance);
            if excess >= MIN_RETURN {
                self.return_excess(excess).await;
            }
            return Ok(());
        }
        let amount = self.top_up_balance - balance;
        let tx_hash = self
            .send(
                &self.sponsor,
                &self.sponsor_id,
                &account_id,
                Action::Transfer { deposit: amount },
            )
            .await?;
        info!(
            "Funded agent account {account_id} with {amount} yoctoNEAR from {} (tx {tx_hash})",
            self.sponsor_id
        );
        Ok(())
    }

    /// Sends `excess` (less gas) from the agent back to the sponsor, in a transaction the
    /// agent signs and pays for. It can't be relayed: NEP-366 charges the deposits inside a
    /// relayed call to the relayer, so the sponsor would only be paying itself.
    async fn return_excess(&self, excess: u128) {
        let amount = excess - RETURN_GAS_RESERVE;
        let account_id = self.account_id();
        let sent = self
            .send(
                &self.agent,
                &account_id,
                &self.sponsor_id,
                Action::Transfer { deposit: amount },
            )
            .await;
        match sent {
            Ok(tx_hash) => info!(
                "Returned {amount} yoctoNEAR of refunded deposit to {} (tx {tx_hash})",
                self.sponsor_id
            ),
            Err(e) => warn!(
                "Could not return {amount} yoctoNEAR to {}: {e}",
                self.sponsor_id
            ),
        }
    }

    /// Logs a warning when the sponsor is running low, so it's topped up before agents stop
    /// registering. Best effort.
    async fn check_sponsor_balance(&self) {
        match self.rpc.balance(&self.sponsor_id).await {
            Ok(balance) if balance.unwrap_or(0) < SPONSOR_LOW_BALANCE => warn!(
                "Sponsor {} is low: {} yoctoNEAR; each registration holds ~0.31 NEAR",
                self.sponsor_id,
                balance.unwrap_or(0)
            ),
            Ok(_) => {}
            Err(e) => debug!("Could not read the sponsor's balance: {e}"),
        }
    }

    async fn agent_view(&self) -> Result<Option<Value>, AgentError> {
        let view = self
            .rpc
            .view_function(
                &self.contract_id,
                "get_agent",
                &json!({ "account_id": self.account_id() }),
            )
            .await?;
        Ok((!view.is_null()).then_some(view))
    }

    /// Funds the account if needed, then registers with a fresh attestation.
    pub async fn register(&self) -> Result<Registration, AgentError> {
        let account_id = self.account_id();
        self.ensure_funded().await?;
        self.check_sponsor_balance().await;

        // Storage is only charged the first time.
        let deposit = match self.agent_view().await? {
            Some(_) => 0,
            None => REGISTER_DEPOSIT,
        };
        let attestation = build_attestation(
            self.tee.as_ref(),
            &self.http,
            &self.pccs_endpoints,
            &self.agent.report_data(),
        )
        .await?;
        if !self.approval_logged.swap(true, Ordering::Relaxed) {
            log_approval_args(&attestation);
        }
        let register = Action::FunctionCall {
            method_name: "register_agent".into(),
            args: serde_json::to_vec(&json!({ "attestation": attestation }))?,
            gas: REGISTER_GAS,
            deposit,
        };
        let tx_hash = self.relay(&self.contract_id, register).await?;

        let valid_until_ms = self
            .agent_view()
            .await?
            .as_ref()
            .and_then(parse_valid_until)
            .ok_or_else(|| {
                AgentError::Rpc(format!(
                    "get_agent has no valid_until_ms after tx {tx_hash}"
                ))
            })?;
        Ok(Registration {
            account_id,
            tx_hash,
            valid_until_ms,
        })
    }

    /// Has the agent sign `action` for `receiver_id` as a delegate action, then sends it
    /// from the sponsor, which pays its gas and deposit. Returns the relay's tx hash.
    async fn relay(&self, receiver_id: &str, action: Action) -> Result<String, AgentError> {
        let account_id = self.account_id();
        let access = self.access(&self.agent, &account_id).await?;
        let signed = DelegateAction {
            sender_id: account_id.clone(),
            receiver_id: receiver_id.to_string(),
            actions: vec![action],
            nonce: access.nonce + 1,
            max_block_height: access.block_height + DELEGATE_TTL_BLOCKS,
            public_key: self.agent.public_key(),
        }
        .sign(&self.agent);
        self.send(
            &self.sponsor,
            &self.sponsor_id,
            &account_id,
            Action::Delegate(Box::new(signed)),
        )
        .await
    }

    /// Registers now and again before each registration expires, retrying with
    /// backoff on failure. Runs until the process exits.
    pub async fn run(self) {
        let mut retry = MIN_RETRY_DELAY;
        loop {
            let attempt = self.register().await;
            self.status.record(&attempt);
            let delay = match attempt {
                Ok(reg) => {
                    info!(
                        "Registered agent {} with {} (tx {}), valid until {} ms",
                        reg.account_id, self.contract_id, reg.tx_hash, reg.valid_until_ms
                    );
                    retry = MIN_RETRY_DELAY;
                    refresh_delay(reg.valid_until_ms, now_ms())
                }
                Err(e) => {
                    warn!("Agent registration failed, retrying in {retry:?}: {e}");
                    let delay = retry;
                    retry = next_retry(retry);
                    delay
                }
            };
            tokio::time::sleep(delay).await;
        }
    }
}

/// Logs what the owner must approve for this build to register, ready to pass as
/// `json-args`. Registration fails until they are approved, so log before sending.
fn log_approval_args(attestation: &Value) {
    match approval_args(attestation) {
        Ok((measurements, ppids)) => info!(
            "To let this build register, the contract owner calls approve_measurements with {measurements} and approve_ppids with {ppids}"
        ),
        Err(e) => warn!("Could not derive approval args from the attestation: {e}"),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The contract returns `valid_until_ms` as a `U64` string; accept a number too.
fn parse_valid_until(view: &Value) -> Option<u64> {
    match view.get("valid_until_ms")? {
        Value::String(s) => s.parse().ok(),
        v => v.as_u64(),
    }
}

/// Re-register `min(validity / 2, MAX_REFRESH_MARGIN)` before expiry, so a short
/// contract validity can't turn into a re-register every `MIN_REFRESH_DELAY`.
pub(crate) fn refresh_delay(valid_until_ms: u64, now_ms: u64) -> Duration {
    let validity = valid_until_ms.saturating_sub(now_ms);
    let margin = (validity / 2).min(MAX_REFRESH_MARGIN.as_millis() as u64);
    Duration::from_millis(validity - margin).max(MIN_REFRESH_DELAY)
}

pub(crate) fn next_retry(delay: Duration) -> Duration {
    (delay * 2).min(MAX_RETRY_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attestation::tests::FakeTee;
    use crate::collateral::tests::mock_pccs;
    use crate::config::tests::config;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ring::signature::{UnparsedPublicKey, ED25519};
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const NEAR: u128 = 1_000_000_000_000_000_000_000_000;
    const SPONSOR_SEED: [u8; 32] = [1u8; 32];
    const AGENT_SEED: [u8; 32] = [2u8; 32];
    const SIGNATURE_LEN: usize = 1 + 64;

    fn rpc_result(value: Value) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .set_body_json(json!({ "jsonrpc": "2.0", "id": "sigstack", "result": value }))
    }

    async fn on_rpc(server: &MockServer, request: Value, response: ResponseTemplate) {
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_partial_json(request))
            .respond_with(response)
            .mount(server)
            .await;
    }

    struct Fixture {
        server: MockServer,
        tee: Arc<FakeTee>,
        registrar: Registrar,
        agent_id: String,
    }

    /// An RPC + PCCS mock where the agent has `agent_balance` and get_agent
    /// returns `agent_view`.
    async fn fixture(agent_balance: Option<u128>, agent_view: Value) -> Fixture {
        let server = MockServer::start().await;
        mock_pccs(&server).await;
        let agent_id = SigningKey::from_seed(&AGENT_SEED)
            .unwrap()
            .implicit_account_id();

        let balance = match agent_balance {
            Some(amount) => rpc_result(json!({ "amount": amount.to_string() })),
            None => ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": "sigstack",
                "error": { "name": "HANDLER_ERROR", "cause": { "name": "UNKNOWN_ACCOUNT" } }
            })),
        };
        on_rpc(
            &server,
            json!({ "params": { "request_type": "view_account", "account_id": agent_id } }),
            balance,
        )
        .await;
        for (account, nonce) in [("sponsor.testnet", 10), (agent_id.as_str(), 20)] {
            on_rpc(
                &server,
                json!({ "params": { "request_type": "view_access_key", "account_id": account } }),
                rpc_result(json!({
                    "nonce": nonce,
                    "block_hash": base58::encode(&[9u8; 32]),
                    "block_height": 1000
                })),
            )
            .await;
        }
        on_rpc(
            &server,
            json!({ "params": { "request_type": "call_function", "method_name": "get_agent" } }),
            rpc_result(json!({ "result": serde_json::to_vec(&agent_view).unwrap() })),
        )
        .await;
        on_rpc(
            &server,
            json!({ "method": "send_tx" }),
            rpc_result(json!({ "status": { "SuccessValue": "" } })),
        )
        .await;

        let tee = Arc::new(FakeTee::new());
        let mut cfg = config();
        cfg.rpc_url = server.uri();
        cfg.pccs_urls = server.uri();
        cfg.sponsor_private_key = format!("ed25519:{}", base58::encode(&SPONSOR_SEED));
        let registrar = Registrar::with_key(
            &cfg,
            tee.clone(),
            SigningKey::from_seed(&AGENT_SEED).unwrap(),
        )
        .unwrap();
        Fixture {
            server,
            tee,
            registrar,
            agent_id,
        }
    }

    /// Signed transactions the mock received, in order.
    async fn sent_transactions(server: &MockServer) -> Vec<Vec<u8>> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .filter(|body| body["method"] == "send_tx")
            .map(|body| {
                STANDARD
                    .decode(body["params"]["signed_tx_base64"].as_str().unwrap())
                    .unwrap()
            })
            .collect()
    }

    /// The trailing u128 deposit of a one-action transfer or function call.
    fn deposit(signed: &[u8]) -> u128 {
        let end = signed.len() - SIGNATURE_LEN;
        u128::from_le_bytes(signed[end - 16..end].try_into().unwrap())
    }

    fn signer(signed: &[u8]) -> String {
        let len = u32::from_le_bytes(signed[..4].try_into().unwrap()) as usize;
        String::from_utf8(signed[4..4 + len].to_vec()).unwrap()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn find(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap()
    }

    fn u32_at(bytes: &[u8], at: usize) -> usize {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
    }

    /// The deposit attached to the `register_agent` call inside a relayed transaction.
    fn register_deposit(signed: &[u8]) -> u128 {
        let args_at = find(signed, b"register_agent") + "register_agent".len();
        let deposit_at = args_at + 4 + u32_at(signed, args_at) + 8;
        u128::from_le_bytes(signed[deposit_at..deposit_at + 16].try_into().unwrap())
    }

    /// The delegate action a sponsor transaction relays, and the agent's signature on it.
    fn relayed_delegate(signed: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let receiver_at = 4 + u32_at(signed, 0) + 33 + 8;
        let tag_at = receiver_at + 4 + u32_at(signed, receiver_at) + 32 + 4;
        assert_eq!(signed[tag_at], 8, "not a Delegate action");
        let end = signed.len() - SIGNATURE_LEN;
        let delegate = signed[tag_at + 1..end - SIGNATURE_LEN].to_vec();
        (delegate, signed[end - 64..end].to_vec())
    }

    #[test]
    fn status_keeps_the_registration_through_a_failed_refresh() {
        let status = AgentStatus::new("agent", "contract.testnet");
        assert_eq!(status.state(), RegistrationState::default());
        let registration = Registration {
            account_id: "agent".into(),
            tx_hash: "tx".into(),
            valid_until_ms: 5,
        };
        status.record(&Ok(registration.clone()));
        status.record(&Err(AgentError::Rpc("down".into())));
        let state = status.state();
        assert_eq!(state.registration, Some(registration.clone()));
        assert_eq!(state.last_error.as_deref(), Some("NEAR RPC: down"));
        status.record(&Ok(registration));
        assert_eq!(status.state().last_error, None);
    }

    #[tokio::test]
    async fn status_names_this_boots_agent_and_contract() {
        let f = fixture(Some(NEAR * 3 / 1000), json!({ "valid_until_ms": "1" })).await;
        let status = f.registrar.status();
        assert_eq!(status.account_id, f.agent_id);
        assert_eq!(status.contract_id, "sigstack-agent.testnet");
    }

    #[tokio::test]
    async fn sponsor_creates_the_missing_agent_account() {
        let f = fixture(None, json!({ "valid_until_ms": "1700000000000" })).await;
        f.registrar.ensure_funded().await.unwrap();
        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(signer(&sent[0]), "sponsor.testnet");
        assert_eq!(deposit(&sent[0]), NEAR * 3 / 1000);
        assert!(contains(&sent[0], f.agent_id.as_bytes()));
    }

    #[tokio::test]
    async fn registers_with_the_agent_key_and_report_data() {
        let f = fixture(
            Some(NEAR * 3 / 1000),
            json!({ "valid_until_ms": "1700000000000" }),
        )
        .await;
        let registration = f.registrar.register().await.unwrap();

        assert_eq!(registration.account_id, f.agent_id);
        assert_eq!(registration.valid_until_ms, 1_700_000_000_000);

        // Account exists: only the relayed register call. The sponsor signs and pays; no
        // storage deposit because get_agent already knows the agent.
        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(signer(&sent[0]), "sponsor.testnet");
        assert_eq!(register_deposit(&sent[0]), 0);
        assert!(contains(&sent[0], b"\"attestation\""));
        assert_eq!(base58::decode(&registration.tx_hash).unwrap().len(), 32);

        // The agent signed the delegate action: from its account, to the contract, with
        // its next nonce, valid for 100 blocks, under the NEP-461 prefix.
        let agent = SigningKey::from_seed(&AGENT_SEED).unwrap();
        let (delegate, signature) = relayed_delegate(&sent[0]);
        let sender = [&64u32.to_le_bytes()[..], f.agent_id.as_bytes()].concat();
        assert!(delegate.starts_with(&sender));
        assert!(contains(&delegate, b"sigstack-agent.testnet"));
        let nonce_and_expiry = [21u64.to_le_bytes(), 1100u64.to_le_bytes()].concat();
        assert!(contains(&delegate, &nonce_and_expiry));
        let mut message = ((1u32 << 30) + 366).to_le_bytes().to_vec();
        message.extend_from_slice(&delegate);
        let hash = Sha256::digest(&message);
        UnparsedPublicKey::new(&ED25519, agent.public_key())
            .verify(&hash, &signature)
            .unwrap();

        assert_eq!(*f.tee.quoted.lock().unwrap(), vec![agent.report_data()]);
    }

    #[tokio::test]
    async fn new_agents_attach_the_storage_deposit() {
        // get_agent returns null: not registered yet, and still null afterwards.
        let f = fixture(Some(NEAR * 3 / 1000), Value::Null).await;
        let err = f.registrar.register().await.unwrap_err();
        assert!(err.to_string().contains("valid_until_ms"), "{err}");

        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(register_deposit(&sent[0]), REGISTER_DEPOSIT);
    }

    #[tokio::test]
    async fn low_balance_is_topped_up_to_target() {
        let f = fixture(Some(NEAR / 1000), json!({ "valid_until_ms": "1" })).await;
        f.registrar.ensure_funded().await.unwrap();
        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(deposit(&sent[0]), NEAR * 3 / 1000 - NEAR / 1000);
    }

    // A failed relayed call refunded its deposit to the agent: the excess goes back to the
    // sponsor.
    #[tokio::test]
    async fn refunded_excess_returns_to_the_sponsor() {
        let f = fixture(Some(NEAR * 8 / 1000), Value::Null).await;
        f.registrar.ensure_funded().await.unwrap();

        // Signed and paid by the agent itself, not relayed: a relayed transfer's deposit
        // would come out of the sponsor.
        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(signer(&sent[0]), f.agent_id);
        assert!(contains(&sent[0], b"sponsor.testnet"));
        assert_eq!(deposit(&sent[0]), NEAR * 5 / 1000 - RETURN_GAS_RESERVE);
    }

    // A low sponsor only warns; registration goes ahead while it can still pay.
    #[tokio::test]
    async fn low_sponsor_still_registers() {
        let f = fixture(
            Some(NEAR * 3 / 1000),
            json!({ "valid_until_ms": "1700000000000" }),
        )
        .await;
        on_rpc(
            &f.server,
            json!({ "params": { "request_type": "view_account", "account_id": "sponsor.testnet" } }),
            rpc_result(json!({ "amount": (NEAR / 2).to_string() })),
        )
        .await;
        f.registrar.register().await.unwrap();
        assert_eq!(sent_transactions(&f.server).await.len(), 1);
    }

    // Dust above the target isn't worth the relay's gas.
    #[tokio::test]
    async fn small_excess_stays() {
        let f = fixture(Some(NEAR * 3 / 1000 + NEAR / 10_000), Value::Null).await;
        f.registrar.ensure_funded().await.unwrap();
        assert!(sent_transactions(&f.server).await.is_empty());
    }

    // Returning the excess is best effort; a failure doesn't stop registration.
    #[tokio::test]
    async fn failed_return_is_not_fatal() {
        let f = fixture(Some(NEAR), Value::Null).await;
        f.server.reset().await;
        on_rpc(
            &f.server,
            json!({ "params": { "request_type": "view_account" } }),
            rpc_result(json!({ "amount": NEAR.to_string() })),
        )
        .await;
        f.registrar.ensure_funded().await.unwrap();
        assert!(sent_transactions(&f.server).await.is_empty());
    }

    #[tokio::test]
    async fn missing_access_key_is_an_error() {
        let f = fixture(None, Value::Null).await;
        let mut cfg = config();
        cfg.rpc_url = f.server.uri();
        cfg.sponsor_account_id = "nobody.testnet".into();
        cfg.sponsor_private_key = format!("ed25519:{}", base58::encode(&SPONSOR_SEED));
        on_rpc(
            &f.server,
            json!({ "params": { "request_type": "view_access_key", "account_id": "nobody.testnet" } }),
            ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0", "id": "sigstack",
                "error": { "name": "HANDLER_ERROR", "cause": { "name": "UNKNOWN_ACCESS_KEY" } }
            })),
        )
        .await;
        let registrar = Registrar::with_key(
            &cfg,
            f.tee.clone(),
            SigningKey::from_seed(&AGENT_SEED).unwrap(),
        )
        .unwrap();
        let err = registrar.ensure_funded().await.unwrap_err();
        assert!(
            err.to_string().contains("nobody.testnet has no access key"),
            "{err}"
        );
    }

    #[test]
    fn new_rejects_bad_config() {
        let tee: Arc<dyn Tee> = Arc::new(FakeTee::new());
        let mut cfg = config();
        cfg.contract_id = String::new();
        assert!(matches!(
            Registrar::new(&cfg, tee.clone()),
            Err(AgentError::Config(_))
        ));
        // config() has a placeholder sponsor key.
        assert!(matches!(
            Registrar::new(&config(), tee),
            Err(AgentError::Key(_))
        ));
    }

    #[test]
    fn refreshes_before_expiry() {
        let hour = 60 * 60 * 1000;
        let day = 24 * hour;
        // Week-long validity: a day before expiry.
        assert_eq!(
            refresh_delay(10 * day, 3 * day),
            Duration::from_millis(6 * day)
        );
        // Short validity: halfway through, not every ten minutes.
        assert_eq!(refresh_delay(day, 0), Duration::from_millis(12 * hour));
        assert_eq!(refresh_delay(4 * hour, 0), Duration::from_millis(2 * hour));
        assert_eq!(refresh_delay(10 * 60 * 1000, 0), MIN_REFRESH_DELAY);
        assert_eq!(refresh_delay(0, 5 * day), MIN_REFRESH_DELAY);
    }

    #[test]
    fn reads_valid_until_as_string_or_number() {
        let parse = |v: Value| parse_valid_until(&v);
        assert_eq!(parse(json!({ "valid_until_ms": "123" })), Some(123));
        assert_eq!(parse(json!({ "valid_until_ms": 123 })), Some(123));
        assert_eq!(parse(json!({ "valid_until_ms": "soon" })), None);
        assert_eq!(parse(json!({})), None);
    }

    #[test]
    fn retry_backs_off_to_a_cap() {
        assert_eq!(next_retry(MIN_RETRY_DELAY), Duration::from_secs(60));
        assert_eq!(next_retry(MAX_RETRY_DELAY), MAX_RETRY_DELAY);
        assert_eq!(next_retry(Duration::from_secs(20 * 60)), MAX_RETRY_DELAY);
        assert!(now_ms() > 1_700_000_000_000);
    }
}
