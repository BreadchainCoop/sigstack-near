//! Minimal NEAR JSON-RPC client: account and access-key lookups, contract views,
//! and sending a signed transaction.

use crate::{base58, tx::SignedTransaction, AgentError};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct NearRpc {
    url: String,
    http: reqwest::Client,
}

/// What signing a transaction for an access key needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessKeyInfo {
    pub nonce: u64,
    pub block_hash: [u8; 32],
    /// Height of the block the key was read at; bounds a delegate action's lifetime.
    pub block_height: u64,
}

impl NearRpc {
    pub fn new(url: impl Into<String>) -> Result<Self, AgentError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()?;
        Ok(Self {
            url: url.into(),
            http,
        })
    }

    /// `Ok(Err(error))` is a JSON-RPC error object, left to the caller to classify.
    async fn call(&self, method: &str, params: Value) -> Result<Result<Value, Value>, AgentError> {
        let body =
            json!({ "jsonrpc": "2.0", "id": "sigstack", "method": method, "params": params });
        let response: Value = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if let Some(error) = response.get("error") {
            return Ok(Err(error.clone()));
        }
        response
            .get("result")
            .cloned()
            .map(Ok)
            .ok_or_else(|| AgentError::Rpc(format!("{method}: response has no result")))
    }

    async fn query(&self, params: Value) -> Result<Option<Value>, AgentError> {
        match self.call("query", params).await? {
            Ok(result) => {
                // Older nodes report a missing account or key inside the result.
                if let Some(msg) = result.get("error").and_then(Value::as_str) {
                    if msg.contains("does not exist") {
                        return Ok(None);
                    }
                    return Err(AgentError::Rpc(msg.to_string()));
                }
                Ok(Some(result))
            }
            Err(error) if is_missing(&error) => Ok(None),
            Err(error) => Err(AgentError::Rpc(error.to_string())),
        }
    }

    /// `None` if the account or key doesn't exist.
    pub async fn access_key(
        &self,
        account_id: &str,
        public_key: &str,
    ) -> Result<Option<AccessKeyInfo>, AgentError> {
        let Some(result) = self
            .query(json!({
                "request_type": "view_access_key",
                "finality": "final",
                "account_id": account_id,
                "public_key": public_key,
            }))
            .await?
        else {
            return Ok(None);
        };
        let nonce = result
            .get("nonce")
            .and_then(Value::as_u64)
            .ok_or_else(|| AgentError::Rpc("access key has no nonce".into()))?;
        let block_hash = result
            .get("block_hash")
            .and_then(Value::as_str)
            .and_then(base58::decode)
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or_else(|| AgentError::Rpc("access key query has no valid block_hash".into()))?;
        let block_height = result
            .get("block_height")
            .and_then(Value::as_u64)
            .ok_or_else(|| AgentError::Rpc("access key query has no block_height".into()))?;
        Ok(Some(AccessKeyInfo {
            nonce,
            block_hash,
            block_height,
        }))
    }

    /// Liquid balance in yoctoNEAR; `None` if the account doesn't exist.
    pub async fn balance(&self, account_id: &str) -> Result<Option<u128>, AgentError> {
        let Some(result) = self
            .query(json!({
                "request_type": "view_account",
                "finality": "final",
                "account_id": account_id,
            }))
            .await?
        else {
            return Ok(None);
        };
        result
            .get("amount")
            .and_then(Value::as_str)
            .and_then(|amount| amount.parse().ok())
            .map(Some)
            .ok_or_else(|| AgentError::Rpc("account has no valid amount".into()))
    }

    /// Calls a view method and parses its JSON return value.
    pub async fn view_function(
        &self,
        contract_id: &str,
        method: &str,
        args: &Value,
    ) -> Result<Value, AgentError> {
        let result = self
            .query(json!({
                "request_type": "call_function",
                "finality": "final",
                "account_id": contract_id,
                "method_name": method,
                "args_base64": STANDARD.encode(serde_json::to_vec(args)?),
            }))
            .await?
            .ok_or_else(|| AgentError::Rpc(format!("{contract_id} does not exist")))?;
        let bytes: Vec<u8> = result
            .get("result")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .ok_or_else(|| AgentError::Rpc(format!("{method}: view has no result")))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// Sends a transaction and waits for it to be final. Errors if it failed.
    pub async fn send_tx(&self, signed: &SignedTransaction) -> Result<(), AgentError> {
        let outcome = self
            .call(
                "send_tx",
                json!({
                    "signed_tx_base64": STANDARD.encode(&signed.bytes),
                    "wait_until": "FINAL",
                }),
            )
            .await?
            .map_err(|error| AgentError::Rpc(error.to_string()))?;
        match outcome.get("status") {
            Some(status) if status.get("SuccessValue").is_some() => Ok(()),
            Some(status) => Err(AgentError::TxFailed(
                status.get("Failure").unwrap_or(status).to_string(),
            )),
            None => Err(AgentError::Rpc("transaction outcome has no status".into())),
        }
    }
}

fn is_missing(error: &Value) -> bool {
    matches!(
        error.pointer("/cause/name").and_then(Value::as_str),
        Some("UNKNOWN_ACCOUNT" | "UNKNOWN_ACCESS_KEY")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn respond(server: &MockServer, request: Value, response: Value) {
        Mock::given(method("POST"))
            .and(body_partial_json(request))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(server)
            .await;
    }

    fn result(value: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": "sigstack", "result": value })
    }

    fn unknown(name: &str) -> Value {
        json!({ "jsonrpc": "2.0", "id": "sigstack", "error": {
            "name": "HANDLER_ERROR", "cause": { "name": name, "info": {} }
        }})
    }

    #[tokio::test]
    async fn access_key_reads_nonce_and_block_hash() {
        let server = MockServer::start().await;
        let hash = [4u8; 32];
        respond(
            &server,
            json!({ "params": { "request_type": "view_access_key", "account_id": "a.testnet" } }),
            result(json!({
                "nonce": 42,
                "block_hash": base58::encode(&hash),
                "block_height": 77,
                "permission": "FullAccess"
            })),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "request_type": "view_access_key", "account_id": "b.testnet" } }),
            unknown("UNKNOWN_ACCESS_KEY"),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "request_type": "view_access_key", "account_id": "c.testnet" } }),
            result(json!({ "error": "access key ed25519:x does not exist while viewing" })),
        )
        .await;

        let rpc = NearRpc::new(server.uri()).unwrap();
        assert_eq!(
            rpc.access_key("a.testnet", "ed25519:x").await.unwrap(),
            Some(AccessKeyInfo {
                nonce: 42,
                block_hash: hash,
                block_height: 77
            })
        );
        assert_eq!(
            rpc.access_key("b.testnet", "ed25519:x").await.unwrap(),
            None
        );
        assert_eq!(
            rpc.access_key("c.testnet", "ed25519:x").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn access_key_rejects_malformed_results() {
        let server = MockServer::start().await;
        respond(
            &server,
            json!({ "params": { "account_id": "no-nonce" } }),
            result(json!({ "block_hash": base58::encode(&[1u8; 32]) })),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "account_id": "short-hash" } }),
            result(json!({ "nonce": 1, "block_hash": base58::encode(&[1u8; 8]) })),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "account_id": "other-error" } }),
            result(json!({ "error": "node is syncing" })),
        )
        .await;

        let rpc = NearRpc::new(server.uri()).unwrap();
        for account in ["no-nonce", "short-hash", "other-error"] {
            let err = rpc.access_key(account, "ed25519:x").await.unwrap_err();
            assert!(matches!(err, AgentError::Rpc(_)), "{account}: {err}");
        }
    }

    #[tokio::test]
    async fn balance_reads_amount_or_none() {
        let server = MockServer::start().await;
        respond(
            &server,
            json!({ "params": { "request_type": "view_account", "account_id": "rich" } }),
            result(json!({ "amount": "100000000000000000000000000", "locked": "0" })),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "request_type": "view_account", "account_id": "new" } }),
            unknown("UNKNOWN_ACCOUNT"),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "request_type": "view_account", "account_id": "broken" } }),
            unknown("INTERNAL_ERROR"),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "request_type": "view_account", "account_id": "odd" } }),
            result(json!({ "amount": 5 })),
        )
        .await;

        let rpc = NearRpc::new(server.uri()).unwrap();
        assert_eq!(
            rpc.balance("rich").await.unwrap(),
            Some(100_000_000_000_000_000_000_000_000)
        );
        assert_eq!(rpc.balance("new").await.unwrap(), None);
        assert!(matches!(
            rpc.balance("broken").await.unwrap_err(),
            AgentError::Rpc(_)
        ));
        assert!(matches!(
            rpc.balance("odd").await.unwrap_err(),
            AgentError::Rpc(_)
        ));
    }

    #[tokio::test]
    async fn view_function_decodes_the_return_value() {
        let server = MockServer::start().await;
        let args = json!({ "account_id": "agent" });
        let returned = serde_json::to_vec(&json!({ "valid_until_ms": "123" })).unwrap();
        respond(
            &server,
            json!({ "params": {
                "request_type": "call_function",
                "account_id": "contract.testnet",
                "method_name": "get_agent",
                "args_base64": STANDARD.encode(serde_json::to_vec(&args).unwrap()),
            }}),
            result(json!({ "result": returned, "logs": [] })),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "method_name": "no_result" } }),
            result(json!({ "logs": [] })),
        )
        .await;
        respond(
            &server,
            json!({ "params": { "account_id": "missing.testnet" } }),
            unknown("UNKNOWN_ACCOUNT"),
        )
        .await;

        let rpc = NearRpc::new(server.uri()).unwrap();
        let value = rpc
            .view_function("contract.testnet", "get_agent", &args)
            .await
            .unwrap();
        assert_eq!(value, json!({ "valid_until_ms": "123" }));

        let err = rpc
            .view_function("contract.testnet", "no_result", &args)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no_result"));
        let err = rpc
            .view_function("missing.testnet", "get_agent", &args)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    fn signed(byte: u8) -> SignedTransaction {
        SignedTransaction {
            bytes: vec![byte; 3],
            hash: [0u8; 32],
        }
    }

    #[tokio::test]
    async fn send_tx_reports_success_and_failure() {
        let server = MockServer::start().await;
        let tx = |byte: u8| {
            json!({
                "method": "send_tx",
                "params": { "signed_tx_base64": STANDARD.encode([byte; 3]) }
            })
        };
        respond(
            &server,
            tx(1),
            result(json!({ "status": { "SuccessValue": "" } })),
        )
        .await;
        respond(
            &server,
            tx(2),
            result(json!({ "status": { "Failure": { "ActionError": { "kind": {
                "FunctionCallError": { "ExecutionError": "Smart contract panicked: Attestation verification failed" }
            }}}}})),
        )
        .await;
        respond(&server, tx(3), result(json!({}))).await;
        respond(
            &server,
            tx(4),
            json!({ "jsonrpc": "2.0", "id": "sigstack", "error": { "name": "REQUEST_VALIDATION_ERROR" } }),
        )
        .await;
        respond(
            &server,
            tx(5),
            json!({ "jsonrpc": "2.0", "id": "sigstack" }),
        )
        .await;

        let rpc = NearRpc::new(server.uri()).unwrap();
        rpc.send_tx(&signed(1)).await.unwrap();

        let err = rpc.send_tx(&signed(2)).await.unwrap_err();
        assert!(matches!(err, AgentError::TxFailed(_)));
        assert!(err.to_string().contains("Attestation verification failed"));

        for byte in [3, 4, 5] {
            let err = rpc.send_tx(&signed(byte)).await.unwrap_err();
            assert!(matches!(err, AgentError::Rpc(_)), "{byte}: {err}");
        }
    }

    #[tokio::test]
    async fn http_errors_surface() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let rpc = NearRpc::new(server.uri()).unwrap();
        assert!(matches!(
            rpc.balance("a").await.unwrap_err(),
            AgentError::Http(_)
        ));
    }
}
