use crate::error::{ContractError, NameInvalidReason};
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::json_types::{U128, U64};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, AccountId, PublicKey};

pub const ONE_NEAR: u128 = 1_000_000_000_000_000_000_000_000;
pub const ONE_YEAR_NS: u64 = 365 * 24 * 60 * 60 * 1_000_000_000;

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, PartialEq)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub enum TlaType {
    Business,
    Open,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, PartialEq)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub enum TlaStatus {
    Registered,
    Active,
    Suspended,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone, PartialEq)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub enum PremiumCategory {
    Legendary,
    Premium,
    Standard,
    Community,
}

impl PremiumCategory {
    pub fn multiplier(&self) -> (u128, u128) {
        match self {
            Self::Legendary => (5, 1),
            Self::Premium => (3, 1),
            Self::Standard => (3, 2),
            Self::Community => (0, 1),
        }
    }
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct TlaEntry {
    pub tla_type: TlaType,
    pub status: TlaStatus,
    pub licensee: Option<AccountId>,
    pub premium_category: PremiumCategory,
    pub activated_at: u64,
    pub expires_at: u64,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct SubAccountEntry {
    pub owner: AccountId,
    pub tla_id: AccountId,
    pub main_wallet: AccountId,
    pub rented_at: u64,
    pub expires_at: u64,
    pub retraction_at: Option<u64>,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct Listing {
    pub price: u128,
    pub settling: bool,
    pub owner_key: PublicKey,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct ParkedEntry {
    pub tla_id: AccountId,
    pub parked_at: u64,
}

#[derive(BorshDeserialize, BorshSerialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
pub struct AcceptedOffer {
    pub buyer: AccountId,
    pub price: u128,
    pub settling: bool,
    pub owner_key: PublicKey,
}

#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[borsh(crate = "near_sdk::borsh")]
#[serde(crate = "near_sdk::serde")]
pub struct FeeConfig {
    pub tla_allocation_fee: U128,
    pub rent_tier_5: U128,
    pub rent_tier_8: U128,
    pub rent_tier_10: U128,
    pub rent_tier_12plus: U128,
    pub sub_fee_per_account: U128,
    pub account_creation_deposit: U128,
    pub business_max_subs: u32,
    pub retraction_notice_ns: U64,
    pub resale_commission_bps: u16,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub enum LifecycleStatus {
    Registered,
    Active,
    Grace,
    Reclaimable,
    Suspended,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct TlaView {
    pub tla_id: AccountId,
    pub tla_type: TlaType,
    pub lifecycle: LifecycleStatus,
    pub licensee: Option<AccountId>,
    pub premium_category: PremiumCategory,
    pub activated_at: U128,
    pub expires_at: U128,
    pub annual_rent: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct SubAccountView {
    pub full_name: String,
    pub owner: AccountId,
    pub tla_id: AccountId,
    pub main_wallet: AccountId,
    pub lifecycle: LifecycleStatus,
    pub rented_at: U128,
    pub expires_at: U128,
    pub annual_rent: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct ListingView {
    pub full_name: String,
    pub price_yocto: U128,
    pub settling: bool,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct AcceptedOfferView {
    pub full_name: String,
    pub buyer: AccountId,
    pub price_yocto: U128,
    pub settling: bool,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct RentPriceView {
    pub rent_yocto: U128,
    pub creation_deposit_yocto: U128,
    pub total_yocto: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct RegistryStats {
    pub tla_count: u64,
    pub sub_account_count: u64,
    pub total_revenue_yocto: U128,
    pub total_pending_refunds_yocto: U128,
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct BusinessRenewalCostView {
    pub tla_id: AccountId,
    pub tla_rent_yocto: U128,
    pub per_sub_yocto: U128,
    pub sub_count: u32,
    pub total_yocto: U128,
}

fn time_lifecycle(expires_at: u64, grace_period_ns: u64) -> LifecycleStatus {
    let now = env::block_timestamp();
    let grace_end = expires_at.saturating_add(grace_period_ns);
    if now < expires_at {
        LifecycleStatus::Active
    } else if now < grace_end {
        LifecycleStatus::Grace
    } else {
        LifecycleStatus::Reclaimable
    }
}

impl TlaEntry {
    pub fn lifecycle(&self, grace_period_ns: u64) -> LifecycleStatus {
        match self.status {
            TlaStatus::Registered => LifecycleStatus::Registered,
            TlaStatus::Suspended => LifecycleStatus::Suspended,
            TlaStatus::Active => time_lifecycle(self.expires_at, grace_period_ns),
        }
    }

    pub fn is_accepting_rentals(&self) -> bool {
        self.status == TlaStatus::Active && env::block_timestamp() < self.expires_at
    }
}

impl SubAccountEntry {
    pub fn lifecycle(&self, grace_period_ns: u64) -> LifecycleStatus {
        time_lifecycle(self.expires_at, grace_period_ns)
    }
}

pub fn validate_name(name: &str) -> Result<(), ContractError> {
    if name.is_empty() || name.len() > 60 {
        return Err(ContractError::InvalidName {
            reason: NameInvalidReason::LengthOutOfBounds,
        });
    }
    for c in name.bytes() {
        if !matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_') {
            return Err(ContractError::InvalidName {
                reason: NameInvalidReason::DisallowedCharacter,
            });
        }
    }
    if name.starts_with('-') || name.starts_with('_') || name.ends_with('-') || name.ends_with('_')
    {
        return Err(ContractError::InvalidName {
            reason: NameInvalidReason::EdgeSeparator,
        });
    }
    Ok(())
}

pub fn sub_account_key(tla_id: &AccountId, name: &str) -> String {
    format!("{}.{}", name, tla_id)
}

pub fn total_name_length(tla_id: &AccountId, name: &str) -> u8 {
    (name.len() + 1 + tla_id.as_str().len()) as u8
}
