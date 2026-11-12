//! The parts of NEAR AI's attestation report (`GET /v1/attestation/report`) the
//! verifier reads.

use super::VerifyError;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct AttestationResponse {
    #[serde(default)]
    pub model_attestations: Vec<ModelAttestation>,
}

/// One model-serving instance: a CVM with GPUs, holding the key it signs responses with.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelAttestation {
    pub model_name: String,
    /// For ed25519, the hex public key.
    pub signing_address: String,
    pub signing_algo: String,
    pub request_nonce: String,
    /// Hex TDX quote.
    pub intel_quote: String,
    /// NVIDIA GPU evidence, as a JSON string.
    #[serde(default)]
    pub nvidia_payload: Option<String>,
    /// dstack's event log: a JSON array, or a string holding one.
    pub event_log: Value,
    pub info: InstanceInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstanceInfo {
    pub compose_hash: String,
    /// An object with `app_compose`, or a string holding one.
    pub tcb_info: Value,
}

/// A dstack event-log entry.
#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub imr: u32,
    pub event_type: u32,
    #[serde(default)]
    pub digest: String,
    pub event: String,
    pub event_payload: String,
}

impl ModelAttestation {
    pub fn events(&self) -> Result<Vec<Event>, VerifyError> {
        json_or_string(&self.event_log, "event_log")
    }

    /// The measured app definition, exactly as hashed.
    pub fn app_compose(&self) -> Result<String, VerifyError> {
        let tcb: Value = json_or_string(&self.info.tcb_info, "tcb_info")?;
        tcb.get("app_compose")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| VerifyError::Report("tcb_info has no app_compose".into()))
    }
}

fn json_or_string<T: DeserializeOwned>(value: &Value, field: &str) -> Result<T, VerifyError> {
    match value {
        Value::String(s) => serde_json::from_str(s),
        other => serde_json::from_value(other.clone()),
    }
    .map_err(|e| VerifyError::Report(format!("{field}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn attestation(event_log: Value, tcb_info: Value) -> ModelAttestation {
        serde_json::from_value(json!({
            "model_name": "m",
            "signing_address": "ab",
            "signing_algo": "ed25519",
            "request_nonce": "00",
            "intel_quote": "00",
            "event_log": event_log,
            "info": {"compose_hash": "h", "tcb_info": tcb_info}
        }))
        .unwrap()
    }

    #[test]
    fn reads_event_log_and_tcb_info_as_json_or_string() {
        let event = json!({"imr": 3, "event_type": 1, "event": "e", "event_payload": "00"});
        let tcb = json!({"app_compose": "{}"});
        for (log, tcb) in [
            (json!([event]), tcb.clone()),
            (json!(json!([event]).to_string()), json!(tcb.to_string())),
        ] {
            let a = attestation(log, tcb);
            let events = a.events().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event, "e");
            assert_eq!(events[0].digest, "");
            assert_eq!(a.app_compose().unwrap(), "{}");
        }
    }

    #[test]
    fn reports_malformed_fields() {
        let a = attestation(json!("not json"), json!({}));
        assert!(a.events().unwrap_err().to_string().contains("event_log"));
        let err = a.app_compose().unwrap_err().to_string();
        assert!(err.contains("no app_compose"), "{err}");
    }
}
