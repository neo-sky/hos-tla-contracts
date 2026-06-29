mod error;
mod events;

use std::collections::BTreeSet;

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
const GAS_FOR_SWAP_CB: Gas = Gas::from_tgas(10);
const GAS_FOR_RESET: Gas = Gas::from_tgas(5);
const GAS_FOR_EXT_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_EXT_VERIFY_CB: Gas = Gas::from_tgas(25);
const GAS_FOR_BALANCE_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_BALANCE_CB: Gas = Gas::from_tgas(35);
const GAS_FOR_STORAGE_DEPOSIT: Gas = Gas::from_tgas(10);
const GAS_FOR_STORAGE_CB: Gas = Gas::from_tgas(30);
const GAS_FOR_EXTENSION_CALL: Gas = Gas::from_tgas(20);
const GAS_FOR_SETTLE_CB: Gas = Gas::from_tgas(8);
const GAS_FOR_FT_TRANSFER: Gas = Gas::from_tgas(10);

const STORAGE_DEPOSIT_AMOUNT: NearToken =
    NearToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO);
const MIN_SWEEP_ATTACHED: NearToken =
    NearToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO + 1);
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);

const EXT_QUERY_FAILED: &str = "could not read wallet extension set";
const NON_CANONICAL_EXTENSIONS: &str = "wallet extension set is not canonical";

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
    pub fn skim(&mut self, amount: U128, to: AccountId) -> Result<Promise, ContractError> {
        self.assert_admin()?;
        let reserve = env::storage_byte_cost()
            .as_yoctonear()
            .saturating_mul(env::storage_usage() as u128);
        let available = env::account_balance().as_yoctonear().saturating_sub(reserve);
        if amount.0 > available {
            return Err(ContractError::InsufficientBalance);
        }
        Event::BalanceSkimmed {
            amount,
            to: to.clone(),
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(Promise::new(to).transfer(NearToken::from_yoctonear(amount.0)))
    }

    #[handle_result]
    pub fn force_transfer(
        &mut self,
        wallet: AccountId,
        new_public_key: PublicKey,
        expected_current: Option<PublicKey>,
    ) -> Result<Promise, ContractError> {
        self.assert_registry()?;
        self.assert_not_paused()?;
        let raw_key = ed25519_base58(&new_public_key)?;
        let expected_raw = match expected_current {
            Some(key) => Some(ed25519_base58(&key)?),
            None => None,
        };
        Event::ForceTransferRequested {
            wallet: wallet.clone(),
            new_public_key,
            by: env::predecessor_account_id(),
        }
        .emit();
        Ok(ext_wallet::ext(wallet.clone())
            .with_static_gas(GAS_FOR_EXT_QUERY)
            .w_extensions()
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_EXT_VERIFY_CB)
                    .after_extensions_checked(wallet, raw_key, expected_raw),
            ))
    }

    #[private]
    pub fn after_extensions_checked(
        &mut self,
        wallet: AccountId,
        new_public_key: String,
        expected_current: Option<String>,
        #[callback_result] extensions: Result<BTreeSet<AccountId>, PromiseError>,
    ) -> Promise {
        let extensions = extensions.unwrap_or_else(|_| env::panic_str(EXT_QUERY_FAILED));
        let mut canonical = BTreeSet::new();
        canonical.insert(self.active_signer.clone());
        canonical.insert(env::current_account_id());
        if extensions != canonical {
            env::panic_str(NON_CANONICAL_EXTENSIONS);
        }
        ext_active_signer::ext(self.active_signer.clone())
            .with_static_gas(GAS_FOR_SWAP_OWNER)
            .swap_owner(wallet.clone(), new_public_key, expected_current)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_SWAP_CB)
                    .after_force_swap(wallet),
            )
    }

    #[private]
    pub fn after_force_swap(
        &mut self,
        wallet: AccountId,
        #[callback_result] swapped: Result<bool, PromiseError>,
    ) -> bool {
        let transferred = matches!(swapped, Ok(true));
        if transferred {
            Event::ForceTransferCompleted {
                wallet: wallet.clone(),
            }
            .emit();
            let _ = ext_mpc_recovery::ext(self.recovery.clone())
                .with_static_gas(GAS_FOR_RESET)
                .on_wallet_transferred(wallet);
        } else {
            Event::ForceTransferVoided { wallet }.emit();
        }
        transferred
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
        if env::attached_deposit() != MIN_SWEEP_ATTACHED {
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
                return self.abort_and_refund(Event::SweepSkipped {
                    wallet,
                    ft,
                    reason: "balance_query_failed".to_string(),
                });
            }
        };
        if balance == 0 {
            return self.abort_and_refund(Event::SweepSkipped {
                wallet,
                ft,
                reason: "zero_balance".to_string(),
            });
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
            return self.abort_and_refund(Event::SweepFailed {
                wallet,
                ft,
                reason: "storage_deposit_failed".to_string(),
            });
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
        // success here means the wallet accepted and scheduled the ft_transfer, which it
        // emits as a detached out-promise; this confirms dispatch, not final settlement.
        if near_sdk::is_promise_success() {
            Event::SweepDispatched {
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
                reason: "wallet_execute_failed".to_string(),
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

    fn refund_registry(&self, amount: NearToken) -> Promise {
        Promise::new(self.registry.clone()).transfer(amount)
    }

    fn abort_and_refund(&self, event: Event) -> PromiseOrValue<()> {
        event.emit();
        PromiseOrValue::Promise(self.refund_registry(MIN_SWEEP_ATTACHED))
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
mod tests;
