use defuse_wallet::signature::ed25519::Ed25519PublicKey;
use defuse_wallet::Nonces;
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::near;

#[derive(BorshSerialize, BorshDeserialize, PartialEq, Eq, Clone, Copy)]
#[borsh(crate = "near_sdk::borsh")]
pub enum FreezeState {
    Unfrozen,
    SelfFrozen,
    RecoveryFrozen,
}

#[near(serializers = [borsh])]
pub struct SignerEntry {
    pub public_key: Ed25519PublicKey,
    pub nonces: Nonces,
    pub freeze_nonces: Nonces,
    pub last_signed_at: u64,
    pub frozen: FreezeState,
}
