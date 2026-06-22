use near_sdk::json_types::U128;
use near_sdk::{near, AccountId, PublicKey};

#[near(event_json(standard = "hos_tla_registry"))]
pub enum Event {
    #[event_version("1.0.0")]
    TlaRegistered {
        tla_id: AccountId,
        tla_type: String,
        premium_category: String,
        licensee: Option<AccountId>,
    },
    #[event_version("1.0.0")]
    TlaActivated {
        tla_id: AccountId,
        expires_at: u64,
        paid_yocto: U128,
    },
    #[event_version("1.0.0")]
    TlaSuspended {
        tla_id: AccountId,
        action: String,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    TlaUnsuspended {
        tla_id: AccountId,
        action: String,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    TlaRenewed {
        tla_id: AccountId,
        new_expires_at: u64,
        paid_yocto: U128,
    },
    #[event_version("1.0.0")]
    ContractPaused { by: AccountId },
    #[event_version("1.0.0")]
    ContractUnpaused { by: AccountId },
    #[event_version("1.0.0")]
    FeeConfigUpdated { by: AccountId },
    #[event_version("1.0.0")]
    AdminAdded {
        action: String,
        account: AccountId,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    AdminRemoved {
        action: String,
        account: AccountId,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    WithdrawalQueued {
        amount_yocto: U128,
        recipient: AccountId,
    },
    #[event_version("1.0.0")]
    FtAllowlistAdded {
        kind: String,
        token: AccountId,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    FtAllowlistRemoved {
        kind: String,
        token: AccountId,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    HosExtensionUpdated {
        hos_extension: AccountId,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    ParkedSignerUpdated { pubkey: PublicKey, by: AccountId },
    #[event_version("1.0.0")]
    BusinessSubCapSet {
        tla_id: AccountId,
        cap: String,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    SubAccountRented {
        full_name: String,
        tla_id: AccountId,
        owner: AccountId,
        rent_yocto: U128,
        expires_at: u64,
    },
    #[event_version("1.0.0")]
    SubAccountReRented {
        full_name: String,
        tla_id: AccountId,
        owner: AccountId,
        rent_yocto: U128,
        expires_at: u64,
    },
    #[event_version("1.0.0")]
    SubAccountRenewed {
        full_name: String,
        new_expires_at: u64,
        paid_yocto: U128,
    },
    #[event_version("1.0.0")]
    MainWalletUpdated {
        full_name: String,
        new_wallet: AccountId,
    },
    #[event_version("1.0.0")]
    RefundPending {
        account: AccountId,
        amount_yocto: U128,
        reason: String,
    },
    #[event_version("1.0.0")]
    SubAccountListed {
        full_name: String,
        price_yocto: U128,
        seller: AccountId,
    },
    #[event_version("1.0.0")]
    SubAccountUnlisted { full_name: String, by: AccountId },
    #[event_version("1.0.0")]
    OfferAccepted {
        full_name: String,
        buyer: AccountId,
        price_yocto: U128,
        seller: AccountId,
    },
    #[event_version("1.0.0")]
    OfferRevoked { full_name: String, by: AccountId },
    #[event_version("1.0.0")]
    SubAccountSold {
        full_name: String,
        tla_id: AccountId,
        seller: AccountId,
        buyer: AccountId,
        price_yocto: U128,
        commission_yocto: U128,
        seller_proceeds_yocto: U128,
    },
    #[event_version("1.0.0")]
    SubAccountSaleFailed { full_name: String, buyer: AccountId },
    #[event_version("1.0.0")]
    SubAccountSaleBlocked {
        full_name: String,
        token: String,
        reason: String,
    },
    #[event_version("1.0.0")]
    SubAccountReclaimed {
        full_name: String,
        tla_id: AccountId,
        swept_to: AccountId,
    },
    #[event_version("1.0.0")]
    ReclaimFinalizeBlocked {
        full_name: String,
        token: String,
        reason: String,
    },
    #[event_version("1.0.0")]
    MotherSet {
        user: AccountId,
        mother: AccountId,
        source: String,
    },
    #[event_version("1.0.0")]
    MotherCleared {
        user: AccountId,
        previous_mother: AccountId,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    SubAccountRetractionScheduled {
        full_name: String,
        retraction_at: u64,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    SubAccountRetractionCanceled { full_name: String, by: AccountId },
    #[event_version("1.0.0")]
    SubAccountSignerPending { full_name: String, owner: AccountId },
    #[event_version("1.0.0")]
    SignerInstalled { full_name: String },
    #[event_version("1.0.0")]
    SettlingCleared { full_name: String, by: AccountId },
}
