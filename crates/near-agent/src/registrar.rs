//! Funds the agent account and keeps it registered with the agent contract.

use crate::{
    attestation::build_attestation,
    base58,
    rpc::NearRpc,
    tx::{Action, Transaction},
    AgentConfig, AgentError, SigningKey, Tee,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// What shade-agent-js attaches; attestation verification is gas-heavy.
pub const REGISTER_GAS: u64 = 300_000_000_000_000;
/// 0.005 NEAR covers the contract's storage for a new agent (486 bytes).
pub const REGISTER_DEPOSIT: u128 = 5_000_000_000_000_000_000_000;

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
            contract_id: config.contract_id.trim().to_string(),
            min_balance,
            top_up_balance,
            pccs_endpoints: config.pccs_endpoints(),
        })
    }

    /// This boot's agent account: the implicit account of the random key.
    pub fn account_id(&self) -> String {
        self.agent.implicit_account_id()
    }

    /// Signs and sends a one-action transaction; returns its base58 hash.
    async fn send(
        &self,
        key: &SigningKey,
        signer_id: &str,
        receiver_id: &str,
        action: Action,
    ) -> Result<String, AgentError> {
        let public_key = key.public_key_string();
        let access = self
            .rpc
            .access_key(signer_id, &public_key)
            .await?
            .ok_or_else(|| {
                AgentError::Rpc(format!("{signer_id} has no access key {public_key}"))
            })?;
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

    /// Tops the agent account up from the sponsor when it runs low. The first
    /// transfer creates the implicit account.
    pub async fn ensure_funded(&self) -> Result<(), AgentError> {
        let account_id = self.account_id();
        let balance = self.rpc.balance(&account_id).await?.unwrap_or(0);
        if balance >= self.min_balance {
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
        let tx_hash = self
            .send(
                &self.agent,
                &account_id,
                &self.contract_id,
                Action::FunctionCall {
                    method_name: "register_agent".into(),
                    args: serde_json::to_vec(&json!({ "attestation": attestation }))?,
                    gas: REGISTER_GAS,
                    deposit,
                },
            )
            .await?;

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

    /// Registers now and again before each registration expires, retrying with
    /// backoff on failure. Runs until the process exits.
    pub async fn run(self) {
        let mut retry = MIN_RETRY_DELAY;
        loop {
            let delay = match self.register().await {
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
                rpc_result(json!({ "nonce": nonce, "block_hash": base58::encode(&[9u8; 32]) })),
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

    #[tokio::test]
    async fn sponsor_creates_the_missing_agent_account() {
        let f = fixture(None, json!({ "valid_until_ms": "1700000000000" })).await;
        f.registrar.ensure_funded().await.unwrap();
        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(signer(&sent[0]), "sponsor.testnet");
        assert_eq!(deposit(&sent[0]), NEAR / 10);
        assert!(contains(&sent[0], f.agent_id.as_bytes()));
    }

    #[tokio::test]
    async fn registers_with_the_agent_key_and_report_data() {
        let f = fixture(Some(NEAR), json!({ "valid_until_ms": "1700000000000" })).await;
        let registration = f.registrar.register().await.unwrap();

        assert_eq!(registration.account_id, f.agent_id);
        assert_eq!(registration.valid_until_ms, 1_700_000_000_000);

        // Well funded: only the register call, signed by the agent, no storage deposit
        // because get_agent already knows it.
        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(signer(&sent[0]), f.agent_id);
        assert_eq!(deposit(&sent[0]), 0);
        assert!(contains(&sent[0], b"register_agent"));
        assert!(contains(&sent[0], b"\"attestation\""));
        assert_eq!(base58::decode(&registration.tx_hash).unwrap().len(), 32);

        let agent = SigningKey::from_seed(&AGENT_SEED).unwrap();
        assert_eq!(*f.tee.quoted.lock().unwrap(), vec![agent.report_data()]);
    }

    #[tokio::test]
    async fn new_agents_attach_the_storage_deposit() {
        // get_agent returns null: not registered yet, and still null afterwards.
        let f = fixture(Some(NEAR), Value::Null).await;
        let err = f.registrar.register().await.unwrap_err();
        assert!(err.to_string().contains("valid_until_ms"), "{err}");

        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(deposit(&sent[0]), REGISTER_DEPOSIT);
    }

    #[tokio::test]
    async fn low_balance_is_topped_up_to_target() {
        let f = fixture(Some(NEAR / 100), json!({ "valid_until_ms": "1" })).await;
        f.registrar.ensure_funded().await.unwrap();
        let sent = sent_transactions(&f.server).await;
        assert_eq!(sent.len(), 1);
        assert_eq!(deposit(&sent[0]), NEAR / 10 - NEAR / 100);
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
