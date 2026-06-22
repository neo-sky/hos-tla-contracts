use near_sdk::{near, AccountId, PublicKey};

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub enum Target {
    Native {
        mpc_public_key: PublicKey,
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

impl Phase {
    pub fn pending(&self) -> Option<(&PublicKey, u64)> {
        match self {
            Phase::Requested {
                new_owner, round, ..
            }
            | Phase::Approving { new_owner, round }
            | Phase::Approved { new_owner, round }
            | Phase::Resolving { new_owner, round } => Some((new_owner, *round)),
            Phase::Idle => None,
        }
    }

    pub fn resolving_owner(&self, round: u64) -> Option<PublicKey> {
        match self {
            Phase::Resolving {
                new_owner,
                round: resolving_round,
            } if *resolving_round == round => Some(new_owner.clone()),
            _ => None,
        }
    }
}

#[near(serializers = [borsh])]
pub struct Account {
    pub policy: Policy,
    pub round: u64,
    pub phase: Phase,
}
