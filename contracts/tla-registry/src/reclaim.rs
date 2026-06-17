use crate::error::ContractError;
use crate::events::Event;
use crate::interfaces::{ext_ft, ext_hos_extension};
use crate::mother::effective_sub_lifecycle;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::{
    env, is_promise_success, near, AccountId, FunctionError, Gas, NearToken, Promise, PromiseError,
    PromiseOrValue,
};

const GAS_FOR_HOS_SWEEP: Gas = Gas::from_tgas(120);
const GAS_FOR_HOS_FORCE_TRANSFER: Gas = Gas::from_tgas(20);
const GAS_FOR_FINALIZE_CB: Gas = Gas::from_tgas(10);
const GAS_FOR_BALANCE_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_BALANCES_CB_TOTAL: Gas = Gas::from_tgas(55);

const FT_BALANCE_MAX_LEN: usize = 256;

const SWEEP_ATTACHED_REQUIRED: NearToken =
    NearToken::from_yoctonear(1_250_000_000_000_000_000_000 + 1);

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
        if env::attached_deposit() < SWEEP_ATTACHED_REQUIRED {
            return Err(ContractError::InsufficientPayment);
        }
        let key = sub_account_key(&tla_id, &name);
        if !self.ft_allowlist.contains(&ft) {
            return Err(ContractError::TokenNotInAllowlist);
        }
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if self
            .mother_use_count
            .get(&sub_account)
            .copied()
            .unwrap_or(0)
            > 0
        {
            return Err(ContractError::SubAccountIsMother);
        }
        let destination;
        {
            let sub = self
                .sub_accounts
                .get(&key)
                .ok_or(ContractError::SubAccountNotFound)?;
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
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
            destination = sub.main_wallet.clone();
        }

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
        let key = sub_account_key(&tla_id, &name);
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if self
            .mother_use_count
            .get(&sub_account)
            .copied()
            .unwrap_or(0)
            > 0
        {
            return Err(ContractError::SubAccountIsMother);
        }
        let destination;
        {
            let sub = self
                .sub_accounts
                .get(&key)
                .ok_or(ContractError::SubAccountNotFound)?;
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
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
            destination = sub.main_wallet.clone();
        }

        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();

        if allowlist.is_empty() {
            return Ok(self.park_wallet(sub_account, tla_id, name, destination));
        }

        let mut chain = ext_ft::ext(allowlist[0].clone())
            .with_static_gas(GAS_FOR_BALANCE_QUERY)
            .ft_balance_of(sub_account.clone());
        for ft in allowlist.iter().skip(1) {
            chain = chain.and(
                ext_ft::ext(ft.clone())
                    .with_static_gas(GAS_FOR_BALANCE_QUERY)
                    .ft_balance_of(sub_account.clone()),
            );
        }

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
        let count = env::promise_results_count();
        for i in 0..count {
            let idx = i as usize;
            let token = match allowlist.get(idx) {
                Some(t) => t,
                None => ContractError::AllowlistLengthMismatch.panic(),
            };
            match env::promise_result_checked(i, FT_BALANCE_MAX_LEN) {
                Ok(bytes) => {
                    let balance: U128 = match near_sdk::serde_json::from_slice(&bytes) {
                        Ok(v) => v,
                        Err(_) => ContractError::InvalidFtBalanceResponse.panic(),
                    };
                    if balance.0 > 0 {
                        Event::ReclaimFinalizeBlocked {
                            full_name: key.clone(),
                            token: token.as_str().to_string(),
                            reason: balance.0.to_string(),
                        }
                        .emit();
                        return PromiseOrValue::Value(());
                    }
                }
                Err(PromiseError::Failed) => {
                    Event::ReclaimFinalizeBlocked {
                        full_name: key.clone(),
                        token: token.as_str().to_string(),
                        reason: "balance_query_failed".to_string(),
                    }
                    .emit();
                    return PromiseOrValue::Value(());
                }
                Err(_) => {
                    Event::ReclaimFinalizeBlocked {
                        full_name: key.clone(),
                        token: token.as_str().to_string(),
                        reason: "balance_query_unverifiable".to_string(),
                    }
                    .emit();
                    return PromiseOrValue::Value(());
                }
            }
        }

        let sub_account: AccountId = match key.parse() {
            Ok(a) => a,
            Err(_) => ContractError::InvalidSubAccountId.panic(),
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
        if !is_promise_success() {
            return;
        }
        let key = sub_account_key(&tla_id, &name);
        self.sub_accounts.remove(&key);
        self.listings.remove(&key);
        self.accepted_offers.remove(&key);
        self.sub_account_count = self.sub_account_count.saturating_sub(1);
        let is_business = self
            .tlas
            .get(&tla_id)
            .map(|t| t.tla_type == TlaType::Business)
            .unwrap_or(false);
        if is_business {
            self.business_count_decrement(&tla_id);
        }
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
    pub(crate) fn park_wallet(
        &self,
        sub_account: AccountId,
        tla_id: AccountId,
        name: String,
        destination: AccountId,
    ) -> Promise {
        ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_HOS_FORCE_TRANSFER)
            .force_transfer(sub_account, self.parked_signer_pubkey.clone())
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_FINALIZE_CB)
                    .on_reclaim_finalized(tla_id, name, destination),
            )
    }
}
