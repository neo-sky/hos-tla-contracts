mod error;
mod events;

use crate::error::ContractError;
use crate::events::Event;
use defuse_wallet::{ext_wallet, FunctionCallAction, PromiseSingle, Request};
use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::store::IterableSet;
use near_sdk::{
    env, ext_contract, near, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError, PromiseOrValue, PublicKey,
};

const CONTRACT_VERSION: u8 = 1;

const GAS_FOR_SWAP_OWNER: Gas = Gas::from_tgas(8);
const GAS_FOR_RESET: Gas = Gas::from_tgas(5);
const GAS_FOR_BALANCE_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_BALANCE_CB: Gas = Gas::from_tgas(35);
const GAS_FOR_STORAGE_DEPOSIT: Gas = Gas::from_tgas(10);
const GAS_FOR_STORAGE_CB: Gas = Gas::from_tgas(30);
const GAS_FOR_EXTENSION_CALL: Gas = Gas::from_tgas(20);
const GAS_FOR_SETTLE_CB: Gas = Gas::from_tgas(8);
const GAS_FOR_FT_TRANSFER: Gas = Gas::from_tgas(10);

const STORAGE_DEPOSIT_AMOUNT: NearToken = NearToken::from_yoctonear(1_250_000_000_000_000_000_000);
const MIN_SWEEP_ATTACHED: NearToken = NearToken::from_yoctonear(1_250_000_000_000_000_000_000 + 1);
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

#[allow(dead_code)]
#[ext_contract(ext_ft)]
trait FungibleToken {
    fn ft_balance_of(&self, account_id: AccountId) -> U128;
    fn storage_deposit(&mut self, account_id: Option<AccountId>, registration_only: Option<bool>);
}

#[allow(dead_code)]
#[ext_contract(ext_active_signer)]
trait ActiveSigner {
    fn swap_owner(
        &mut self,
        wallet: AccountId,
        new_public_key: String,
        expected_current: Option<String>,
    );
}

#[allow(dead_code)]
#[ext_contract(ext_mpc_recovery)]
trait MpcRecovery {
    fn on_wallet_transferred(&mut self, wallet: AccountId);
}

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Admins,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct HosExtension {
    pub(crate) admins: IterableSet<AccountId>,
    pub(crate) registry: AccountId,
    pub(crate) active_signer: AccountId,
    pub(crate) recovery: AccountId,
    pub(crate) paused: bool,
    pub(crate) version: u8,
}

#[near]
impl HosExtension {
    #[init]
    pub fn new(
        admin: AccountId,
        registry: AccountId,
        active_signer: AccountId,
        recovery: AccountId,
    ) -> Self {
        let mut admins = IterableSet::new(StorageKey::Admins);
        admins.insert(admin);
        Self {
            admins,
            registry,
            active_signer,
            recovery,
            paused: false,
            version: CONTRACT_VERSION,
        }
    }

    #[handle_result]
    pub fn pause(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.paused = true;
        Event::ContractPaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn unpause(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.paused = false;
        Event::ContractUnpaused {
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn add_admin(&mut self, account: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.admins.insert(account.clone());
        Event::AdminAdded {
            account,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn remove_admin(&mut self, account: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        if self.admins.len() <= 1 {
            return Err(ContractError::CannotRemoveLastAdmin);
        }
        self.admins.remove(&account);
        Event::AdminRemoved {
            account,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn set_registry(&mut self, registry: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.registry = registry.clone();
        Event::RegistryUpdated {
            registry,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn set_active_signer(&mut self, active_signer: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.active_signer = active_signer.clone();
        Event::ActiveSignerUpdated {
            active_signer,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn set_recovery(&mut self, recovery: AccountId) -> Result<(), ContractError> {
        self.assert_admin()?;
        self.recovery = recovery.clone();
        Event::RecoveryUpdated {
            recovery,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    pub fn migrate(&mut self) -> Result<(), ContractError> {
        self.assert_admin()?;
        if self.version >= CONTRACT_VERSION {
            return Err(ContractError::AlreadyAtCurrentVersion);
        }
        self.version = CONTRACT_VERSION;
        Ok(())
    }

    #[handle_result]
    pub fn force_transfer(
        &mut self,
        wallet: AccountId,
        new_public_key: PublicKey,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        let raw_key = ed25519_base58(&new_public_key)?;
        Event::ForceTransferRequested {
            wallet: wallet.clone(),
            new_public_key,
            by: env::predecessor_account_id(),
        }
        .emit();
        let _ = ext_mpc_recovery::ext(self.recovery.clone())
            .with_static_gas(GAS_FOR_RESET)
            .on_wallet_transferred(wallet.clone());
        Ok(ext_active_signer::ext(self.active_signer.clone())
            .with_static_gas(GAS_FOR_SWAP_OWNER)
            .swap_owner(wallet, raw_key, None))
    }

    #[payable]
    #[handle_result]
    pub fn sweep_ft(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        if env::attached_deposit() < MIN_SWEEP_ATTACHED {
            return Err(ContractError::InsufficientDeposit);
        }
        Event::SweepRequested {
            wallet: wallet.clone(),
            ft: ft.clone(),
            destination: destination.clone(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(ext_ft::ext(ft.clone())
            .with_static_gas(GAS_FOR_BALANCE_QUERY)
            .ft_balance_of(wallet.clone())
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_BALANCE_CB)
                    .after_balance_for_sweep(wallet, ft, destination),
            ))
    }

    #[private]
    pub fn after_balance_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        #[callback_result] balance: Result<U128, PromiseError>,
    ) -> PromiseOrValue<()> {
        let balance = match balance {
            Ok(v) => v.0,
            Err(_) => {
                Event::SweepSkipped {
                    wallet: wallet.clone(),
                    ft: ft.clone(),
                    reason: "balance_query_failed".to_string(),
                }
                .emit();
                return PromiseOrValue::Value(());
            }
        };
        if balance == 0 {
            Event::SweepSkipped {
                wallet: wallet.clone(),
                ft: ft.clone(),
                reason: "zero_balance".to_string(),
            }
            .emit();
            return PromiseOrValue::Value(());
        }

        PromiseOrValue::Promise(
            ext_ft::ext(ft.clone())
                .with_static_gas(GAS_FOR_STORAGE_DEPOSIT)
                .with_attached_deposit(STORAGE_DEPOSIT_AMOUNT)
                .storage_deposit(Some(destination.clone()), Some(true))
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_STORAGE_CB)
                        .after_storage_for_sweep(wallet, ft, destination, U128(balance)),
                ),
        )
    }

    #[private]
    pub fn after_storage_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        balance: U128,
    ) -> PromiseOrValue<()> {
        if !near_sdk::is_promise_success() {
            Event::SweepFailed {
                wallet: wallet.clone(),
                ft: ft.clone(),
                reason: "storage_deposit_failed".to_string(),
            }
            .emit();
            return PromiseOrValue::Value(());
        }

        let request = sweep_request(&ft, &destination, balance);

        PromiseOrValue::Promise(
            ext_wallet::ext(wallet.clone())
                .with_attached_deposit(ONE_YOCTO)
                .with_static_gas(GAS_FOR_EXTENSION_CALL)
                .w_execute_extension(request)
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_SETTLE_CB)
                        .after_sweep_settled(wallet, ft, destination, balance),
                ),
        )
    }

    #[private]
    pub fn after_sweep_settled(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        amount: U128,
    ) {
        if near_sdk::is_promise_success() {
            Event::SweepCompleted {
                wallet,
                ft,
                destination,
                amount,
            }
            .emit();
        } else {
            Event::SweepFailed {
                wallet,
                ft,
                reason: "ft_transfer_failed".to_string(),
            }
            .emit();
        }
    }

    pub fn get_version(&self) -> u8 {
        self.version
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn get_admins(&self) -> Vec<AccountId> {
        self.admins.iter().cloned().collect()
    }

    pub fn get_registry(&self) -> AccountId {
        self.registry.clone()
    }

    pub fn get_active_signer(&self) -> AccountId {
        self.active_signer.clone()
    }

    pub fn get_recovery(&self) -> AccountId {
        self.recovery.clone()
    }

    pub fn min_sweep_attached(&self) -> U128 {
        U128(MIN_SWEEP_ATTACHED.as_yoctonear())
    }
}

impl HosExtension {
    fn assert_admin(&self) -> Result<(), ContractError> {
        if !self.admins.contains(&env::predecessor_account_id()) {
            return Err(ContractError::OnlyAdmin);
        }
        Ok(())
    }

    fn assert_registry(&self) -> Result<(), ContractError> {
        if env::predecessor_account_id() != self.registry {
            return Err(ContractError::OnlyRegistry);
        }
        Ok(())
    }

    fn assert_not_paused(&self) -> Result<(), ContractError> {
        if self.paused {
            return Err(ContractError::Paused);
        }
        Ok(())
    }
}

fn ed25519_base58(key: &PublicKey) -> Result<String, ContractError> {
    hos_common::ed25519_base58(key).ok_or(ContractError::NotEd25519)
}

fn sweep_request(ft: &AccountId, destination: &AccountId, amount: U128) -> Request {
    let transfer = FunctionCallAction::new("ft_transfer")
        .args_json(json!({
            "receiver_id": destination,
            "amount": amount,
            "memo": "hos-tla reclaim",
        }))
        .attached_deposit(ONE_YOCTO)
        .min_gas(GAS_FOR_FT_TRANSFER);
    Request::new().out(PromiseSingle::new(ft.clone()).function_call(transfer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;
    use std::str::FromStr;

    const ADMIN: &str = "hos.testnet";
    const REGISTRY: &str = "tla-registry.testnet";
    const SIGNER: &str = "active-signer.testnet";
    const RECOVERY: &str = "mpc-recovery.testnet";
    const WALLET: &str = "alice.tla.testnet";
    const TOKEN: &str = "token.testnet";
    const DEST: &str = "treasury.testnet";
    const NEW_KEY: &str = "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847";

    fn acc(s: &str) -> AccountId {
        AccountId::from_str(s).unwrap()
    }

    fn key() -> PublicKey {
        PublicKey::from_str(NEW_KEY).unwrap()
    }

    fn ctx(predecessor: &str, deposit: u128) {
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc("hos-extension.testnet"))
            .predecessor_account_id(acc(predecessor))
            .attached_deposit(NearToken::from_yoctonear(deposit))
            .build());
    }

    fn deploy() -> HosExtension {
        ctx(ADMIN, 0);
        HosExtension::new(acc(ADMIN), acc(REGISTRY), acc(SIGNER), acc(RECOVERY))
    }

    fn sweep_deposit() -> u128 {
        MIN_SWEEP_ATTACHED.as_yoctonear()
    }

    #[test]
    fn registry_force_transfer_returns_promise() {
        let mut c = deploy();
        ctx(REGISTRY, 0);
        let _ = c.force_transfer(acc(WALLET), key());
    }

    #[test]
    fn force_transfer_emits_nep297_event() {
        let mut c = deploy();
        ctx(REGISTRY, 0);
        let _ = c.force_transfer(acc(WALLET), key());
        let logs = near_sdk::test_utils::get_logs();
        let event = logs
            .iter()
            .find(|l| l.contains("force_transfer_requested"))
            .expect("force_transfer_requested event emitted");
        let json: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_str(event.trim_start_matches("EVENT_JSON:")).unwrap();
        assert_eq!(json["standard"], "hos_tla_extension");
        assert_eq!(json["version"], "1.0.0");
        assert_eq!(json["event"], "force_transfer_requested");
        assert_eq!(json["data"]["wallet"], WALLET);
        assert_eq!(json["data"]["new_public_key"], NEW_KEY);
        assert_eq!(json["data"]["by"], REGISTRY);
    }

    #[test]
    fn force_transfer_rejects_outsider() {
        let mut c = deploy();
        ctx("attacker.testnet", 0);
        assert!(matches!(
            c.force_transfer(acc(WALLET), key()),
            Err(ContractError::OnlyRegistry)
        ));
    }

    #[test]
    fn force_transfer_rejects_when_paused() {
        let mut c = deploy();
        ctx(ADMIN, 0);
        c.pause().unwrap();
        ctx(REGISTRY, 0);
        assert!(matches!(
            c.force_transfer(acc(WALLET), key()),
            Err(ContractError::Paused)
        ));
    }

    #[test]
    fn force_transfer_rejects_secp256k1() {
        let mut c = deploy();
        ctx(REGISTRY, 0);
        let secp = PublicKey::from_str(
            "secp256k1:qMoRgcoXai4mBPsdbHi1wfyxF9TdbPCF4qSDQTRP3TfescSRoUdSx6nmeQoN3aiwGzwMyGXAb1gUjBTv5AY8DXj",
        )
        .unwrap();
        assert!(matches!(
            c.force_transfer(acc(WALLET), secp),
            Err(ContractError::NotEd25519)
        ));
    }

    #[test]
    fn registry_sweep_ft_returns_promise() {
        let mut c = deploy();
        ctx(REGISTRY, sweep_deposit());
        let _ = c.sweep_ft(acc(WALLET), acc(TOKEN), acc(DEST));
    }

    #[test]
    fn sweep_ft_rejects_outsider() {
        let mut c = deploy();
        ctx("attacker.testnet", sweep_deposit());
        assert!(matches!(
            c.sweep_ft(acc(WALLET), acc(TOKEN), acc(DEST)),
            Err(ContractError::OnlyRegistry)
        ));
    }

    #[test]
    fn sweep_ft_rejects_underfunded() {
        let mut c = deploy();
        ctx(REGISTRY, sweep_deposit() - 1);
        assert!(matches!(
            c.sweep_ft(acc(WALLET), acc(TOKEN), acc(DEST)),
            Err(ContractError::InsufficientDeposit)
        ));
    }

    #[test]
    fn sweep_ft_rejects_when_paused() {
        let mut c = deploy();
        ctx(ADMIN, 0);
        c.pause().unwrap();
        ctx(REGISTRY, sweep_deposit());
        assert!(matches!(
            c.sweep_ft(acc(WALLET), acc(TOKEN), acc(DEST)),
            Err(ContractError::Paused)
        ));
    }

    #[test]
    fn zero_balance_skips_sweep() {
        let mut c = deploy();
        ctx(acc("hos-extension.testnet").as_str(), 0);
        let out = c.after_balance_for_sweep(acc(WALLET), acc(TOKEN), acc(DEST), Ok(U128(0)));
        assert!(matches!(out, PromiseOrValue::Value(())));
    }

    #[test]
    fn failed_balance_query_skips_sweep() {
        let mut c = deploy();
        ctx(acc("hos-extension.testnet").as_str(), 0);
        let out = c.after_balance_for_sweep(
            acc(WALLET),
            acc(TOKEN),
            acc(DEST),
            Err(PromiseError::Failed),
        );
        assert!(matches!(out, PromiseOrValue::Value(())));
    }

    #[test]
    fn nonzero_balance_continues_sweep() {
        let mut c = deploy();
        ctx(acc("hos-extension.testnet").as_str(), 0);
        let out =
            c.after_balance_for_sweep(acc(WALLET), acc(TOKEN), acc(DEST), Ok(U128(1_000_000)));
        assert!(matches!(out, PromiseOrValue::Promise(_)));
    }

    #[test]
    fn ed25519_base58_strips_curve_prefix() {
        let raw = ed25519_base58(&key()).unwrap();
        assert!(!raw.contains(':'));
        assert_eq!(format!("ed25519:{raw}"), NEW_KEY);
    }

    #[test]
    fn sweep_request_targets_token_contract() {
        let request = sweep_request(&acc(TOKEN), &acc(DEST), U128(5));
        assert!(!request.out.is_empty());
        assert!(request.ops.is_empty());
    }

    #[test]
    fn admin_lifecycle() {
        let mut c = deploy();
        ctx(ADMIN, 0);
        c.add_admin(acc("ops.testnet")).unwrap();
        assert_eq!(c.get_admins().len(), 2);
        c.remove_admin(acc("ops.testnet")).unwrap();
        assert_eq!(c.get_admins().len(), 1);
    }

    #[test]
    fn last_admin_protected() {
        let mut c = deploy();
        ctx(ADMIN, 0);
        assert!(matches!(
            c.remove_admin(acc(ADMIN)),
            Err(ContractError::CannotRemoveLastAdmin)
        ));
    }

    #[test]
    fn outsider_cannot_pause() {
        let mut c = deploy();
        ctx("attacker.testnet", 0);
        assert!(matches!(c.pause(), Err(ContractError::OnlyAdmin)));
    }

    #[test]
    fn admin_can_rewire() {
        let mut c = deploy();
        ctx(ADMIN, 0);
        c.set_registry(acc("registry2.testnet")).unwrap();
        c.set_active_signer(acc("signer2.testnet")).unwrap();
        c.set_recovery(acc("recovery2.testnet")).unwrap();
        assert_eq!(c.get_registry(), acc("registry2.testnet"));
        assert_eq!(c.get_active_signer(), acc("signer2.testnet"));
        assert_eq!(c.get_recovery(), acc("recovery2.testnet"));
    }

    #[test]
    fn views_expose_config() {
        let c = deploy();
        assert_eq!(c.get_registry(), acc(REGISTRY));
        assert_eq!(c.get_active_signer(), acc(SIGNER));
        assert_eq!(c.get_recovery(), acc(RECOVERY));
        assert_eq!(c.get_version(), 1);
        assert!(!c.is_paused());
        assert_eq!(c.min_sweep_attached().0, MIN_SWEEP_ATTACHED.as_yoctonear());
    }
}
