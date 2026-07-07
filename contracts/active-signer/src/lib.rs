mod error;
mod events;
mod nep413;
mod state;

use core::marker::PhantomData;
use std::str::FromStr;
use std::time::Duration;

use defuse_wallet::signature::ed25519::{Ed25519, Ed25519PublicKey};
use defuse_wallet::signature::{Borsh, Deadline, Sha256, SigningStandard};
use defuse_wallet::Nonces;
use hos_common::tx::{build_transaction, mpc_path, to_hex, TxAction};
use near_sdk::borsh::BorshSerialize;
use near_sdk::json_types::{Base58CryptoHash, Base64VecU8, U64};
use near_sdk::serde_json::{json, Value};
use near_sdk::store::{IterableSet, LookupMap};
use near_sdk::{
    env, near, require, AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise,
    PromiseError, PublicKey,
};

use crate::events::Event;
use crate::state::{FreezeState, SignerEntry};

pub const CHAIN_ID: &str = "mainnet";
const MIN_TIMEOUT_SECS: u32 = 60;
const MAX_TIMEOUT_SECS: u32 = 2_592_000;
const SIGN_GAS: Gas = Gas::from_tgas(60);
const ON_SIGNED_GAS: Gas = Gas::from_tgas(10);
const ONE_YOCTO: NearToken = NearToken::from_yoctonear(1);
const ED25519_DOMAIN: u64 = 1;
const BY_MARKETPLACE: &str = "marketplace";
const BY_RECOVERY: &str = "recovery";
pub const FREEZE_DOMAIN: &[u8] = b"NEAR_HOS_ACTIVE_SIGNER_FREEZE/V1";
pub const TX_DOMAIN: &[u8] = b"NEAR_HOS_ACTIVE_SIGNER_TX/V1";
pub const MESSAGE_DOMAIN: &[u8] = b"NEAR_HOS_ACTIVE_SIGNER_MSG/V1";

trait DomainTag {
    const DOMAIN: &'static [u8];
}

struct FreezeTag;
struct TxTag;
struct MessageTag;

impl DomainTag for FreezeTag {
    const DOMAIN: &'static [u8] = FREEZE_DOMAIN;
}

impl DomainTag for TxTag {
    const DOMAIN: &'static [u8] = TX_DOMAIN;
}

impl DomainTag for MessageTag {
    const DOMAIN: &'static [u8] = MESSAGE_DOMAIN;
}

struct TaggedDomain<T, S>(PhantomData<(T, S)>)
where
    T: DomainTag,
    S: SigningStandard<Vec<u8>> + ?Sized;

impl<T, M, S> SigningStandard<M> for TaggedDomain<T, S>
where
    T: DomainTag,
    S: SigningStandard<Vec<u8>> + ?Sized,
    M: AsRef<[u8]>,
{
    type PublicKey = S::PublicKey;

    fn verify(msg: M, public_key: &Self::PublicKey, signature: &str) -> bool {
        S::verify([T::DOMAIN, msg.as_ref()].concat(), public_key, signature)
    }
}

type FreezePipeline = Borsh<TaggedDomain<FreezeTag, Sha256<Ed25519>>>;
type TxPipeline = Borsh<TaggedDomain<TxTag, Sha256<Ed25519>>>;
type MessagePipeline = Borsh<TaggedDomain<MessageTag, Sha256<Ed25519>>>;

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct FreezeMessage {
    pub chain_id: String,
    pub signer_id: AccountId,
    pub nonce: u32,
    pub created_at_secs: u32,
    pub timeout_secs: u32,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct TxMessage {
    pub chain_id: String,
    pub signer_id: AccountId,
    pub nonce: u32,
    pub created_at_secs: u32,
    pub timeout_secs: u32,
    pub receiver_id: AccountId,
    pub actions: Vec<TxAction>,
}

#[near(serializers = [borsh, json])]
#[derive(Clone)]
pub struct MessageRequest {
    pub chain_id: String,
    pub signer_id: AccountId,
    pub nonce: u32,
    pub created_at_secs: u32,
    pub timeout_secs: u32,
    pub message: String,
    pub recipient: String,
    pub message_nonce: Base64VecU8,
    pub callback_url: Option<String>,
}

#[near(serializers = [json])]
pub struct MpcSigned {
    pub payload_hash: String,
    pub unsigned_tx_hex: Option<String>,
    pub mpc_signature: Value,
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
    mpc_signer: AccountId,
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
        mpc_signer: AccountId,
        timeout_secs: u32,
    ) -> Self {
        require!(
            (MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&timeout_secs),
            error::INVALID_TIMEOUT
        );
        let mut admins = IterableSet::new(StorageKey::Admins);
        admins.insert(admin);
        Self {
            admins,
            minters: IterableSet::new(StorageKey::Minters),
            marketplace_authority,
            recovery_authority,
            mpc_signer,
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

    pub fn install_signer(
        &mut self,
        wallet: AccountId,
        public_key: String,
        mpc_public_key: PublicKey,
    ) {
        self.assert_minter();
        require!(
            is_direct_subaccount(&wallet, &env::predecessor_account_id()),
            error::WALLET_NOT_UNDER_MINTER
        );
        require!(self.signers.get(&wallet).is_none(), error::SIGNER_EXISTS);
        require!(
            hos_common::is_ed25519(&mpc_public_key),
            error::MPC_NOT_ED25519
        );
        let public_key = parse_key(&public_key);
        let timeout = Duration::from_secs(self.timeout_secs.into());
        self.signers.insert(
            wallet.clone(),
            SignerEntry {
                public_key,
                mpc_public_key,
                nonces: Nonces::new(timeout),
                freeze_nonces: Nonces::new(timeout),
                last_signed_at: 0,
                frozen: FreezeState::Unfrozen,
            },
        );
        Event::SignerInstalled { wallet }.emit();
    }

    #[payable]
    pub fn submit_signed_tx(
        &mut self,
        wallet: AccountId,
        msg: TxMessage,
        proof: String,
        tx_nonce: U64,
        block_hash: Base58CryptoHash,
    ) -> Promise {
        require!(
            env::attached_deposit() == ONE_YOCTO,
            error::ONE_YOCTO_REQUIRED
        );
        require!(msg.chain_id == CHAIN_ID, error::WRONG_CHAIN);
        require!(msg.signer_id == wallet, error::SIGNER_MISMATCH);
        require!(!msg.actions.is_empty(), error::NO_ACTIONS);
        let mpc_public_key = self.authorize(
            &wallet,
            msg.nonce,
            msg.created_at_secs,
            msg.timeout_secs,
            |key| TxPipeline::verify(&msg, key, &proof),
        );
        let unsigned = build_transaction(
            &wallet,
            &mpc_public_key,
            tx_nonce.0,
            &msg.receiver_id,
            &block_hash.into(),
            &msg.actions,
        );
        Event::TxSigned {
            wallet: wallet.clone(),
            nonce: msg.nonce,
        }
        .emit();
        self.sign_payload(&wallet, &env::sha256(&unsigned), Some(to_hex(&unsigned)))
    }

    #[payable]
    pub fn submit_signed_message(
        &mut self,
        wallet: AccountId,
        msg: MessageRequest,
        proof: String,
    ) -> Promise {
        require!(
            env::attached_deposit() == ONE_YOCTO,
            error::ONE_YOCTO_REQUIRED
        );
        require!(msg.chain_id == CHAIN_ID, error::WRONG_CHAIN);
        require!(msg.signer_id == wallet, error::SIGNER_MISMATCH);
        let message_nonce = <[u8; 32]>::try_from(msg.message_nonce.0.as_slice())
            .unwrap_or_else(|_| env::panic_str(error::BAD_MESSAGE_NONCE));
        self.authorize(
            &wallet,
            msg.nonce,
            msg.created_at_secs,
            msg.timeout_secs,
            |key| MessagePipeline::verify(&msg, key, &proof),
        );
        let payload = nep413::payload(
            &msg.message,
            &message_nonce,
            &msg.recipient,
            msg.callback_url.as_deref(),
        );
        Event::MessageSigned {
            wallet: wallet.clone(),
            nonce: msg.nonce,
        }
        .emit();
        self.sign_payload(&wallet, &env::sha256(&payload), None)
    }

    #[payable]
    pub fn authority_sign_tx(
        &mut self,
        wallet: AccountId,
        receiver_id: AccountId,
        actions: Vec<TxAction>,
        tx_nonce: U64,
        block_hash: Base58CryptoHash,
    ) -> Promise {
        require!(
            env::attached_deposit() == ONE_YOCTO,
            error::ONE_YOCTO_REQUIRED
        );
        require!(
            env::predecessor_account_id() == self.marketplace_authority,
            error::ONLY_MARKETPLACE
        );
        require!(!actions.is_empty(), error::NO_ACTIONS);
        let entry = self
            .signers
            .get_mut(&wallet)
            .unwrap_or_else(|| env::panic_str(error::NO_SIGNER));
        require!(entry.frozen == FreezeState::Unfrozen, error::FROZEN);
        entry.last_signed_at = env::block_timestamp();
        let mpc_public_key = entry.mpc_public_key.clone();
        let unsigned = build_transaction(
            &wallet,
            &mpc_public_key,
            tx_nonce.0,
            &receiver_id,
            &block_hash.into(),
            &actions,
        );
        Event::AuthorityTxSigned {
            wallet: wallet.clone(),
        }
        .emit();
        self.sign_payload(&wallet, &env::sha256(&unsigned), Some(to_hex(&unsigned)))
    }

    #[private]
    pub fn on_signed(
        &self,
        payload_hash: String,
        unsigned_tx_hex: Option<String>,
        #[callback_result] mpc_signature: Result<Value, PromiseError>,
    ) -> Option<MpcSigned> {
        match mpc_signature {
            Ok(mpc_signature) => Some(MpcSigned {
                payload_hash,
                unsigned_tx_hex,
                mpc_signature,
            }),
            Err(_) => None,
        }
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
            require!(entry.frozen == FreezeState::Unfrozen, error::FROZEN);
        } else {
            require!(entry.frozen != FreezeState::Unfrozen, error::NOT_FROZEN);
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
        entry.frozen = FreezeState::Unfrozen;
        entry.public_key = new_public_key;
        entry.nonces = Nonces::new(timeout);
        entry.freeze_nonces = Nonces::new(timeout);
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
            .freeze_nonces
            .commit(msg.nonce, created_at, timeout)
            .unwrap_or_else(|_| env::panic_str(error::NONCE_REJECTED));
        entry.frozen = FreezeState::SelfFrozen;
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
        if entry.frozen != FreezeState::SelfFrozen {
            entry.frozen = FreezeState::RecoveryFrozen;
        }
        Event::Frozen { wallet }.emit();
    }

    pub fn unfreeze(&mut self, wallet: AccountId) {
        require!(
            env::predecessor_account_id() == self.recovery_authority,
            error::ONLY_RECOVERY
        );
        let entry = self
            .signers
            .get_mut(&wallet)
            .unwrap_or_else(|| env::panic_str(error::NO_SIGNER));
        if entry.frozen == FreezeState::SelfFrozen {
            Event::UnfreezeRefused { wallet }.emit();
            return;
        }
        entry.frozen = FreezeState::Unfrozen;
        Event::Unfrozen { wallet }.emit();
    }

    pub fn signer_of(&self, wallet: AccountId) -> Option<String> {
        self.signers.get(&wallet).map(|e| e.public_key.to_string())
    }

    pub fn mpc_key_of(&self, wallet: AccountId) -> Option<PublicKey> {
        self.signers.get(&wallet).map(|e| e.mpc_public_key.clone())
    }

    pub fn last_signed_at(&self, wallet: AccountId) -> Option<u64> {
        self.signers.get(&wallet).map(|e| e.last_signed_at)
    }

    pub fn is_frozen(&self, wallet: AccountId) -> Option<bool> {
        self.signers
            .get(&wallet)
            .map(|e| e.frozen != FreezeState::Unfrozen)
    }

    pub fn freeze_state(&self, wallet: AccountId) -> Option<FreezeState> {
        self.signers.get(&wallet).map(|e| e.frozen)
    }

    pub fn mpc_signer(&self) -> AccountId {
        self.mpc_signer.clone()
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

    fn authorize(
        &mut self,
        wallet: &AccountId,
        nonce: u32,
        created_at_secs: u32,
        timeout_secs: u32,
        verify: impl FnOnce(&Ed25519PublicKey) -> bool,
    ) -> PublicKey {
        let entry = self
            .signers
            .get_mut(wallet)
            .unwrap_or_else(|| env::panic_str(error::NO_SIGNER));
        require!(entry.frozen == FreezeState::Unfrozen, error::FROZEN);
        require!(verify(&entry.public_key), error::BAD_SIGNATURE);
        let created_at = Deadline::UNIX_EPOCH + Duration::from_secs(created_at_secs.into());
        let timeout = Duration::from_secs(timeout_secs.into());
        entry
            .nonces
            .commit(nonce, created_at, timeout)
            .unwrap_or_else(|_| env::panic_str(error::NONCE_REJECTED));
        entry.last_signed_at = env::block_timestamp();
        entry.mpc_public_key.clone()
    }

    fn sign_payload(
        &self,
        wallet: &AccountId,
        payload: &[u8],
        unsigned_tx_hex: Option<String>,
    ) -> Promise {
        let payload_hash = to_hex(payload);
        let args = json!({
            "request": {
                "path": mpc_path(wallet),
                "payload_v2": { "Eddsa": payload_hash },
                "domain_id": ED25519_DOMAIN,
            }
        })
        .to_string()
        .into_bytes();
        Promise::new(self.mpc_signer.clone())
            .function_call("sign".to_string(), args, ONE_YOCTO, SIGN_GAS)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(ON_SIGNED_GAS)
                    .on_signed(payload_hash, unsigned_tx_hex),
            )
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
