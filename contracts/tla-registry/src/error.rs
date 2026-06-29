use near_sdk::serde::Serialize;
use near_sdk::FunctionError;

#[derive(Debug, Serialize)]
#[serde(crate = "near_sdk::serde", tag = "code", rename_all = "snake_case")]
pub enum ContractError {
    OnlyAdmin,
    OnlyLicensee,
    OnlyOwner,
    Paused,
    NoPendingRefund,
    TlaNotFound,
    TlaAlreadyRegistered,
    TlaNotInRegisteredState,
    TlaNotActive,
    TlaNotSuspended,
    TlaNotAcceptingRentals,
    TlaPastGracePeriod,
    BusinessTlaRequiresLicensee,
    BusinessTlaMissingLicensee,
    WrongActivationEndpoint,
    SubAccountNotFound,
    SubAccountNameTaken,
    SubAccountPastGracePeriod,
    SubAccountNotReclaimable,
    SignerNotPending,
    MainWalletEqualsSubAccount,
    InvalidSubAccountId,
    InvalidName { reason: NameInvalidReason },
    InsufficientPayment,
    InsufficientRevenue,
    WithdrawalAmountZero,
    TokenNotInAllowlist,
    AllowlistFull,
    AllRentTiersZero,
    CreationDepositZero,
    CannotRemoveLastAdmin,
    AlreadyAtCurrentVersion,
    SubAccountIsMother,
    MotherIsReclaimable,
    MotherNotSet,
    MaxBusinessSubsReached,
    NoRetractionScheduled,
    RetractionAlreadyScheduled,
    RetractionAlreadyElapsed,
    RetractionPending,
    NotBusinessTla,
    RequiresOneYocto,
    InsufficientContractBalance,
    NotListed,
    SaleInProgress,
    ReclaimInProgress,
    SubAccountNotSellable,
    BusinessSubNotResellable,
    PriceNotMet,
    InvalidPrice,
    NoAcceptedOffer,
    InvalidCommissionRate,
    NotEd25519,
}

#[derive(Debug, Serialize)]
#[serde(crate = "near_sdk::serde", rename_all = "snake_case")]
pub enum NameInvalidReason {
    LengthOutOfBounds,
    DisallowedCharacter,
    EdgeSeparator,
}

impl FunctionError for ContractError {
    fn panic(&self) -> ! {
        hos_common::panic_json(self)
    }
}
