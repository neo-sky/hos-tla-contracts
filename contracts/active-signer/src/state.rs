use defuse_wallet::signature::ed25519::Ed25519PublicKey;
use defuse_wallet::Nonces;
use near_sdk::near;

#[near(serializers = [borsh])]
pub struct SignerEntry {
    pub public_key: Ed25519PublicKey,
    pub nonces: Nonces,
    pub last_signed_at: u64,
    pub frozen: bool,
}
