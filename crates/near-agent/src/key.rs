//! ed25519 signing keys for the agent and sponsor accounts.

use crate::{base58, AgentError};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::fmt;

/// An ed25519 key pair. `Debug` shows only the public key.
pub struct SigningKey {
    pair: Ed25519KeyPair,
}

impl SigningKey {
    /// A fresh key from the system CSPRNG. Inside the CVM this never leaves TEE memory.
    pub fn generate() -> Result<Self, AgentError> {
        let mut seed = [0u8; 32];
        SystemRandom::new()
            .fill(&mut seed)
            .map_err(|_| AgentError::Key("system randomness unavailable".into()))?;
        let key = Self::from_seed(&seed);
        seed.fill(0);
        key
    }

    pub(crate) fn from_seed(seed: &[u8; 32]) -> Result<Self, AgentError> {
        Ed25519KeyPair::from_seed_unchecked(seed)
            .map(|pair| Self { pair })
            .map_err(|e| AgentError::Key(format!("invalid ed25519 seed: {e}")))
    }

    /// Parses a NEAR CLI secret key, `ed25519:<base58>` of the 64-byte
    /// seed-plus-public-key form or of a bare 32-byte seed.
    pub fn from_near_secret(secret: &str) -> Result<Self, AgentError> {
        let encoded = secret
            .trim()
            .strip_prefix("ed25519:")
            .ok_or_else(|| AgentError::Key("secret key must start with `ed25519:`".into()))?;
        let bytes = base58::decode(encoded)
            .ok_or_else(|| AgentError::Key("secret key is not valid base58".into()))?;
        let pair = match bytes.len() {
            64 => Ed25519KeyPair::from_seed_and_public_key(&bytes[..32], &bytes[32..]),
            32 => Ed25519KeyPair::from_seed_unchecked(&bytes),
            n => {
                return Err(AgentError::Key(format!(
                    "secret key must be 32 or 64 bytes, got {n}"
                )))
            }
        };
        pair.map(|pair| Self { pair })
            .map_err(|e| AgentError::Key(format!("invalid ed25519 secret key: {e}")))
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.pair
            .public_key()
            .as_ref()
            .try_into()
            .expect("ed25519 public keys are 32 bytes")
    }

    /// `ed25519:<base58>`, as NEAR RPC expects.
    pub fn public_key_string(&self) -> String {
        format!("ed25519:{}", base58::encode(&self.public_key()))
    }

    /// The NEAR implicit account ID: the lowercase hex public key.
    pub fn implicit_account_id(&self) -> String {
        hex::encode(self.public_key())
    }

    /// Report data the agent contract expects in the quote: the 32 public-key bytes
    /// (the implicit account ID, decoded) followed by 32 zero bytes.
    pub fn report_data(&self) -> [u8; 64] {
        let mut data = [0u8; 64];
        data[..32].copy_from_slice(&self.public_key());
        data
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.pair
            .sign(message)
            .as_ref()
            .try_into()
            .expect("ed25519 signatures are 64 bytes")
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigningKey({})", self.public_key_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{UnparsedPublicKey, ED25519};

    // RFC 8032 section 7.1, test 1.
    const RFC_SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
    const RFC_PUBLIC: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

    fn rfc_seed() -> [u8; 32] {
        hex::decode(RFC_SEED).unwrap().try_into().unwrap()
    }

    #[test]
    fn seed_gives_rfc_public_key_and_implicit_account() {
        let key = SigningKey::from_seed(&rfc_seed()).unwrap();
        assert_eq!(key.implicit_account_id(), RFC_PUBLIC);
        assert_eq!(
            key.public_key_string(),
            format!("ed25519:{}", base58::encode(&hex::decode(RFC_PUBLIC).unwrap()))
        );
    }

    #[test]
    fn report_data_is_account_id_then_zeros() {
        let key = SigningKey::from_seed(&rfc_seed()).unwrap();
        let data = key.report_data();
        assert_eq!(hex::encode(&data[..32]), RFC_PUBLIC);
        assert_eq!(data[32..], [0u8; 32]);
    }

    #[test]
    fn generated_keys_are_distinct_and_sign() {
        let a = SigningKey::generate().unwrap();
        let b = SigningKey::generate().unwrap();
        assert_ne!(a.public_key(), b.public_key());

        let sig = a.sign(b"hello");
        UnparsedPublicKey::new(&ED25519, a.public_key())
            .verify(b"hello", &sig)
            .unwrap();
        assert!(UnparsedPublicKey::new(&ED25519, b.public_key())
            .verify(b"hello", &sig)
            .is_err());
    }

    #[test]
    fn parses_near_secret_keys() {
        let seed = rfc_seed();
        let public = hex::decode(RFC_PUBLIC).unwrap();

        let full = format!("ed25519:{}", base58::encode(&[&seed[..], &public[..]].concat()));
        let key = SigningKey::from_near_secret(&full).unwrap();
        assert_eq!(key.implicit_account_id(), RFC_PUBLIC);

        let bare = format!(" ed25519:{} ", base58::encode(&seed));
        let key = SigningKey::from_near_secret(&bare).unwrap();
        assert_eq!(key.implicit_account_id(), RFC_PUBLIC);
    }

    #[test]
    fn rejects_bad_near_secret_keys() {
        let seed = rfc_seed();
        // No prefix, not base58, wrong length, public half not matching the seed.
        let cases = [
            base58::encode(&seed),
            "ed25519:0OIl".to_string(),
            format!("ed25519:{}", base58::encode(&seed[..16])),
            format!("ed25519:{}", base58::encode(&[seed, seed].concat())),
        ];
        for case in cases {
            let err = SigningKey::from_near_secret(&case).unwrap_err();
            assert!(matches!(err, AgentError::Key(_)), "{case}");
            assert!(!err.to_string().contains(&base58::encode(&seed)));
        }
    }

    #[test]
    fn debug_shows_only_the_public_key() {
        let key = SigningKey::from_seed(&rfc_seed()).unwrap();
        let debug = format!("{key:?}");
        assert_eq!(debug, format!("SigningKey({})", key.public_key_string()));
        assert!(!debug.contains(RFC_SEED));
    }
}
