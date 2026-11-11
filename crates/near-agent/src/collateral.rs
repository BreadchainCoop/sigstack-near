//! Fetches quote collateral from a PCCS, in the JSON shape the agent contract's
//! `Collateral` deserializes. Mirrors dcap-qvl's `CollateralClient::fetch` (v0.5.2).
//!
//! Collateral is public Intel data, signed by Intel; the contract checks it, so
//! where it is fetched from doesn't affect what the attestation proves.

use crate::{
    quote::{pck_info, PckInfo},
    AgentError,
};
use serde_json::{json, value::RawValue, Value};
use std::collections::HashMap;
use tracing::warn;

pub const PHALA_PCCS_URL: &str = "https://pccs.phala.network";
pub const INTEL_PCS_URL: &str = "https://api.trustedservices.intel.com";
const INTEL_ROOT_CA_CRL_URL: &str =
    "https://certificates.trustedservices.intel.com/IntelSGXRootCA.der";

/// Tries each endpoint in order and returns the first complete collateral.
pub async fn fetch_collateral(
    http: &reqwest::Client,
    endpoints: &[String],
    quote: &[u8],
) -> Result<Value, AgentError> {
    let pck = pck_info(quote)?;
    let mut failures = Vec::new();
    for base in endpoints {
        match fetch_from(http, base, &pck).await {
            Ok(collateral) => return Ok(collateral),
            Err(e) => {
                warn!("Collateral fetch from {base} failed: {e}");
                failures.push(format!("{base}: {e}"));
            }
        }
    }
    Err(AgentError::Collateral(if failures.is_empty() {
        "no PCCS endpoints configured".into()
    } else {
        failures.join("; ")
    }))
}

struct Response {
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

async fn get(http: &reqwest::Client, url: &str) -> Result<Response, AgentError> {
    let response = http.get(url).send().await?;
    if !response.status().is_success() {
        return Err(AgentError::Collateral(format!(
            "{url}: HTTP {}",
            response.status()
        )));
    }
    let headers = response.headers().clone();
    let body = response.bytes().await?.to_vec();
    Ok(Response { headers, body })
}

impl Response {
    /// The first of `names` present, URL-decoded (issuer chains are PEM, URL-encoded).
    fn header(&self, names: &[&str]) -> Result<String, AgentError> {
        names
            .iter()
            .find_map(|name| self.headers.get(*name))
            .and_then(|v| v.to_str().ok())
            .map(percent_decode)
            .ok_or_else(|| AgentError::Collateral(format!("missing {} header", names[0])))?
    }

    fn text(self) -> Result<String, AgentError> {
        String::from_utf8(self.body)
            .map_err(|_| AgentError::Collateral("response body is not UTF-8".into()))
    }
}

fn percent_decode(s: &str) -> Result<String, AgentError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let byte = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| AgentError::Collateral("bad percent-encoding in header".into()))?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| AgentError::Collateral("header is not UTF-8".into()))
}

/// Splits Intel's `{"<key>": {...}, "signature": "<hex>"}` into the signed JSON,
/// byte-for-byte as served, and the signature.
fn split_signed(body: &str, key: &str) -> Result<(String, String), AgentError> {
    let parts: HashMap<String, Box<RawValue>> = serde_json::from_str(body)
        .map_err(|e| AgentError::Collateral(format!("{key} response is not JSON: {e}")))?;
    let signed = parts
        .get(key)
        .ok_or_else(|| AgentError::Collateral(format!("response has no {key}")))?
        .get()
        .to_string();
    let signature: String = parts
        .get("signature")
        .map(|raw| serde_json::from_str(raw.get()))
        .transpose()?
        .ok_or_else(|| AgentError::Collateral(format!("{key} has no signature")))?;
    hex::decode(&signature)
        .map_err(|_| AgentError::Collateral(format!("{key} signature is not hex")))?;
    Ok((signed, signature))
}

enum RootCaCrl {
    /// PCCS serves the root CA CRL hex-encoded.
    PccsHex(String),
    /// Intel serves DER from its certificate host.
    Der(&'static str),
}

fn root_ca_crl_source(base: &str) -> RootCaCrl {
    if base.starts_with(INTEL_PCS_URL) {
        RootCaCrl::Der(INTEL_ROOT_CA_CRL_URL)
    } else {
        RootCaCrl::PccsHex(format!("{base}/sgx/certification/v4/rootcacrl"))
    }
}

async fn fetch_from(
    http: &reqwest::Client,
    base: &str,
    pck: &PckInfo,
) -> Result<Value, AgentError> {
    let base = base.trim_end_matches('/');

    let response = get(
        http,
        &format!(
            "{base}/sgx/certification/v4/pckcrl?ca={}&encoding=der",
            pck.ca
        ),
    )
    .await?;
    let pck_crl_issuer_chain = response.header(&["SGX-PCK-CRL-Issuer-Chain"])?;
    let pck_crl = response.body;

    let response = get(
        http,
        &format!("{base}/tdx/certification/v4/tcb?fmspc={}", pck.fmspc),
    )
    .await?;
    let tcb_info_issuer_chain =
        response.header(&["SGX-TCB-Info-Issuer-Chain", "TCB-Info-Issuer-Chain"])?;
    let (tcb_info, tcb_info_signature) = split_signed(&response.text()?, "tcbInfo")?;

    let response = get(
        http,
        &format!("{base}/tdx/certification/v4/qe/identity?update=standard"),
    )
    .await?;
    let qe_identity_issuer_chain = response.header(&["SGX-Enclave-Identity-Issuer-Chain"])?;
    let (qe_identity, qe_identity_signature) =
        split_signed(&response.text()?, "enclaveIdentity")?;

    let root_ca_crl = match root_ca_crl_source(base) {
        RootCaCrl::PccsHex(url) => {
            let text = get(http, &url).await?.text()?;
            hex::decode(text.trim())
                .map_err(|_| AgentError::Collateral("root CA CRL is not hex".into()))?
        }
        RootCaCrl::Der(url) => get(http, url).await?.body,
    };

    Ok(json!({
        "pck_crl_issuer_chain": pck_crl_issuer_chain,
        "root_ca_crl": hex::encode(root_ca_crl),
        "pck_crl": hex::encode(pck_crl),
        "tcb_info_issuer_chain": tcb_info_issuer_chain,
        "tcb_info": tcb_info,
        "tcb_info_signature": tcb_info_signature,
        "qe_identity_issuer_chain": qe_identity_issuer_chain,
        "qe_identity": qe_identity,
        "qe_identity_signature": qe_identity_signature,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::quote::tests::tdx_quote;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Keys deliberately not in sorted order: the signed bytes must survive as served.
    pub(crate) const TCB_INFO: &str = r#"{"version":3,"id":"TDX","fmspc":"90C06F000000"}"#;
    pub(crate) const QE_IDENTITY: &str = r#"{"version":2,"id":"TD_QE"}"#;

    /// Mounts a PCCS that serves collateral for the test quote's FMSPC.
    pub(crate) async fn mock_pccs(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/sgx/certification/v4/pckcrl"))
            .and(query_param("ca", "platform"))
            .and(query_param("encoding", "der"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("SGX-PCK-CRL-Issuer-Chain", "-----BEGIN%20CERT%2B-----%0A")
                    .set_body_bytes(vec![0xde, 0xad]),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tdx/certification/v4/tcb"))
            .and(query_param("fmspc", "90C06F000000"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("TCB-Info-Issuer-Chain", "tcb%20chain")
                    .set_body_string(format!(r#"{{"tcbInfo":{TCB_INFO},"signature":"abcd"}}"#)),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tdx/certification/v4/qe/identity"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("SGX-Enclave-Identity-Issuer-Chain", "qe%20chain")
                    .set_body_string(format!(
                        r#"{{"enclaveIdentity":{QE_IDENTITY},"signature":"0102"}}"#
                    )),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/sgx/certification/v4/rootcacrl"))
            .respond_with(ResponseTemplate::new(200).set_body_string("beef\n"))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn fetches_collateral_in_contract_shape() {
        let server = MockServer::start().await;
        mock_pccs(&server).await;
        let quote = tdx_quote("Intel SGX PCK Platform CA");

        let collateral = fetch_collateral(
            &reqwest::Client::new(),
            &[format!("{}/", server.uri())],
            &quote,
        )
        .await
        .unwrap();

        assert_eq!(
            collateral,
            json!({
                "pck_crl_issuer_chain": "-----BEGIN CERT+-----\n",
                "root_ca_crl": "beef",
                "pck_crl": "dead",
                "tcb_info_issuer_chain": "tcb chain",
                "tcb_info": TCB_INFO,
                "tcb_info_signature": "abcd",
                "qe_identity_issuer_chain": "qe chain",
                "qe_identity": QE_IDENTITY,
                "qe_identity_signature": "0102",
            })
        );
    }

    #[tokio::test]
    async fn falls_back_to_the_next_endpoint() {
        let broken = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&broken)
            .await;
        let good = MockServer::start().await;
        mock_pccs(&good).await;
        let quote = tdx_quote("Intel SGX PCK Platform CA");
        let http = reqwest::Client::new();

        let collateral = fetch_collateral(&http, &[broken.uri(), good.uri()], &quote)
            .await
            .unwrap();
        assert_eq!(collateral["root_ca_crl"], "beef");

        let err = fetch_collateral(&http, &[broken.uri()], &quote)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 503"), "{err}");

        let err = fetch_collateral(&http, &[], &quote).await.unwrap_err();
        assert!(err.to_string().contains("no PCCS endpoints"));
    }

    #[tokio::test]
    async fn rejects_incomplete_responses() {
        // Missing issuer-chain header on the PCK CRL.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let quote = tdx_quote("Intel SGX PCK Platform CA");
        let err = fetch_collateral(&reqwest::Client::new(), &[server.uri()], &quote)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SGX-PCK-CRL-Issuer-Chain"), "{err}");
    }

    #[test]
    fn splits_signed_json_without_reformatting() {
        let body = r#"{"signature":"00ff","tcbInfo":{"z":1,  "a":[2]}}"#;
        let (signed, sig) = split_signed(body, "tcbInfo").unwrap();
        assert_eq!(signed, r#"{"z":1,  "a":[2]}"#);
        assert_eq!(sig, "00ff");

        for (body, needle) in [
            ("not json", "not JSON"),
            (r#"{"signature":"00"}"#, "no tcbInfo"),
            (r#"{"tcbInfo":{}}"#, "no signature"),
            (r#"{"tcbInfo":{},"signature":"zz"}"#, "not hex"),
        ] {
            let err = split_signed(body, "tcbInfo").unwrap_err();
            assert!(err.to_string().contains(needle), "{body}: {err}");
        }
        assert!(matches!(
            split_signed(r#"{"tcbInfo":{},"signature":5}"#, "tcbInfo"),
            Err(AgentError::Json(_))
        ));
    }

    #[test]
    fn percent_decodes_headers() {
        assert_eq!(percent_decode("a%20b%2Bc%0A").unwrap(), "a b+c\n");
        assert_eq!(percent_decode("plain").unwrap(), "plain");
        assert!(percent_decode("bad%2").is_err());
        assert!(percent_decode("bad%zz").is_err());
        assert!(percent_decode("%ff").is_err());
    }

    #[test]
    fn root_ca_crl_comes_from_intel_or_the_pccs() {
        assert!(matches!(
            root_ca_crl_source(INTEL_PCS_URL),
            RootCaCrl::Der(INTEL_ROOT_CA_CRL_URL)
        ));
        match root_ca_crl_source(PHALA_PCCS_URL) {
            RootCaCrl::PccsHex(url) => assert_eq!(
                url,
                "https://pccs.phala.network/sgx/certification/v4/rootcacrl"
            ),
            RootCaCrl::Der(_) => panic!("Phala PCCS serves hex"),
        }
    }
}
