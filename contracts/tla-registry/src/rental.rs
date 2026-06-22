use crate::asset_gate::{ft_balances_clear, BalanceGate};
use crate::error::ContractError;
use crate::events::Event;
use crate::fees;
use crate::interfaces::{ext_ft, ext_hos_extension, ext_tla_manager};
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::{
    env, is_promise_success, near, AccountId, Gas, NearToken, Promise, PromiseOrValue, PublicKey,
};

const GAS_FOR_CREATE: Gas = Gas::from_tgas(90);
const GAS_FOR_CALLBACK: Gas = Gas::from_tgas(15);
const GAS_FOR_RERENT_FORCE: Gas = Gas::from_tgas(45);
const GAS_FOR_RETRY_INSTALL: Gas = Gas::from_tgas(20);
const GAS_FOR_FT_BALANCE: Gas = Gas::from_tgas(5);
const GAS_FOR_RERENT_BALANCES_CB: Gas = Gas::from_tgas(85);

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn activate_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.assert_not_paused()?;
        let caller = env::predecessor_account_id();
        let tla_len = tla_id.as_str().len() as u8;
        let rent = fees::base_rent(tla_len, &self.fee_config);
        let required = self.fee_config.tla_allocation_fee.0.saturating_add(rent);
        let attached_yocto = env::attached_deposit().as_yoctonear();
        if attached_yocto < required {
            return Err(ContractError::InsufficientPayment);
        }

        let new_expires_at = {
            let entry = self
                .tlas
                .get_mut(&tla_id)
                .ok_or(ContractError::TlaNotFound)?;
            if entry.status != TlaStatus::Registered {
                return Err(ContractError::TlaNotInRegisteredState);
            }
            if entry.tla_type != TlaType::Business {
                return Err(ContractError::WrongActivationEndpoint);
            }
            let licensee = entry
                .licensee
                .as_ref()
                .ok_or(ContractError::BusinessTlaMissingLicensee)?;
            if &caller != licensee {
                return Err(ContractError::OnlyLicensee);
            }
            let now = env::block_timestamp();
            entry.status = TlaStatus::Active;
            entry.activated_at = now;
            entry.expires_at = now.saturating_add(ONE_YEAR_NS);
            entry.expires_at
        };

        self.total_revenue = self.total_revenue.saturating_add(required);
        self.refund_excess(&caller, attached_yocto, required);

        Event::TlaActivated {
            tla_id,
            expires_at: new_expires_at,
            paid_yocto: U128(required),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn rent_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        owner_key: PublicKey,
        main_wallet: AccountId,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        validate_name(&name)?;

        let key = sub_account_key(&tla_id, &name);
        if self.sub_accounts.contains_key(&key) {
            return Err(ContractError::SubAccountNameTaken);
        }
        if main_wallet.as_str() == key {
            return Err(ContractError::MainWalletEqualsSubAccount);
        }

        let is_re_rent = self.parked_names.contains_key(&key);

        let caller = env::predecessor_account_id();
        let rent;
        let is_business;
        {
            let entry = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if !entry.is_accepting_rentals() {
                return Err(ContractError::TlaNotAcceptingRentals);
            }
            is_business = entry.tla_type == TlaType::Business;
            if is_business {
                let licensee = entry
                    .licensee
                    .as_ref()
                    .ok_or(ContractError::BusinessTlaMissingLicensee)?;
                if &caller != licensee {
                    return Err(ContractError::OnlyLicensee);
                }
            }
            rent = fees::calculate_rent(entry, &tla_id, &name, &self.fee_config);
        }

        let total = if is_re_rent {
            rent
        } else {
            rent.saturating_add(self.fee_config.account_creation_deposit.0)
        };
        let attached = env::attached_deposit();
        if attached.as_yoctonear() < total {
            return Err(ContractError::InsufficientPayment);
        }

        self.ensure_mother_default(&caller);
        if is_business {
            self.business_count_check_and_bump(&tla_id)?;
        }

        let now = env::block_timestamp();
        let sub_entry = SubAccountEntry {
            owner: caller.clone(),
            tla_id: tla_id.clone(),
            main_wallet,
            rented_at: now,
            expires_at: now.saturating_add(ONE_YEAR_NS),
            retraction_at: None,
        };
        self.sub_accounts.insert(key.clone(), sub_entry);

        if is_re_rent {
            let sub_account: AccountId = key
                .parse()
                .map_err(|_| ContractError::InvalidSubAccountId)?;
            let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
            if allowlist.is_empty() {
                return Ok(re_rent_transfer(
                    &self.hos_extension,
                    sub_account,
                    owner_key,
                    tla_id,
                    name,
                    caller,
                    rent,
                    attached.as_yoctonear(),
                ));
            }
            let mut chain = ext_ft::ext(allowlist[0].clone())
                .with_static_gas(GAS_FOR_FT_BALANCE)
                .ft_balance_of(sub_account.clone());
            for ft in allowlist.iter().skip(1) {
                chain = chain.and(
                    ext_ft::ext(ft.clone())
                        .with_static_gas(GAS_FOR_FT_BALANCE)
                        .ft_balance_of(sub_account.clone()),
                );
            }
            Ok(chain.then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_RERENT_BALANCES_CB)
                    .on_re_rent_balances_checked(
                        tla_id,
                        name,
                        caller,
                        owner_key,
                        U128(rent),
                        U128(attached.as_yoctonear()),
                        allowlist,
                    ),
            ))
        } else {
            let creation_deposit =
                NearToken::from_yoctonear(self.fee_config.account_creation_deposit.0);
            Ok(ext_tla_manager::ext(tla_id.clone())
                .with_attached_deposit(creation_deposit)
                .with_static_gas(GAS_FOR_CREATE)
                .create_sub_account(name.clone(), owner_key.clone())
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_CALLBACK)
                        .on_sub_account_created(
                            tla_id,
                            name,
                            caller,
                            owner_key,
                            U128(rent),
                            U128(attached.as_yoctonear()),
                        ),
                ))
        }
    }

    #[handle_result]
    #[payable]
    pub fn renew_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.assert_not_paused()?;
        let caller = env::predecessor_account_id();
        let tla_len = tla_id.as_str().len() as u8;
        let rent = fees::base_rent(tla_len, &self.fee_config);
        let now = env::block_timestamp();

        let is_business;
        {
            let entry = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if entry.status != TlaStatus::Active {
                return Err(ContractError::TlaNotActive);
            }
            if now >= entry.expires_at.saturating_add(self.grace_period_ns) {
                return Err(ContractError::TlaPastGracePeriod);
            }
            is_business = entry.tla_type == TlaType::Business;
            if is_business {
                let licensee = entry
                    .licensee
                    .as_ref()
                    .ok_or(ContractError::BusinessTlaMissingLicensee)?;
                if &caller != licensee {
                    return Err(ContractError::OnlyLicensee);
                }
            }
        }
        if !is_business {
            self.assert_admin()?;
        }

        let attached = env::attached_deposit();
        if attached.as_yoctonear() < rent {
            return Err(ContractError::InsufficientPayment);
        }

        let new_expires_at = {
            let entry = self
                .tlas
                .get_mut(&tla_id)
                .ok_or(ContractError::TlaNotFound)?;
            let base = now.max(entry.expires_at);
            entry.expires_at = base.saturating_add(ONE_YEAR_NS);
            entry.expires_at
        };
        self.total_revenue = self.total_revenue.saturating_add(rent);
        self.refund_excess(&caller, attached.as_yoctonear(), rent);

        Event::TlaRenewed {
            tla_id,
            new_expires_at,
            paid_yocto: U128(rent),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn set_main_wallet(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_wallet: AccountId,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        if new_wallet.as_str() == key {
            return Err(ContractError::MainWalletEqualsSubAccount);
        }
        let caller = env::predecessor_account_id();
        let sub = self
            .sub_accounts
            .get_mut(&key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if caller != sub.owner {
            return Err(ContractError::OnlyOwner);
        }
        sub.main_wallet = new_wallet.clone();
        Event::MainWalletUpdated {
            full_name: key,
            new_wallet,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn renew_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
        self.assert_not_paused()?;
        let key = sub_account_key(&tla_id, &name);
        let caller = env::predecessor_account_id();
        let now = env::block_timestamp();

        let rent;
        {
            let sub = self
                .sub_accounts
                .get(&key)
                .ok_or(ContractError::SubAccountNotFound)?;
            if now >= sub.expires_at.saturating_add(self.grace_period_ns) {
                return Err(ContractError::SubAccountPastGracePeriod);
            }
            if caller != sub.owner {
                return Err(ContractError::OnlyOwner);
            }
            if sub.retraction_at.is_some() {
                return Err(ContractError::RetractionPending);
            }
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if matches!(
                tla.lifecycle(self.grace_period_ns),
                LifecycleStatus::Reclaimable
            ) {
                return Err(ContractError::TlaPastGracePeriod);
            }
            rent = fees::calculate_rent(tla, &tla_id, &name, &self.fee_config);
        }

        let attached = env::attached_deposit();
        if attached.as_yoctonear() < rent {
            return Err(ContractError::InsufficientPayment);
        }

        let new_expires_at = {
            let sub = self
                .sub_accounts
                .get_mut(&key)
                .ok_or(ContractError::SubAccountNotFound)?;
            let base = now.max(sub.expires_at);
            sub.expires_at = base.saturating_add(ONE_YEAR_NS);
            sub.expires_at
        };
        self.total_revenue = self.total_revenue.saturating_add(rent);
        self.refund_excess(&caller, attached.as_yoctonear(), rent);

        Event::SubAccountRenewed {
            full_name: key,
            new_expires_at,
            paid_yocto: U128(rent),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn retry_signer_install(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        let key = sub_account_key(&tla_id, &name);
        let owner_key = self
            .signer_pending
            .get(&key)
            .cloned()
            .ok_or(ContractError::SignerNotPending)?;
        let sub = self
            .sub_accounts
            .get(&key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if env::predecessor_account_id() != sub.owner {
            return Err(ContractError::OnlyOwner);
        }
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        Ok(ext_tla_manager::ext(tla_id)
            .with_static_gas(GAS_FOR_RETRY_INSTALL)
            .retry_install(sub_account, owner_key)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_CALLBACK)
                    .on_retry_signer_settled(key),
            ))
    }

    #[private]
    pub fn on_retry_signer_settled(&mut self, key: String) {
        if !is_promise_success() {
            return;
        }
        self.signer_pending.remove(&key);
        Event::SignerInstalled { full_name: key }.emit();
    }

    #[allow(clippy::too_many_arguments)]
    #[private]
    pub fn on_re_rent_balances_checked(
        &mut self,
        tla_id: AccountId,
        name: String,
        payer: AccountId,
        owner_key: PublicKey,
        rent_yocto: U128,
        attached_yocto: U128,
        allowlist: Vec<AccountId>,
    ) -> PromiseOrValue<()> {
        let key = sub_account_key(&tla_id, &name);
        if let BalanceGate::Blocked { token, reason } = ft_balances_clear(&allowlist) {
            self.settle_failed_mint(
                &key,
                &tla_id,
                &payer,
                attached_yocto,
                "re-rent blocked by asset gate",
            );
            Event::SubAccountSaleBlocked {
                full_name: key,
                token,
                reason,
            }
            .emit();
            return PromiseOrValue::Value(());
        }
        let sub_account: AccountId = match key.parse() {
            Ok(a) => a,
            Err(_) => {
                self.settle_failed_mint(
                    &key,
                    &tla_id,
                    &payer,
                    attached_yocto,
                    "invalid sub-account id",
                );
                return PromiseOrValue::Value(());
            }
        };
        PromiseOrValue::Promise(re_rent_transfer(
            &self.hos_extension,
            sub_account,
            owner_key,
            tla_id,
            name,
            payer,
            rent_yocto.0,
            attached_yocto.0,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn re_rent_transfer(
    hos_extension: &AccountId,
    sub_account: AccountId,
    owner_key: PublicKey,
    tla_id: AccountId,
    name: String,
    payer: AccountId,
    rent: u128,
    attached: u128,
) -> Promise {
    ext_hos_extension::ext(hos_extension.clone())
        .with_static_gas(GAS_FOR_RERENT_FORCE)
        .force_transfer(sub_account, owner_key, None)
        .then(
            TlaRegistry::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_CALLBACK)
                .on_sub_account_re_rented(tla_id, name, payer, U128(rent), U128(attached)),
        )
}
