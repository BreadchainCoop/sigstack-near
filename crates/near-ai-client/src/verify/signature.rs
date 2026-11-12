//! NEAR AI's per-response signature (`GET /v1/signature/{chat_id}`).

use super::VerifyError;
use crate::ChatExchange;
use ring::signature::{UnparsedPublicKey, ED25519};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseSignature {
    pub text: String,
    pub signature: String,
    /// For ed25519, the hex public key.
    pub signing_address: String,
    pub signing_algo: String,
    /// `provider_tee` when the model's own TEE signed; `gateway` when only NEAR AI's
    /// gateway did.
    #[serde(default)]
    pub signature_kind: Option<String>,
}

/// Checks that the model's TEE signed exactly this request and response:
/// `text` is `{model}:{sha256(request)}:{sha256(response)}`, signed with ed25519.
pub fn check_signature(
    sig: &ResponseSignature,
    exchange: &ChatExchange,
) -> Result<(), VerifyError> {
    let kind = sig.signature_kind.as_deref().unwrap_or("unknown");
    if kind != "provider_tee" {
        return Err(VerifyError::Signature(format!(
            "signed by the {kind}, not the model's TEE"
        )));
    }
    if sig.signing_algo != "ed25519" {
        return Err(VerifyError::Signature(format!(
            "unexpected algorithm {}",
            sig.signing_algo
        )));
    }
    let expected = format!(
        "{}:{}:{}",
        exchange.model,
        hex::encode(Sha256::digest(&exchange.request)),
        hex::encode(Sha256::digest(&exchange.response))
    );
    if sig.text != expected {
        return Err(VerifyError::Signature(
            "the signed text doesn't match this request and response".into(),
        ));
    }
    let key = hex::decode(&sig.signing_address)
        .map_err(|_| VerifyError::Signature("signing key is not hex".into()))?;
    let signature = hex::decode(&sig.signature)
        .map_err(|_| VerifyError::Signature("signature is not hex".into()))?;
    UnparsedPublicKey::new(&ED25519, &key)
        .verify(sig.text.as_bytes(), &signature)
        .map_err(|_| VerifyError::Signature("invalid ed25519 signature".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify::tests::{fixture_exchange, fixture_signature};

    #[test]
    fn accepts_the_real_signature() {
        check_signature(&fixture_signature(), &fixture_exchange()).unwrap();
    }

    #[test]
    fn rejects_mismatches() {
        let exchange = fixture_exchange();
        let expect = |sig: ResponseSignature, ex: &ChatExchange, msg: &str| {
            let err = check_signature(&sig, ex).unwrap_err().to_string();
            assert!(err.contains(msg), "{err} should mention {msg}");
        };

        let mut gateway = fixture_signature();
        gateway.signature_kind = Some("gateway".into());
        expect(gateway, &exchange, "signed by the gateway");
        let mut unknown = fixture_signature();
        unknown.signature_kind = None;
        expect(unknown, &exchange, "signed by the unknown");
        let mut ecdsa = fixture_signature();
        ecdsa.signing_algo = "ecdsa".into();
        expect(ecdsa, &exchange, "unexpected algorithm");

        let mut other_response = exchange.clone();
        other_response.response.push(b' ');
        expect(fixture_signature(), &other_response, "doesn't match");
        let mut other_model = exchange.clone();
        other_model.model = "deepseek-ai/DeepSeek-V4-Flash".into();
        expect(fixture_signature(), &other_model, "doesn't match");

        let mut bad_key = fixture_signature();
        bad_key.signing_address = "zz".into();
        expect(bad_key, &exchange, "key is not hex");
        let mut bad_sig = fixture_signature();
        bad_sig.signature = "zz".into();
        expect(bad_sig, &exchange, "signature is not hex");
        let mut forged = fixture_signature();
        forged.signature = "00".repeat(64);
        expect(forged, &exchange, "invalid ed25519");
    }
}
