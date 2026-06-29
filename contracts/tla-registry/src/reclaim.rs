use crate::asset_gate::{ft_balance_fanout, ft_balances_clear, BalanceGate};
use crate::error::ContractError;
use crate::events::Event;
use crate::interfaces::{ext_hos_extension, ext_tla_manager};
use crate::mother::effective_sub_lifecycle;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::{env, is_promise_success, near, AccountId, Gas, NearToken, Promise, PromiseOrValue};

const GAS_FOR_HOS_SWEEP: Gas = Gas::from_tgas(120);
const GAS_FOR_HOS_FORCE_TRANSFER: Gas = Gas::from_tgas(45);
const GAS_FOR_RETRY_INSTALL: Gas = Gas::from_tgas(20);
const GAS_FOR_FINALIZE_CB: Gas = Gas::from_tgas(10);
const GAS_FOR_BALANCES_CB_TOTAL: Gas = Gas::from_tgas(80);

const SWEEP_ATTACHED_REQUIRED: NearToken =
    NearToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO + 1);

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn reclaim_sweep_ft(
        &mut self,
        tla_id: AccountId,
        name: String,
        ft: AccountId,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        validate_name(&name)?;
        if env::attached_deposit() < SWEEP_ATTACHED_REQUIRED {
            return Err(ContractError::InsufficientPayment);
        }
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        if !self.ft_allowlist.contains(&ft) {
            return Err(ContractError::TokenNotInAllowlist);
        }
        let (sub_account, destination) = self.resolve_reclaimable(&tla_id, &key)?;

        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_HOS_SWEEP)
            .with_attached_deposit(SWEEP_ATTACHED_REQUIRED)
            .sweep_ft(sub_account, ft, destination))
    }

    #[handle_result]
    pub fn reclaim_finalize(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        if self.reclaim_pending.contains_key(&key) {
            return Err(ContractError::ReclaimInProgress);
        }
        let (sub_account, destination) = self.resolve_reclaimable(&tla_id, &key)?;
        self.reclaim_pending.insert(key.clone(), true);

        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
        let Some(chain) = ft_balance_fanout(&allowlist, &sub_account) else {
            return Ok(self.park_wallet(sub_account, tla_id, name, destination));
        };
        Ok(chain.then(
            Self::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_BALANCES_CB_TOTAL)
                .on_balances_checked(tla_id, name, destination, allowlist),
        ))
    }

    #[private]
    pub fn on_balances_checked(
        &mut self,
        tla_id: AccountId,
        name: String,
        destination: AccountId,
        allowlist: Vec<AccountId>,
    ) -> PromiseOrValue<()> {
        let key = sub_account_key(&tla_id, &name);
        if self.assert_sale_idle(&key).is_err() {
            self.reclaim_pending.remove(&key);
            Event::ReclaimFinalizeBlocked {
                full_name: key,
                token: String::new(),
                reason: "sale_in_progress".to_string(),
            }
            .emit();
            return PromiseOrValue::Value(());
        }
        if let BalanceGate::Blocked { token, reason } = ft_balances_clear(&allowlist) {
            self.reclaim_pending.remove(&key);
            Event::ReclaimFinalizeBlocked {
                full_name: key,
                token,
                reason,
            }
            .emit();
            return PromiseOrValue::Value(());
        }
        let sub_account = match self.resolve_reclaimable(&tla_id, &key) {
            Ok((sub_account, _)) => sub_account,
            Err(_) => {
                self.reclaim_pending.remove(&key);
                Event::ReclaimFinalizeBlocked {
                    full_name: key,
                    token: String::new(),
                    reason: "no_longer_reclaimable".to_string(),
                }
                .emit();
                return PromiseOrValue::Value(());
            }
        };
        PromiseOrValue::Promise(self.park_wallet(sub_account, tla_id, name, destination))
    }

    #[private]
    pub fn on_reclaim_finalized(
        &mut self,
        tla_id: AccountId,
        name: String,
        destination: AccountId,
    ) {
        let key = sub_account_key(&tla_id, &name);
        self.reclaim_pending.remove(&key);
        if !is_promise_success() {
            return;
        }
        let Some(removed) = self.sub_accounts.remove(&key) else {
            return;
        };
        if let Ok(sub_account) = key.parse::<AccountId>() {
            if self.mothers.get(&removed.owner) == Some(&sub_account) {
                self.mothers.remove(&removed.owner);
                self.decrement_mother_use(&sub_account);
            }
        }
        self.listings.remove(&key);
        self.accepted_offers.remove(&key);
        self.signer_pending.remove(&key);
        self.sub_account_count = self.sub_account_count.saturating_sub(1);
        self.business_count_decrement_if_business(&tla_id);
        let now = env::block_timestamp();
        self.parked_names.insert(
            key.clone(),
            ParkedEntry {
                tla_id: tla_id.clone(),
                parked_at: now,
            },
        );
        Event::SubAccountReclaimed {
            full_name: key,
            tla_id,
            swept_to: destination,
        }
        .emit();
    }
}

impl TlaRegistry {
    fn resolve_reclaimable(
        &self,
        tla_id: &AccountId,
        key: &str,
    ) -> Result<(AccountId, AccountId), ContractError> {
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        let sub = self
            .sub_accounts
            .get(key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if sub.tla_id != *tla_id {
            return Err(ContractError::SubAccountTlaMismatch);
        }
        let total_refs = self
            .mother_use_count
            .get(&sub_account)
            .copied()
            .unwrap_or(0);
        let owner_self_ref = self.mothers.get(&sub.owner) == Some(&sub_account);
        let external_refs = if owner_self_ref {
            total_refs.saturating_sub(1)
        } else {
            total_refs
        };
        if external_refs > 0 {
            return Err(ContractError::SubAccountIsMother);
        }
        let tla = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        if !matches!(
            effective_sub_lifecycle(
                sub,
                tla,
                self.fee_config.retraction_notice_ns.0,
                self.grace_period_ns,
            ),
            LifecycleStatus::Reclaimable
        ) {
            return Err(ContractError::SubAccountNotReclaimable);
        }
        Ok((sub_account, sub.main_wallet.clone()))
    }

    pub(crate) fn park_wallet(
        &self,
        sub_account: AccountId,
        tla_id: AccountId,
        name: String,
        destination: AccountId,
    ) -> Promise {
        let key = sub_account_key(&tla_id, &name);
        let finalize = Self::ext(env::current_account_id())
            .with_static_gas(GAS_FOR_FINALIZE_CB)
            .on_reclaim_finalized(tla_id.clone(), name, destination);
        if self.signer_pending.contains_key(&key) {
            return ext_tla_manager::ext(tla_id)
                .with_static_gas(GAS_FOR_RETRY_INSTALL)
                .retry_install(sub_account, self.parked_signer_pubkey.clone())
                .then(finalize);
        }
        ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_HOS_FORCE_TRANSFER)
            .force_transfer(sub_account, self.parked_signer_pubkey.clone(), None)
            .then(finalize)
    }
}
