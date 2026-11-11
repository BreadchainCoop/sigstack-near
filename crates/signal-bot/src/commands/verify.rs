//! Verify command - provides cryptographic attestation proofs.

use crate::commands::CommandHandler;
use crate::error::AppResult;
use async_trait::async_trait;
use dstack_client::DstackClient;
use hex;
use sha2::{Digest, Sha256};
use signal_bot_core::REPLY_BREAK;
use signal_client::BotMessage;
use std::sync::Arc;
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
}

impl VerifyHandler {
    pub fn new(dstack: Arc<DstackClient>) -> Self {
        Self {
            dstack,
            operator_addresses: None,
            repo_url: None,
            verifier_url: None,
        }
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
        lines.push(NEAR_AI_NOTE.into());

        // The quote goes out as its own message, one unbroken hex string, so it copies whole.
        let mut reply = lines.join("\n");
        if let Some(quote) = &result.quote {
            reply.push(REPLY_BREAK);
            reply.push_str(quote);
        }
        reply
    }
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
}
