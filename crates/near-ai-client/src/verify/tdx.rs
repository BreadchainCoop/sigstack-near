//! The model instance's TDX quote: Intel's signature chain (via dcap-qvl), then what
//! the quote binds: the signing key and our nonce in report data, and the measured
//! app definition through the event log.

use super::report::{Event, ModelAttestation};
use super::VerifyError;
use dcap_qvl::QuoteCollateralV3;
use serde_json::Value;
use sha2::{Digest, Sha256, Sha384};

/// What a verified quote says about the TD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteFacts {
    /// Intel's TCB status for the platform, e.g. `UpToDate`.
    pub status: String,
    pub advisory_ids: Vec<String>,
    pub td_attributes: [u8; 8],
    pub mr_config_id: [u8; 48],
    pub rt_mr3: [u8; 48],
    pub report_data: [u8; 64],
}

/// Collateral in the JSON shape `near_agent::collateral::fetch_collateral` returns
/// (binary fields hex-encoded), as dcap-qvl's struct.
pub fn collateral_from_json(value: &Value) -> Result<QuoteCollateralV3, VerifyError> {
    let text = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| VerifyError::Collateral(format!("missing {key}")))
    };
    let bytes = |key: &str| {
        hex::decode(text(key)?).map_err(|_| VerifyError::Collateral(format!("{key} is not hex")))
    };
    Ok(QuoteCollateralV3 {
        pck_crl_issuer_chain: text("pck_crl_issuer_chain")?,
        root_ca_crl: bytes("root_ca_crl")?,
        pck_crl: bytes("pck_crl")?,
        tcb_info_issuer_chain: text("tcb_info_issuer_chain")?,
        tcb_info: text("tcb_info")?,
        tcb_info_signature: bytes("tcb_info_signature")?,
        qe_identity_issuer_chain: text("qe_identity_issuer_chain")?,
        qe_identity: text("qe_identity")?,
        qe_identity_signature: bytes("qe_identity_signature")?,
        // dcap-qvl then reads the PCK chain from the quote itself.
        pck_certificate_chain: None,
    })
}

/// Verifies the quote against Intel's root of trust at `now_secs`. dcap-qvl rejects
/// revoked or otherwise invalid TCB levels and debug TDs.
pub fn verify_quote(
    quote: &[u8],
    collateral: &QuoteCollateralV3,
    now_secs: u64,
) -> Result<QuoteFacts, VerifyError> {
    let verified = dcap_qvl::verify::ring::verify(quote, collateral, now_secs)
        .map_err(|e| VerifyError::Quote(format!("{e:#}")))?;
    let td = verified
        .report
        .as_td10()
        .ok_or_else(|| VerifyError::Quote("not a TDX quote".into()))?;
    Ok(QuoteFacts {
        status: verified.status,
        advisory_ids: verified.advisory_ids,
        td_attributes: td.td_attributes,
        mr_config_id: td.mr_config_id,
        rt_mr3: td.rt_mr3,
        report_data: td.report_data,
    })
}

/// Checks that the verified quote binds this attestation: report data is the signing
/// key followed by our nonce, the TD isn't in debug mode, and the event log replays to
/// RTMR3 and records the hash of the reported app definition. Returns that hash.
pub fn check_bindings(
    attestation: &ModelAttestation,
    facts: &QuoteFacts,
    nonce: &[u8; 32],
) -> Result<String, VerifyError> {
    let key = hex::decode(attestation.signing_address.trim_start_matches("0x"))
        .map_err(|_| VerifyError::Binding("signing address is not hex".into()))?;
    if key.len() > 32 {
        return Err(VerifyError::Binding("signing address is too long".into()));
    }
    let mut expected = [0u8; 64];
    expected[..key.len()].copy_from_slice(&key);
    expected[32..].copy_from_slice(nonce);
    if facts.report_data != expected {
        return Err(VerifyError::Binding(
            "report data doesn't bind this signing key and our nonce".into(),
        ));
    }
    if facts.td_attributes[0] & 1 != 0 {
        return Err(VerifyError::Binding("the TD is in debug mode".into()));
    }

    let compose_hash = hex::encode(Sha256::digest(attestation.app_compose()?.as_bytes()));
    if !attestation
        .info
        .compose_hash
        .eq_ignore_ascii_case(&compose_hash)
    {
        return Err(VerifyError::Binding(
            "compose_hash isn't the hash of app_compose".into(),
        ));
    }
    let events = attestation.events()?;
    if replay_rtmr3(&events)? != facts.rt_mr3 {
        return Err(VerifyError::Binding(
            "the event log doesn't replay to the quote's RTMR3".into(),
        ));
    }
    let measured = events
        .iter()
        .find(|e| e.imr == 3 && e.event == "compose-hash")
        .map(|e| e.event_payload.as_str());
    if measured != Some(compose_hash.as_str()) {
        return Err(VerifyError::Binding(
            "the event log doesn't measure this app definition".into(),
        ));
    }
    if facts.mr_config_id != [0; 48] {
        let mut config_id = [0u8; 48];
        config_id[0] = 1;
        config_id[1..33].copy_from_slice(&Sha256::digest(attestation.app_compose()?.as_bytes()));
        if facts.mr_config_id != config_id {
            return Err(VerifyError::Binding(
                "MRCONFIGID doesn't match the app definition".into(),
            ));
        }
    }
    Ok(compose_hash)
}

/// RTMR3 as dstack extends it: SHA-384 over the previous value and each IMR 3 event's
/// digest, where a digest is SHA-384(event type LE || ":" || name || ":" || payload).
fn replay_rtmr3(events: &[Event]) -> Result<[u8; 48], VerifyError> {
    let mut rtmr = [0u8; 48];
    for event in events.iter().filter(|e| e.imr == 3) {
        let payload = hex::decode(&event.event_payload).map_err(|_| {
            VerifyError::Binding(format!("event {} payload is not hex", event.event))
        })?;
        let mut hasher = Sha384::new();
        hasher.update(event.event_type.to_le_bytes());
        hasher.update(b":");
        hasher.update(event.event.as_bytes());
        hasher.update(b":");
        hasher.update(&payload);
        let digest = hasher.finalize();
        if !event.digest.is_empty() && !event.digest.eq_ignore_ascii_case(&hex::encode(digest)) {
            return Err(VerifyError::Binding(format!(
                "event {} has the wrong digest",
                event.event
            )));
        }
        let mut hasher = Sha384::new();
        hasher.update(rtmr);
        hasher.update(digest);
        rtmr.copy_from_slice(&hasher.finalize());
    }
    Ok(rtmr)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::verify::tests::{fixture, fixture_attestation, fixture_nonce};

    /// The fixture's quote fields, read at their v4 offsets (no signature check).
    pub(crate) fn raw_facts(quote: &[u8]) -> QuoteFacts {
        let body = &quote[48..];
        QuoteFacts {
            status: "UpToDate".into(),
            advisory_ids: vec![],
            td_attributes: body[120..128].try_into().unwrap(),
            mr_config_id: body[184..232].try_into().unwrap(),
            rt_mr3: body[472..520].try_into().unwrap(),
            report_data: body[520..584].try_into().unwrap(),
        }
    }

    fn quote() -> Vec<u8> {
        hex::decode(&fixture_attestation().intel_quote).unwrap()
    }

    #[test]
    fn verifies_a_real_near_ai_quote() {
        let f = fixture();
        let collateral = collateral_from_json(&f["collateral"]).unwrap();
        let now = f["fetched_at_secs"].as_u64().unwrap();
        let facts = verify_quote(&quote(), &collateral, now).unwrap();
        assert_eq!(facts.report_data, raw_facts(&quote()).report_data);
        assert_eq!(facts.rt_mr3, raw_facts(&quote()).rt_mr3);
        assert!(!facts.status.is_empty());

        // A tampered quote fails Intel's signature check.
        let mut tampered = quote();
        tampered[48 + 520] ^= 1;
        assert!(matches!(
            verify_quote(&tampered, &collateral, now),
            Err(VerifyError::Quote(_))
        ));
        // So does verifying long after the collateral expired.
        assert!(verify_quote(&quote(), &collateral, now + 400 * 86_400).is_err());
    }

    #[test]
    fn rejects_malformed_collateral() {
        let mut c = fixture()["collateral"].clone();
        c["pck_crl"] = "zz".into();
        let err = collateral_from_json(&c).unwrap_err().to_string();
        assert!(err.contains("pck_crl is not hex"), "{err}");
        c.as_object_mut().unwrap().remove("tcb_info");
        assert!(collateral_from_json(&c).is_err());
    }

    #[test]
    fn binds_the_signing_key_nonce_and_app_definition() {
        let a = fixture_attestation();
        let facts = raw_facts(&quote());
        let hash = check_bindings(&a, &facts, &fixture_nonce()).unwrap();
        assert_eq!(hash, a.info.compose_hash);
    }

    #[test]
    fn rejects_broken_bindings() {
        let a = fixture_attestation();
        let facts = raw_facts(&quote());
        let nonce = fixture_nonce();
        let expect = |a: &ModelAttestation, f: &QuoteFacts, n: &[u8; 32], msg: &str| {
            let err = check_bindings(a, f, n).unwrap_err().to_string();
            assert!(err.contains(msg), "{err} should mention {msg}");
        };

        expect(&a, &facts, &[0; 32], "report data");
        let mut other_key = a.clone();
        other_key.signing_address = "00".repeat(32);
        expect(&other_key, &facts, &nonce, "report data");
        let mut not_hex = a.clone();
        not_hex.signing_address = "xyz".into();
        expect(&not_hex, &facts, &nonce, "not hex");
        let mut too_long = a.clone();
        too_long.signing_address = "00".repeat(33);
        expect(&too_long, &facts, &nonce, "too long");

        let mut debug = facts.clone();
        debug.td_attributes[0] |= 1;
        expect(&a, &debug, &nonce, "debug");

        let mut wrong_hash = a.clone();
        wrong_hash.info.compose_hash = "00".repeat(32);
        expect(&wrong_hash, &facts, &nonce, "compose_hash");

        let mut rtmr = facts.clone();
        rtmr.rt_mr3[0] ^= 1;
        expect(&a, &rtmr, &nonce, "RTMR3");

        let mut config = facts.clone();
        config.mr_config_id[5] ^= 1;
        expect(&a, &config, &nonce, "MRCONFIGID");
        // An all-zero MRCONFIGID (older dstack) isn't checked.
        let mut unset = facts.clone();
        unset.mr_config_id = [0; 48];
        assert!(check_bindings(&a, &unset, &nonce).is_ok());
    }

    #[test]
    fn rejects_tampered_event_logs() {
        let a = fixture_attestation();
        let facts = raw_facts(&quote());
        let nonce = fixture_nonce();
        let mut events: Vec<Value> = match &a.event_log {
            Value::String(s) => serde_json::from_str(s).unwrap(),
            other => other.as_array().unwrap().clone(),
        };
        let at = events
            .iter()
            .position(|e| e["event"] == "compose-hash")
            .unwrap();

        let mut wrong_digest = events.clone();
        wrong_digest[at]["digest"] = "00".repeat(48).into();
        let mut bad_payload = events.clone();
        bad_payload[at]["event_payload"] = "zz".into();
        // Renaming the event changes its digest; with the digest dropped it still
        // replays wrong, and the compose-hash event is gone.
        events[at]["event"] = "renamed".into();
        events[at]["digest"] = "".into();
        for (log, msg) in [
            (wrong_digest, "wrong digest"),
            (bad_payload, "not hex"),
            (events, "RTMR3"),
        ] {
            let mut tampered = a.clone();
            tampered.event_log = Value::Array(log);
            let err = check_bindings(&tampered, &facts, &nonce)
                .unwrap_err()
                .to_string();
            assert!(err.contains(msg), "{err} should mention {msg}");
        }
    }

    #[test]
    fn requires_the_compose_hash_event() {
        // An event log without the compose-hash event, replayed into a matching RTMR3.
        let a = fixture_attestation();
        let mut facts = raw_facts(&quote());
        let nonce = fixture_nonce();
        let events: Vec<Value> = match &a.event_log {
            Value::String(s) => serde_json::from_str(s).unwrap(),
            other => other.as_array().unwrap().clone(),
        };
        let without: Vec<Value> = events
            .into_iter()
            .filter(|e| e["event"] != "compose-hash")
            .collect();
        let mut stripped = a.clone();
        stripped.event_log = Value::Array(without);
        facts.rt_mr3 = replay_rtmr3(&stripped.events().unwrap()).unwrap();
        let err = check_bindings(&stripped, &facts, &nonce)
            .unwrap_err()
            .to_string();
        assert!(err.contains("doesn't measure"), "{err}");
    }
}
