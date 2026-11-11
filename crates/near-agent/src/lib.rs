//! Registers the bot as a Sigstack agent on NEAR.
//!
//! On every boot the bot generates a random ed25519 key in TEE memory. Its implicit
//! account (the hex public key) is funded by a sponsor, then calls the agent
//! contract's `register_agent` with a dstack attestation whose report data is that
//! account ID plus 32 zero bytes, so the quote can't be replayed for another account.
//! The bot re-registers before the registration expires.
//!
//! Path-derived dstack keys are deliberately not used: every instance of the app
//! would derive the same account.

pub mod attestation;
pub mod base58;
pub mod collateral;
pub mod config;
pub mod error;
pub mod key;
pub mod quote;
pub mod registrar;
pub mod rpc;
pub mod tx;

pub use attestation::Tee;
pub use config::AgentConfig;
pub use error::AgentError;
pub use key::SigningKey;
pub use registrar::{Registrar, Registration};
