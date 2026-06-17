mod error;
mod events;
mod proof;
mod state;
mod tx;

use near_sdk::json_types::{Base58CryptoHash, Base64VecU8, U64};
use near_sdk::serde_json::{json, Value};
use near_sdk::store::LookupMap;
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue, PublicKey,
};

use crate::events::Event;
use crate::state::{Account, Phase, Policy, Target};

const SIGN_GAS: Gas = Gas::from_tgas(60);
const SWAP_GAS: Gas = Gas::from_tgas(15);
const FREEZE_GAS: Gas = Gas::from_tgas(15);
const CALLBACK_GAS: Gas = Gas::from_tgas(20);
const ED25519_DOMAIN: u64 = 1;
const NS_PER_SEC: u64 = 1_000_000_000;

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct MpcRecovery {
    owner: AccountId,
    signer: AccountId,
    transfer_authority: AccountId,
    watchers: Vec<PublicKey>,
    threshold: u32,
    accounts: LookupMap<AccountId, Account>,
}

#[near(serializers = [json])]
pub struct WatcherSignature {
    pub public_key: PublicKey,
    pub signature: Base64VecU8,
}

#[near(serializers = [json])]
pub struct RecoveryResult {
    pub signed_tx_hash: String,
    pub mpc_signature: Value,
}

#[near]
impl MpcRecovery {
    #[init]
    pub fn new(
        owner: AccountId,
        signer: AccountId,
        transfer_authority: AccountId,
        watchers: Vec<PublicKey>,
        threshold: u32,
    ) -> Self {
        require!(
            threshold > 0 && (threshold as usize) <= watchers.len(),
            error::BAD_THRESHOLD
        );
        Self {
            owner,
            signer,
            transfer_authority,
            watchers,
            threshold,
            accounts: LookupMap::new(b"a"),
        }
    }

    pub fn install_policy(
        &mut self,
        account: AccountId,
        target: Target,
        attestation_key: PublicKey,
        timelock_secs: u32,
    ) {
        require!(
            env::predecessor_account_id() == self.owner,
            error::ONLY_OWNER
        );
        self.accounts.insert(
            account.clone(),
            Account {
                policy: Policy {
                    target,
                    attestation_key,
                    timelock_secs,
                },
                round: 0,
                phase: Phase::Idle,
            },
        );
        Event::PolicyInstalled { account }.emit();
    }

    pub fn request_recovery(
        &mut self,
        account: AccountId,
        new_owner: PublicKey,
        round: U64,
        attestation: Base64VecU8,
    ) {
        let contract = env::current_account_id();
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        require!(matches!(entry.phase, Phase::Idle), error::NOT_IDLE);
        require!(round.0 == entry.round, error::STALE_ROUND);
        let message = proof::request_message(&contract, &account, &new_owner, entry.round);
        require!(
            proof::verify(
                &message,
                &into_sig(attestation.into()),
                &entry.policy.attestation_key
            ),
            error::BAD_ATTESTATION
        );
        let round = entry.round;
        entry.phase = Phase::Requested {
            new_owner,
            round,
            requested_at: env::block_timestamp(),
        };
        entry.round += 1;
        Event::Requested { account, round }.emit();
    }

    pub fn submit_verdict(
        &mut self,
        account: AccountId,
        silent: bool,
        signatures: Vec<WatcherSignature>,
    ) -> PromiseOrValue<()> {
        let contract = env::current_account_id();
        let watchers = self.watchers.clone();
        let threshold = self.threshold;
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        let (new_owner, round, requested_at) = match &entry.phase {
            Phase::Requested {
                new_owner,
                round,
                requested_at,
            } => (new_owner.clone(), *round, *requested_at),
            _ => env::panic_str(error::NOT_REQUESTED),
        };
        require!(
            env::block_timestamp()
                >= requested_at + (entry.policy.timelock_secs as u64) * NS_PER_SEC,
            error::TIMELOCK
        );
        let message = proof::verdict_message(&contract, &account, round, silent);
        let sigs: Vec<(PublicKey, [u8; 64])> = signatures
            .into_iter()
            .map(|w| (w.public_key, into_sig(w.signature.into())))
            .collect();
        require!(
            proof::verify_quorum(&message, &sigs, &watchers, threshold),
            error::NO_QUORUM
        );
        if !silent {
            entry.phase = Phase::Idle;
            Event::Cancelled { account, round }.emit();
            return PromiseOrValue::Value(());
        }
        match entry.policy.target.clone() {
            Target::Wallet {
                active_signer,
                bound_owner,
            } => {
                entry.phase = Phase::Approving { new_owner, round };
                PromiseOrValue::Promise(
                    freeze(active_signer, &account, &bound_owner).then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(CALLBACK_GAS)
                            .on_frozen(account, round),
                    ),
                )
            }
            Target::Native { .. } => {
                entry.phase = Phase::Approved { new_owner, round };
                Event::Approved { account, round }.emit();
                PromiseOrValue::Value(())
            }
        }
    }

    #[private]
    pub fn on_frozen(
        &mut self,
        account: AccountId,
        round: u64,
        #[callback_result] result: Result<(), PromiseError>,
    ) {
        let Some(entry) = self.accounts.get_mut(&account) else {
            return;
        };
        let new_owner = match &entry.phase {
            Phase::Approving {
                new_owner,
                round: approving_round,
            } if *approving_round == round => new_owner.clone(),
            _ => return,
        };
        if result.is_ok() {
            entry.phase = Phase::Approved { new_owner, round };
            Event::Approved { account, round }.emit();
        } else {
            entry.phase = Phase::Idle;
            Event::Cancelled { account, round }.emit();
        }
    }

    pub fn finalize_recovery(
        &mut self,
        account: AccountId,
        nonce: U64,
        block_hash: Base58CryptoHash,
    ) -> Promise {
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        let (new_owner, round) = match &entry.phase {
            Phase::Approved { new_owner, round } => (new_owner.clone(), *round),
            _ => env::panic_str(error::NOT_APPROVED),
        };
        entry.phase = Phase::Resolving {
            new_owner: new_owner.clone(),
            round,
        };
        match entry.policy.target.clone() {
            Target::Native {
                mpc_public_key,
                derivation_path,
            } => self.sign_add_key(
                &account,
                &mpc_public_key,
                derivation_path,
                nonce.0,
                block_hash.into(),
                &new_owner,
                round,
            ),
            Target::Wallet {
                active_signer,
                bound_owner,
            } => swap_owner(active_signer, &account, &new_owner, &bound_owner).then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .on_finalized(account, round),
            ),
        }
    }

    pub fn abort_recovery(&mut self, account: AccountId) -> PromiseOrValue<()> {
        require!(
            env::predecessor_account_id() == self.owner,
            error::ONLY_OWNER
        );
        enum AbortAction {
            Release(u64),
            Unfreeze(AccountId, PublicKey, u64),
        }
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        let action = match (&entry.phase, &entry.policy.target) {
            (Phase::Requested { round, .. }, _)
            | (Phase::Approved { round, .. }, Target::Native { .. }) => {
                AbortAction::Release(*round)
            }
            (Phase::Approved { new_owner, round }, Target::Wallet { active_signer, .. }) => {
                AbortAction::Unfreeze(active_signer.clone(), new_owner.clone(), *round)
            }
            (Phase::Idle, _) | (Phase::Approving { .. }, _) | (Phase::Resolving { .. }, _) => {
                env::panic_str(error::NOT_ACTIVE)
            }
        };
        match action {
            AbortAction::Release(round) => {
                entry.phase = Phase::Idle;
                Event::Aborted { account, round }.emit();
                PromiseOrValue::Value(())
            }
            AbortAction::Unfreeze(active_signer, new_owner, round) => {
                entry.phase = Phase::Resolving { new_owner, round };
                PromiseOrValue::Promise(
                    unfreeze(active_signer, &account).then(
                        Self::ext(env::current_account_id())
                            .with_static_gas(CALLBACK_GAS)
                            .on_aborted(account, round),
                    ),
                )
            }
        }
    }

    #[private]
    pub fn on_finalized(
        &mut self,
        account: AccountId,
        round: u64,
        #[callback_result] result: Result<bool, PromiseError>,
    ) {
        let Some(entry) = self.accounts.get_mut(&account) else {
            return;
        };
        let new_owner = match &entry.phase {
            Phase::Resolving {
                new_owner,
                round: resolving_round,
            } if *resolving_round == round => new_owner.clone(),
            _ => return,
        };
        match result {
            Ok(true) => {
                if let Target::Wallet { bound_owner, .. } = &mut entry.policy.target {
                    *bound_owner = new_owner;
                }
                entry.phase = Phase::Idle;
                Event::Finalized { account, round }.emit();
            }
            Ok(false) => {
                entry.phase = Phase::Idle;
                Event::Voided { account, round }.emit();
            }
            Err(_) => {
                entry.phase = Phase::Approved { new_owner, round };
            }
        }
    }

    #[private]
    pub fn on_aborted(
        &mut self,
        account: AccountId,
        round: u64,
        #[callback_result] result: Result<(), PromiseError>,
    ) {
        if self.settle_resolution(&account, round, result.is_ok()) {
            Event::Aborted { account, round }.emit();
        }
    }

    #[private]
    pub fn on_signed(
        &mut self,
        account: AccountId,
        round: u64,
        signed_tx_hash: String,
        #[callback_result] mpc_signature: Result<Value, PromiseError>,
    ) -> Option<RecoveryResult> {
        match mpc_signature {
            Ok(mpc_signature) => {
                if self.settle_resolution(&account, round, true) {
                    Event::Finalized { account, round }.emit();
                    Some(RecoveryResult {
                        signed_tx_hash,
                        mpc_signature,
                    })
                } else {
                    None
                }
            }
            Err(_) => {
                self.settle_resolution(&account, round, false);
                None
            }
        }
    }

    pub fn round_of(&self, account: AccountId) -> Option<u64> {
        self.accounts.get(&account).map(|a| a.round)
    }

    pub fn on_wallet_transferred(&mut self, wallet: AccountId) {
        require!(
            env::predecessor_account_id() == self.transfer_authority,
            error::ONLY_TRANSFER_AUTHORITY
        );
        let removable = match self.accounts.get(&wallet) {
            Some(account) => matches!(account.phase, Phase::Idle | Phase::Requested { .. }),
            None => false,
        };
        if removable {
            self.accounts.remove(&wallet);
            Event::PolicyReset { account: wallet }.emit();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sign_add_key(
        &self,
        account: &AccountId,
        mpc_public_key: &PublicKey,
        path: String,
        nonce: u64,
        block_hash: [u8; 32],
        new_owner: &PublicKey,
        round: u64,
    ) -> Promise {
        let unsigned = tx::add_key_tx(account, mpc_public_key, nonce, &block_hash, new_owner);
        let hash_hex = tx::to_hex(&env::sha256(&unsigned));
        let args = json!({"request": {"path": path, "payload_v2": {"Eddsa": hash_hex}, "domain_id": ED25519_DOMAIN}})
            .to_string()
            .into_bytes();
        Promise::new(self.signer.clone())
            .function_call(
                "sign".to_string(),
                args,
                NearToken::from_yoctonear(1),
                SIGN_GAS,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .on_signed(account.clone(), round, hash_hex),
            )
    }

    fn settle_resolution(&mut self, account: &AccountId, round: u64, success: bool) -> bool {
        let Some(entry) = self.accounts.get_mut(account) else {
            return false;
        };
        let new_owner = match &entry.phase {
            Phase::Resolving {
                new_owner,
                round: resolving_round,
            } if *resolving_round == round => new_owner.clone(),
            _ => return false,
        };
        if success {
            entry.phase = Phase::Idle;
        } else {
            entry.phase = Phase::Approved { new_owner, round };
        }
        success
    }
}

fn call_signer(active_signer: AccountId, method: &str, args: Value, gas: Gas) -> Promise {
    Promise::new(active_signer).function_call(
        method.to_string(),
        args.to_string().into_bytes(),
        NearToken::from_yoctonear(0),
        gas,
    )
}

fn swap_owner(
    active_signer: AccountId,
    account: &AccountId,
    new_owner: &PublicKey,
    bound_owner: &PublicKey,
) -> Promise {
    call_signer(
        active_signer,
        "swap_owner",
        json!({
            "wallet": account,
            "new_public_key": ed25519_base58(new_owner),
            "expected_current": ed25519_base58(bound_owner),
        }),
        SWAP_GAS,
    )
}

fn freeze(active_signer: AccountId, account: &AccountId, bound_owner: &PublicKey) -> Promise {
    call_signer(
        active_signer,
        "freeze",
        json!({ "wallet": account, "expected_current": ed25519_base58(bound_owner) }),
        FREEZE_GAS,
    )
}

fn unfreeze(active_signer: AccountId, account: &AccountId) -> Promise {
    call_signer(
        active_signer,
        "unfreeze",
        json!({ "wallet": account }),
        FREEZE_GAS,
    )
}

fn ed25519_base58(key: &PublicKey) -> String {
    hos_common::ed25519_base58(key).unwrap_or_else(|| env::panic_str(error::NOT_ED25519))
}

fn into_sig(bytes: Vec<u8>) -> [u8; 64] {
    <[u8; 64]>::try_from(bytes.as_slice())
        .unwrap_or_else(|_| env::panic_str(error::BAD_SIGNATURE_LEN))
}

#[cfg(test)]
mod tests;
