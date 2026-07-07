mod error;
mod events;

use crate::error::ContractError;
use crate::events::Event;
use hos_common::tx::TxAction;
use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::{Base58CryptoHash, Base64VecU8, U128, U64};
use near_sdk::serde_json::{json, Value};
use near_sdk::store::IterableSet;
use near_sdk::{
    env, ext_contract, near, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError, PromiseOrValue, PublicKey,
};

const CONTRACT_VERSION: u8 = 1;

const GAS_FOR_SWAP_OWNER: Gas = Gas::from_tgas(8);
const GAS_FOR_SWAP_CB: Gas = Gas::from_tgas(10);
const GAS_FOR_RESET: Gas = Gas::from_tgas(5);
const GAS_FOR_BALANCE_QUERY: Gas = Gas::from_tgas(5);
const GAS_FOR_BALANCE_CB: Gas = Gas::from_tgas(105);
const GAS_FOR_STORAGE_DEPOSIT: Gas = Gas::from_tgas(10);
const GAS_FOR_STORAGE_CB: Gas = Gas::from_tgas(90);
const GAS_FOR_AUTHORITY_SIGN: Gas = Gas::from_tgas(75);
const GAS_FOR_SETTLE_CB: Gas = Gas::from_tgas(8);
const GAS_FOR_FT_TRANSFER: Gas = Gas::from_tgas(10);

const STORAGE_DEPOSIT_AMOUNT: NearToken =
    NearToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO);
const MIN_SWEEP_ATTACHED: NearToken =
    NearToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO + 1);
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
    fn authority_sign_tx(
        &mut self,
        wallet: AccountId,
        receiver_id: AccountId,
        actions: Vec<TxAction>,
        tx_nonce: U64,
        block_hash: Base58CryptoHash,
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
        let available = env::account_balance()
            .as_yoctonear()
            .saturating_sub(reserve);
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
        Ok(ext_active_signer::ext(self.active_signer.clone())
            .with_static_gas(GAS_FOR_SWAP_OWNER)
            .swap_owner(wallet.clone(), raw_key, expected_raw)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_SWAP_CB)
                    .after_force_swap(wallet),
            ))
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
        tx_nonce: U64,
        block_hash: Base58CryptoHash,
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
                    .after_balance_for_sweep(wallet, ft, destination, tx_nonce, block_hash),
            ))
    }

    #[private]
    pub fn after_balance_for_sweep(
        &mut self,
        wallet: AccountId,
        ft: AccountId,
        destination: AccountId,
        tx_nonce: U64,
        block_hash: Base58CryptoHash,
        #[callback_result] balance: Result<U128, PromiseError>,
    ) -> PromiseOrValue<Option<Value>> {
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
                        .after_storage_for_sweep(
                            wallet,
                            ft,
                            destination,
                            U128(balance),
                            tx_nonce,
                            block_hash,
                        ),
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
        tx_nonce: U64,
        block_hash: Base58CryptoHash,
    ) -> PromiseOrValue<Option<Value>> {
        if !near_sdk::is_promise_success() {
            return self.abort_and_refund(Event::SweepFailed {
                wallet,
                ft,
                reason: "storage_deposit_failed".to_string(),
            });
        }

        let action = sweep_action(&destination, balance);

        PromiseOrValue::Promise(
            ext_active_signer::ext(self.active_signer.clone())
                .with_attached_deposit(ONE_YOCTO)
                .with_static_gas(GAS_FOR_AUTHORITY_SIGN)
                .authority_sign_tx(
                    wallet.clone(),
                    ft.clone(),
                    vec![action],
                    tx_nonce,
                    block_hash,
                )
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
        #[callback_result] signed: Result<Value, PromiseError>,
    ) -> Option<Value> {
        match signed {
            Ok(signed) if !signed.is_null() => {
                Event::SweepDispatched {
                    wallet,
                    ft,
                    destination,
                    amount,
                }
                .emit();
                Some(signed)
            }
            _ => {
                Event::SweepFailed {
                    wallet,
                    ft,
                    reason: "authority_sign_failed".to_string(),
                }
                .emit();
                None
            }
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

    fn abort_and_refund(&self, event: Event) -> PromiseOrValue<Option<Value>> {
        event.emit();
        let _ = self.refund_registry(MIN_SWEEP_ATTACHED);
        PromiseOrValue::Value(None)
    }
}

fn ed25519_base58(key: &PublicKey) -> Result<String, ContractError> {
    hos_common::ed25519_base58(key).ok_or(ContractError::NotEd25519)
}

fn sweep_action(destination: &AccountId, amount: U128) -> TxAction {
    let args = json!({
        "receiver_id": destination,
        "amount": amount,
        "memo": "hos-tla reclaim",
    })
    .to_string()
    .into_bytes();
    TxAction::FunctionCall {
        method_name: "ft_transfer".to_string(),
        args: Base64VecU8(args),
        gas: GAS_FOR_FT_TRANSFER,
        deposit: ONE_YOCTO,
    }
}

#[cfg(test)]
mod tests;
