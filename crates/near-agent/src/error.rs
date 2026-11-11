//! Agent registration errors. Messages never contain private key material.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("config: {0}")]
    Config(String),

    #[error("key: {0}")]
    Key(String),

    #[error("TEE: {0}")]
    Tee(String),

    #[error("quote: {0}")]
    Quote(String),

    #[error("collateral: {0}")]
    Collateral(String),

    #[error("NEAR RPC: {0}")]
    Rpc(String),

    #[error("transaction failed: {0}")]
    TxFailed(String),

    #[error("HTTP: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<dstack_client::DstackError> for AgentError {
    fn from(e: dstack_client::DstackError) -> Self {
        AgentError::Tee(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dstack_errors_become_tee_errors() {
        let err: AgentError = dstack_client::DstackError::NotInTee.into();
        assert!(matches!(err, AgentError::Tee(_)));
        assert_eq!(err.to_string(), "TEE: Not running in TEE");
    }
}
