use near_sdk::{near, AccountId};

#[near(event_json(standard = "hos_tla_recovery"))]
pub enum Event {
    #[event_version("1.0.0")]
    PolicyInstalled { account: AccountId },
    #[event_version("1.0.0")]
    Requested { account: AccountId, round: u64 },
    #[event_version("1.0.0")]
    Approved { account: AccountId, round: u64 },
    #[event_version("1.0.0")]
    Cancelled { account: AccountId, round: u64 },
    #[event_version("1.0.0")]
    Finalized { account: AccountId, round: u64 },
    #[event_version("1.0.0")]
    Aborted { account: AccountId, round: u64 },
    #[event_version("1.0.0")]
    Voided { account: AccountId, round: u64 },
    #[event_version("1.0.0")]
    PolicyReset { account: AccountId },
}
