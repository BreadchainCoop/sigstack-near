//! The model instance's NVIDIA GPU evidence, checked by NVIDIA's Remote Attestation
//! Service (NRAS).
//!
//! NRAS returns signed EAT tokens. We read their claims without checking NVIDIA's
//! token signature: the verdict comes straight from NRAS over TLS, so the signature
//! would only matter if the tokens were passed on to someone else.

use super::VerifyError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::Value;

pub const NRAS_URL: &str = "https://nras.attestation.nvidia.com/v3/attest/gpu";

/// Sends the evidence to NRAS and returns how many GPUs it vouched for. The evidence
/// must have been generated for our nonce.
pub async fn verify_gpus(
    http: &reqwest::Client,
    nras_url: &str,
    payload: &str,
    nonce_hex: &str,
) -> Result<usize, VerifyError> {
    let evidence: Value = serde_json::from_str(payload)
        .map_err(|e| VerifyError::Gpu(format!("evidence is not JSON: {e}")))?;
    let bound = evidence
        .get("nonce")
        .and_then(Value::as_str)
        .is_some_and(|n| n.eq_ignore_ascii_case(nonce_hex));
    if !bound {
        return Err(VerifyError::Gpu(
            "the evidence wasn't generated for our nonce".into(),
        ));
    }

    let response = http
        .post(nras_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(payload.to_owned())
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(VerifyError::Gpu(format!(
            "NRAS: HTTP {}",
            response.status()
        )));
    }
    check_verdict(&response.json().await?)
}

/// NRAS answers `[["JWT", <overall token>], {"GPU-0": <token>, ...}]`. The overall
/// result must be true, and each GPU's evidence must have matched the nonce and its
/// reference measurements.
pub fn check_verdict(verdict: &Value) -> Result<usize, VerifyError> {
    let overall = verdict
        .get(0)
        .and_then(|v| v.get(1))
        .and_then(Value::as_str)
        .ok_or_else(|| VerifyError::Gpu("NRAS returned no overall token".into()))?;
    if claims(overall)?["x-nvidia-overall-att-result"] != Value::Bool(true) {
        return Err(VerifyError::Gpu(
            "NVIDIA's attestation service rejected the GPU evidence".into(),
        ));
    }
    let gpus = verdict
        .get(1)
        .and_then(Value::as_object)
        .filter(|gpus| !gpus.is_empty())
        .ok_or_else(|| VerifyError::Gpu("NRAS returned no GPU tokens".into()))?;
    for (gpu, token) in gpus {
        let c = claims(token.as_str().unwrap_or_default())?;
        if c["x-nvidia-gpu-attestation-report-nonce-match"] != Value::Bool(true) {
            return Err(VerifyError::Gpu(format!("{gpu}: evidence nonce mismatch")));
        }
        if c["measres"] != "success" {
            return Err(VerifyError::Gpu(format!("{gpu}: measurements don't match")));
        }
    }
    Ok(gpus.len())
}

fn claims(token: &str) -> Result<Value, VerifyError> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| VerifyError::Gpu("NRAS token is not a JWT".into()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| VerifyError::Gpu("NRAS token is not base64url".into()))?;
    serde_json::from_slice(&bytes).map_err(|_| VerifyError::Gpu("NRAS token is not JSON".into()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::verify::tests::fixture;
    use serde_json::json;
    use wiremock::matchers::{body_string, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub(crate) fn token(claims: Value) -> String {
        format!(
            "eyJhbGciOiJFUzM4NCJ9.{}.sig",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    fn verdict(overall: bool, gpu: Value) -> Value {
        json!([
            ["JWT", token(json!({"x-nvidia-overall-att-result": overall}))],
            {"GPU-0": token(gpu)}
        ])
    }

    fn good_gpu() -> Value {
        json!({"x-nvidia-gpu-attestation-report-nonce-match": true, "measres": "success"})
    }

    #[test]
    fn accepts_the_real_nras_verdict() {
        let gpus = check_verdict(&fixture()["nras_response"]).unwrap();
        assert_eq!(gpus, 8);
    }

    #[test]
    fn rejects_failing_verdicts() {
        let cases = [
            (json!([]), "no overall token"),
            (verdict(false, good_gpu()), "rejected"),
            (
                json!([
                    ["JWT", token(json!({"x-nvidia-overall-att-result": true}))],
                    {}
                ]),
                "no GPU tokens",
            ),
            (
                verdict(
                    true,
                    json!({"x-nvidia-gpu-attestation-report-nonce-match": false}),
                ),
                "nonce mismatch",
            ),
            (
                verdict(
                    true,
                    json!({"x-nvidia-gpu-attestation-report-nonce-match": true, "measres": "fail"}),
                ),
                "measurements",
            ),
            (json!([["JWT", "nodots"]]), "not a JWT"),
            (json!([["JWT", "a.!!!.c"]]), "base64url"),
            (json!([["JWT", "a.e30x.c"]]), "not JSON"),
        ];
        for (verdict, msg) in cases {
            let err = check_verdict(&verdict).unwrap_err().to_string();
            assert!(err.contains(msg), "{err} should mention {msg}");
        }
    }

    #[tokio::test]
    async fn posts_the_evidence_to_nras() {
        let server = MockServer::start().await;
        let payload = json!({"nonce": "AB12", "evidence_list": []}).to_string();
        Mock::given(method("POST"))
            .and(path("/attest"))
            .and(body_string(payload.clone()))
            .respond_with(ResponseTemplate::new(200).set_body_json(verdict(true, good_gpu())))
            .expect(1)
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let url = format!("{}/attest", server.uri());
        assert_eq!(verify_gpus(&http, &url, &payload, "ab12").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn rejects_evidence_for_another_nonce_or_an_nras_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let url = server.uri();
        let payload = json!({"nonce": "ab12"}).to_string();

        for (payload, nonce, msg) in [
            ("not json", "ab12", "not JSON"),
            (payload.as_str(), "cd34", "our nonce"),
            ("{}", "ab12", "our nonce"),
            (payload.as_str(), "ab12", "HTTP 503"),
        ] {
            let err = verify_gpus(&http, &url, payload, nonce)
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(msg), "{err} should mention {msg}");
        }
    }
}
