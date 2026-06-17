use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::{is_promise_success, near, AccountId, FunctionError};

#[near]
impl TlaRegistry {
    #[private]
    pub fn on_sub_account_created(
        &mut self,
        tla_id: AccountId,
        name: String,
        payer: AccountId,
        rent_yocto: U128,
        attached_yocto: U128,
    ) {
        let key = sub_account_key(&tla_id, &name);

        if is_promise_success() {
            self.sub_account_count = self.sub_account_count.saturating_add(1);
            self.total_revenue = self.total_revenue.saturating_add(rent_yocto.0);

            let charged = rent_yocto
                .0
                .saturating_add(self.fee_config.account_creation_deposit.0);
            let excess = attached_yocto.0.saturating_sub(charged);
            if excess > 0 {
                self.add_pending_refund(&payer, excess);
            }

            let sub = match self.sub_accounts.get(&key) {
                Some(s) => s,
                None => ContractError::SubAccountNotFound.panic(),
            };
            Event::SubAccountRented {
                full_name: key.clone(),
                tla_id: tla_id.clone(),
                owner: payer.clone(),
                rent_yocto,
                expires_at: sub.expires_at,
            }
            .emit();
        } else {
            self.sub_accounts.remove(&key);

            let is_business = self
                .tlas
                .get(&tla_id)
                .map(|t| t.tla_type == TlaType::Business)
                .unwrap_or(false);
            if is_business {
                self.business_count_decrement(&tla_id);
            }

            self.add_pending_refund(&payer, attached_yocto.0);

            Event::RefundPending {
                account: payer.clone(),
                amount_yocto: attached_yocto,
                reason: "sub-account creation failed".to_string(),
            }
            .emit();
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

        if is_promise_success() {
            self.parked_names.remove(&key);
            self.sub_account_count = self.sub_account_count.saturating_add(1);
            self.total_revenue = self.total_revenue.saturating_add(rent_yocto.0);

            let excess = attached_yocto.0.saturating_sub(rent_yocto.0);
            if excess > 0 {
                self.add_pending_refund(&payer, excess);
            }

            let sub = match self.sub_accounts.get(&key) {
                Some(s) => s,
                None => ContractError::SubAccountNotFound.panic(),
            };
            Event::SubAccountReRented {
                full_name: key.clone(),
                tla_id: tla_id.clone(),
                owner: payer.clone(),
                rent_yocto,
                expires_at: sub.expires_at,
            }
            .emit();
        } else {
            self.sub_accounts.remove(&key);

            let is_business = self
                .tlas
                .get(&tla_id)
                .map(|t| t.tla_type == TlaType::Business)
                .unwrap_or(false);
            if is_business {
                self.business_count_decrement(&tla_id);
            }

            self.add_pending_refund(&payer, attached_yocto.0);

            Event::RefundPending {
                account: payer.clone(),
                amount_yocto: attached_yocto,
                reason: "sub-account re-rent failed".to_string(),
            }
            .emit();
        }
    }
}
