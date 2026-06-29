use near_sdk::json_types::U128;
use near_sdk::{near, AccountId, PublicKey};

#[near(event_json(standard = "hos_tla_extension"))]
pub enum Event {
    #[event_version("1.0.0")]
    ContractPaused { by: AccountId },
    #[event_version("1.0.0")]
    ContractUnpaused { by: AccountId },
    #[event_version("1.0.0")]
    AdminAdded { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    AdminRemoved { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    ForceTransferRequested {
        wallet: AccountId,
        new_public_key: PublicKey,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    ForceTransferCompleted { wallet: AccountId },
    #[event_version("1.0.0")]
    ForceTransferVoided { wallet: AccountId },
    #[event_version("1.0.0")]
    BalanceSkimmed {
        amount: U128,
        to: AccountId,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    SweepRequested {
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    SweepDispatched {
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    SweepSkipped {
        wallet: AccountId,
        ft: AccountId,
        reason: String,
    },
    #[event_version("1.0.0")]
    SweepFailed {
        wallet: AccountId,
        ft: AccountId,
        reason: String,
    },
}
