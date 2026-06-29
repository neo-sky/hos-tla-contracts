use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::{env, near, AccountId};

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn set_mother(&mut self, new_mother: AccountId) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        if !self.sub_accounts.contains_key(new_mother.as_str()) {
            return Err(ContractError::SubAccountNotFound);
        }
        let caller = env::predecessor_account_id();
        self.set_mother_internal(&caller, &new_mother, "explicit")
    }

    pub fn get_mother(&self, user: AccountId) -> Option<AccountId> {
        self.mothers.get(&user).cloned()
    }

    pub fn is_mother(&self, account: AccountId) -> bool {
        self.mother_use_count.get(&account).copied().unwrap_or(0) > 0
    }

    pub fn get_mother_use_count(&self, account: AccountId) -> u32 {
        self.mother_use_count.get(&account).copied().unwrap_or(0)
    }

    #[handle_result]
    pub fn admin_clear_mother(&mut self, user: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        let previous = self
            .mothers
            .remove(&user)
            .ok_or(ContractError::MotherNotSet)?;
        if self.sub_accounts.contains_key(previous.as_str()) {
            self.decrement_mother_use(&previous);
        }
        Event::MotherCleared {
            user,
            previous_mother: previous,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }
}

impl TlaRegistry {
    pub(crate) fn ensure_mother_default(&mut self, user: &AccountId) {
        if self.mothers.contains_key(user) {
            return;
        }
        self.mothers.insert(user.clone(), user.clone());
        if self.sub_accounts.contains_key(user.as_str()) {
            self.increment_mother_use(user);
        }
        Event::MotherSet {
            user: user.clone(),
            mother: user.clone(),
            source: "default".to_string(),
        }
        .emit();
    }

    fn set_mother_internal(
        &mut self,
        user: &AccountId,
        new_mother: &AccountId,
        source: &str,
    ) -> Result<(), ContractError> {
        let key = new_mother.as_str().to_string();
        let retraction_notice_ns = self.fee_config.retraction_notice_ns.0;
        let grace_period_ns = self.grace_period_ns;
        let new_mother_is_managed = self.sub_accounts.contains_key(&key);
        if let Some(sub) = self.sub_accounts.get(&key) {
            if &sub.owner != user {
                return Err(ContractError::OnlyOwner);
            }
            let tla = self
                .tlas
                .get(&sub.tla_id)
                .ok_or(ContractError::TlaNotFound)?;
            if matches!(
                effective_sub_lifecycle(sub, tla, retraction_notice_ns, grace_period_ns),
                LifecycleStatus::Reclaimable
            ) {
                return Err(ContractError::MotherIsReclaimable);
            }
        }
        if let Some(old_mother) = self.mothers.get(user).cloned() {
            if &old_mother == new_mother {
                return Ok(());
            }
            if self.sub_accounts.contains_key(old_mother.as_str()) {
                self.decrement_mother_use(&old_mother);
            }
        }
        self.mothers.insert(user.clone(), new_mother.clone());
        if new_mother_is_managed {
            self.increment_mother_use(new_mother);
        }
        Event::MotherSet {
            user: user.clone(),
            mother: new_mother.clone(),
            source: source.to_string(),
        }
        .emit();
        Ok(())
    }

    fn increment_mother_use(&mut self, account: &AccountId) {
        let next = self
            .mother_use_count
            .get(account)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        self.mother_use_count.insert(account.clone(), next);
    }

    pub(crate) fn decrement_mother_use(&mut self, account: &AccountId) {
        let next = self
            .mother_use_count
            .get(account)
            .copied()
            .unwrap_or(0)
            .saturating_sub(1);
        if next == 0 {
            self.mother_use_count.remove(account);
        } else {
            self.mother_use_count.insert(account.clone(), next);
        }
    }
}

pub(crate) fn effective_sub_lifecycle(
    sub: &SubAccountEntry,
    tla: &TlaEntry,
    retraction_notice_ns: u64,
    grace_period_ns: u64,
) -> LifecycleStatus {
    if matches!(tla.lifecycle(grace_period_ns), LifecycleStatus::Reclaimable) {
        return LifecycleStatus::Reclaimable;
    }
    if let Some(retraction_at) = sub.retraction_at {
        let elapsed_at = retraction_at.saturating_add(retraction_notice_ns);
        if env::block_timestamp() >= elapsed_at {
            return LifecycleStatus::Reclaimable;
        }
    }
    sub.lifecycle(grace_period_ns)
}
