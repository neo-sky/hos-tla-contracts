mod admin;
mod asset_gate;
mod business;
mod callbacks;
mod error;
mod events;
mod fees;
mod interfaces;
mod marketplace;
mod mother;
mod reclaim;
mod rental;
#[cfg(test)]
mod tests;
mod types;
mod views;

use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::{U128, U64};
use near_sdk::store::{IterableMap, IterableSet, LookupMap};
use near_sdk::{
    env, is_promise_success, near, require, AccountId, BorshStorageKey, Gas, NearToken,
    PanicOnDefault, Promise, PublicKey,
};

const CONTRACT_VERSION: u8 = 1;
const MIN_GRACE_PERIOD_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

const GAS_FOR_CLAIM_REFUND_CB: Gas = Gas::from_tgas(10);

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Tlas,
    SubAccounts,
    Admins,
    PendingRefunds,
    FtAllowlist,
    Mothers,
    MotherUseCount,
    BusinessSubCount,
    BusinessSubCapOverride,
    Listings,
    AcceptedOffers,
    ParkedNames,
    SignerPending,
    ReclaimPending,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct TlaRegistry {
    pub(crate) tlas: IterableMap<AccountId, TlaEntry>,
    pub(crate) sub_accounts: LookupMap<String, SubAccountEntry>,
    pub(crate) admins: IterableSet<AccountId>,
    pub(crate) fee_config: FeeConfig,
    pub(crate) total_revenue: u128,
    pub(crate) sub_account_count: u64,
    pub(crate) paused: bool,
    pub(crate) version: u8,
    pub(crate) pending_refunds: LookupMap<AccountId, u128>,
    pub(crate) total_pending_refunds: u128,
    pub(crate) ft_allowlist: IterableSet<AccountId>,
    pub(crate) mothers: LookupMap<AccountId, AccountId>,
    pub(crate) mother_use_count: LookupMap<AccountId, u32>,
    pub(crate) business_sub_count: LookupMap<AccountId, u32>,
    pub(crate) business_sub_cap_override: LookupMap<AccountId, u32>,
    pub(crate) listings: LookupMap<String, Listing>,
    pub(crate) accepted_offers: LookupMap<String, AcceptedOffer>,
    pub(crate) parked_names: LookupMap<String, ParkedEntry>,
    pub(crate) signer_pending: LookupMap<String, PublicKey>,
    pub(crate) reclaim_pending: LookupMap<String, bool>,
    pub(crate) hos_extension: AccountId,
    pub(crate) active_signer: AccountId,
    pub(crate) parked_signer_pubkey: PublicKey,
    pub(crate) grace_period_ns: u64,
}

#[near]
impl TlaRegistry {
    #[init]
    pub fn new(
        admin: AccountId,
        hos_extension: AccountId,
        active_signer: AccountId,
        parked_signer_pubkey: PublicKey,
        grace_period_ns: U64,
    ) -> Self {
        require!(
            hos_common::is_ed25519(&parked_signer_pubkey),
            "parked signer key must be ed25519"
        );
        require!(
            grace_period_ns.0 >= MIN_GRACE_PERIOD_NS,
            "grace period too short"
        );
        let mut admins = IterableSet::new(StorageKey::Admins);
        admins.insert(admin);

        Self {
            tlas: IterableMap::new(StorageKey::Tlas),
            sub_accounts: LookupMap::new(StorageKey::SubAccounts),
            admins,
            fee_config: fees::default_fee_config(),
            total_revenue: 0,
            sub_account_count: 0,
            paused: false,
            version: CONTRACT_VERSION,
            pending_refunds: LookupMap::new(StorageKey::PendingRefunds),
            total_pending_refunds: 0,
            ft_allowlist: IterableSet::new(StorageKey::FtAllowlist),
            mothers: LookupMap::new(StorageKey::Mothers),
            mother_use_count: LookupMap::new(StorageKey::MotherUseCount),
            business_sub_count: LookupMap::new(StorageKey::BusinessSubCount),
            business_sub_cap_override: LookupMap::new(StorageKey::BusinessSubCapOverride),
            listings: LookupMap::new(StorageKey::Listings),
            accepted_offers: LookupMap::new(StorageKey::AcceptedOffers),
            parked_names: LookupMap::new(StorageKey::ParkedNames),
            signer_pending: LookupMap::new(StorageKey::SignerPending),
            reclaim_pending: LookupMap::new(StorageKey::ReclaimPending),
            hos_extension,
            active_signer,
            parked_signer_pubkey,
            grace_period_ns: grace_period_ns.0,
        }
    }

    #[handle_result]
    pub fn pause(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.paused = true;
        Event::ContractPaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn unpause(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.paused = false;
        Event::ContractUnpaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn claim_refund(&mut self) -> Result<Promise, ContractError> {
        let caller = env::predecessor_account_id();
        let amount = self.pending_refunds.get(&caller).copied().unwrap_or(0);
        if amount == 0 {
            return Err(ContractError::NoPendingRefund);
        }
        if amount > self.available_balance() {
            return Err(ContractError::InsufficientContractBalance);
        }
        self.pending_refunds.remove(&caller);
        self.total_pending_refunds = self.total_pending_refunds.saturating_sub(amount);
        Ok(Promise::new(caller.clone())
            .transfer(NearToken::from_yoctonear(amount))
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_CLAIM_REFUND_CB)
                    .on_claim_refund_settled(caller, U128(amount)),
            ))
    }

    #[private]
    pub fn on_claim_refund_settled(&mut self, caller: AccountId, amount: U128) {
        if is_promise_success() {
            return;
        }
        self.add_pending_refund(&caller, amount.0);
        Event::RefundPending {
            account: caller,
            amount_yocto: amount,
            reason: "transfer_failed".to_string(),
        }
        .emit();
    }

    #[handle_result]
    pub fn migrate(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        if self.version >= CONTRACT_VERSION {
            return Err(ContractError::AlreadyAtCurrentVersion);
        }
        self.version = CONTRACT_VERSION;
        Ok(())
    }

    pub fn get_version(&self) -> u8 {
        self.version
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn get_pending_refund(&self, account_id: AccountId) -> U128 {
        U128(self.pending_refunds.get(&account_id).copied().unwrap_or(0))
    }

    pub fn get_total_pending_refunds(&self) -> U128 {
        U128(self.total_pending_refunds)
    }
}

impl TlaRegistry {
    pub(crate) fn assert_admin(&self) -> Result<(), ContractError> {
        if !self.admins.contains(&env::predecessor_account_id()) {
            return Err(ContractError::OnlyAdmin);
        }
        Ok(())
    }

    pub(crate) fn assert_not_paused(&self) -> Result<(), ContractError> {
        if self.paused {
            return Err(ContractError::Paused);
        }
        Ok(())
    }

    pub(crate) fn add_pending_refund(&mut self, account: &AccountId, amount: u128) {
        let existing = self.pending_refunds.get(account).copied().unwrap_or(0);
        self.pending_refunds
            .insert(account.clone(), existing.saturating_add(amount));
        self.total_pending_refunds = self.total_pending_refunds.saturating_add(amount);
    }

    pub(crate) fn refund_excess(&mut self, payer: &AccountId, attached: u128, charged: u128) {
        let excess = attached.saturating_sub(charged);
        if excess > 0 {
            self.add_pending_refund(payer, excess);
        }
    }

    pub(crate) fn available_balance(&self) -> u128 {
        let total = env::account_balance().as_yoctonear();
        let reserve = env::storage_byte_cost()
            .as_yoctonear()
            .saturating_mul(env::storage_usage() as u128);
        total.saturating_sub(reserve)
    }
}

pub(crate) fn assert_one_yocto() -> Result<(), ContractError> {
    if env::attached_deposit() != NearToken::from_yoctonear(1) {
        return Err(ContractError::RequiresOneYocto);
    }
    Ok(())
}
