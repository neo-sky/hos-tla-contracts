use super::*;
use ed25519_dalek::{Signer, SigningKey};
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::{testing_env, CurveType};
use rand::rngs::OsRng;
use std::str::FromStr;

const OWNER: &str = "hos.testnet";
const SIGNER: &str = "v1.signer-prod.testnet";
const VICTIM: &str = "victim.testnet";

fn keypair() -> (SigningKey, PublicKey) {
    let sk = SigningKey::generate(&mut OsRng);
    let pk =
        PublicKey::from_parts(CurveType::ED25519, sk.verifying_key().to_bytes().to_vec()).unwrap();
    (sk, pk)
}

fn ctx(predecessor: &str, ts: u64, height: u64) {
    let acct = AccountId::from_str(predecessor).unwrap();
    testing_env!(VMContextBuilder::new()
        .current_account_id(AccountId::from_str(OWNER).unwrap())
        .predecessor_account_id(acct)
        .block_timestamp(ts)
        .block_height(height)
        .build());
}

fn native_target() -> Target {
    Target::Native {
        mpc_public_key: PublicKey::from_str("ed25519:DZdWKDt29SBdPqeyfykg8TFF5Zkb5Qzdd6FJiJMvftZG")
            .unwrap(),
        derivation_path: "recover/victim".to_string(),
    }
}

const TRANSFER_AUTHORITY: &str = "hos-extension.testnet";
const BOUND_OWNER: &str = "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847";

fn wallet_target() -> Target {
    Target::Wallet {
        active_signer: AccountId::from_str("active-signer.testnet").unwrap(),
        bound_owner: PublicKey::from_str(BOUND_OWNER).unwrap(),
    }
}

fn deploy(watcher_keys: &[PublicKey], threshold: u32) -> MpcRecovery {
    ctx(OWNER, 0, 0);
    MpcRecovery::new(
        AccountId::from_str(OWNER).unwrap(),
        AccountId::from_str(SIGNER).unwrap(),
        AccountId::from_str(TRANSFER_AUTHORITY).unwrap(),
        watcher_keys.to_vec(),
        threshold,
    )
}

fn account_id() -> AccountId {
    AccountId::from_str(VICTIM).unwrap()
}

fn install(c: &mut MpcRecovery, attestation_key: PublicKey) {
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), native_target(), attestation_key, 60);
}

fn install_wallet(c: &mut MpcRecovery, attestation_key: PublicKey) {
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), wallet_target(), attestation_key, 60);
}

fn attest(sk: &SigningKey, new_owner: &PublicKey, round: u64) -> Base64VecU8 {
    let msg = proof::request_message(
        &AccountId::from_str(OWNER).unwrap(),
        &account_id(),
        new_owner,
        round,
    );
    Base64VecU8::from(sk.sign(&msg).to_bytes().to_vec())
}

fn watcher_sigs(
    sks: &[&SigningKey],
    pks: &[PublicKey],
    round: u64,
    silent: bool,
) -> Vec<WatcherSignature> {
    let msg = proof::verdict_message(
        &AccountId::from_str(OWNER).unwrap(),
        &account_id(),
        round,
        silent,
    );
    sks.iter()
        .zip(pks)
        .map(|(sk, pk)| WatcherSignature {
            public_key: pk.clone(),
            signature: Base64VecU8::from(sk.sign(&msg).to_bytes().to_vec()),
        })
        .collect()
}

#[test]
fn happy_path_to_approved() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), wk2.clone()], 2);
    install(&mut c, mother_pk);

    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );

    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(
        account_id(),
        true,
        watcher_sigs(&[&w1, &w2], &[wk1, wk2], 0, true),
    );

    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
    assert_eq!(c.round_of(account_id()), Some(1));
}

#[test]
#[should_panic(expected = "invalid attestation")]
fn request_rejects_forged_attestation() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let (wrong, _) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&wrong, &new_owner, 0),
    );
}

#[test]
#[should_panic(expected = "stale or invalid round")]
fn request_rejects_wrong_round() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(7),
        attest(&mother, &new_owner, 7),
    );
}

#[test]
#[should_panic(expected = "recovery already in progress")]
fn request_rejects_when_active() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 2, 2);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(1),
        attest(&mother, &new_owner, 1),
    );
}

#[test]
#[should_panic(expected = "timelock not elapsed")]
fn verdict_rejected_before_timelock() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 2, 2);
    let _ = c.submit_verdict(account_id(), true, watcher_sigs(&[&w1], &[wk1], 0, true));
}

#[test]
#[should_panic(expected = "watcher quorum not met")]
fn verdict_rejected_without_quorum() {
    let (w1, wk1) = keypair();
    let (_, wk2) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), wk2], 2);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(account_id(), true, watcher_sigs(&[&w1], &[wk1], 0, true));
}

#[test]
fn active_verdict_cancels_back_to_idle() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(account_id(), false, watcher_sigs(&[&w1], &[wk1], 0, false));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
#[should_panic(expected = "recovery not approved")]
fn finalize_rejected_without_approval() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let zeros = Base58CryptoHash::from([0u8; 32]);
    let _ = c.finalize_recovery(account_id(), U64(1), zeros);
}

#[test]
fn ed25519_base58_strips_curve_prefix() {
    let (_, pk) = keypair();
    let raw = ed25519_base58(&pk);
    assert!(!raw.contains(':'));
    assert_eq!(format!("ed25519:{raw}"), pk.to_string());
}

#[test]
fn wallet_verdict_approves_and_freezes() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let out = c.submit_verdict(account_id(), true, watcher_sigs(&[&w1], &[wk1], 0, true));
    assert!(matches!(out, PromiseOrValue::Promise(_)));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approving { .. }
    ));
    c.on_frozen(account_id(), 0, Ok(()));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
}

fn approve_wallet_recovery(
    c: &mut MpcRecovery,
    mother: &SigningKey,
    w1: &SigningKey,
    wk1: PublicKey,
) -> PublicKey {
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(account_id(), true, watcher_sigs(&[w1], &[wk1], 0, true));
    c.on_frozen(account_id(), 0, Ok(()));
    new_owner
}

fn block_hash() -> Base58CryptoHash {
    Base58CryptoHash::from([0u8; 32])
}

#[test]
fn wallet_finalize_resolves_only_after_swap_confirms() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Resolving { .. }
    ));
    c.on_finalized(account_id(), 0, Ok(true));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
fn finalize_restores_approved_when_swap_fails() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    c.on_finalized(account_id(), 0, Err(PromiseError::Failed));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
    let _ = c.finalize_recovery(account_id(), U64(2), block_hash());
    c.on_finalized(account_id(), 0, Ok(true));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
#[should_panic(expected = "recovery not approved")]
fn second_finalize_rejected_while_resolving() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let _ = c.finalize_recovery(account_id(), U64(2), block_hash());
}

#[test]
fn abort_from_requested_returns_to_idle() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install_wallet(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx(OWNER, 0, 2);
    let out = c.abort_recovery(account_id());
    assert!(matches!(out, PromiseOrValue::Value(())));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
fn abort_from_approved_wallet_unfreezes_and_resolves() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 0, 6);
    let out = c.abort_recovery(account_id());
    assert!(matches!(out, PromiseOrValue::Promise(_)));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Resolving { .. }
    ));
    c.on_aborted(account_id(), 0, Ok(()));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
fn abort_restores_approved_when_unfreeze_fails() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 0, 6);
    let _ = c.abort_recovery(account_id());
    c.on_aborted(account_id(), 0, Err(PromiseError::Failed));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
}

#[test]
#[should_panic(expected = "only owner")]
fn abort_rejects_non_owner() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx("attacker.testnet", 0, 6);
    let _ = c.abort_recovery(account_id());
}

#[test]
#[should_panic(expected = "no abortable recovery")]
fn abort_rejects_when_idle() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install_wallet(&mut c, mother_pk);
    ctx(OWNER, 0, 2);
    let _ = c.abort_recovery(account_id());
}

#[test]
#[should_panic(expected = "no abortable recovery")]
fn abort_rejects_while_resolving() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    ctx(OWNER, 0, 7);
    let _ = c.abort_recovery(account_id());
}

fn bound_owner_of(c: &MpcRecovery) -> PublicKey {
    match &c.accounts.get(&account_id()).unwrap().policy.target {
        Target::Wallet { bound_owner, .. } => bound_owner.clone(),
        Target::Native { .. } => unreachable!(),
    }
}

#[test]
fn approve_freeze_rejection_cancels() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(account_id(), true, watcher_sigs(&[&w1], &[wk1], 0, true));
    c.on_frozen(account_id(), 0, Err(PromiseError::Failed));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
fn finalize_void_fails_recovery_without_rebinding() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    c.on_finalized(account_id(), 0, Ok(false));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
    assert_eq!(bound_owner_of(&c).to_string(), BOUND_OWNER);
}

#[test]
fn finalize_success_rebinds_bound_owner() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    let new_owner = approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    c.on_finalized(account_id(), 0, Ok(true));
    assert_eq!(bound_owner_of(&c), new_owner);
}

#[test]
fn transfer_resets_idle_policy() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install_wallet(&mut c, mother_pk);
    ctx(TRANSFER_AUTHORITY, 0, 1);
    c.on_wallet_transferred(account_id());
    assert!(c.accounts.get(&account_id()).is_none());
}

#[test]
fn transfer_resets_requested_policy() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install_wallet(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx(TRANSFER_AUTHORITY, 0, 2);
    c.on_wallet_transferred(account_id());
    assert!(c.accounts.get(&account_id()).is_none());
}

#[test]
fn transfer_leaves_inflight_recovery() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install_wallet(&mut c, mother_pk);
    approve_wallet_recovery(&mut c, &mother, &w1, wk1);
    ctx(TRANSFER_AUTHORITY, 0, 6);
    c.on_wallet_transferred(account_id());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
}

#[test]
#[should_panic(expected = "only transfer authority")]
fn transfer_rejects_non_authority() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install_wallet(&mut c, mother_pk);
    ctx("attacker.testnet", 0, 1);
    c.on_wallet_transferred(account_id());
}

fn approve_native_recovery(
    c: &mut MpcRecovery,
    mother: &SigningKey,
    w1: &SigningKey,
    wk1: PublicKey,
) -> PublicKey {
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(account_id(), true, watcher_sigs(&[w1], &[wk1], 0, true));
    new_owner
}

#[test]
fn native_finalize_signs_and_resolves() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Resolving { .. }
    ));
}

#[test]
fn native_on_signed_success_resolves_to_idle() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let out = c.on_signed(
        account_id(),
        0,
        "ab".to_string(),
        Ok(json!({"signature": "stub"})),
    );
    assert!(out.is_some());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
fn native_on_signed_failure_restores_approved() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone()], 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let out = c.on_signed(account_id(), 0, "ab".to_string(), Err(PromiseError::Failed));
    assert!(out.is_none());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
}
