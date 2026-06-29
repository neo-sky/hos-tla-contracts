mod error;
mod events;

use near_sdk::json_types::Base58CryptoHash;
use near_sdk::serde_json::json;
use near_sdk::{
    env, ext_contract, near, require, AccountId, CryptoHash, Gas, NearToken, PanicOnDefault,
    Promise, PromiseError, PromiseOrValue, PublicKey,
};

use crate::events::Event;
use hos_common::MintOutcome;

const WALLET_INIT_GAS: Gas = Gas::from_tgas(30);
const INSTALL_SIGNER_GAS: Gas = Gas::from_tgas(10);
const CALLBACK_GAS: Gas = Gas::from_tgas(20);
const ON_CREATED_GAS: Gas = Gas::from_tgas(50);

#[ext_contract(ext_active_signer)]
#[allow(dead_code)]
trait ActiveSigner {
    fn install_signer(&mut self, wallet: AccountId, public_key: String);
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct TlaManager {
    registry: AccountId,
    active_signer: AccountId,
    hos_extension: AccountId,
    wallet_code_hash: CryptoHash,
    min_balance: NearToken,
}

#[near]
impl TlaManager {
    #[init]
    pub fn new(
        registry: AccountId,
        active_signer: AccountId,
        hos_extension: AccountId,
        wallet_code_hash: Base58CryptoHash,
        min_balance: NearToken,
    ) -> Self {
        Self {
            registry,
            active_signer,
            hos_extension,
            wallet_code_hash: wallet_code_hash.into(),
            min_balance,
        }
    }

    #[payable]
    pub fn create_sub_account(&mut self, name: String, owner_public_key: PublicKey) -> Promise {
        require!(
            env::predecessor_account_id() == self.registry,
            error::ONLY_REGISTRY
        );
        require!(!name.is_empty() && !name.contains('.'), error::INVALID_NAME);
        require!(
            hos_common::is_ed25519(&owner_public_key),
            hos_common::NOT_ED25519
        );
        let funding = env::attached_deposit();
        require!(funding >= self.min_balance, error::INSUFFICIENT_DEPOSIT);

        let account: AccountId = format!("{}.{}", name, env::current_account_id())
            .parse()
            .unwrap_or_else(|_| env::panic_str(error::INVALID_NAME));
        let extensions = [self.active_signer.clone(), self.hos_extension.clone()];
        let init_args = json!({ "extensions": extensions }).to_string().into_bytes();

        Promise::new(account.clone())
            .create_account()
            .transfer(funding)
            .use_global_contract(self.wallet_code_hash)
            .function_call(
                "new".to_string(),
                init_args,
                NearToken::ZERO,
                WALLET_INIT_GAS,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(ON_CREATED_GAS)
                    .on_wallet_created(account, owner_public_key, funding),
            )
    }

    #[private]
    pub fn on_wallet_created(
        &mut self,
        account: AccountId,
        owner_public_key: PublicKey,
        funding: NearToken,
        #[callback_result] result: Result<(), PromiseError>,
    ) -> PromiseOrValue<MintOutcome> {
        if result.is_err() {
            Event::MintFailed {
                account: account.clone(),
            }
            .emit();
            return PromiseOrValue::Promise(
                Promise::new(self.registry.clone()).transfer(funding).then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(CALLBACK_GAS)
                        .on_creation_failed(),
                ),
            );
        }
        Event::SubAccountMinted {
            account: account.clone(),
            owner: owner_public_key.to_string(),
        }
        .emit();
        PromiseOrValue::Promise(
            ext_active_signer::ext(self.active_signer.clone())
                .with_static_gas(INSTALL_SIGNER_GAS)
                .install_signer(
                    account.clone(),
                    hos_common::ed25519_base58_or_panic(&owner_public_key),
                )
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(CALLBACK_GAS)
                        .on_signer_settled(account),
                ),
        )
    }

    #[private]
    pub fn on_signer_settled(
        &mut self,
        account: AccountId,
        #[callback_result] result: Result<(), PromiseError>,
    ) -> MintOutcome {
        if result.is_ok() {
            MintOutcome::Active
        } else {
            Event::SignerInstallFailed { account }.emit();
            MintOutcome::SignerPending
        }
    }

    #[private]
    pub fn on_creation_failed(&self) -> MintOutcome {
        MintOutcome::CreationFailed
    }

    pub fn retry_install(&mut self, account: AccountId, owner_public_key: PublicKey) -> Promise {
        require!(
            env::predecessor_account_id() == self.registry,
            error::ONLY_REGISTRY
        );
        require!(
            hos_common::is_ed25519(&owner_public_key),
            hos_common::NOT_ED25519
        );
        ext_active_signer::ext(self.active_signer.clone())
            .with_static_gas(INSTALL_SIGNER_GAS)
            .install_signer(
                account,
                hos_common::ed25519_base58_or_panic(&owner_public_key),
            )
    }

    pub fn registry(&self) -> &AccountId {
        &self.registry
    }

    pub fn min_balance(&self) -> NearToken {
        self.min_balance
    }

    pub fn config(&self) -> (AccountId, AccountId, Base58CryptoHash, NearToken) {
        (
            self.active_signer.clone(),
            self.hos_extension.clone(),
            Base58CryptoHash::from(self.wallet_code_hash),
            self.min_balance,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;
    use std::str::FromStr;

    const REGISTRY: &str = "tla-registry.testnet";
    const SIGNER: &str = "active-signer.testnet";
    const HOSEXT: &str = "hos-extension.testnet";
    const MANAGER: &str = "tla.testnet";
    const OWNER_KEY: &str = "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847";

    fn acc(s: &str) -> AccountId {
        AccountId::from_str(s).unwrap()
    }

    fn owner_key() -> PublicKey {
        PublicKey::from_str(OWNER_KEY).unwrap()
    }

    fn hash() -> Base58CryptoHash {
        Base58CryptoHash::from([7u8; 32])
    }

    fn ctx(predecessor: &str, deposit: u128) {
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc(MANAGER))
            .predecessor_account_id(acc(predecessor))
            .attached_deposit(NearToken::from_yoctonear(deposit))
            .build());
    }

    fn deploy() -> TlaManager {
        ctx(REGISTRY, 0);
        TlaManager::new(
            acc(REGISTRY),
            acc(SIGNER),
            acc(HOSEXT),
            hash(),
            NearToken::from_millinear(100),
        )
    }

    fn min() -> u128 {
        NearToken::from_millinear(100).as_yoctonear()
    }

    #[test]
    fn registry_can_mint() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let _ = c.create_sub_account("alice".to_string(), owner_key());
    }

    #[test]
    #[should_panic(expected = "only registry")]
    fn outsider_cannot_mint() {
        let mut c = deploy();
        ctx("attacker.testnet", min());
        let _ = c.create_sub_account("alice".to_string(), owner_key());
    }

    #[test]
    #[should_panic(expected = "invalid sub-account name")]
    fn dotted_name_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let _ = c.create_sub_account("alice.bob".to_string(), owner_key());
    }

    #[test]
    #[should_panic(expected = "invalid sub-account name")]
    fn empty_name_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let _ = c.create_sub_account(String::new(), owner_key());
    }

    #[test]
    #[should_panic(expected = "deposit below minimum balance")]
    fn underfunded_mint_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min() - 1);
        let _ = c.create_sub_account("alice".to_string(), owner_key());
    }

    #[test]
    fn callback_success_installs_signer() {
        let mut c = deploy();
        ctx(MANAGER, 0);
        let _ = c.on_wallet_created(
            acc("alice.tla.testnet"),
            owner_key(),
            NearToken::from_millinear(100),
            Ok(()),
        );
    }

    #[test]
    fn callback_failure_refunds_registry() {
        let mut c = deploy();
        ctx(MANAGER, 0);
        let out = c.on_wallet_created(
            acc("alice.tla.testnet"),
            owner_key(),
            NearToken::from_millinear(100),
            Err(PromiseError::Failed),
        );
        assert!(matches!(out, PromiseOrValue::Promise(_)));
    }

    #[test]
    fn on_creation_failed_reports_creation_failed() {
        let c = deploy();
        ctx(MANAGER, 0);
        assert!(matches!(
            c.on_creation_failed(),
            MintOutcome::CreationFailed
        ));
    }

    #[test]
    #[should_panic(expected = "owner key must be ed25519")]
    fn secp256k1_owner_key_rejected() {
        let mut c = deploy();
        ctx(REGISTRY, min());
        let secp = PublicKey::from_str(
            "secp256k1:qMoRgcoXai4mBPsdbHi1wfyxF9TdbPCF4qSDQTRP3TfescSRoUdSx6nmeQoN3aiwGzwMyGXAb1gUjBTv5AY8DXj",
        )
        .unwrap();
        let _ = c.create_sub_account("alice".to_string(), secp);
    }

    #[test]
    fn config_roundtrips() {
        let c = deploy();
        let (signer, hosext, code, bal) = c.config();
        assert_eq!(signer, acc(SIGNER));
        assert_eq!(hosext, acc(HOSEXT));
        assert_eq!(code, hash());
        assert_eq!(bal, NearToken::from_millinear(100));
    }

    #[test]
    fn signer_settled_ok_reports_active() {
        let mut c = deploy();
        ctx(MANAGER, 0);
        let out = c.on_signer_settled(acc("alice.tla.testnet"), Ok(()));
        assert_eq!(out, MintOutcome::Active);
    }

    #[test]
    fn signer_settled_err_reports_pending() {
        let mut c = deploy();
        ctx(MANAGER, 0);
        let out = c.on_signer_settled(acc("alice.tla.testnet"), Err(PromiseError::Failed));
        assert_eq!(out, MintOutcome::SignerPending);
    }

    #[test]
    fn retry_install_accepts_registry() {
        let mut c = deploy();
        ctx(REGISTRY, 0);
        let _ = c.retry_install(acc("alice.tla.testnet"), owner_key());
    }

    #[test]
    #[should_panic(expected = "only registry")]
    fn retry_install_rejects_outsider() {
        let mut c = deploy();
        ctx("attacker.testnet", 0);
        let _ = c.retry_install(acc("alice.tla.testnet"), owner_key());
    }
}
