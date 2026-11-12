//! Verifies that NEAR AI responses come from an attested model TEE.
//!
//! For each chat completion, in the background (replies never wait on it):
//!
//! 1. Fetch the response's signature and check it: ed25519, made by the model's own
//!    TEE (`provider_tee`, which needs `x-no-aliasing`), over
//!    `{model}:{sha256(request)}:{sha256(response)}` of the exact bytes exchanged.
//! 2. Make sure the signing key belongs to an attested instance of that model. If it
//!    isn't cached (or the cache entry is older than the TTL), fetch an attestation
//!    report with a fresh nonce and verify it:
//!    - the TDX quote against Intel's root of trust (dcap-qvl, collateral from PCCS);
//!    - report data = the signing key, then our nonce;
//!    - not a debug TD; the event log replays to RTMR3 and measures the reported
//!      app definition;
//!    - the GPU evidence carries our nonce and NVIDIA's attestation service accepts it.
//!
//! Failures are logged and counted per model, for `!verify` to report. They don't
//! block or alter replies.
//!
//! Whisper transcriptions return no ID, and NEAR AI signs no transcription, so for
//! Whisper only the model's attestation can be checked.

pub mod gpu;
pub mod report;
pub mod signature;
pub mod tdx;

use crate::{ChatExchange, ChatObserver};
use report::{AttestationResponse, ModelAttestation};
use ring::rand::{SecureRandom, SystemRandom};
use secrecy::{ExposeSecret, SecretString};
use signature::ResponseSignature;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::{info, warn};

/// How many fresh reports to fetch looking for a response's signer: NEAR AI
/// load-balances, so each report may describe a different instance.
const ATTEST_ATTEMPTS: usize = 4;
/// Signatures can take a moment to reach the node that serves them.
const SIGNATURE_ATTEMPTS: usize = 4;

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Fetch(String),
    #[error("attestation report: {0}")]
    Report(String),
    #[error("collateral: {0}")]
    Collateral(String),
    #[error("TDX quote: {0}")]
    Quote(String),
    #[error("quote binding: {0}")]
    Binding(String),
    #[error("GPU: {0}")]
    Gpu(String),
    #[error("response signature: {0}")]
    Signature(String),
}

impl VerifyError {
    /// Failures of the infrastructure around a check (NEAR AI, PCCS or NRAS being
    /// unreachable or erroring) rather than of what it checked. Worth retrying.
    fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Http(_) | Self::Fetch(_) | Self::Collateral(_) | Self::Gpu(_)
        )
    }
}

#[derive(Debug, Clone)]
pub struct VerifierConfig {
    /// NEAR AI's API base, e.g. `https://cloud-api.near.ai/v1`.
    pub base_url: String,
    pub nras_url: String,
    /// PCCS endpoints for quote collateral, tried in order.
    pub pccs_endpoints: Vec<String>,
    /// How long a verified instance is trusted before it's attested again.
    pub attestation_ttl: Duration,
    /// Wait between attempts to fetch a signature.
    pub retry_delay: Duration,
}

impl VerifierConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            nras_url: gpu::NRAS_URL.into(),
            pccs_endpoints: vec![
                near_agent::collateral::PHALA_PCCS_URL.into(),
                near_agent::collateral::INTEL_PCS_URL.into(),
            ],
            attestation_ttl: Duration::from_secs(60 * 60),
            retry_delay: Duration::from_secs(1),
        }
    }
}

/// A model instance whose attestation verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestedInstance {
    pub model: String,
    /// Hex ed25519 public key the instance signs responses with.
    pub signing_key: String,
    /// sha256 of the instance's app definition, as measured into RTMR3.
    pub compose_hash: String,
    /// Intel's TCB status for the platform, e.g. `UpToDate`.
    pub tcb_status: String,
    pub advisory_ids: Vec<String>,
    /// GPUs NVIDIA's attestation service vouched for.
    pub gpus: usize,
    pub verified_at: SystemTime,
}

/// A failed attestation or response check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub at: SystemTime,
    pub error: String,
}

/// What `!verify` can report about one model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelStatus {
    pub model: String,
    /// The most recently verified instance.
    pub instance: Option<AttestedInstance>,
    pub verified_responses: u64,
    pub failed_responses: u64,
    /// When a response last verified.
    pub last_verified_at: Option<SystemTime>,
    /// The last attestation or response failure. Compare its time with
    /// `last_verified_at` and `instance.verified_at` to tell whether it's still current.
    pub last_failure: Option<Failure>,
}

#[derive(Default)]
struct State {
    /// Verified instances by signing key.
    instances: HashMap<String, AttestedInstance>,
    models: BTreeMap<String, ModelStatus>,
}

type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;
type NonceSource = Box<dyn Fn() -> [u8; 32] + Send + Sync>;

pub struct ModelVerifier {
    http: reqwest::Client,
    config: VerifierConfig,
    api_key: SecretString,
    state: Mutex<State>,
    /// One attestation at a time, so a burst of responses fetches one report.
    attesting: tokio::sync::Mutex<()>,
    clock: Clock,
    nonce: NonceSource,
}

impl ModelVerifier {
    pub fn new(config: VerifierConfig, api_key: impl Into<String>) -> Result<Self, VerifyError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self {
            http,
            config,
            api_key: SecretString::new(api_key.into()),
            state: Mutex::default(),
            attesting: tokio::sync::Mutex::new(()),
            clock: Box::new(SystemTime::now),
            nonce: Box::new(|| {
                let mut nonce = [0u8; 32];
                SystemRandom::new()
                    .fill(&mut nonce)
                    .expect("system randomness");
                nonce
            }),
        })
    }

    /// Fetches an attestation report for `model` with a fresh nonce and verifies the
    /// instance it describes. Failures are recorded for `status`.
    pub async fn attest(&self, model: &str) -> Result<AttestedInstance, VerifyError> {
        let result = self.fetch_and_verify(model).await;
        let now = (self.clock)();
        let mut state = self.state();
        let status = model_entry(&mut state, model);
        match &result {
            Ok(instance) => {
                status.instance = Some(instance.clone());
                state
                    .instances
                    .insert(instance.signing_key.clone(), instance.clone());
                info!(
                    "Attested {model} instance {} (compose {}, TCB {}, {} GPUs)",
                    instance.signing_key, instance.compose_hash, instance.tcb_status, instance.gpus
                );
            }
            Err(e) => {
                status.last_failure = Some(Failure {
                    at: now,
                    error: format!("attestation: {e}"),
                });
                warn!("Attestation of {model} failed: {e}");
            }
        }
        result
    }

    async fn fetch_and_verify(&self, model: &str) -> Result<AttestedInstance, VerifyError> {
        let nonce = (self.nonce)();
        let nonce_hex = hex::encode(nonce);
        let response = self
            .http
            .get(format!("{}/attestation/report", self.config.base_url))
            .query(&[
                ("model", model),
                ("provider", "near"),
                ("signing_algo", "ed25519"),
                ("nonce", &nonce_hex),
            ])
            .header("x-no-aliasing", "true")
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(VerifyError::Fetch(format!(
                "attestation report: HTTP {}",
                response.status()
            )));
        }
        let report: AttestationResponse = response.json().await?;
        let attestation = report
            .model_attestations
            .into_iter()
            .find(|a| a.model_name == model)
            .ok_or_else(|| VerifyError::Report(format!("no attestation for {model}")))?;
        self.verify_attestation(&attestation, &nonce).await
    }

    async fn verify_attestation(
        &self,
        attestation: &ModelAttestation,
        nonce: &[u8; 32],
    ) -> Result<AttestedInstance, VerifyError> {
        let nonce_hex = hex::encode(nonce);
        if !attestation.request_nonce.eq_ignore_ascii_case(&nonce_hex) {
            return Err(VerifyError::Report("the report isn't for our nonce".into()));
        }
        if attestation.signing_algo != "ed25519" {
            return Err(VerifyError::Report(format!(
                "unexpected signing algorithm {}",
                attestation.signing_algo
            )));
        }

        let quote = hex::decode(&attestation.intel_quote)
            .map_err(|_| VerifyError::Report("intel_quote is not hex".into()))?;
        let collateral = near_agent::collateral::fetch_collateral(
            &self.http,
            &self.config.pccs_endpoints,
            &quote,
        )
        .await
        .map_err(|e| VerifyError::Collateral(e.to_string()))?;
        let now = (self.clock)();
        let facts = tdx::verify_quote(
            &quote,
            &tdx::collateral_from_json(&collateral)?,
            unix_secs(now),
        )?;
        let compose_hash = tdx::check_bindings(attestation, &facts, nonce)?;

        let payload = attestation
            .nvidia_payload
            .as_deref()
            .ok_or_else(|| VerifyError::Gpu("the report has no GPU evidence".into()))?;
        let gpus = gpu::verify_gpus(&self.http, &self.config.nras_url, payload, &nonce_hex).await?;

        Ok(AttestedInstance {
            model: attestation.model_name.clone(),
            signing_key: normalize_key(&attestation.signing_address),
            compose_hash,
            tcb_status: facts.status,
            advisory_ids: facts.advisory_ids,
            gpus,
            verified_at: now,
        })
    }

    /// Checks one exchange's signature and that its signer is an attested instance of
    /// the model.
    pub async fn verify_exchange(
        &self,
        exchange: &ChatExchange,
    ) -> Result<AttestedInstance, VerifyError> {
        let sig = self.fetch_signature(exchange).await?;
        signature::check_signature(&sig, exchange)?;
        let key = normalize_key(&sig.signing_address);
        if let Some(instance) = self.fresh_instance(&key, &exchange.model) {
            return Ok(instance);
        }
        let _one_at_a_time = self.attesting.lock().await;
        let mut transient = None;
        for _ in 0..ATTEST_ATTEMPTS {
            if let Some(instance) = self.fresh_instance(&key, &exchange.model) {
                return Ok(instance);
            }
            match self.attest(&exchange.model).await {
                Ok(_) => {}
                // An outage on one attempt shouldn't use up the rest.
                Err(e) if e.is_transient() => transient = Some(e),
                // A report that fails verification is a finding, not a hiccup.
                Err(e) => return Err(e),
            }
        }
        self.fresh_instance(&key, &exchange.model).ok_or_else(|| {
            transient.unwrap_or_else(|| {
                VerifyError::Signature(format!(
                    "signer {key} wasn't in {ATTEST_ATTEMPTS} fresh attestation reports"
                ))
            })
        })
    }

    /// Re-attests `models` every half TTL, starting now. Keeps each model's status
    /// current even when no responses arrive to prompt it: Whisper never has any.
    pub fn spawn_refresh(self: &Arc<Self>, models: Vec<String>) -> tokio::task::JoinHandle<()> {
        let verifier = self.clone();
        let period = (self.config.attestation_ttl / 2).max(Duration::from_millis(1));
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(period);
            loop {
                ticks.tick().await;
                for model in &models {
                    let _one_at_a_time = verifier.attesting.lock().await;
                    // Failures are recorded and logged by attest.
                    let _ = verifier.attest(model).await;
                }
            }
        })
    }

    async fn fetch_signature(
        &self,
        exchange: &ChatExchange,
    ) -> Result<ResponseSignature, VerifyError> {
        let url = format!("{}/signature/{}", self.config.base_url, exchange.chat_id);
        let mut last = String::new();
        for attempt in 0..SIGNATURE_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(self.config.retry_delay).await;
            }
            let response = self
                .http
                .get(&url)
                .query(&[
                    ("model", exchange.model.as_str()),
                    ("signing_algo", "ed25519"),
                ])
                .header(
                    "Authorization",
                    format!("Bearer {}", self.api_key.expose_secret()),
                )
                .send()
                .await?;
            let status = response.status();
            if status.is_success() {
                let body: serde_json::Value = response.json().await?;
                return serde_json::from_value(body.clone())
                    .map_err(|_| VerifyError::Fetch(format!("signature unavailable: {body}")));
            }
            last = format!("signature: HTTP {status}");
            // Not there yet (404), throttled, or a server error: worth another try.
            let retry = status == reqwest::StatusCode::NOT_FOUND
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error();
            if !retry {
                break;
            }
        }
        Err(VerifyError::Fetch(last))
    }

    fn fresh_instance(&self, key: &str, model: &str) -> Option<AttestedInstance> {
        let now = (self.clock)();
        self.state()
            .instances
            .get(key)
            .filter(|i| i.model == model)
            .filter(|i| {
                now.duration_since(i.verified_at).unwrap_or_default() < self.config.attestation_ttl
            })
            .cloned()
    }

    /// Verifies `exchange` and records the outcome for `status`.
    pub async fn check(&self, exchange: ChatExchange) {
        let result = self.verify_exchange(&exchange).await;
        let now = (self.clock)();
        let mut state = self.state();
        let status = model_entry(&mut state, &exchange.model);
        match result {
            Ok(instance) => {
                status.verified_responses += 1;
                status.last_verified_at = Some(now);
                info!("{}", verified_line(&exchange, &instance, status));
            }
            Err(e) => {
                status.failed_responses += 1;
                status.last_failure = Some(Failure {
                    at: now,
                    error: e.to_string(),
                });
                warn!(
                    "Response {} from {} failed verification: {e}",
                    exchange.chat_id, exchange.model
                );
            }
        }
    }

    /// Per-model results so far, by model name.
    pub fn status(&self) -> Vec<ModelStatus> {
        self.state().models.values().cloned().collect()
    }

    /// The state stays usable even if a task panicked while holding it: each update
    /// is a few field writes, so there's no half-done invariant to protect.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Signing keys as hex without `0x`, lowercase, wherever they're compared.
fn normalize_key(key: &str) -> String {
    key.trim_start_matches("0x").to_ascii_lowercase()
}

/// The info line for a verified response, so the logs show each check passing rather than only
/// the absence of failures. IDs, model, key and counts only; never message content.
fn verified_line(
    exchange: &ChatExchange,
    instance: &AttestedInstance,
    status: &ModelStatus,
) -> String {
    let (verified, failed) = (status.verified_responses, status.failed_responses);
    format!(
        "Response {} verified: signed by attested {} instance {} (compose {}); \
         {verified} verified, {failed} failed so far",
        exchange.chat_id, exchange.model, instance.signing_key, instance.compose_hash
    )
}

fn model_entry<'a>(state: &'a mut State, model: &str) -> &'a mut ModelStatus {
    state
        .models
        .entry(model.to_owned())
        .or_insert_with(|| ModelStatus {
            model: model.to_owned(),
            ..ModelStatus::default()
        })
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Verifies each exchange on a background task, so replies never wait on it.
pub struct BackgroundVerifier(pub Arc<ModelVerifier>);

impl ChatObserver for BackgroundVerifier {
    fn observe(&self, exchange: ChatExchange) {
        let verifier = self.0.clone();
        tokio::spawn(async move { verifier.check(exchange).await });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ring::signature::KeyPair;
    use serde_json::{json, Value};
    use std::sync::OnceLock;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const FIXTURE: &str = include_str!("../../tests/fixtures/near_ai_model.json");

    /// A real report, its PCCS collateral and NRAS verdict, and a signed chat exchange,
    /// captured from NEAR AI on 2026-11-11 (the fixture's `source` says how).
    pub(crate) fn fixture() -> &'static Value {
        static PARSED: OnceLock<Value> = OnceLock::new();
        PARSED.get_or_init(|| serde_json::from_str(FIXTURE).unwrap())
    }

    pub(crate) fn fixture_attestation() -> ModelAttestation {
        serde_json::from_value(fixture()["attestation"].clone()).unwrap()
    }

    pub(crate) fn fixture_nonce() -> [u8; 32] {
        hex::decode(fixture()["nonce"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap()
    }

    pub(crate) fn fixture_exchange() -> ChatExchange {
        let chat = &fixture()["chat"];
        let response = chat["response"].as_str().unwrap();
        let id = serde_json::from_str::<Value>(response).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        ChatExchange {
            chat_id: id,
            model: fixture_attestation().model_name,
            request: chat["request"].as_str().unwrap().as_bytes().to_vec(),
            response: response.as_bytes().to_vec(),
        }
    }

    pub(crate) fn fixture_signature() -> ResponseSignature {
        serde_json::from_value(fixture()["chat"]["signature"].clone()).unwrap()
    }

    fn fetched_at() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(fixture()["fetched_at_secs"].as_u64().unwrap())
    }

    /// The report as NEAR AI serves it, with GPU evidence for the fixture's nonce
    /// (NRAS is mocked, so its contents don't matter beyond the nonce).
    fn report_json() -> Value {
        let mut attestation = fixture()["attestation"].clone();
        attestation["nvidia_payload"] = json!({"nonce": fixture()["nonce"], "evidence_list": []})
            .to_string()
            .into();
        json!({"gateway_attestation": {}, "model_attestations": [attestation]})
    }

    struct Mocks {
        server: MockServer,
    }

    impl Mocks {
        async fn start() -> Self {
            let server = MockServer::start().await;
            mount_pccs(&server).await;
            Mock::given(method("POST"))
                .and(path("/nras"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(fixture()["nras_response"].clone()),
                )
                .mount(&server)
                .await;
            Self { server }
        }

        async fn report(&self, body: Value, times: u64) {
            Mock::given(method("GET"))
                .and(path("/v1/attestation/report"))
                .and(query_param("signing_algo", "ed25519"))
                .and(query_param("nonce", fixture()["nonce"].as_str().unwrap()))
                .and(header("x-no-aliasing", "true"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(times)
                .mount(&self.server)
                .await;
        }

        async fn signature(&self, body: Value) {
            Mock::given(method("GET"))
                .and(path(format!(
                    "/v1/signature/{}",
                    fixture_exchange().chat_id
                )))
                .and(query_param("signing_algo", "ed25519"))
                .and(header("Authorization", "Bearer key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&self.server)
                .await;
        }

        fn verifier(&self) -> ModelVerifier {
            let mut config = VerifierConfig::new(format!("{}/v1", self.server.uri()));
            config.nras_url = format!("{}/nras", self.server.uri());
            config.pccs_endpoints = vec![self.server.uri()];
            config.retry_delay = Duration::from_millis(1);
            let mut v = ModelVerifier::new(config, "key").unwrap();
            v.clock = Box::new(fetched_at);
            v.nonce = Box::new(fixture_nonce);
            v
        }
    }

    /// Serves the fixture's collateral the way a PCCS does.
    async fn mount_pccs(server: &MockServer) {
        let c = &fixture()["collateral"];
        let text = |k: &str| c[k].as_str().unwrap().to_owned();
        let bytes = |k: &str| hex::decode(text(k)).unwrap();
        let encode = |s: String| s.replace('\n', "%0A").replace(' ', "%20");
        Mock::given(method("GET"))
            .and(path("/sgx/certification/v4/pckcrl"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "SGX-PCK-CRL-Issuer-Chain",
                        encode(text("pck_crl_issuer_chain")).as_str(),
                    )
                    .set_body_bytes(bytes("pck_crl")),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tdx/certification/v4/tcb"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "TCB-Info-Issuer-Chain",
                        encode(text("tcb_info_issuer_chain")).as_str(),
                    )
                    .set_body_string(format!(
                        r#"{{"tcbInfo":{},"signature":"{}"}}"#,
                        text("tcb_info"),
                        text("tcb_info_signature")
                    )),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tdx/certification/v4/qe/identity"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "SGX-Enclave-Identity-Issuer-Chain",
                        encode(text("qe_identity_issuer_chain")).as_str(),
                    )
                    .set_body_string(format!(
                        r#"{{"enclaveIdentity":{},"signature":"{}"}}"#,
                        text("qe_identity"),
                        text("qe_identity_signature")
                    )),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/sgx/certification/v4/rootcacrl"))
            .respond_with(ResponseTemplate::new(200).set_body_string(text("root_ca_crl")))
            .mount(server)
            .await;
    }

    fn signature_json() -> Value {
        fixture()["chat"]["signature"].clone()
    }

    #[test]
    fn verified_line_names_ids_and_counts_without_content() {
        let exchange = ChatExchange {
            chat_id: "chatcmpl-1".into(),
            model: "m/Model".into(),
            request: b"secret request".to_vec(),
            response: b"secret response".to_vec(),
        };
        let instance = AttestedInstance {
            model: "m/Model".into(),
            signing_key: "ab12".into(),
            compose_hash: "c0ffee".into(),
            tcb_status: "UpToDate".into(),
            advisory_ids: vec![],
            gpus: 8,
            verified_at: SystemTime::UNIX_EPOCH,
        };
        let status = ModelStatus {
            verified_responses: 3,
            failed_responses: 1,
            ..ModelStatus::default()
        };
        let line = verified_line(&exchange, &instance, &status);
        assert_eq!(
            line,
            "Response chatcmpl-1 verified: signed by attested m/Model instance ab12 \
             (compose c0ffee); 3 verified, 1 failed so far"
        );
        assert!(!line.contains("secret"));
    }

    #[tokio::test]
    async fn verifies_a_real_response_end_to_end() {
        let mocks = Mocks::start().await;
        mocks.report(report_json(), 1).await;
        mocks.signature(signature_json()).await;
        let verifier = mocks.verifier();

        let exchange = fixture_exchange();
        verifier.check(exchange.clone()).await;
        // The second response from the same instance uses the cached attestation.
        verifier.check(exchange.clone()).await;

        let status = verifier.status();
        assert_eq!(status.len(), 1);
        let s = &status[0];
        assert_eq!(s.model, exchange.model);
        assert_eq!((s.verified_responses, s.failed_responses), (2, 0));
        assert_eq!(s.last_failure, None);
        assert_eq!(s.last_verified_at, Some(fetched_at()));
        let instance = s.instance.as_ref().unwrap();
        assert_eq!(instance.signing_key, fixture_signature().signing_address);
        assert_eq!(
            instance.compose_hash,
            fixture()["attestation"]["info"]["compose_hash"]
        );
        assert_eq!(instance.gpus, 8);
        assert!(!instance.tcb_status.is_empty());
        assert_eq!(instance.verified_at, fetched_at());
    }

    #[tokio::test]
    async fn never_trusts_an_expired_attestation() {
        let mocks = Mocks::start().await;
        mocks.report(report_json(), ATTEST_ATTEMPTS as u64).await;
        mocks.signature(signature_json()).await;
        let mut verifier = mocks.verifier();
        verifier.config.attestation_ttl = Duration::ZERO;
        verifier.check(fixture_exchange()).await;
        assert_eq!(verifier.status()[0].failed_responses, 1);
        let err = verifier.status()[0].last_failure.clone().unwrap().error;
        assert!(err.contains("wasn't in 4"), "{err}");
    }

    #[tokio::test]
    async fn fails_when_the_signer_is_never_attested() {
        let mocks = Mocks::start().await;
        mocks.report(report_json(), ATTEST_ATTEMPTS as u64).await;
        let mut sig = signature_json();
        // A valid signature by a key no report attests: re-sign with a fresh key.
        let rng = SystemRandom::new();
        let pkcs8 = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let key = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        sig["signing_address"] = hex::encode(key.public_key().as_ref()).into();
        sig["signature"] = hex::encode(key.sign(sig["text"].as_str().unwrap().as_bytes())).into();
        mocks.signature(sig).await;

        let verifier = mocks.verifier();
        let err = verifier
            .verify_exchange(&fixture_exchange())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("wasn't in 4 fresh attestation reports"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn records_signature_failures() {
        let mocks = Mocks::start().await;
        let mut gateway = signature_json();
        gateway["signature_kind"] = "gateway".into();
        mocks.signature(gateway).await;
        let verifier = mocks.verifier();
        verifier.check(fixture_exchange()).await;
        let s = &verifier.status()[0];
        assert_eq!((s.verified_responses, s.failed_responses), (0, 1));
        assert!(s.last_failure.as_ref().unwrap().error.contains("gateway"));
        assert_eq!(s.instance, None);
    }

    #[tokio::test]
    async fn retries_missing_signatures_then_gives_up() {
        for (status, attempts, msg) in [
            (404, SIGNATURE_ATTEMPTS, "404 Not Found"),
            (429, SIGNATURE_ATTEMPTS, "429 Too Many Requests"),
            (503, SIGNATURE_ATTEMPTS, "503 Service Unavailable"),
            (401, 1, "401 Unauthorized"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(status))
                .expect(attempts as u64)
                .mount(&server)
                .await;
            let err = Mocks { server }
                .verifier()
                .verify_exchange(&fixture_exchange())
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(err, format!("signature: HTTP {msg}"));
        }
    }

    #[tokio::test]
    async fn rides_out_a_transient_attestation_failure() {
        let mocks = Mocks::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/attestation/report"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&mocks.server)
            .await;
        mocks.report(report_json(), 1).await;
        mocks.signature(signature_json()).await;
        let instance = mocks
            .verifier()
            .verify_exchange(&fixture_exchange())
            .await
            .unwrap();
        assert_eq!(instance.gpus, 8);
    }

    #[tokio::test]
    async fn reports_the_outage_when_every_attempt_fails() {
        let mocks = Mocks::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/attestation/report"))
            .respond_with(ResponseTemplate::new(503))
            .expect(ATTEST_ATTEMPTS as u64)
            .mount(&mocks.server)
            .await;
        mocks.signature(signature_json()).await;
        let err = mocks
            .verifier()
            .verify_exchange(&fixture_exchange())
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(err, "attestation report: HTTP 503 Service Unavailable");
    }

    #[tokio::test]
    async fn stops_at_a_report_that_fails_verification() {
        let mocks = Mocks::start().await;
        let mut wrong_nonce = report_json();
        wrong_nonce["model_attestations"][0]["request_nonce"] = "00".repeat(32).into();
        mocks.report(wrong_nonce, 1).await;
        mocks.signature(signature_json()).await;
        let err = mocks
            .verifier()
            .verify_exchange(&fixture_exchange())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("isn't for our nonce"), "{err}");
    }

    #[tokio::test]
    async fn matches_signers_however_the_report_writes_the_key() {
        let mocks = Mocks::start().await;
        let mut prefixed = report_json();
        let key = fixture_signature().signing_address;
        prefixed["model_attestations"][0]["signing_address"] =
            format!("0x{}", key.to_uppercase()).into();
        mocks.report(prefixed, 1).await;
        mocks.signature(signature_json()).await;
        let instance = mocks
            .verifier()
            .verify_exchange(&fixture_exchange())
            .await
            .unwrap();
        assert_eq!(instance.signing_key, key);
        assert_eq!(normalize_key("0xABcd"), "abcd");
    }

    #[tokio::test]
    async fn refresh_reattests_each_model_periodically() {
        let mocks = Mocks::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/attestation/report"))
            .respond_with(ResponseTemplate::new(200).set_body_json(report_json()))
            .mount(&mocks.server)
            .await;
        let mut verifier = mocks.verifier();
        verifier.config.attestation_ttl = Duration::from_millis(100);
        let verifier = Arc::new(verifier);
        let model = fixture_exchange().model;
        let refresh = verifier.spawn_refresh(vec![model.clone(), "other".into()]);

        let reports = || async {
            mocks
                .server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|r| r.url.path() == "/v1/attestation/report")
                .count()
        };
        for _ in 0..200 {
            if reports().await >= 4 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        refresh.abort();
        assert!(reports().await >= 4, "only {} reports", reports().await);
        let status = verifier.status();
        assert!(status[0].instance.is_some(), "{status:?}");
        // "other" has no attestation in the report; its failure is recorded.
        assert_eq!(status[1].model, "other");
        assert!(status[1].last_failure.is_some());
    }

    #[test]
    fn state_survives_a_panic_while_locked() {
        let verifier = Arc::new(ModelVerifier::new(VerifierConfig::new("http://x"), "k").unwrap());
        let poisoner = verifier.clone();
        let _ = std::thread::spawn(move || {
            let _held = poisoner.state.lock().unwrap();
            panic!("poison the state");
        })
        .join();
        assert!(verifier.state.is_poisoned());
        assert!(verifier.status().is_empty());
    }

    #[tokio::test]
    async fn reports_unavailable_signatures() {
        let mocks = Mocks::start().await;
        mocks
            .signature(json!({"error_code": "x", "message": "not ready"}))
            .await;
        let err = mocks
            .verifier()
            .verify_exchange(&fixture_exchange())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("signature unavailable"), "{err}");
    }

    #[tokio::test]
    async fn records_attestation_failures() {
        let model = fixture_exchange().model;
        let cases: Vec<(Value, &str)> = {
            let mut wrong_nonce = report_json();
            wrong_nonce["model_attestations"][0]["request_nonce"] = "00".repeat(32).into();
            let mut ecdsa = report_json();
            ecdsa["model_attestations"][0]["signing_algo"] = "ecdsa".into();
            let mut bad_quote = report_json();
            bad_quote["model_attestations"][0]["intel_quote"] = "zz".into();
            let mut no_gpu = report_json();
            no_gpu["model_attestations"][0]
                .as_object_mut()
                .unwrap()
                .remove("nvidia_payload");
            let mut other_model = report_json();
            other_model["model_attestations"][0]["model_name"] = "other".into();
            vec![
                (wrong_nonce, "isn't for our nonce"),
                (ecdsa, "unexpected signing algorithm"),
                (bad_quote, "not hex"),
                (no_gpu, "no GPU evidence"),
                (other_model, "no attestation for"),
            ]
        };
        for (body, msg) in cases {
            let mocks = Mocks::start().await;
            mocks.report(body, 1).await;
            let verifier = mocks.verifier();
            let err = verifier.attest(&model).await.unwrap_err().to_string();
            assert!(err.contains(msg), "{err} should mention {msg}");
            let s = &verifier.status()[0];
            let failure = s.last_failure.as_ref().unwrap();
            assert!(failure.error.contains(msg));
            assert_eq!(failure.at, fetched_at());
        }
    }

    #[tokio::test]
    async fn reports_report_and_collateral_errors() {
        let model = fixture_exchange().model;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/attestation/report"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let err = Mocks { server }
            .verifier()
            .attest(&model)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("HTTP 429"), "{err}");

        // No PCCS mounted: collateral can't be fetched.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/attestation/report"))
            .respond_with(ResponseTemplate::new(200).set_body_json(report_json()))
            .mount(&server)
            .await;
        let err = Mocks { server }
            .verifier()
            .attest(&model)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("collateral:"), "{err}");
    }

    #[tokio::test]
    async fn background_verifier_records_without_blocking() {
        let mocks = Mocks::start().await;
        mocks.report(report_json(), 1).await;
        mocks.signature(signature_json()).await;
        let verifier = Arc::new(mocks.verifier());
        BackgroundVerifier(verifier.clone()).observe(fixture_exchange());
        for _ in 0..200 {
            if verifier.status().first().map(|s| s.verified_responses) == Some(1) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "background verification didn't finish: {:?}",
            verifier.status()
        );
    }

    #[test]
    fn default_config_uses_nras_and_both_pccs() {
        let c = VerifierConfig::new("https://cloud-api.near.ai/v1");
        assert_eq!(c.nras_url, gpu::NRAS_URL);
        assert_eq!(c.pccs_endpoints.len(), 2);
        assert_eq!(c.attestation_ttl, Duration::from_secs(3600));
        let v = ModelVerifier::new(c, "k").unwrap();
        assert_ne!((v.nonce)(), (v.nonce)());
        assert!(unix_secs((v.clock)()) > 1_700_000_000);
    }
}
