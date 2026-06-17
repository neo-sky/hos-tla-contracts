use near_sdk::{near, AccountId};

#[near(event_json(standard = "hos_tla_manager"))]
pub enum Event {
    #[event_version("1.0.0")]
    SubAccountMinted { account: AccountId, owner: String },
    #[event_version("1.0.0")]
    MintFailed { account: AccountId },
    #[event_version("1.0.0")]
    SignerInstallFailed { account: AccountId },
}
