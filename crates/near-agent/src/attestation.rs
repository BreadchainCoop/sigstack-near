//! Builds the `DstackAttestation` argument for `register_agent`, matching
//! shade-agent-js's `internalGetAttestation` at shade-agent-framework@35758f3.

use crate::{collateral::fetch_collateral, AgentError};
use async_trait::async_trait;
use dstack_client::{AppInfo, DstackClient};
use serde_json::{json, Value};

/// The two things registration needs from the TEE.
#[async_trait]
pub trait Tee: Send + Sync {
    /// dstack's `tcb_info`: measurements, app compose and the RTMR3 event log.
    async fn tcb_info(&self) -> Result<Value, AgentError>;
    /// A raw TDX quote over `report_data`.
    async fn quote(&self, report_data: &[u8; 64]) -> Result<Vec<u8>, AgentError>;
}

#[async_trait]
impl Tee for DstackClient {
    async fn tcb_info(&self) -> Result<Value, AgentError> {
        tcb_info_from_app_info(self.get_app_info().await?)
    }

    async fn quote(&self, report_data: &[u8; 64]) -> Result<Vec<u8>, AgentError> {
        decode_quote(&self.get_quote(report_data).await?.quote)
    }
}

/// dstack returns `tcb_info` as a JSON-encoded string; accept an object too.
pub(crate) fn tcb_info_from_app_info(info: AppInfo) -> Result<Value, AgentError> {
    match info.extra.get("tcb_info") {
        Some(Value::String(s)) => Ok(serde_json::from_str(s)?),
        Some(v @ Value::Object(_)) => Ok(v.clone()),
        _ => Err(AgentError::Tee("dstack /Info has no tcb_info".into())),
    }
}

/// dstack returns the quote hex-encoded. Hex only: a base64 fallback could
/// silently mis-decode a base64 string that happens to look like hex.
pub(crate) fn decode_quote(quote: &str) -> Result<Vec<u8>, AgentError> {
    let quote = quote.trim();
    hex::decode(quote.strip_prefix("0x").unwrap_or(quote))
        .map_err(|_| AgentError::Tee("quote is not hex".into()))
}

/// Maps dstack's `tcb_info` to the contract's `TcbInfo`, keeping only its fields.
pub fn contract_tcb_info(raw: &Value) -> Result<Value, AgentError> {
    let field = |key: &str| raw.get(key).and_then(Value::as_str).unwrap_or("");
    let events = match raw.get("event_log") {
        Some(Value::Array(events)) => events.clone(),
        Some(Value::String(s)) => serde_json::from_str(s)?,
        _ => Vec::new(),
    };
    let event_log = events
        .iter()
        .map(|e| {
            let number = |key: &str| {
                e.get(key)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| AgentError::Tee(format!("event log entry has no {key}")))
            };
            let text = |key: &str| e.get(key).and_then(Value::as_str).unwrap_or("");
            Ok(json!({
                "imr": number("imr")?,
                "event_type": number("event_type")?,
                "digest": text("digest"),
                "event": text("event"),
                "event_payload": text("event_payload"),
            }))
        })
        .collect::<Result<Vec<_>, AgentError>>()?;
    Ok(json!({
        "mrtd": field("mrtd"),
        "rtmr0": field("rtmr0"),
        "rtmr1": field("rtmr1"),
        "rtmr2": field("rtmr2"),
        "rtmr3": field("rtmr3"),
        "os_image_hash": field("os_image_hash"),
        "compose_hash": field("compose_hash"),
        "device_id": field("device_id"),
        "app_compose": field("app_compose"),
        "event_log": event_log,
    }))
}

/// `{ quote, collateral, tcb_info }` for a quote over `report_data`.
pub async fn build_attestation(
    tee: &dyn Tee,
    http: &reqwest::Client,
    pccs_endpoints: &[String],
    report_data: &[u8; 64],
) -> Result<Value, AgentError> {
    let tcb_info = contract_tcb_info(&tee.tcb_info().await?)?;
    let quote = tee.quote(report_data).await?;
    let collateral = fetch_collateral(http, pccs_endpoints, &quote).await?;
    // `quote` serializes as a JSON array of bytes: the contract's `QuoteBytes(Vec<u8>)`
    // derives serde, and shade-agent-js sends `number[]` the same way.
    Ok(json!({
        "quote": quote,
        "collateral": collateral,
        "tcb_info": tcb_info,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::collateral::tests::mock_pccs;
    use crate::quote::tests::tdx_quote;
    use std::sync::Mutex;
    use wiremock::MockServer;

    pub(crate) fn dstack_tcb_info() -> Value {
        json!({
            "mrtd": "aa",
            "rtmr0": "b0",
            "rtmr1": "b1",
            "rtmr2": "b2",
            "rtmr3": "b3",
            "mr_aggregated": "dropped",
            "os_image_hash": "",
            "compose_hash": "cc",
            "device_id": "dd",
            "app_compose": "{\"runner\":\"docker-compose\"}",
            "event_log": [
                { "imr": 3, "event_type": 134217729, "digest": "ee", "event": "compose-hash", "event_payload": "cc" }
            ],
        })
    }

    /// A TEE that records the report data it was asked to quote.
    pub(crate) struct FakeTee {
        pub(crate) quoted: Mutex<Vec<[u8; 64]>>,
    }

    impl FakeTee {
        pub(crate) fn new() -> Self {
            Self {
                quoted: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Tee for FakeTee {
        async fn tcb_info(&self) -> Result<Value, AgentError> {
            Ok(dstack_tcb_info())
        }

        async fn quote(&self, report_data: &[u8; 64]) -> Result<Vec<u8>, AgentError> {
            self.quoted.lock().unwrap().push(*report_data);
            Ok(tdx_quote("Intel SGX PCK Platform CA"))
        }
    }

    fn app_info(extra: Value) -> AppInfo {
        serde_json::from_value(extra).unwrap()
    }

    #[test]
    fn reads_tcb_info_string_or_object() {
        let tcb = dstack_tcb_info();
        let as_string = app_info(json!({ "app_id": "a", "tcb_info": tcb.to_string() }));
        assert_eq!(tcb_info_from_app_info(as_string).unwrap(), tcb);

        let as_object = app_info(json!({ "tcb_info": tcb.clone() }));
        assert_eq!(tcb_info_from_app_info(as_object).unwrap(), tcb);

        let missing = app_info(json!({ "app_id": "a" }));
        assert!(matches!(
            tcb_info_from_app_info(missing),
            Err(AgentError::Tee(_))
        ));
        let bad = app_info(json!({ "tcb_info": "{not json" }));
        assert!(matches!(
            tcb_info_from_app_info(bad),
            Err(AgentError::Json(_))
        ));
    }

    #[test]
    fn decodes_hex_quotes_only() {
        assert_eq!(decode_quote("0a0b").unwrap(), [0x0a, 0x0b]);
        assert_eq!(decode_quote(" 0x0a0b\n").unwrap(), [0x0a, 0x0b]);
        assert!(decode_quote("cXVvdGU=").is_err());
        assert!(decode_quote("!!").is_err());
    }

    #[test]
    fn maps_tcb_info_to_contract_shape() {
        let mapped = contract_tcb_info(&dstack_tcb_info()).unwrap();
        assert_eq!(
            mapped,
            json!({
                "mrtd": "aa",
                "rtmr0": "b0",
                "rtmr1": "b1",
                "rtmr2": "b2",
                "rtmr3": "b3",
                "os_image_hash": "",
                "compose_hash": "cc",
                "device_id": "dd",
                "app_compose": "{\"runner\":\"docker-compose\"}",
                "event_log": [
                    { "imr": 3, "event_type": 134217729, "digest": "ee", "event": "compose-hash", "event_payload": "cc" }
                ],
            })
        );

        let mut as_string = dstack_tcb_info();
        as_string["event_log"] = Value::String(dstack_tcb_info()["event_log"].to_string());
        assert_eq!(contract_tcb_info(&as_string).unwrap(), mapped);

        let empty = contract_tcb_info(&json!({})).unwrap();
        assert_eq!(empty["event_log"], json!([]));
        assert_eq!(empty["mrtd"], "");

        let bad_event = json!({ "event_log": [{ "imr": "three" }] });
        let err = contract_tcb_info(&bad_event).unwrap_err();
        assert!(err.to_string().contains("no imr"));
    }

    #[tokio::test]
    async fn builds_attestation_over_the_report_data() {
        let server = MockServer::start().await;
        mock_pccs(&server).await;
        let tee = FakeTee::new();
        let report_data = [5u8; 64];

        let attestation =
            build_attestation(&tee, &reqwest::Client::new(), &[server.uri()], &report_data)
                .await
                .unwrap();

        assert_eq!(*tee.quoted.lock().unwrap(), vec![report_data]);
        let quote: Vec<u8> = serde_json::from_value(attestation["quote"].clone()).unwrap();
        assert_eq!(quote, tdx_quote("Intel SGX PCK Platform CA"));
        assert_eq!(attestation["collateral"]["pck_crl"], "dead");
        assert_eq!(attestation["tcb_info"]["compose_hash"], "cc");
    }
}
