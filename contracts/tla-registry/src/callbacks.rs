use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use hos_common::MintOutcome;
use near_sdk::json_types::U128;
use near_sdk::{is_promise_success, near, AccountId, FunctionError, PromiseError, PublicKey};

#[near]
impl TlaRegistry {
    #[allow(clippy::too_many_arguments)]
    #[private]
    pub fn on_sub_account_created(
        &mut self,
        tla_id: AccountId,
        name: String,
        payer: AccountId,
        owner_key: PublicKey,
        rent_yocto: U128,
        attached_yocto: U128,
        #[callback_result] outcome: Result<MintOutcome, PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        match outcome {
            Ok(MintOutcome::Active) => {
                let expires_at = self.record_rental(&key, &payer, rent_yocto, attached_yocto);
                Event::SubAccountRented {
                    full_name: key,
                    tla_id,
                    owner: payer,
                    rent_yocto,
                    expires_at,
                }
                .emit();
            }
            Ok(MintOutcome::SignerPending) => {
                let expires_at = self.record_rental(&key, &payer, rent_yocto, attached_yocto);
                self.signer_pending.insert(key.clone(), owner_key);
                Event::SubAccountRented {
                    full_name: key.clone(),
                    tla_id,
                    owner: payer.clone(),
                    rent_yocto,
                    expires_at,
                }
                .emit();
                Event::SubAccountSignerPending {
                    full_name: key,
                    owner: payer,
                }
                .emit();
            }
            Ok(MintOutcome::CreationFailed) | Err(_) => {
                self.settle_failed_mint(
                    &key,
                    &tla_id,
                    &payer,
                    attached_yocto,
                    "sub-account creation failed",
                );
            }
        }
    }

    #[private]
    pub fn on_sub_account_re_rented(
        &mut self,
        tla_id: AccountId,
        name: String,
        payer: AccountId,
        rent_yocto: U128,
        attached_yocto: U128,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !is_promise_success() {
            self.settle_failed_mint(
                &key,
                &tla_id,
                &payer,
                attached_yocto,
                "sub-account re-rent failed",
            );
            return;
        }
        self.parked_names.remove(&key);
        self.sub_account_count = self.sub_account_count.saturating_add(1);
        self.total_revenue = self.total_revenue.saturating_add(rent_yocto.0);
        self.refund_excess(&payer, attached_yocto.0, rent_yocto.0);
        let Some(sub) = self.sub_accounts.get(&key) else {
            return;
        };
        Event::SubAccountReRented {
            full_name: key.clone(),
            tla_id: tla_id.clone(),
            owner: payer.clone(),
            rent_yocto,
            expires_at: sub.expires_at,
        }
        .emit();
    }
}

impl TlaRegistry {
    fn record_rental(
        &mut self,
        key: &str,
        payer: &AccountId,
        rent_yocto: U128,
        attached_yocto: U128,
    ) -> u64 {
        self.sub_account_count = self.sub_account_count.saturating_add(1);
        self.total_revenue = self.total_revenue.saturating_add(rent_yocto.0);
        let charged = rent_yocto
            .0
            .saturating_add(self.fee_config.account_creation_deposit.0);
        self.refund_excess(payer, attached_yocto.0, charged);
        match self.sub_accounts.get(key) {
            Some(s) => s.expires_at,
            None => ContractError::SubAccountNotFound.panic(),
        }
    }

    pub(crate) fn settle_failed_mint(
        &mut self,
        key: &str,
        tla_id: &AccountId,
        payer: &AccountId,
        attached: U128,
        reason: &str,
    ) {
        self.sub_accounts.remove(key);
        let is_business = self
            .tlas
            .get(tla_id)
            .map(|t| t.tla_type == TlaType::Business)
            .unwrap_or(false);
        if is_business {
            self.business_count_decrement(tla_id);
        }
        self.add_pending_refund(payer, attached.0);
        Event::RefundPending {
            account: payer.clone(),
            amount_yocto: attached,
            reason: reason.to_string(),
        }
        .emit();
    }
}
