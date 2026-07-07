use near_sdk::json_types::{Base58CryptoHash, U128, U64};
use near_sdk::{ext_contract, AccountId, PublicKey};

#[allow(dead_code)]
#[ext_contract(ext_hos_extension)]
pub trait HosExtension {
    fn sweep_ft(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        tx_nonce: U64,
        block_hash: Base58CryptoHash,
    );
    fn force_transfer(
        &mut self,
        wallet: AccountId,
        new_public_key: PublicKey,
        expected_current: Option<PublicKey>,
    );
}

#[allow(dead_code)]
#[ext_contract(ext_ft)]
pub trait FungibleToken {
    fn ft_balance_of(&self, account_id: AccountId) -> U128;
}

#[allow(dead_code)]
#[ext_contract(ext_active_signer)]
pub trait ActiveSigner {
    fn signer_of(&self, wallet: AccountId) -> Option<String>;
}

#[allow(dead_code)]
#[ext_contract(ext_tla_manager)]
pub trait TlaManager {
    fn create_sub_account(&mut self, name: String, owner_public_key: PublicKey);
    fn retry_install(&mut self, account: AccountId, owner_public_key: PublicKey);
}
