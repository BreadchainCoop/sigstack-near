//! Registers the bot as a Sigstack agent on NEAR.
//!
//! On every boot the bot generates a random ed25519 key in TEE memory. A sponsor
//! creates its implicit account (the hex public key) with just enough for storage.
//! The agent then signs a call to the agent contract's `register_agent` as a NEP-366
//! delegate action, and the sponsor relays it, paying the gas and deposit. The call
//! carries a dstack attestation whose report data is the agent's account ID plus 32
//! zero bytes, so the quote can't be replayed for another account. The bot
//! re-registers before the registration expires.
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
pub use registrar::{AgentStatus, Registrar, Registration, RegistrationState};
