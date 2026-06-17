mod error;
mod events;
mod state;

use std::str::FromStr;
use std::time::Duration;

use defuse_wallet::ext_wallet;
use defuse_wallet::signature::ed25519::{Ed25519, Ed25519PublicKey};
use defuse_wallet::signature::{Borsh, DomainPrefix, RequestMessage, Sha256, SigningStandard};
use defuse_wallet::Nonces;
use near_sdk::borsh::BorshSerialize;
use near_sdk::store::{IterableSet, LookupMap};
use near_sdk::{env, near, require, AccountId, BorshStorageKey, Gas, PanicOnDefault, Promise};

use crate::events::Event;
use crate::state::SignerEntry;

type Pipeline = Borsh<DomainPrefix<Sha256<Ed25519>>>;

const CHAIN_ID: &str = "mainnet";
const WALLET_GAS: Gas = Gas::from_tgas(50);
const BY_MARKETPLACE: &str = "marketplace";
const BY_RECOVERY: &str = "recovery";

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
        if let Some(existing) = self.signers.get(&wallet) {
            require!(!existing.frozen, error::FROZEN);
        }
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
mod tests {
    use super::*;
    use defuse_wallet::signature::Deadline;
    use defuse_wallet::Request;
    use defuse_wallet_sdk::ed25519::ed25519_dalek::SigningKey;
    use defuse_wallet_sdk::Signer;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::{testing_env, NearToken};

    const OWNER: &str = "hos.testnet";
    const MINTER: &str = "tla.testnet";
    const MARKET: &str = "hos-extension.testnet";
    const RECOVERY: &str = "mpc-recovery.testnet";
    const WALLET: &str = "alice.tla.testnet";
    const TS: u64 = 1_000_000_000_000;

    fn acc(s: &str) -> AccountId {
        AccountId::from_str(s).unwrap()
    }

    fn ctx(predecessor: &str, deposit: u128, ts: u64) {
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc("active-signer.testnet"))
            .predecessor_account_id(acc(predecessor))
            .attached_deposit(NearToken::from_yoctonear(deposit))
            .block_timestamp(ts)
            .build());
    }

    fn deploy() -> ActiveSigner {
        ctx(OWNER, 0, 0);
        let mut c = ActiveSigner::new(acc(OWNER), acc(MARKET), acc(RECOVERY), 3600);
        ctx(OWNER, 0, 0);
        c.add_minter(acc(MINTER));
        c
    }

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn install(c: &mut ActiveSigner, k: &SigningKey) {
        ctx(MINTER, 0, TS);
        c.install_signer(acc(WALLET), Signer::public_key(k).to_string());
    }

    fn sign(k: &SigningKey, nonce: u32) -> (RequestMessage, String) {
        ctx("client.testnet", 0, TS);
        let msg = RequestMessage {
            chain_id: CHAIN_ID.to_string(),
            signer_id: acc(WALLET),
            nonce,
            created_at: Deadline::now() - Duration::from_secs(60),
            timeout: Duration::from_secs(3600),
            request: Request::new(),
        };
        let proof = Signer::sign(k, &msg).unwrap();
        (msg, proof)
    }

    #[test]
    fn submit_verifies_and_updates_last_signed_at() {
        let mut c = deploy();
        let k = key(7);
        install(&mut c, &k);
        let (msg, proof) = sign(&k, 1);
        ctx("relayer.testnet", 1, TS);
        let _ = c.submit_signed_request(acc(WALLET), msg, proof);
        assert_eq!(c.last_signed_at(acc(WALLET)), Some(TS));
    }

    #[test]
    #[should_panic(expected = "nonce already used")]
    fn replayed_nonce_rejected() {
        let mut c = deploy();
        let k = key(7);
        install(&mut c, &k);
        let (msg, proof) = sign(&k, 1);
        ctx("relayer.testnet", 1, TS);
        let _ = c.submit_signed_request(acc(WALLET), msg.clone(), proof.clone());
        ctx("relayer.testnet", 1, TS);
        let _ = c.submit_signed_request(acc(WALLET), msg, proof);
    }

    #[test]
    #[should_panic(expected = "invalid signature")]
    fn wrong_key_rejected() {
        let mut c = deploy();
        install(&mut c, &key(7));
        let (msg, proof) = sign(&key(9), 1);
        ctx("relayer.testnet", 1, TS);
        let _ = c.submit_signed_request(acc(WALLET), msg, proof);
    }

    #[test]
    #[should_panic(expected = "wrong chain id")]
    fn wrong_chain_rejected() {
        let mut c = deploy();
        let k = key(7);
        install(&mut c, &k);
        let (mut msg, proof) = sign(&k, 1);
        msg.chain_id = "testnet".to_string();
        ctx("relayer.testnet", 1, TS);
        let _ = c.submit_signed_request(acc(WALLET), msg, proof);
    }

    #[test]
    #[should_panic(expected = "non-zero deposit required")]
    fn zero_deposit_rejected() {
        let mut c = deploy();
        let k = key(7);
        install(&mut c, &k);
        let (msg, proof) = sign(&k, 1);
        ctx("relayer.testnet", 0, TS);
        let _ = c.submit_signed_request(acc(WALLET), msg, proof);
    }

    #[test]
    fn marketplace_swaps_owner() {
        let mut c = deploy();
        install(&mut c, &key(7));
        let new = key(8);
        ctx(MARKET, 0, TS);
        c.swap_owner(acc(WALLET), Signer::public_key(&new).to_string(), None);
        assert_eq!(
            c.signer_of(acc(WALLET)),
            Some(Signer::public_key(&new).to_string())
        );
    }

    #[test]
    #[should_panic(expected = "wallet not frozen")]
    fn recovery_swap_requires_freeze() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(RECOVERY, 0, TS);
        c.swap_owner(acc(WALLET), Signer::public_key(&key(8)).to_string(), None);
    }

    #[test]
    fn recovery_freeze_then_swap_unfreezes() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(RECOVERY, 0, TS);
        c.freeze(acc(WALLET), None);
        assert_eq!(c.is_frozen(acc(WALLET)), Some(true));
        ctx(RECOVERY, 0, TS);
        c.swap_owner(acc(WALLET), Signer::public_key(&key(8)).to_string(), None);
        assert_eq!(c.is_frozen(acc(WALLET)), Some(false));
    }

    #[test]
    fn recovery_swap_with_matching_cas_succeeds() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(RECOVERY, 0, TS);
        c.freeze(acc(WALLET), None);
        ctx(RECOVERY, 0, TS);
        let swapped = c.swap_owner(
            acc(WALLET),
            Signer::public_key(&key(8)).to_string(),
            Some(Signer::public_key(&key(7)).to_string()),
        );
        assert!(swapped);
        assert_eq!(
            c.signer_of(acc(WALLET)),
            Some(Signer::public_key(&key(8)).to_string())
        );
        assert_eq!(c.is_frozen(acc(WALLET)), Some(false));
    }

    #[test]
    fn recovery_swap_with_stale_cas_voids_and_releases() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(RECOVERY, 0, TS);
        c.freeze(acc(WALLET), None);
        ctx(RECOVERY, 0, TS);
        let swapped = c.swap_owner(
            acc(WALLET),
            Signer::public_key(&key(8)).to_string(),
            Some(Signer::public_key(&key(9)).to_string()),
        );
        assert!(!swapped);
        assert_eq!(
            c.signer_of(acc(WALLET)),
            Some(Signer::public_key(&key(7)).to_string())
        );
        assert_eq!(c.is_frozen(acc(WALLET)), Some(false));
    }

    #[test]
    fn freeze_with_matching_cas_succeeds() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(RECOVERY, 0, TS);
        c.freeze(acc(WALLET), Some(Signer::public_key(&key(7)).to_string()));
        assert_eq!(c.is_frozen(acc(WALLET)), Some(true));
    }

    #[test]
    #[should_panic(expected = "owner changed")]
    fn freeze_with_stale_cas_rejected() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(RECOVERY, 0, TS);
        c.freeze(acc(WALLET), Some(Signer::public_key(&key(9)).to_string()));
    }

    #[test]
    fn marketplace_swap_ignores_cas() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(MARKET, 0, TS);
        let swapped = c.swap_owner(acc(WALLET), Signer::public_key(&key(8)).to_string(), None);
        assert!(swapped);
    }

    #[test]
    #[should_panic(expected = "unauthorized")]
    fn unauthorized_swap_rejected() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx("attacker.testnet", 0, TS);
        c.swap_owner(acc(WALLET), Signer::public_key(&key(8)).to_string(), None);
    }

    #[test]
    #[should_panic(expected = "frozen by recovery")]
    fn frozen_blocks_submit() {
        let mut c = deploy();
        let k = key(7);
        install(&mut c, &k);
        ctx(RECOVERY, 0, TS);
        c.freeze(acc(WALLET), None);
        let (msg, proof) = sign(&k, 1);
        ctx("relayer.testnet", 1, TS);
        let _ = c.submit_signed_request(acc(WALLET), msg, proof);
    }

    #[test]
    #[should_panic(expected = "frozen by recovery")]
    fn frozen_blocks_marketplace_swap() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(RECOVERY, 0, TS);
        c.freeze(acc(WALLET), None);
        ctx(MARKET, 0, TS);
        c.swap_owner(acc(WALLET), Signer::public_key(&key(8)).to_string(), None);
    }

    #[test]
    #[should_panic(expected = "only minter")]
    fn non_minter_cannot_install() {
        let mut c = deploy();
        ctx("attacker.testnet", 0, TS);
        c.install_signer(acc(WALLET), Signer::public_key(&key(7)).to_string());
    }

    #[test]
    #[should_panic(expected = "only minter")]
    fn admin_is_not_implicitly_a_minter() {
        let mut c = deploy();
        ctx(OWNER, 0, TS);
        c.install_signer(acc(WALLET), Signer::public_key(&key(7)).to_string());
    }

    #[test]
    #[should_panic(expected = "not a sub-account of the minter")]
    fn install_rejects_wallet_outside_minter_namespace() {
        let mut c = deploy();
        ctx(MINTER, 0, TS);
        c.install_signer(
            acc("victim.other.testnet"),
            Signer::public_key(&key(7)).to_string(),
        );
    }

    #[test]
    #[should_panic(expected = "not a sub-account of the minter")]
    fn install_rejects_indirect_subaccount() {
        let mut c = deploy();
        ctx(MINTER, 0, TS);
        c.install_signer(
            acc("deep.alice.tla.testnet"),
            Signer::public_key(&key(7)).to_string(),
        );
    }

    #[test]
    #[should_panic(expected = "not a sub-account of the minter")]
    fn install_rejects_suffix_collision() {
        let mut c = deploy();
        ctx(MINTER, 0, TS);
        c.install_signer(
            acc("eviltla.testnet"),
            Signer::public_key(&key(7)).to_string(),
        );
    }

    #[test]
    #[should_panic(expected = "not a sub-account of the minter")]
    fn install_rejects_minter_account_itself() {
        let mut c = deploy();
        ctx(MINTER, 0, TS);
        c.install_signer(acc(MINTER), Signer::public_key(&key(7)).to_string());
    }

    #[test]
    fn reinstall_on_unfrozen_wallet_overwrites() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(MINTER, 0, TS);
        c.install_signer(acc(WALLET), Signer::public_key(&key(8)).to_string());
        assert_eq!(
            c.signer_of(acc(WALLET)),
            Some(Signer::public_key(&key(8)).to_string())
        );
    }

    #[test]
    #[should_panic(expected = "frozen by recovery")]
    fn reinstall_on_frozen_wallet_rejected() {
        let mut c = deploy();
        install(&mut c, &key(7));
        ctx(RECOVERY, 0, TS);
        c.freeze(acc(WALLET), None);
        ctx(MINTER, 0, TS);
        c.install_signer(acc(WALLET), Signer::public_key(&key(8)).to_string());
    }

    #[test]
    #[should_panic(expected = "only admin")]
    fn non_admin_cannot_add_minter() {
        let mut c = deploy();
        ctx("attacker.testnet", 0, TS);
        c.add_minter(acc("evil.testnet"));
    }

    #[test]
    fn admin_can_manage_minters() {
        let mut c = deploy();
        ctx(OWNER, 0, TS);
        c.add_minter(acc("tla2.testnet"));
        assert!(c.is_minter(acc("tla2.testnet")));
        assert!(c.is_minter(acc(MINTER)));
        ctx(OWNER, 0, TS);
        c.remove_minter(acc(MINTER));
        assert!(!c.is_minter(acc(MINTER)));
    }

    #[test]
    fn second_minter_can_install_a_different_wallet() {
        let mut c = deploy();
        ctx(OWNER, 0, TS);
        c.add_minter(acc("tla2.testnet"));
        ctx("tla2.testnet", 0, TS);
        c.install_signer(
            acc("bob.tla2.testnet"),
            Signer::public_key(&key(9)).to_string(),
        );
        assert!(c.signer_of(acc("bob.tla2.testnet")).is_some());
    }

    #[test]
    #[should_panic(expected = "cannot remove last admin")]
    fn last_admin_protected() {
        let mut c = deploy();
        ctx(OWNER, 0, TS);
        c.remove_admin(acc(OWNER));
    }

    #[test]
    fn admin_can_add_and_remove_admins() {
        let mut c = deploy();
        ctx(OWNER, 0, TS);
        c.add_admin(acc("hos2.testnet"));
        assert!(c.is_admin(acc("hos2.testnet")));
        assert_eq!(c.admins().len(), 2);
        ctx(OWNER, 0, TS);
        c.remove_admin(acc("hos2.testnet"));
        assert_eq!(c.admins().len(), 1);
    }
}
