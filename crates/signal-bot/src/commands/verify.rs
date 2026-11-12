//! Verify command - provides cryptographic attestation proofs.

use crate::commands::CommandHandler;
use crate::error::AppResult;
use async_trait::async_trait;
use dstack_client::DstackClient;
use hex;
use near_agent::AgentStatus;
use near_ai_client::verify::{ModelStatus, ModelVerifier};
use sha2::{Digest, Sha256};
use signal_bot_core::REPLY_BREAK;
use signal_client::BotMessage;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

// Reply text, sent with Signal text styles (`**bold**`, `*italic*`, `` `mono` ``).
const HASHED_NOTE: &str = "*Over 64 bytes, so the quote holds its SHA-256 hash.*";
const NO_CHALLENGE_NOTE: &str = "*Send !verify <random text> so the quote proves it's fresh.*";
const HEX_CHECK: &str = "Check: `printf '%s' 'Sigstack: <your text>' | xxd -p`";
const HASH_CHECK: &str = "Check: `printf '%s' 'Sigstack: <your text>' | sha256sum`";
const HOW_TO_VERIFY: &str = "\
**How to verify**
1. **Signature:** copy the quote from the next message and paste it into \
https://proof.t16z.com. It checks that Intel signed it.
2. **Freshness:** the quote's report data must be the hex above, padded with zeros to \
64 bytes. Only code in this TEE could have quoted your challenge.
3. **Code:** the compose hash must be one the agent contract approved before this bot \
could register.";
const NEAR_AI_NOTE: &str = "\
**NEAR AI:** transcription and translation run on NEAR AI Cloud's GPU TEEs. Verify them \
at https://docs.near.ai/cloud/verification/";
const NEAR_AI_CHECKED_NOTE: &str = "\
**NEAR AI:** how this bot checks the models: \
https://docs.near.ai/cloud/verification/ and docs/model-verification.md in the source.";
const SUMMARY_TITLE: &str = "**🔐 Is this bot what it says it is?**";
const TEE_LINE: &str = "\
✅ **This bot** runs in an Intel TDX TEE on Phala. Its quote, in the next message, signs \
your challenge, so it's fresh.";
const NO_QUOTE_LINE: &str = "⚠️ **This bot** runs in a TEE, but couldn't produce a quote just now.";
const VOICE_UNSIGNED: &str = "NEAR AI doesn't sign individual transcripts yet.";

type StatusSource = Arc<dyn Fn() -> Vec<ModelStatus> + Send + Sync>;

/// The NEAR AI models this bot uses, and where their verification status comes from.
struct ModelChecks {
    status: StatusSource,
    chat_model: String,
    whisper_model: Option<String>,
}

/// Operator addresses for fund sweeping (from payment config).
#[derive(Debug, Clone, Default)]
pub struct OperatorAddresses {
    /// Base (EVM) operator address.
    pub base: Option<String>,
    /// NEAR operator account.
    pub near: Option<String>,
    /// Solana operator address.
    pub solana: Option<String>,
}

impl OperatorAddresses {
    /// Check if any operator address is configured.
    pub fn has_any(&self) -> bool {
        self.base.is_some() || self.near.is_some() || self.solana.is_some()
    }
}

pub struct VerifyHandler {
    dstack: Arc<DstackClient>,
    /// Optional operator addresses to display.
    operator_addresses: Option<OperatorAddresses>,
    /// Where the source lives (`BOT__GITHUB_REPO`).
    repo_url: Option<String>,
    /// Page listing approved builds (`BOT__VERIFIER_URL`).
    verifier_url: Option<String>,
    /// This boot's agent registration, when agent registration is on.
    agent: Option<Arc<AgentStatus>>,
    /// NEAR AI model checks, when response verification is on.
    models: Option<ModelChecks>,
}

impl VerifyHandler {
    pub fn new(dstack: Arc<DstackClient>) -> Self {
        Self {
            dstack,
            operator_addresses: None,
            repo_url: None,
            verifier_url: None,
            agent: None,
            models: None,
        }
    }

    /// Reports this boot's registration with the agent contract.
    pub fn with_agent(mut self, agent: Option<Arc<AgentStatus>>) -> Self {
        self.agent = agent;
        self
    }

    /// Reports what `verifier` has found about the chat model and, if voice is on,
    /// the Whisper model.
    pub fn with_model_verifier(
        self,
        verifier: Arc<ModelVerifier>,
        chat_model: impl Into<String>,
        whisper_model: Option<String>,
    ) -> Self {
        self.with_model_status(
            Arc::new(move || verifier.status()),
            chat_model,
            whisper_model,
        )
    }

    fn with_model_status(
        mut self,
        status: StatusSource,
        chat_model: impl Into<String>,
        whisper_model: Option<String>,
    ) -> Self {
        self.models = Some(ModelChecks {
            status,
            chat_model: chat_model.into(),
            whisper_model,
        });
        self
    }

    /// Links the reply's code check to the source repo and the verifier page. Blank
    /// values are ignored.
    pub fn with_links(mut self, repo_url: Option<String>, verifier_url: Option<String>) -> Self {
        let present = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
        self.repo_url = present(repo_url);
        self.verifier_url = present(verifier_url);
        self
    }

    /// Create handler with operator addresses to display.
    pub fn with_operator_addresses(
        dstack: Arc<DstackClient>,
        addresses: OperatorAddresses,
    ) -> Self {
        Self {
            dstack,
            operator_addresses: Some(addresses),
            repo_url: None,
            verifier_url: None,
            agent: None,
            models: None,
        }
    }

    pub(crate) fn prefixed_challenge(&self, raw: Option<String>) -> String {
        let user_part = raw.unwrap_or_else(|| "no-challenge-provided".into());
        format!("Sigstack: {user_part}")
    }

    /// Parse the challenge nonce from the message text.
    /// Expected format: "!verify <nonce>" or just "!verify"
    fn parse_challenge(&self, text: &str) -> Option<String> {
        let trimmed = text.trim();
        const PREFIX: &str = "!verify";
        if trimmed.len() < PREFIX.len() || !trimmed[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
            return None;
        }
        let rest = trimmed[PREFIX.len()..].trim();
        if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        }
    }

    async fn generate_attestation(&self, challenge: Option<&str>) -> AttestationResult {
        // Check if we're in a TEE
        if !self.dstack.is_in_tee().await {
            return AttestationResult {
                in_tee: false,
                error: Some("Not running in TEE environment".into()),
                ..Default::default()
            };
        }

        // Get app info
        let app_info = match self.dstack.get_app_info().await {
            Ok(info) => info,
            Err(e) => {
                return AttestationResult {
                    in_tee: true,
                    error: Some(format!("Failed to get app info: {}", e)),
                    ..Default::default()
                };
            }
        };

        // Prepare report_data - hash if challenge is too long
        let default_challenge = "no-challenge-provided";
        let challenge_str = challenge.unwrap_or(default_challenge);
        let challenge_bytes = challenge_str.as_bytes();

        let (report_data, was_hashed) = if challenge_bytes.len() > 64 {
            // Hash the challenge with SHA-256 (produces 32 bytes)
            let mut hasher = Sha256::new();
            hasher.update(challenge_bytes);
            let hash = hasher.finalize();
            (hash.to_vec(), true)
        } else {
            // Use challenge as-is
            (challenge_bytes.to_vec(), false)
        };

        let report_data_hex = hex::encode(&report_data);

        // Generate quote with report_data
        let quote = match self.dstack.get_quote(&report_data).await {
            Ok(q) => Some(q),
            Err(e) => {
                return AttestationResult {
                    in_tee: true,
                    compose_hash: app_info.compose_hash,
                    app_id: app_info.app_id,
                    error: Some(format!("Failed to generate quote: {}", e)),
                    report_data_hex: Some(report_data_hex),
                    was_hashed,
                    ..Default::default()
                };
            }
        };

        AttestationResult {
            in_tee: true,
            compose_hash: app_info.compose_hash,
            app_id: app_info.app_id,
            quote: quote.map(|q| q.quote),
            challenge: challenge.map(String::from),
            report_data_hex: Some(report_data_hex),
            was_hashed,
            error: None,
            operator_addresses: self.operator_addresses.clone(),
        }
    }

    fn format_response(&self, result: AttestationResult) -> String {
        self.format_response_at(result, SystemTime::now())
    }

    fn format_response_at(&self, result: AttestationResult, now: SystemTime) -> String {
        let mut lines = vec![];

        if !result.in_tee {
            lines.push("**⚠️ NOT RUNNING IN TEE**".into());
            lines.push(String::new());
            if let Some(err) = &result.error {
                lines.push(format!("Error: {}", err));
            }
            lines.push(String::new());
            lines.push("This bot is NOT running in a Trusted Execution Environment.".into());
            lines.push("Your messages may not be private.".into());
            return lines.join("\n");
        }

        // Plain-language summary first, then the details behind each line.
        lines.push(SUMMARY_TITLE.into());
        lines.push(
            if result.quote.is_some() {
                TEE_LINE
            } else {
                NO_QUOTE_LINE
            }
            .into(),
        );
        lines.extend(self.agent_summary(now));
        lines.extend(self.model_summary(now));
        lines.push(String::new());

        lines.push("**🔐 TEE attestation**".into());
        lines.push(String::new());

        match &result.challenge {
            Some(challenge) => {
                // A reply break in the echo would split the reply early. The quote binds the
                // original text, so this only changes what's shown.
                let shown = challenge.replace(REPLY_BREAK, "\u{fffd}");
                lines.push(format!("**Your challenge:** {shown}"));
                if result.was_hashed {
                    lines.push(HASHED_NOTE.into());
                }
            }
            None => {
                lines.push("**Your challenge:** none".into());
                lines.push(NO_CHALLENGE_NOTE.into());
            }
        }
        if let Some(report_data_hex) = &result.report_data_hex {
            lines.push(format!("**Report data:** `{report_data_hex}`"));
            let check = if result.was_hashed {
                HASH_CHECK
            } else {
                HEX_CHECK
            };
            lines.push(check.into());
        }
        lines.push(String::new());
        if let Some(hash) = &result.compose_hash {
            lines.push(format!("**Compose hash:** `{hash}`"));
        }
        if let Some(id) = &result.app_id {
            lines.push(format!("**App ID:** `{id}`"));
        }
        lines.extend(self.agent_details());
        lines.extend(self.model_details(now));

        if result.quote.is_some() {
            lines.push(String::new());
            lines.push(HOW_TO_VERIFY.into());
            if let Some(url) = &self.verifier_url {
                lines.push(format!("   Approved builds, traced to source: {url}"));
            }
            if let Some(repo) = &self.repo_url {
                lines.push(format!("   Source: {repo}"));
            }
        } else if let Some(err) = &result.error {
            lines.push(String::new());
            lines.push(format!("**Quote error:** {err}"));
        }

        if let Some(ref addresses) = result.operator_addresses {
            if addresses.has_any() {
                lines.push(String::new());
                lines.push("**Operator fund addresses:**".into());
                if let Some(ref addr) = addresses.base {
                    lines.push(format!("- Base: `{addr}`"));
                }
                if let Some(ref addr) = addresses.near {
                    lines.push(format!("- NEAR: `{addr}`"));
                }
                if let Some(ref addr) = addresses.solana {
                    lines.push(format!("- Solana: `{addr}`"));
                }
                lines.push(String::new());
                lines.push("*Deposits are automatically swept to these addresses.*".into());
                lines.push("*Confirm them against the compose hash.*".into());
            }
        }

        lines.push(String::new());
        lines.push(
            if self.models.is_some() {
                NEAR_AI_CHECKED_NOTE
            } else {
                NEAR_AI_NOTE
            }
            .into(),
        );

        // The quote goes out as its own message, one unbroken hex string, so it copies whole.
        let mut reply = lines.join("\n");
        if let Some(quote) = &result.quote {
            reply.push(REPLY_BREAK);
            reply.push_str(quote);
        }
        reply
    }
}

impl VerifyHandler {
    fn agent_summary(&self, now: SystemTime) -> Option<String> {
        let agent = self.agent.as_ref()?;
        let state = agent.state();
        let who = format!(
            "agent `{}` with {}",
            short(&agent.account_id),
            agent.contract_id
        );
        Some(match (&state.registration, &state.last_error) {
            (Some(reg), _) if reg.valid_until_ms > unix_ms(now) => format!(
                "✅ **Its code** is a build approved on NEAR: it registered as {who}. {}",
                explorer(&agent.contract_id, "txns", &reg.tx_hash)
            ),
            (Some(_), err) => format!(
                "⚠️ **Its code:** the registration of {who} has expired.{}",
                err.as_deref()
                    .map(|e| format!(" Last refresh failed: {}", brief(e)))
                    .unwrap_or_default()
            ),
            (None, Some(err)) => format!(
                "⏳ **Its code:** {who} isn't registered yet: {}",
                brief(err)
            ),
            (None, None) => format!("⏳ **Its code:** {who} is registering now."),
        })
    }

    fn agent_details(&self) -> Vec<String> {
        let Some(agent) = &self.agent else {
            return vec![];
        };
        let mut line = format!(
            "**Agent:** `{}` {}",
            agent.account_id,
            explorer(&agent.contract_id, "address", &agent.account_id)
        );
        if let Some(reg) = agent.state().registration {
            line.push_str(&format!(
                "\nRegistered in tx `{}`, valid until {}",
                reg.tx_hash,
                utc(reg.valid_until_ms)
            ));
        }
        vec![line]
    }

    fn model_summary(&self, now: SystemTime) -> Vec<String> {
        let Some(models) = &self.models else {
            return vec![];
        };
        let statuses = (models.status)();
        let find = |model: &str| statuses.iter().find(|s| s.model == model);

        let chat = find(&models.chat_model);
        let chat_line = match chat.filter(|s| s.instance.is_some()) {
            None => format!(
                "⏳ **Translations:** {} hasn't been attested yet.{}",
                model_name(&models.chat_model),
                failure_suffix(chat)
            ),
            Some(s) => {
                let replies = match (s.verified_responses, s.failed_responses) {
                    (0, 0) => " No replies checked yet.".to_string(),
                    (ok, failed) => format!(
                        " {ok} of {} replies checked so far carried its signature.",
                        ok + failed
                    ),
                };
                let (icon, failure) = if failing_now(s) {
                    ("⚠️", failure_suffix(Some(s)))
                } else {
                    ("✅", String::new())
                };
                format!(
                    "{icon} **Translations** come from {} in an attested NEAR AI GPU TEE \
                     ({}).{replies}{failure}",
                    model_name(&models.chat_model),
                    ago(attested_at(s), now),
                )
            }
        };
        let mut lines = vec![chat_line];
        if let Some(whisper) = &models.whisper_model {
            let w = find(whisper);
            lines.push(match w.filter(|s| s.instance.is_some()) {
                Some(s) => {
                    // Whisper has no signed responses, so a failed re-attestation is the
                    // only sign that the serving instance changed to one we can't verify.
                    let (icon, failure) = if failing_now(s) {
                        ("⚠️", failure_suffix(Some(s)))
                    } else {
                        ("✅", String::new())
                    };
                    format!(
                        "{icon} **Voice** is transcribed by {} in an attested NEAR AI GPU TEE \
                         ({}). {VOICE_UNSIGNED}{failure}",
                        model_name(whisper),
                        ago(attested_at(s), now)
                    )
                }
                None => format!(
                    "⏳ **Voice:** {} hasn't been attested yet.{}",
                    model_name(whisper),
                    failure_suffix(w)
                ),
            });
        }
        lines
    }

    fn model_details(&self, now: SystemTime) -> Vec<String> {
        let Some(models) = &self.models else {
            return vec![];
        };
        let statuses = (models.status)();
        let mut lines = vec![String::new(), "**NEAR AI models**".to_string()];
        let names = std::iter::once(&models.chat_model).chain(models.whisper_model.as_ref());
        for model in names {
            let status = statuses.iter().find(|s| &s.model == model);
            let Some((s, Some(instance))) = status.map(|s| (s, s.instance.as_ref())) else {
                lines.push(format!("- `{model}`: not attested yet"));
                continue;
            };
            let advisories = if instance.advisory_ids.is_empty() {
                String::new()
            } else {
                format!(" ({})", instance.advisory_ids.join(", "))
            };
            let mut line = format!(
                "- `{model}`: signing key `{}`, compose hash `{}`, Intel TCB {}{advisories}, \
                 {} GPUs passed NVIDIA's check, {}.",
                short(&instance.signing_key),
                short(&instance.compose_hash),
                instance.tcb_status,
                instance.gpus,
                ago(instance.verified_at, now)
            );
            if model == &models.chat_model {
                line.push_str(&format!(
                    " Replies: {} verified, {} failed.",
                    s.verified_responses, s.failed_responses
                ));
            }
            lines.push(line);
        }
        lines
    }
}

/// When the model's current instance was attested. Only called with one present.
fn attested_at(status: &ModelStatus) -> SystemTime {
    status
        .instance
        .as_ref()
        .map_or(UNIX_EPOCH, |i| i.verified_at)
}

/// Whether the model's latest event was a failure: a failed check newer than both
/// the last verified reply and the last attestation.
fn failing_now(status: &ModelStatus) -> bool {
    let Some(failure) = &status.last_failure else {
        return false;
    };
    let newest_ok = [
        status.last_verified_at,
        status.instance.as_ref().map(|i| i.verified_at),
    ]
    .into_iter()
    .flatten()
    .max();
    newest_ok.is_none_or(|ok| failure.at > ok)
}

fn failure_suffix(status: Option<&ModelStatus>) -> String {
    match status.and_then(|s| s.last_failure.as_ref()) {
        Some(failure) => format!(" Last check failed: {}", brief(&failure.error)),
        None => String::new(),
    }
}

/// Longest error text shown in a summary line.
const MAX_ERROR_CHARS: usize = 200;

/// An error's first line, cut to `MAX_ERROR_CHARS`, so a verbose RPC or attestation
/// error can't flood the reply.
fn brief(error: &str) -> String {
    let line = error.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= MAX_ERROR_CHARS {
        return line.to_string();
    }
    let cut: String = line.chars().take(MAX_ERROR_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// `deepseek-ai/DeepSeek-V4.1-Flash-1017` → `DeepSeek-V4.1-Flash-1017`.
fn model_name(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// Long hex as `01234567…cdef`.
fn short(s: &str) -> String {
    let n = s.chars().count();
    if n <= 16 {
        return s.to_string();
    }
    let head: String = s.chars().take(8).collect();
    let tail: String = s.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

fn ago(then: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(then).unwrap_or_default().as_secs();
    match secs {
        0..=59 => "checked just now".into(),
        60..=3599 => format!("checked {} min ago", secs / 60),
        _ => format!("checked {} h ago", secs / 3600),
    }
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}

fn utc(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| format!("{ms} ms"))
}

/// A NEAR explorer link on the contract's network.
fn explorer(contract_id: &str, kind: &str, id: &str) -> String {
    let host = if contract_id.ends_with(".testnet") {
        "testnet.nearblocks.io"
    } else {
        "nearblocks.io"
    };
    format!("https://{host}/{kind}/{id}")
}

#[derive(Default)]
struct AttestationResult {
    in_tee: bool,
    compose_hash: Option<String>,
    app_id: Option<String>,
    quote: Option<String>,
    challenge: Option<String>,
    report_data_hex: Option<String>,
    was_hashed: bool,
    error: Option<String>,
    operator_addresses: Option<OperatorAddresses>,
}

#[async_trait]
impl CommandHandler for VerifyHandler {
    fn trigger(&self) -> Option<&str> {
        Some("!verify")
    }

    fn styled_reply(&self) -> bool {
        true
    }

    fn matches(&self, message: &BotMessage) -> bool {
        let t = message.text.trim();
        const PREFIX: &str = "!verify";
        t.len() >= PREFIX.len() && t[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
    }

    async fn execute(&self, message: &BotMessage) -> AppResult<String> {
        let raw = self.parse_challenge(&message.text);
        let prefixed = self.prefixed_challenge(raw);

        info!(
            "Attestation requested by {} with challenge: {:?}",
            message.source,
            prefixed.chars().take(40).collect::<String>()
        );

        let result = self.generate_attestation(Some(&prefixed)).await;
        Ok(self.format_response(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_handler() -> VerifyHandler {
        VerifyHandler::new(Arc::new(DstackClient::new("/fake")))
    }

    #[test]
    fn prefixed_challenge_uses_bot_label() {
        let handler = create_test_handler();
        assert_eq!(
            handler.prefixed_challenge(Some("hello".into())),
            "Sigstack: hello"
        );
        assert_eq!(
            handler.prefixed_challenge(None),
            "Sigstack: no-challenge-provided"
        );
    }

    #[test]
    fn test_parse_challenge_with_nonce() {
        let handler = create_test_handler();

        assert_eq!(
            handler.parse_challenge("!verify abc123"),
            Some("abc123".into())
        );
        assert_eq!(
            handler.parse_challenge("!VERIFY abc123"),
            Some("abc123".into())
        );
        assert_eq!(
            handler.parse_challenge("!verify   my random challenge  "),
            Some("my random challenge".into())
        );
    }

    #[test]
    fn test_parse_challenge_without_nonce() {
        let handler = create_test_handler();

        assert_eq!(handler.parse_challenge("!verify"), None);
        assert_eq!(handler.parse_challenge("!verify   "), None);
    }

    #[test]
    fn test_format_response_not_in_tee() {
        let handler = create_test_handler();
        let result = AttestationResult {
            in_tee: false,
            error: Some("Not running in TEE".into()),
            ..Default::default()
        };

        let response = handler.format_response(result);
        assert!(response.contains("NOT RUNNING IN TEE"));
        assert!(response.contains("not be private"));
    }

    #[test]
    fn test_short_challenge_not_hashed() {
        // Test that challenges <= 64 bytes are NOT hashed
        let challenge = "short-nonce-123";
        assert!(challenge.len() <= 64);

        let expected_hex = hex::encode(challenge.as_bytes());

        let handler = create_test_handler();
        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            app_id: Some("app-456".into()),
            quote: Some("base64quote".into()),
            challenge: Some(challenge.into()),
            report_data_hex: Some(expected_hex.clone()),
            was_hashed: false,
            error: None,
            ..Default::default()
        };

        let response = handler.format_response(result);
        assert!(response.contains(&expected_hex));
        assert!(response.contains("xxd -p"));
        assert!(!response.contains("SHA-256"));
    }

    #[test]
    fn test_long_challenge_hashed() {
        // Test that challenges > 64 bytes ARE hashed
        let long_challenge = "a".repeat(65); // 65 bytes
        assert!(long_challenge.len() > 64);

        let mut hasher = Sha256::new();
        hasher.update(long_challenge.as_bytes());
        let expected_hash = hasher.finalize();
        let expected_hex = hex::encode(expected_hash);

        let handler = create_test_handler();
        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            app_id: Some("app-456".into()),
            quote: Some("base64quote".into()),
            challenge: Some(long_challenge.clone()),
            report_data_hex: Some(expected_hex.clone()),
            was_hashed: true,
            error: None,
            ..Default::default()
        };

        let response = handler.format_response(result);
        assert!(response.contains(&expected_hex));
        assert!(response.contains("SHA-256 hash"));
        assert!(response.contains("Over 64 bytes"));
        assert!(response.contains("sha256sum"));
    }

    #[test]
    fn test_format_response_with_challenge() {
        let handler = create_test_handler();
        let challenge = "my-nonce";
        let report_data_hex = hex::encode(challenge.as_bytes());

        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            app_id: Some("app-456".into()),
            quote: Some("base64quote".into()),
            challenge: Some(challenge.into()),
            report_data_hex: Some(report_data_hex.clone()),
            was_hashed: false,
            error: None,
            ..Default::default()
        };

        let response = handler.format_response(result);
        assert!(response.contains("TEE attestation"));
        assert!(response.contains("my-nonce"));
        assert!(response.contains("abc123"));
        assert!(response.contains("base64quote"));
        assert!(response.contains(&report_data_hex));
        assert!(response.contains("Report data"));
    }

    #[test]
    fn test_format_response_without_challenge() {
        let handler = create_test_handler();
        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            quote: Some("base64quote".into()),
            report_data_hex: Some(hex::encode("no-challenge-provided".as_bytes())),
            was_hashed: false,
            ..Default::default()
        };

        let response = handler.format_response(result);
        assert!(response.contains("**Your challenge:** none"));
        assert!(response.contains("!verify <random text>"));
    }

    #[test]
    fn test_verification_instructions_present() {
        let handler = create_test_handler();
        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            app_id: Some("app-456".into()),
            quote: Some("base64quote".into()),
            challenge: Some("test".into()),
            report_data_hex: Some(hex::encode("test".as_bytes())),
            was_hashed: false,
            error: None,
            ..Default::default()
        };

        let response = handler.format_response(result);
        assert!(response.contains("How to verify"));
        assert!(response.contains("**Signature:**"));
        assert!(response.contains("**Freshness:**"));
        assert!(response.contains("**Code:**"));
        assert!(response.contains("https://proof.t16z.com"));
        assert!(response.contains("xxd -p"));
        // Nothing from the upstream bot, and only Signal's styled-text syntax.
        assert!(!response.contains("zmanian"));
        assert!(!response.contains("phala.yaml"));
        assert!(!response.contains("```"));
        assert!(!response.contains("\n_"));
        // No links unless configured.
        assert!(!response.contains("Source:"));
    }

    // The quote goes out alone, after the explanation, as one unbroken string.
    #[test]
    fn quote_is_sent_as_its_own_message() {
        let handler = create_test_handler();
        let quote = "04000200".repeat(40);
        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            quote: Some(quote.clone()),
            challenge: Some("c".into()),
            report_data_hex: Some(hex::encode("c")),
            ..Default::default()
        };

        let response = handler.format_response(result);
        let parts: Vec<&str> = response.split(REPLY_BREAK).collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].contains("next message"));
        assert!(!parts[0].contains(&quote));
        assert_eq!(parts[1], quote);
        assert!(handler.styled_reply());
    }

    // A reply break typed into the challenge can't split the reply: the quote is still
    // the one and only second message.
    #[test]
    fn challenge_cannot_split_the_reply() {
        let handler = create_test_handler();
        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            quote: Some("0400".into()),
            challenge: Some(format!("Sigstack: a{REPLY_BREAK}b")),
            report_data_hex: Some("00".into()),
            ..Default::default()
        };

        let response = handler.format_response(result);
        let parts: Vec<&str> = response.split(REPLY_BREAK).collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].contains("Sigstack: a\u{fffd}b"));
        assert_eq!(parts[1], "0400");
    }

    // Without a quote there's nothing to send separately.
    #[test]
    fn quote_error_is_a_single_message() {
        let handler = create_test_handler();
        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            error: Some("socket closed".into()),
            report_data_hex: Some(hex::encode("c")),
            ..Default::default()
        };

        let response = handler.format_response(result);
        assert!(!response.contains(REPLY_BREAK));
        assert!(response.contains("**Quote error:** socket closed"));
    }

    // The code check links the verifier page and source when configured; blanks are ignored.
    #[test]
    fn code_check_links_verifier_and_source() {
        let handler = create_test_handler().with_links(
            Some("https://github.com/example/repo".into()),
            Some("https://example.github.io/repo/".into()),
        );
        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            quote: Some("00".into()),
            ..Default::default()
        };
        let response = handler.format_response(result);
        assert!(response.contains("traced to source: https://example.github.io/repo/"));
        assert!(response.contains("Source: https://github.com/example/repo"));

        let blank = create_test_handler().with_links(Some(" ".into()), None);
        assert!(blank.repo_url.is_none());
        assert!(blank.verifier_url.is_none());
    }

    #[test]
    fn test_operator_addresses_displayed() {
        let handler = VerifyHandler::with_operator_addresses(
            Arc::new(DstackClient::new("/fake")),
            OperatorAddresses {
                base: Some("0xABC123".into()),
                near: Some("operator.near".into()),
                solana: None,
            },
        );

        let result = AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            quote: Some("base64quote".into()),
            operator_addresses: Some(OperatorAddresses {
                base: Some("0xABC123".into()),
                near: Some("operator.near".into()),
                solana: None,
            }),
            ..Default::default()
        };

        let response = handler.format_response(result);
        assert!(response.contains("Operator fund addresses"));
        assert!(response.contains("0xABC123"));
        assert!(response.contains("operator.near"));
        assert!(response.contains("swept to these addresses"));
    }

    #[tokio::test]
    async fn execute_reports_not_in_tee() {
        let handler = create_test_handler();
        let msg = BotMessage {
            source: "+15550002222".into(),
            source_number: Some("+15550002222".into()),
            source_name: None,
            text: "!verify challenge-1".into(),
            timestamp: 1,
            message_timestamp: 1,
            is_group: false,
            group_id: None,
            group_name: None,
            receiving_account: "+15550001111".into(),
            attachments: vec![],
            quote: None,
        };
        assert!(handler.matches(&msg));
        let out = handler.execute(&msg).await.unwrap();
        assert!(out.contains("NOT RUNNING IN TEE") || out.contains("Not running in TEE"));
    }

    #[test]
    fn operator_addresses_has_any() {
        assert!(!OperatorAddresses::default().has_any());
        assert!(OperatorAddresses {
            base: Some("0x1".into()),
            near: None,
            solana: None,
        }
        .has_any());
    }

    // ---- Both layers: agent registration and NEAR AI model checks ----

    use near_agent::{AgentError, Registration};
    use near_ai_client::verify::{AttestedInstance, Failure, VerifierConfig};

    const CHAT: &str = "deepseek-ai/DeepSeek-V4.1-Flash-1017";
    const WHISPER: &str = "openai/whisper-large-v3";
    const AGENT: &str = "60b051d8aaa7c107d720d29fc016cf837ca915d5fe11637158bdc59c7912a661";

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn quoted() -> AttestationResult {
        AttestationResult {
            in_tee: true,
            compose_hash: Some("abc123".into()),
            quote: Some("0400".into()),
            challenge: Some("c".into()),
            report_data_hex: Some("63".into()),
            ..Default::default()
        }
    }

    fn summary(response: &str) -> &str {
        response.split("**🔐 TEE attestation**").next().unwrap()
    }

    fn instance(model: &str, verified_at: SystemTime) -> AttestedInstance {
        AttestedInstance {
            model: model.into(),
            signing_key: "236f5a6b91dd47dc3b7b36e43d4eb531a042ffb8f8fbf3a1555b78cef4fdda4c".into(),
            compose_hash: "c53e254e8b70aa14b45ba3043730a0736ca2afb1df7fa3db9c61578845e470fe".into(),
            tcb_status: "UpToDate".into(),
            advisory_ids: vec![],
            gpus: 8,
            verified_at,
        }
    }

    fn with_statuses(statuses: Vec<ModelStatus>, whisper: bool) -> VerifyHandler {
        create_test_handler().with_model_status(
            Arc::new(move || statuses.clone()),
            CHAT,
            whisper.then(|| WHISPER.to_string()),
        )
    }

    #[test]
    fn summary_comes_first_in_plain_language() {
        let response = create_test_handler().format_response(quoted());
        assert!(response.starts_with(SUMMARY_TITLE));
        assert!(summary(&response).contains("✅ **This bot** runs in an Intel TDX TEE"));
        // Without agent or model checks, the old NEAR AI pointer stays.
        assert!(response.contains(NEAR_AI_NOTE));

        let mut no_quote = quoted();
        no_quote.quote = None;
        no_quote.error = Some("socket closed".into());
        let response = create_test_handler().format_response(no_quote);
        assert!(summary(&response).contains("couldn't produce a quote"));
    }

    #[test]
    fn reports_the_agent_registration_with_explorer_links() {
        let now = at(1_800_000_000);
        let status = Arc::new(AgentStatus::new(AGENT, "agent.sigstack.testnet"));
        let handler = create_test_handler().with_agent(Some(status.clone()));

        let pending = handler.format_response_at(quoted(), now);
        assert!(summary(&pending).contains("⏳ **Its code:** agent `60b051d8…a661`"));
        assert!(summary(&pending).contains("registering now"));
        assert!(pending.contains(&format!(
            "**Agent:** `{AGENT}` https://testnet.nearblocks.io/address/{AGENT}"
        )));

        status.record(&Err(AgentError::TxFailed(
            "wrong expected_measurements".into(),
        )));
        let failing = handler.format_response_at(quoted(), now);
        assert!(summary(&failing).contains("isn't registered yet: transaction failed"));

        status.record(&Ok(Registration {
            account_id: AGENT.into(),
            tx_hash: "8F1gnMdfnXivCfwzp9t9g1guLE66bt8bYxAJ9odx9tCD".into(),
            valid_until_ms: 1_800_600_000_000,
        }));
        let registered = handler.format_response_at(quoted(), now);
        assert!(summary(&registered).contains(
            "✅ **Its code** is a build approved on NEAR: it registered as agent \
             `60b051d8…a661` with agent.sigstack.testnet. \
             https://testnet.nearblocks.io/txns/8F1gnMdfnXivCfwzp9t9g1guLE66bt8bYxAJ9odx9tCD"
        ));
        assert!(registered.contains(
            "Registered in tx `8F1gnMdfnXivCfwzp9t9g1guLE66bt8bYxAJ9odx9tCD`, valid until \
             2027-01-22 06:40 UTC"
        ));

        let expired = handler.format_response_at(quoted(), at(1_900_000_000));
        assert!(summary(&expired).contains("agent.sigstack.testnet has expired."));
        assert!(!summary(&expired).contains("Last refresh failed"));

        // Expired with the refresh still failing says why.
        status.record(&Err(AgentError::Rpc("timeout\nbacktrace…".into())));
        let stuck = handler.format_response_at(quoted(), at(1_900_000_000));
        assert!(summary(&stuck).contains("has expired. Last refresh failed: NEAR RPC: timeout\n"));
        assert!(!stuck.contains("backtrace"));
    }

    #[test]
    fn reports_both_models_when_verified() {
        let now = at(1_800_000_000);
        let chat = ModelStatus {
            model: CHAT.into(),
            instance: Some(instance(CHAT, at(1_800_000_000 - 720))),
            verified_responses: 41,
            failed_responses: 0,
            last_verified_at: Some(at(1_799_999_990)),
            last_failure: None,
        };
        let mut voice_instance = instance(WHISPER, at(1_800_000_000 - 7_300));
        voice_instance.advisory_ids = vec!["INTEL-SA-00837".into()];
        let voice = ModelStatus {
            model: WHISPER.into(),
            instance: Some(voice_instance),
            ..Default::default()
        };
        let response = with_statuses(vec![chat, voice], true).format_response_at(quoted(), now);

        let top = summary(&response);
        assert!(top.contains(
            "✅ **Translations** come from DeepSeek-V4.1-Flash-1017 in an attested NEAR AI GPU \
             TEE (checked 12 min ago). 41 of 41 replies checked so far carried its signature."
        ));
        assert!(top.contains(
            "✅ **Voice** is transcribed by whisper-large-v3 in an attested NEAR AI GPU TEE \
             (checked 2 h ago). NEAR AI doesn't sign individual transcripts yet."
        ));
        assert!(response.contains("**NEAR AI models**"));
        assert!(response.contains(
            "- `deepseek-ai/DeepSeek-V4.1-Flash-1017`: signing key `236f5a6b…da4c`, compose hash \
             `c53e254e…70fe`, Intel TCB UpToDate, 8 GPUs passed NVIDIA's check, checked 12 min \
             ago. Replies: 41 verified, 0 failed."
        ));
        assert!(response.contains("Intel TCB UpToDate (INTEL-SA-00837)"));
        assert!(response.contains(NEAR_AI_CHECKED_NOTE));
        // The quote is still the one separate message.
        assert_eq!(response.split(REPLY_BREAK).count(), 2);
    }

    #[test]
    fn flags_only_a_failure_that_is_still_current() {
        let now = at(1_800_000_000);
        let failure = |secs| Failure {
            at: at(secs),
            error: "response signature: signed by the gateway, not the model's TEE".into(),
        };
        let status = |failure_at| ModelStatus {
            model: CHAT.into(),
            instance: Some(instance(CHAT, at(1_799_999_000))),
            verified_responses: 9,
            failed_responses: 1,
            last_verified_at: Some(at(1_799_999_500)),
            last_failure: Some(failure(failure_at)),
        };

        let current =
            with_statuses(vec![status(1_799_999_900)], false).format_response_at(quoted(), now);
        assert!(summary(&current).contains("⚠️ **Translations**"));
        assert!(summary(&current).contains("9 of 10 replies"));
        assert!(summary(&current).contains("Last check failed: response signature: signed by"));

        let cleared =
            with_statuses(vec![status(1_799_999_100)], false).format_response_at(quoted(), now);
        assert!(summary(&cleared).contains("✅ **Translations**"));
        assert!(!summary(&cleared).contains("Last check failed"));
        // No Whisper line when voice is off.
        assert!(!cleared.contains("**Voice**"));
    }

    #[test]
    fn flags_voice_when_reattestation_is_failing() {
        let now = at(1_800_000_000);
        let voice = |failure_at| ModelStatus {
            model: WHISPER.into(),
            instance: Some(instance(WHISPER, at(1_799_990_000))),
            last_failure: Some(Failure {
                at: at(failure_at),
                error: "attestation: quote binding: report data doesn't bind this signing key"
                    .into(),
            }),
            ..Default::default()
        };

        let current =
            with_statuses(vec![voice(1_799_999_000)], true).format_response_at(quoted(), now);
        let line = summary(&current)
            .lines()
            .find(|l| l.contains("**Voice**"))
            .unwrap();
        assert!(line.starts_with("⚠️ **Voice**"), "{line}");
        assert!(line.contains("Last check failed: attestation: quote binding"));

        let cleared =
            with_statuses(vec![voice(1_799_980_000)], true).format_response_at(quoted(), now);
        assert!(summary(&cleared).contains("✅ **Voice**"));
        assert!(!summary(&cleared).contains("Last check failed"));
    }

    #[test]
    fn reports_models_not_attested_yet() {
        let now = at(1_800_000_000);
        let chat = ModelStatus {
            model: CHAT.into(),
            last_failure: Some(Failure {
                at: at(1_799_999_000),
                error: "attestation: GPU: NRAS: HTTP 503".into(),
            }),
            ..Default::default()
        };
        let response = with_statuses(vec![chat], true).format_response_at(quoted(), now);
        let top = summary(&response);
        assert!(top.contains(
            "⏳ **Translations:** DeepSeek-V4.1-Flash-1017 hasn't been attested yet. Last check \
             failed: attestation: GPU: NRAS: HTTP 503"
        ));
        assert!(top.contains("⏳ **Voice:** whisper-large-v3 hasn't been attested yet."));
        assert!(response.contains("- `openai/whisper-large-v3`: not attested yet"));

        // Attested, but no replies yet.
        let fresh = ModelStatus {
            model: CHAT.into(),
            instance: Some(instance(CHAT, now)),
            ..Default::default()
        };
        let response = with_statuses(vec![fresh], false).format_response_at(quoted(), now);
        assert!(summary(&response).contains("(checked just now). No replies checked yet."));
    }

    #[tokio::test]
    async fn reads_a_live_verifier() {
        let verifier =
            Arc::new(ModelVerifier::new(VerifierConfig::new("http://127.0.0.1:9"), "key").unwrap());
        let handler = create_test_handler().with_model_verifier(verifier, CHAT, None);
        let response = handler.format_response(quoted());
        assert!(summary(&response).contains("hasn't been attested yet."));
    }

    #[test]
    fn errors_are_brief() {
        assert_eq!(brief("short"), "short");
        assert_eq!(brief("first line\nsecond"), "first line");
        assert_eq!(brief(""), "");
        let long = "x".repeat(MAX_ERROR_CHARS + 50);
        let cut = brief(&long);
        assert_eq!(cut.chars().count(), MAX_ERROR_CHARS + 1);
        assert!(cut.ends_with('…'));

        let status = ModelStatus {
            model: CHAT.into(),
            last_failure: Some(Failure {
                at: at(1),
                error: long,
            }),
            ..Default::default()
        };
        let response = with_statuses(vec![status], false).format_response_at(quoted(), at(2));
        assert!(response.contains(&format!("Last check failed: {cut}")));
        assert!(!response.contains(&"x".repeat(MAX_ERROR_CHARS + 1)));
    }

    #[test]
    fn formatting_helpers() {
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(AGENT), "60b051d8…a661");
        assert_eq!(model_name("openai/whisper-large-v3"), "whisper-large-v3");
        assert_eq!(model_name("plain"), "plain");
        assert_eq!(ago(at(0), at(59)), "checked just now");
        assert_eq!(ago(at(0), at(3_599)), "checked 59 min ago");
        assert_eq!(ago(at(10), at(0)), "checked just now");
        assert_eq!(ago(at(0), at(7_200)), "checked 2 h ago");
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(i64::MAX as u64), format!("{} ms", i64::MAX));
        assert_eq!(
            explorer("agent.near", "txns", "abc"),
            "https://nearblocks.io/txns/abc"
        );
        let mut empty = ModelStatus::default();
        assert_eq!(attested_at(&empty), UNIX_EPOCH);
        assert!(!failing_now(&empty));
        empty.last_failure = Some(Failure {
            at: at(1),
            error: "e".into(),
        });
        assert!(failing_now(&empty));
    }
}
