mod error;
mod events;
mod state;

use core::marker::PhantomData;
use std::str::FromStr;
use std::time::Duration;

use defuse_wallet::ext_wallet;
use defuse_wallet::signature::ed25519::{Ed25519, Ed25519PublicKey};
use defuse_wallet::signature::{
    Borsh, Deadline, DomainPrefix, RequestMessage, Sha256, SigningStandard,
};
use defuse_wallet::Nonces;
use near_sdk::borsh::BorshSerialize;
use near_sdk::store::{IterableSet, LookupMap};
use near_sdk::{env, near, require, AccountId, BorshStorageKey, Gas, PanicOnDefault, Promise};

use crate::events::Event;
use crate::state::SignerEntry;

type Pipeline = Borsh<DomainPrefix<Sha256<Ed25519>>>;
type FreezePipeline = Borsh<FreezeDomainPrefix<Sha256<Ed25519>>>;

const CHAIN_ID: &str = "mainnet";
const WALLET_GAS: Gas = Gas::from_tgas(50);
const BY_MARKETPLACE: &str = "marketplace";
const BY_RECOVERY: &str = "recovery";
const FREEZE_DOMAIN: &[u8] = b"NEAR_HOS_ACTIVE_SIGNER_FREEZE/V1";

struct FreezeDomainPrefix<S>(PhantomData<S>)
where
    S: SigningStandard<Vec<u8>> + ?Sized;

impl<M, S> SigningStandard<M> for FreezeDomainPrefix<S>
where
    S: SigningStandard<Vec<u8>> + ?Sized,
    M: AsRef<[u8]>,
{
    type PublicKey = S::PublicKey;

    fn verify(msg: M, public_key: &Self::PublicKey, signature: &str) -> bool {
        S::verify(
            [FREEZE_DOMAIN, msg.as_ref()].concat(),
            public_key,
            signature,
        )
    }
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct FreezeMessage {
    pub chain_id: String,
    pub signer_id: AccountId,
    pub nonce: u32,
    pub created_at_secs: u32,
    pub timeout_secs: u32,
}

#[derive(BorshSerialize, BorshStorageKey)]
#[borsh(crate = "near_sdk::borsh")]
enum StorageKey {
    Admins,
    Minters,
    Signers,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct ActiveSigner {
    admins: IterableSet<AccountId>,
    minters: IterableSet<AccountId>,
    marketplace_authority: AccountId,
    recovery_authority: AccountId,
    timeout_secs: u32,
    signers: LookupMap<AccountId, SignerEntry>,
}

#[near]
impl ActiveSigner {
    #[init]
    pub fn new(
        admin: AccountId,
        marketplace_authority: AccountId,
        recovery_authority: AccountId,
        timeout_secs: u32,
    ) -> Self {
        let mut admins = IterableSet::new(StorageKey::Admins);
        admins.insert(admin);
        Self {
            admins,
            minters: IterableSet::new(StorageKey::Minters),
            marketplace_authority,
            recovery_authority,
            timeout_secs,
            signers: LookupMap::new(StorageKey::Signers),
        }
    }

    pub fn add_minter(&mut self, minter: AccountId) {
        self.assert_admin();
        if self.minters.insert(minter.clone()) {
            Event::MinterAdded { minter }.emit();
        }
    }

    pub fn remove_minter(&mut self, minter: AccountId) {
        self.assert_admin();
        if self.minters.remove(&minter) {
            Event::MinterRemoved { minter }.emit();
        }
    }

    pub fn add_admin(&mut self, admin: AccountId) {
        self.assert_admin();
        if self.admins.insert(admin.clone()) {
            Event::AdminAdded { admin }.emit();
        }
    }

    pub fn remove_admin(&mut self, admin: AccountId) {
        self.assert_admin();
        require!(self.admins.len() > 1, error::LAST_ADMIN);
        if self.admins.remove(&admin) {
            Event::AdminRemoved { admin }.emit();
        }
    }

    pub fn install_signer(&mut self, wallet: AccountId, public_key: String) {
        self.assert_minter();
        require!(
            is_direct_subaccount(&wallet, &env::predecessor_account_id()),
            error::WALLET_NOT_UNDER_MINTER
        );
        require!(self.signers.get(&wallet).is_none(), error::SIGNER_EXISTS);
        let public_key = parse_key(&public_key);
        let timeout = Duration::from_secs(self.timeout_secs.into());
        self.signers.insert(
            wallet.clone(),
            SignerEntry {
                public_key,
                nonces: Nonces::new(timeout),
                last_signed_at: 0,
                frozen: false,
            },
        );
        Event::SignerInstalled { wallet }.emit();
    }

    #[payable]
    pub fn submit_signed_request(
        &mut self,
        wallet: AccountId,
        msg: RequestMessage,
        proof: String,
    ) -> Promise {
        require!(!env::attached_deposit().is_zero(), error::DEPOSIT_REQUIRED);
        require!(msg.chain_id == CHAIN_ID, error::WRONG_CHAIN);
        require!(msg.signer_id == wallet, error::SIGNER_MISMATCH);
        require!(msg.request.ops.is_empty(), error::OPS_NOT_ALLOWED);
        let deposit = env::attached_deposit();
        let entry = self
            .signers
            .get_mut(&wallet)
            .unwrap_or_else(|| env::panic_str(error::NO_SIGNER));
        require!(!entry.frozen, error::FROZEN);
        require!(
            Pipeline::verify(&msg, &entry.public_key, &proof),
            error::BAD_SIGNATURE
        );
        entry
            .nonces
            .commit(msg.nonce, msg.created_at, msg.timeout)
            .unwrap_or_else(|_| env::panic_str(error::NONCE_REJECTED));
        entry.last_signed_at = env::block_timestamp();
        Event::RequestExecuted {
            wallet: wallet.clone(),
            nonce: msg.nonce,
        }
        .emit();
        ext_wallet::ext(wallet)
            .with_attached_deposit(deposit)
            .with_static_gas(WALLET_GAS)
            .w_execute_extension(msg.request)
    }

    pub fn swap_owner(
        &mut self,
        wallet: AccountId,
        new_public_key: String,
        expected_current: Option<String>,
    ) -> bool {
        let new_public_key = parse_key(&new_public_key);
        let timeout = Duration::from_secs(self.timeout_secs.into());
        let caller = env::predecessor_account_id();
        let marketplace = caller == self.marketplace_authority;
        let recovery = caller == self.recovery_authority;
        require!(marketplace || recovery, error::UNAUTHORIZED);
        let entry = self
            .signers
            .get_mut(&wallet)
            .unwrap_or_else(|| env::panic_str(error::NO_SIGNER));
        if marketplace {
            require!(!entry.frozen, error::FROZEN);
        } else {
            require!(entry.frozen, error::NOT_FROZEN);
            entry.frozen = false;
        }
        if let Some(expected) = expected_current {
            if entry.public_key != parse_key(&expected) {
                Event::OwnerSwapVoided {
                    wallet: wallet.clone(),
                }
                .emit();
                return false;
            }
        }
        entry.public_key = new_public_key;
        entry.nonces = Nonces::new(timeout);
        let by = if marketplace {
            BY_MARKETPLACE
        } else {
            BY_RECOVERY
        };
        Event::OwnerSwapped {
            wallet,
            by: by.to_string(),
        }
        .emit();
        true
    }

    pub fn self_freeze(&mut self, wallet: AccountId, msg: FreezeMessage, proof: String) {
        require!(msg.chain_id == CHAIN_ID, error::WRONG_CHAIN);
        require!(msg.signer_id == wallet, error::SIGNER_MISMATCH);
        let entry = self
            .signers
            .get_mut(&wallet)
            .unwrap_or_else(|| env::panic_str(error::NO_SIGNER));
        require!(
            FreezePipeline::verify(&msg, &entry.public_key, &proof),
            error::BAD_SIGNATURE
        );
        let created_at = Deadline::UNIX_EPOCH + Duration::from_secs(msg.created_at_secs.into());
        let timeout = Duration::from_secs(msg.timeout_secs.into());
        entry
            .nonces
            .commit(msg.nonce, created_at, timeout)
            .unwrap_or_else(|_| env::panic_str(error::NONCE_REJECTED));
        entry.frozen = true;
        Event::SelfFrozen { wallet }.emit();
    }

    pub fn freeze(&mut self, wallet: AccountId, expected_current: Option<String>) {
        require!(
            env::predecessor_account_id() == self.recovery_authority,
            error::ONLY_RECOVERY
        );
        let entry = self
            .signers
            .get_mut(&wallet)
            .unwrap_or_else(|| env::panic_str(error::NO_SIGNER));
        if let Some(expected) = expected_current {
            require!(
                entry.public_key == parse_key(&expected),
                error::OWNER_CHANGED
            );
        }
        entry.frozen = true;
        Event::Frozen { wallet }.emit();
    }

    pub fn unfreeze(&mut self, wallet: AccountId) {
        require!(
            env::predecessor_account_id() == self.recovery_authority,
            error::ONLY_RECOVERY
        );
        self.signers
            .get_mut(&wallet)
            .unwrap_or_else(|| env::panic_str(error::NO_SIGNER))
            .frozen = false;
        Event::Unfrozen { wallet }.emit();
    }

    pub fn signer_of(&self, wallet: AccountId) -> Option<String> {
        self.signers.get(&wallet).map(|e| e.public_key.to_string())
    }

    pub fn last_signed_at(&self, wallet: AccountId) -> Option<u64> {
        self.signers.get(&wallet).map(|e| e.last_signed_at)
    }

    pub fn is_frozen(&self, wallet: AccountId) -> Option<bool> {
        self.signers.get(&wallet).map(|e| e.frozen)
    }

    pub fn is_minter(&self, account: AccountId) -> bool {
        self.minters.contains(&account)
    }

    pub fn is_admin(&self, account: AccountId) -> bool {
        self.admins.contains(&account)
    }

    pub fn minters(&self) -> Vec<AccountId> {
        self.minters.iter().cloned().collect()
    }

    pub fn admins(&self) -> Vec<AccountId> {
        self.admins.iter().cloned().collect()
    }
}

impl ActiveSigner {
    fn assert_admin(&self) {
        require!(
            self.admins.contains(&env::predecessor_account_id()),
            error::ONLY_ADMIN
        );
    }

    fn assert_minter(&self) {
        require!(
            self.minters.contains(&env::predecessor_account_id()),
            error::ONLY_MINTER
        );
    }
}

fn parse_key(s: &str) -> Ed25519PublicKey {
    Ed25519PublicKey::from_str(s).unwrap_or_else(|_| env::panic_str(error::BAD_KEY))
}

fn is_direct_subaccount(wallet: &AccountId, parent: &AccountId) -> bool {
    wallet
        .as_str()
        .strip_suffix(parent.as_str())
        .and_then(|prefix| prefix.strip_suffix('.'))
        .is_some_and(|label| !label.is_empty() && !label.contains('.'))
}

#[cfg(test)]
mod tests;
