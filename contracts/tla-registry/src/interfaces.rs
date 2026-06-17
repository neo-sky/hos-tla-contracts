use near_sdk::json_types::U128;
use near_sdk::{ext_contract, AccountId, PublicKey};

#[allow(dead_code)]
#[ext_contract(ext_hos_extension)]
pub trait HosExtension {
    fn sweep_ft(&mut self, wallet: AccountId, ft: AccountId, destination: AccountId);
    fn force_transfer(&mut self, wallet: AccountId, new_public_key: PublicKey);
}

#[allow(dead_code)]
#[ext_contract(ext_ft)]
pub trait FungibleToken {
    fn ft_balance_of(&self, account_id: AccountId) -> U128;
}

#[allow(dead_code)]
#[ext_contract(ext_tla_manager)]
pub trait TlaManager {
    fn create_sub_account(&mut self, name: String, owner_public_key: PublicKey);
    fn retry_install(&mut self, account: AccountId, owner_public_key: PublicKey);
}
