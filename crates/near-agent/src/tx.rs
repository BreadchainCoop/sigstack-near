//! Borsh encoding and signing of the two NEAR transaction kinds the bot sends:
//! a sponsor transfer and the agent's `register_agent` call.

use crate::SigningKey;
use sha2::{Digest, Sha256};

pub enum Action {
    FunctionCall {
        method_name: String,
        args: Vec<u8>,
        gas: u64,
        deposit: u128,
    },
    Transfer {
        deposit: u128,
    },
}

/// A NEAR `Transaction` (V0 layout, which carries no version tag).
pub struct Transaction {
    pub signer_id: String,
    pub public_key: [u8; 32],
    pub nonce: u64,
    pub receiver_id: String,
    pub block_hash: [u8; 32],
    pub actions: Vec<Action>,
}

pub struct SignedTransaction {
    /// Borsh-encoded `SignedTransaction`.
    pub bytes: Vec<u8>,
    /// sha256 of the borsh-encoded `Transaction`: the transaction hash.
    pub hash: [u8; 32],
}

// Borsh enum tags.
const KEY_TYPE_ED25519: u8 = 0;
const ACTION_FUNCTION_CALL: u8 = 2;
const ACTION_TRANSFER: u8 = 3;

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

impl Transaction {
    pub fn to_borsh(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, self.signer_id.as_bytes());
        out.push(KEY_TYPE_ED25519);
        out.extend_from_slice(&self.public_key);
        out.extend_from_slice(&self.nonce.to_le_bytes());
        put_bytes(&mut out, self.receiver_id.as_bytes());
        out.extend_from_slice(&self.block_hash);
        out.extend_from_slice(&(self.actions.len() as u32).to_le_bytes());
        for action in &self.actions {
            match action {
                Action::FunctionCall {
                    method_name,
                    args,
                    gas,
                    deposit,
                } => {
                    out.push(ACTION_FUNCTION_CALL);
                    put_bytes(&mut out, method_name.as_bytes());
                    put_bytes(&mut out, args);
                    out.extend_from_slice(&gas.to_le_bytes());
                    out.extend_from_slice(&deposit.to_le_bytes());
                }
                Action::Transfer { deposit } => {
                    out.push(ACTION_TRANSFER);
                    out.extend_from_slice(&deposit.to_le_bytes());
                }
            }
        }
        out
    }

    pub fn sign(&self, key: &SigningKey) -> SignedTransaction {
        let mut bytes = self.to_borsh();
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        let signature = key.sign(&hash);
        bytes.push(KEY_TYPE_ED25519);
        bytes.extend_from_slice(&signature);
        SignedTransaction { bytes, hash }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{UnparsedPublicKey, ED25519};

    fn key() -> SigningKey {
        SigningKey::from_seed(&[7u8; 32]).unwrap()
    }

    #[test]
    fn transfer_encodes_as_borsh() {
        let key = key();
        let tx = Transaction {
            signer_id: "sponsor.testnet".into(),
            public_key: key.public_key(),
            nonce: 5,
            receiver_id: "ab".into(),
            block_hash: [9u8; 32],
            actions: vec![Action::Transfer { deposit: 1 }],
        };

        let mut expected = vec![15, 0, 0, 0];
        expected.extend_from_slice(b"sponsor.testnet");
        expected.push(0);
        expected.extend_from_slice(&key.public_key());
        expected.extend_from_slice(&[5, 0, 0, 0, 0, 0, 0, 0]);
        expected.extend_from_slice(&[2, 0, 0, 0, b'a', b'b']);
        expected.extend_from_slice(&[9u8; 32]);
        expected.extend_from_slice(&[1, 0, 0, 0]);
        expected.push(3);
        expected.push(1);
        expected.extend_from_slice(&[0u8; 15]);

        assert_eq!(tx.to_borsh(), expected);
    }

    #[test]
    fn function_call_encodes_as_borsh() {
        let tx = Transaction {
            signer_id: "a".into(),
            public_key: [1u8; 32],
            nonce: 0x0102,
            receiver_id: "c".into(),
            block_hash: [0u8; 32],
            actions: vec![Action::FunctionCall {
                method_name: "m".into(),
                args: b"{}".to_vec(),
                gas: 300_000_000_000_000,
                deposit: 5_000_000_000_000_000_000_000,
            }],
        };
        let borsh = tx.to_borsh();
        // signer(4+1) key(1+32) nonce(8) receiver(4+1) hash(32) count(4)
        let action = &borsh[5 + 33 + 8 + 5 + 32 + 4..];
        assert_eq!(&borsh[38..46], &[2, 1, 0, 0, 0, 0, 0, 0]);

        let mut expected = vec![2, 1, 0, 0, 0, b'm', 2, 0, 0, 0, b'{', b'}'];
        expected.extend_from_slice(&300_000_000_000_000u64.to_le_bytes());
        expected.extend_from_slice(&5_000_000_000_000_000_000_000u128.to_le_bytes());
        assert_eq!(action, &expected[..]);
    }

    #[test]
    fn signature_covers_the_transaction_hash() {
        let key = key();
        let tx = Transaction {
            signer_id: key.implicit_account_id(),
            public_key: key.public_key(),
            nonce: 1,
            receiver_id: "agent.testnet".into(),
            block_hash: [3u8; 32],
            actions: vec![Action::Transfer { deposit: 10 }],
        };
        let borsh = tx.to_borsh();
        let signed = tx.sign(&key);

        let hash: [u8; 32] = Sha256::digest(&borsh).into();
        assert_eq!(signed.hash, hash);
        assert_eq!(&signed.bytes[..borsh.len()], &borsh[..]);
        assert_eq!(signed.bytes.len(), borsh.len() + 1 + 64);
        assert_eq!(signed.bytes[borsh.len()], 0);
        UnparsedPublicKey::new(&ED25519, key.public_key())
            .verify(&hash, &signed.bytes[borsh.len() + 1..])
            .unwrap();
    }
}
