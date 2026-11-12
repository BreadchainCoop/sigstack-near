//! Signal client errors.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SignalError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("API error: {0}")]
    Api(String),

    /// signal-api answered 4xx: the request itself was refused, so retrying it won't help.
    #[error("Rejected ({status}): {message}")]
    Rejected { status: u16, message: String },

    #[error("Not registered")]
    NotRegistered,

    #[error("Send failed: {0}")]
    SendFailed(String),

    #[error("Attachment download failed: {0}")]
    AttachmentDownloadFailed(String),
}
