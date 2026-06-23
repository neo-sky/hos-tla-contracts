use crate::error::ContractError;
use crate::fees;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::serde::Serialize;
use near_sdk::{near, AccountId, FunctionError};

#[near]
impl TlaRegistry {
    pub fn get_tla(&self, tla_id: AccountId) -> Option<TlaView> {
        self.tlas
            .get(&tla_id)
            .map(|e| to_tla_view(&tla_id, e, &self.fee_config, self.grace_period_ns))
    }

    pub fn get_sub_account(&self, tla_id: AccountId, name: String) -> Option<SubAccountView> {
        let key = sub_account_key(&tla_id, &name);
        let sub = self.sub_accounts.get(&key)?;
        let tla = match self.tlas.get(&tla_id) {
            Some(t) => t,
            None => ContractError::TlaNotFound.panic(),
        };
        Some(to_sub_view(
            &key,
            sub,
            tla,
            &tla_id,
            &name,
            &self.fee_config,
            self.grace_period_ns,
        ))
    }

    pub fn get_parked_sub_account(
        &self,
        tla_id: AccountId,
        name: String,
    ) -> Option<ParkedSubAccountView> {
        let key = sub_account_key(&tla_id, &name);
        self.parked_names.get(&key).map(|p| ParkedSubAccountView {
            full_name: key.clone(),
            tla_id: p.tla_id.clone(),
            parked_at: U128(p.parked_at as u128),
        })
    }

    pub fn is_name_re_rentable(&self, tla_id: AccountId, name: String) -> bool {
        let key = sub_account_key(&tla_id, &name);
        self.parked_names.contains_key(&key)
    }

    pub fn get_hos_extension(&self) -> AccountId {
        self.hos_extension.clone()
    }

    pub fn get_active_signer(&self) -> AccountId {
        self.active_signer.clone()
    }

    pub fn get_parked_signer_pubkey(&self) -> near_sdk::PublicKey {
        self.parked_signer_pubkey.clone()
    }

    pub fn get_grace_period_ns(&self) -> near_sdk::json_types::U64 {
        near_sdk::json_types::U64(self.grace_period_ns)
    }

    #[handle_result]
    pub fn get_rent_price(
        &self,
        tla_id: AccountId,
        name: String,
    ) -> Result<RentPriceView, ContractError> {
        let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
        let rent = fees::calculate_rent(tla, &tla_id, &name, &self.fee_config);
        let deposit = self.fee_config.account_creation_deposit.0;
        Ok(RentPriceView {
            rent_yocto: U128(rent),
            creation_deposit_yocto: U128(deposit),
            total_yocto: U128(rent.saturating_add(deposit)),
        })
    }

    pub fn is_name_available(&self, tla_id: AccountId, name: String) -> bool {
        let key = sub_account_key(&tla_id, &name);
        !self.sub_accounts.contains_key(&key)
    }

    pub fn is_signer_pending(&self, tla_id: AccountId, name: String) -> bool {
        let key = sub_account_key(&tla_id, &name);
        self.signer_pending.contains_key(&key)
    }

    pub fn list_tlas(&self, from_index: u64, limit: u64) -> Vec<TlaView> {
        self.tlas
            .iter()
            .skip(from_index as usize)
            .take(limit as usize)
            .map(|(id, entry)| to_tla_view(id, entry, &self.fee_config, self.grace_period_ns))
            .collect()
    }

    pub fn get_fee_config(&self) -> FeeConfig {
        self.fee_config.clone()
    }

    pub fn get_stats(&self) -> RegistryStats {
        RegistryStats {
            tla_count: u64::from(self.tlas.len()),
            sub_account_count: self.sub_account_count,
            total_revenue_yocto: U128(self.total_revenue),
            total_pending_refunds_yocto: U128(self.total_pending_refunds),
        }
    }

    pub fn get_admins(&self) -> Vec<AccountId> {
        self.admins.iter().cloned().collect()
    }

    pub fn get_ft_allowlist(&self) -> Vec<AccountId> {
        self.ft_allowlist.iter().cloned().collect()
    }
}

pub(crate) fn to_tla_view(
    tla_id: &AccountId,
    entry: &TlaEntry,
    config: &FeeConfig,
    grace_period_ns: u64,
) -> TlaView {
    let tla_len = tla_id.as_str().len() as u8;
    let rent = fees::base_rent(tla_len, config);
    TlaView {
        tla_id: tla_id.clone(),
        tla_type: entry.tla_type.clone(),
        lifecycle: entry.lifecycle(grace_period_ns),
        licensee: entry.licensee.clone(),
        premium_category: entry.premium_category.clone(),
        activated_at: U128(entry.activated_at as u128),
        expires_at: U128(entry.expires_at as u128),
        annual_rent: U128(rent),
    }
}

pub(crate) fn to_sub_view(
    key: &str,
    entry: &SubAccountEntry,
    tla: &TlaEntry,
    tla_id: &AccountId,
    name: &str,
    config: &FeeConfig,
    grace_period_ns: u64,
) -> SubAccountView {
    let rent = fees::calculate_rent(tla, tla_id, name, config);
    SubAccountView {
        full_name: key.to_string(),
        owner: entry.owner.clone(),
        tla_id: entry.tla_id.clone(),
        main_wallet: entry.main_wallet.clone(),
        lifecycle: entry.lifecycle(grace_period_ns),
        rented_at: U128(entry.rented_at as u128),
        expires_at: U128(entry.expires_at as u128),
        annual_rent: U128(rent),
    }
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct ParkedSubAccountView {
    pub full_name: String,
    pub tla_id: AccountId,
    pub parked_at: U128,
}
