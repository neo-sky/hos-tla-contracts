use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::{env, near, AccountId, CurveType, PublicKey};

const MAX_ALLOWLIST_SIZE: u32 = 40;

#[near]
impl TlaRegistry {
    #[handle_result]
    pub fn register_tla(
        &mut self,
        tla_id: AccountId,
        tla_type: TlaType,
        premium_category: PremiumCategory,
        licensee: Option<AccountId>,
    ) -> Result<(), ContractError> {
        self.assert_admin()?;
        if self.tlas.contains_key(&tla_id) {
            return Err(ContractError::TlaAlreadyRegistered);
        }
        if tla_type == TlaType::Business && licensee.is_none() {
            return Err(ContractError::BusinessTlaRequiresLicensee);
        }

        let entry = TlaEntry {
            tla_type: tla_type.clone(),
            status: TlaStatus::Registered,
            licensee: licensee.clone(),
            premium_category: premium_category.clone(),
            activated_at: 0,
            expires_at: 0,
        };
        self.tlas.insert(tla_id.clone(), entry);

        let type_str = match tla_type {
            TlaType::Business => "business",
            TlaType::Open => "open",
        }
        .to_string();
        let premium_str = match premium_category {
            PremiumCategory::Legendary => "legendary",
            PremiumCategory::Premium => "premium",
            PremiumCategory::Standard => "standard",
            PremiumCategory::Community => "community",
        }
        .to_string();
        Event::TlaRegistered {
            tla_id,
            tla_type: type_str,
            premium_category: premium_str,
            licensee,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn suspend_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        let entry = self
            .tlas
            .get_mut(&tla_id)
            .ok_or(ContractError::TlaNotFound)?;
        entry.status = TlaStatus::Suspended;
        Event::TlaSuspended {
            tla_id,
            action: "suspended".to_string(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn unsuspend_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        let entry = self
            .tlas
            .get_mut(&tla_id)
            .ok_or(ContractError::TlaNotFound)?;
        if entry.status != TlaStatus::Suspended {
            return Err(ContractError::TlaNotSuspended);
        }
        if entry.tla_type == TlaType::Business && entry.licensee.is_none() {
            return Err(ContractError::BusinessTlaMissingLicensee);
        }
        entry.status = TlaStatus::Active;
        Event::TlaUnsuspended {
            tla_id,
            action: "unsuspended".to_string(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn add_admin(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        if !self.admins.insert(account_id.clone()) {
            return Ok(());
        }
        Event::AdminAdded {
            action: "added".to_string(),
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn remove_admin(&mut self, account_id: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        if self.admins.len() <= 1 {
            return Err(ContractError::CannotRemoveLastAdmin);
        }
        self.admins.remove(&account_id);
        Event::AdminRemoved {
            action: "removed".to_string(),
            account: account_id,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn update_fee_config(&mut self, config: FeeConfig) -> Result<(), ContractError> {
        self.assert_admin()?;
        if config.rent_tier_5.0 == 0
            && config.rent_tier_8.0 == 0
            && config.rent_tier_10.0 == 0
            && config.rent_tier_12plus.0 == 0
        {
            return Err(ContractError::AllRentTiersZero);
        }
        if config.account_creation_deposit.0 == 0 {
            return Err(ContractError::CreationDepositZero);
        }
        if config.resale_commission_bps > 10_000 {
            return Err(ContractError::InvalidCommissionRate);
        }
        self.fee_config = config;
        Event::FeeConfigUpdated {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn set_hos_extension(&mut self, hos_extension: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.hos_extension = hos_extension.clone();
        Event::HosExtensionUpdated {
            hos_extension,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn set_parked_signer_pubkey(&mut self, pubkey: PublicKey) -> Result<(), ContractError> {
        self.assert_admin()?;
        if pubkey.curve_type() != CurveType::ED25519 {
            return Err(ContractError::NotEd25519);
        }
        self.parked_signer_pubkey = pubkey.clone();
        Event::ParkedSignerUpdated {
            pubkey,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn withdraw(&mut self, amount: U128, recipient: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        let amount_yocto = amount.0;
        if amount_yocto == 0 {
            return Err(ContractError::WithdrawalAmountZero);
        }
        if amount_yocto > self.total_revenue {
            return Err(ContractError::InsufficientRevenue);
        }
        if self.total_pending_refunds.saturating_add(amount_yocto) > self.available_balance() {
            return Err(ContractError::InsufficientContractBalance);
        }
        self.total_revenue = self.total_revenue.saturating_sub(amount_yocto);
        self.add_pending_refund(&recipient, amount_yocto);

        Event::WithdrawalQueued {
            amount_yocto: amount,
            recipient,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn add_ft_allowlist(&mut self, token: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        if self.ft_allowlist.contains(&token) {
            return Ok(());
        }
        if self.ft_allowlist.len() >= MAX_ALLOWLIST_SIZE {
            return Err(ContractError::AllowlistFull);
        }
        self.ft_allowlist.insert(token.clone());
        Event::FtAllowlistAdded {
            kind: "ft".to_string(),
            token,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn remove_ft_allowlist(&mut self, token: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        if !self.ft_allowlist.remove(&token) {
            return Ok(());
        }
        Event::FtAllowlistRemoved {
            kind: "ft".to_string(),
            token,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn add_nft_allowlist(&mut self, token: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        if self.nft_allowlist.contains(&token) {
            return Ok(());
        }
        if self.nft_allowlist.len() >= MAX_ALLOWLIST_SIZE {
            return Err(ContractError::AllowlistFull);
        }
        self.nft_allowlist.insert(token.clone());
        Event::NftAllowlistAdded {
            kind: "nft".to_string(),
            token,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn remove_nft_allowlist(&mut self, token: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        if !self.nft_allowlist.remove(&token) {
            return Ok(());
        }
        Event::NftAllowlistRemoved {
            kind: "nft".to_string(),
            token,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn activate_open_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        let now = env::block_timestamp();
        let new_expires_at = {
            let entry = self
                .tlas
                .get_mut(&tla_id)
                .ok_or(ContractError::TlaNotFound)?;
            if entry.status != TlaStatus::Registered {
                return Err(ContractError::TlaNotInRegisteredState);
            }
            if entry.tla_type != TlaType::Open {
                return Err(ContractError::WrongActivationEndpoint);
            }
            entry.status = TlaStatus::Active;
            entry.activated_at = now;
            entry.expires_at = now.saturating_add(ONE_YEAR_NS);
            entry.expires_at
        };

        Event::TlaActivated {
            tla_id,
            expires_at: new_expires_at,
            paid_yocto: U128(0),
        }
        .emit();
        Ok(())
    }
}
