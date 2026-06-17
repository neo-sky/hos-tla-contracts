use crate::error::ContractError;
use crate::events::Event;
use crate::fees;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::{env, near, AccountId};

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn schedule_retraction(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        let key = sub_account_key(&tla_id, &name);
        let caller = env::predecessor_account_id();
        let now = env::block_timestamp();
        {
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if tla.tla_type != TlaType::Business {
                return Err(ContractError::NotBusinessTla);
            }
        }
        let sub = self
            .sub_accounts
            .get_mut(&key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if caller != sub.owner {
            return Err(ContractError::OnlyOwner);
        }
        if sub.retraction_at.is_some() {
            return Err(ContractError::RetractionAlreadyScheduled);
        }
        sub.retraction_at = Some(now);
        Event::SubAccountRetractionScheduled {
            full_name: key,
            retraction_at: now,
            by: caller,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn cancel_retraction(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        let key = sub_account_key(&tla_id, &name);
        let caller = env::predecessor_account_id();
        let retraction_notice_ns = self.fee_config.retraction_notice_ns.0;
        let now = env::block_timestamp();
        let sub = self
            .sub_accounts
            .get_mut(&key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if caller != sub.owner {
            return Err(ContractError::OnlyOwner);
        }
        let retraction_at = sub
            .retraction_at
            .ok_or(ContractError::NoRetractionScheduled)?;
        if now >= retraction_at.saturating_add(retraction_notice_ns) {
            return Err(ContractError::RetractionAlreadyElapsed);
        }
        sub.retraction_at = None;
        Event::SubAccountRetractionCanceled {
            full_name: key,
            by: caller,
        }
        .emit();
        Ok(())
    }

    pub fn get_business_sub_count(&self, tla_id: AccountId) -> u32 {
        self.business_sub_count.get(&tla_id).copied().unwrap_or(0)
    }

    pub fn get_business_sub_cap(&self, tla_id: AccountId) -> u32 {
        self.effective_business_cap(&tla_id)
    }

    #[handle_result]
    pub fn set_business_sub_cap(
        &mut self,
        tla_id: AccountId,
        cap: Option<u32>,
    ) -> Result<(), ContractError> {
        self.assert_admin()?;
        {
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if tla.tla_type != TlaType::Business {
                return Err(ContractError::NotBusinessTla);
            }
        }
        let cap_str = match cap {
            Some(value) => {
                self.business_sub_cap_override.insert(tla_id.clone(), value);
                value.to_string()
            }
            None => {
                self.business_sub_cap_override.remove(&tla_id);
                String::from("default")
            }
        };
        Event::BusinessSubCapSet {
            tla_id,
            cap: cap_str,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn get_business_renewal_cost(
        &self,
        tla_id: AccountId,
    ) -> Result<BusinessRenewalCostView, ContractError> {
        let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
        if tla.tla_type != TlaType::Business {
            return Err(ContractError::NotBusinessTla);
        }
        let tla_len = tla_id.as_str().len() as u8;
        let tla_rent = fees::base_rent(tla_len, &self.fee_config);
        let per_sub = self.fee_config.sub_fee_per_account.0;
        let count = self.business_sub_count.get(&tla_id).copied().unwrap_or(0);
        let subs_total = per_sub.saturating_mul(u128::from(count));
        let total = tla_rent.saturating_add(subs_total);
        Ok(BusinessRenewalCostView {
            tla_id,
            tla_rent_yocto: U128(tla_rent),
            per_sub_yocto: U128(per_sub),
            sub_count: count,
            total_yocto: U128(total),
        })
    }

    pub fn get_retraction_at(&self, tla_id: AccountId, name: String) -> Option<u64> {
        let key = sub_account_key(&tla_id, &name);
        self.sub_accounts.get(&key).and_then(|s| s.retraction_at)
    }
}

impl TlaRegistry {
    pub(crate) fn effective_business_cap(&self, tla_id: &AccountId) -> u32 {
        self.business_sub_cap_override
            .get(tla_id)
            .copied()
            .unwrap_or(self.fee_config.business_max_subs)
    }

    pub(crate) fn business_count_check_and_bump(
        &mut self,
        tla_id: &AccountId,
    ) -> Result<(), ContractError> {
        let cap = self.effective_business_cap(tla_id);
        let count = self.business_sub_count.get(tla_id).copied().unwrap_or(0);
        if count >= cap {
            return Err(ContractError::MaxBusinessSubsReached);
        }
        self.business_sub_count
            .insert(tla_id.clone(), count.saturating_add(1));
        Ok(())
    }

    pub(crate) fn business_count_decrement(&mut self, tla_id: &AccountId) {
        let count = self.business_sub_count.get(tla_id).copied().unwrap_or(0);
        if count == 0 {
            return;
        }
        let next = count.saturating_sub(1);
        if next == 0 {
            self.business_sub_count.remove(tla_id);
        } else {
            self.business_sub_count.insert(tla_id.clone(), next);
        }
    }
}
