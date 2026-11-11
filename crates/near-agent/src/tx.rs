//! Borsh encoding and signing of the NEAR transactions the bot sends: a sponsor
//! transfer that creates the agent account, and the agent's `register_agent` call,
//! which the agent signs as a NEP-366 delegate action and the sponsor relays.

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
    /// NEP-366 meta-transaction: actions signed by `sender_id` and relayed by this
    /// transaction's signer, who pays their gas and deposits.
    Delegate(Box<SignedDelegateAction>),
}

/// Actions an account authorizes another account to submit (and pay for) on its behalf.
/// The relaying transaction's `receiver_id` must be `sender_id`.
pub struct DelegateAction {
    pub sender_id: String,
    pub receiver_id: String,
    /// Must not contain another `Delegate`; NEAR rejects nesting.
    pub actions: Vec<Action>,
    /// Next nonce of `sender_id`'s access key `public_key`.
    pub nonce: u64,
    /// Last block height at which the action can be included.
    pub max_block_height: u64,
    pub public_key: [u8; 32],
}

pub struct SignedDelegateAction {
    pub delegate_action: DelegateAction,
    pub signature: [u8; 64],
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
const ACTION_DELEGATE: u8 = 8;

/// NEP-461 prefix signed in front of a delegate action (2^30 + NEP number 366), so its
/// signature can never be mistaken for a transaction signature.
const DELEGATE_ACTION_PREFIX: u32 = (1 << 30) + 366;

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn put_public_key(out: &mut Vec<u8>, key: &[u8; 32]) {
    out.push(KEY_TYPE_ED25519);
    out.extend_from_slice(key);
}

fn put_actions(out: &mut Vec<u8>, actions: &[Action]) {
    out.extend_from_slice(&(actions.len() as u32).to_le_bytes());
    for action in actions {
        match action {
            Action::FunctionCall {
                method_name,
                args,
                gas,
                deposit,
            } => {
                out.push(ACTION_FUNCTION_CALL);
                put_bytes(out, method_name.as_bytes());
                put_bytes(out, args);
                out.extend_from_slice(&gas.to_le_bytes());
                out.extend_from_slice(&deposit.to_le_bytes());
            }
            Action::Transfer { deposit } => {
                out.push(ACTION_TRANSFER);
                out.extend_from_slice(&deposit.to_le_bytes());
            }
            Action::Delegate(signed) => {
                out.push(ACTION_DELEGATE);
                out.extend(signed.delegate_action.to_borsh());
                out.push(KEY_TYPE_ED25519);
                out.extend_from_slice(&signed.signature);
            }
        }
    }
}

impl DelegateAction {
    pub fn to_borsh(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, self.sender_id.as_bytes());
        put_bytes(&mut out, self.receiver_id.as_bytes());
        put_actions(&mut out, &self.actions);
        out.extend_from_slice(&self.nonce.to_le_bytes());
        out.extend_from_slice(&self.max_block_height.to_le_bytes());
        put_public_key(&mut out, &self.public_key);
        out
    }

    /// The hash the sender signs: sha256 of the NEP-461 prefix and the borsh encoding.
    pub fn signing_hash(&self) -> [u8; 32] {
        let mut message = DELEGATE_ACTION_PREFIX.to_le_bytes().to_vec();
        message.extend(self.to_borsh());
        Sha256::digest(&message).into()
    }

    pub fn sign(self, key: &SigningKey) -> SignedDelegateAction {
        let signature = key.sign(&self.signing_hash());
        SignedDelegateAction {
            delegate_action: self,
            signature,
        }
    }
}

impl Transaction {
    pub fn to_borsh(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, self.signer_id.as_bytes());
        put_public_key(&mut out, &self.public_key);
        out.extend_from_slice(&self.nonce.to_le_bytes());
        put_bytes(&mut out, self.receiver_id.as_bytes());
        out.extend_from_slice(&self.block_hash);
        put_actions(&mut out, &self.actions);
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

    // A meta-transaction NEAR testnet accepted: the sponsor relayed this delegate action
    // (a 1-yoctoNEAR transfer back to it) in tx 4Q6ShtCusGbfdYPeSj6GwC46W3KJNgcGf5DNVrV8DvJW.
    // ed25519 is deterministic, so matching these bytes proves the encoding and the
    // NEP-461 signing hash.
    const VECTOR_SEED: &str = "28beb82c3a31a9028e7b6d000d23c203e5741df5e8183a36a3f23504c57acf21";
    const VECTOR_SENDER: &str = "2c435c32903ba05b57fde501900c2d138a19b015240b270f9da6644256a5a54f";
    const VECTOR_HASH: &str = "b074be4909a284af1422bf36c249cfcb6570d6f31bedccb925789996a41a447d";
    const VECTOR_SIGNED_DELEGATE: &str = concat!(
        "4000000032633433356333323930336261303562353766646535303139303063",
        "3264313338613139623031353234306232373066396461363634343235366135",
        "6135346618000000736967737461636b2d73706f6e736f722e746573746e6574",
        "01000000030100000000000000000000000000000081e3a3652cfc00000ac586",
        "1000000000002c435c32903ba05b57fde501900c2d138a19b015240b270f9da6",
        "644256a5a54f007ccc53302a043011c8841a1f136cb0a554dbb6135e4a6f47e1",
        "d2a61a30ed01a95585c4ea817e7be2176aff4439be7521e7e19178435fdd592b",
        "868b58cd03420b",
    );

    fn vector_delegate(key: &SigningKey) -> DelegateAction {
        DelegateAction {
            sender_id: key.implicit_account_id(),
            receiver_id: "sigstack-sponsor.testnet".into(),
            actions: vec![Action::Transfer { deposit: 1 }],
            nonce: 277_267_614_000_001,
            max_block_height: 277_267_722,
            public_key: key.public_key(),
        }
    }

    #[test]
    fn delegate_action_matches_a_relayed_testnet_vector() {
        let seed: [u8; 32] = hex::decode(VECTOR_SEED).unwrap().try_into().unwrap();
        let key = SigningKey::from_seed(&seed).unwrap();
        assert_eq!(key.implicit_account_id(), VECTOR_SENDER);

        let delegate = vector_delegate(&key);
        assert_eq!(hex::encode(delegate.signing_hash()), VECTOR_HASH);

        let expected = hex::decode(VECTOR_SIGNED_DELEGATE).unwrap();
        let unsigned_len = expected.len() - 1 - 64;
        assert_eq!(delegate.to_borsh(), &expected[..unsigned_len]);

        // As a relayed action: the Delegate tag, the delegate action, then its signature.
        let relay = Transaction {
            signer_id: "sigstack-sponsor.testnet".into(),
            public_key: [1u8; 32],
            nonce: 1,
            receiver_id: VECTOR_SENDER.into(),
            block_hash: [0u8; 32],
            actions: vec![Action::Delegate(Box::new(delegate.sign(&key)))],
        };
        let borsh = relay.to_borsh();
        let action = &borsh[borsh.len() - expected.len() - 1..];
        assert_eq!(action[0], 8);
        assert_eq!(&action[1..], &expected[..]);
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
