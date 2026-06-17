use near_sdk::{near, AccountId};

#[near(event_json(standard = "hos_tla_active_signer"))]
pub enum Event {
    #[event_version("1.0.0")]
    SignerInstalled { wallet: AccountId },
    #[event_version("1.0.0")]
    RequestExecuted { wallet: AccountId, nonce: u32 },
    #[event_version("1.0.0")]
    OwnerSwapped { wallet: AccountId, by: String },
    #[event_version("1.0.0")]
    OwnerSwapVoided { wallet: AccountId },
    #[event_version("1.0.0")]
    Frozen { wallet: AccountId },
    #[event_version("1.0.0")]
    Unfrozen { wallet: AccountId },
    #[event_version("1.0.0")]
    MinterAdded { minter: AccountId },
    #[event_version("1.0.0")]
    MinterRemoved { minter: AccountId },
    #[event_version("1.0.0")]
    AdminAdded { admin: AccountId },
    #[event_version("1.0.0")]
    AdminRemoved { admin: AccountId },
}
