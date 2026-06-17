use near_sdk::{near, AccountId, PublicKey};

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub enum Target {
    Native {
        mpc_public_key: PublicKey,
        derivation_path: String,
    },
    Wallet {
        active_signer: AccountId,
        bound_owner: PublicKey,
    },
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub struct Policy {
    pub target: Target,
    pub attestation_key: PublicKey,
    pub timelock_secs: u32,
}

#[near(serializers = [borsh])]
pub enum Phase {
    Idle,
    Requested {
        new_owner: PublicKey,
        round: u64,
        requested_at: u64,
    },
    Approving {
        new_owner: PublicKey,
        round: u64,
    },
    Approved {
        new_owner: PublicKey,
        round: u64,
    },
    Resolving {
        new_owner: PublicKey,
        round: u64,
    },
}

#[near(serializers = [borsh])]
pub struct Account {
    pub policy: Policy,
    pub round: u64,
    pub phase: Phase,
}
