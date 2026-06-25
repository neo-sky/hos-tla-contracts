use super::*;
use defuse_wallet::signature::ed25519::Ed25519Signature;
use defuse_wallet::signature::Deadline;
use defuse_wallet::{Request, WalletOp};
use defuse_wallet_sdk::ed25519::ed25519_dalek::ed25519::signature::Signer as DalekSigner;
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

fn sign_freeze_msg(k: &SigningKey, msg: FreezeMessage, domain: &[u8]) -> (FreezeMessage, String) {
    ctx("client.testnet", 0, TS);
    let serialized = near_sdk::borsh::to_vec(&msg).unwrap();
    let hash = env::sha256_array([domain, &serialized].concat());
    let signature = <SigningKey as DalekSigner<_>>::sign(k, &hash).to_bytes();
    let proof = Ed25519Signature(signature).to_string();
    (msg, proof)
}

fn freeze_msg(nonce: u32) -> FreezeMessage {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32;
    FreezeMessage {
        chain_id: CHAIN_ID.to_string(),
        signer_id: acc(WALLET),
        nonce,
        created_at_secs: now_secs - 60,
        timeout_secs: 3600,
    }
}

fn sign_freeze(k: &SigningKey, nonce: u32) -> (FreezeMessage, String) {
    sign_freeze_msg(k, freeze_msg(nonce), FREEZE_DOMAIN)
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
#[should_panic(expected = "wallet ops are not allowed")]
fn signed_request_with_ops_rejected() {
    let mut c = deploy();
    let k = key(7);
    install(&mut c, &k);
    ctx("client.testnet", 0, TS);
    let mut request = Request::new();
    request.ops.push(WalletOp::AddExtension {
        account_id: acc("backdoor.testnet"),
    });
    let msg = RequestMessage {
        chain_id: CHAIN_ID.to_string(),
        signer_id: acc(WALLET),
        nonce: 1,
        created_at: Deadline::now() - Duration::from_secs(60),
        timeout: Duration::from_secs(3600),
        request,
    };
    let proof = Signer::sign(&k, &msg).unwrap();
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
#[should_panic(expected = "signer already installed")]
fn reinstall_rejected_when_signer_exists() {
    let mut c = deploy();
    install(&mut c, &key(7));
    ctx(MINTER, 0, TS);
    c.install_signer(acc(WALLET), Signer::public_key(&key(8)).to_string());
}

#[test]
#[should_panic(expected = "signer already installed")]
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

#[test]
#[should_panic(expected = "frozen")]
fn self_freeze_halts_submit() {
    let mut c = deploy();
    let k = key(7);
    install(&mut c, &k);
    let (fmsg, fproof) = sign_freeze(&k, 1);
    ctx("relayer.testnet", 0, TS);
    c.self_freeze(acc(WALLET), fmsg, fproof);
    assert_eq!(c.is_frozen(acc(WALLET)), Some(true));
    let (msg, proof) = sign(&k, 2);
    ctx("relayer.testnet", 1, TS);
    let _ = c.submit_signed_request(acc(WALLET), msg, proof);
}

#[test]
#[should_panic(expected = "invalid signature")]
fn self_freeze_wrong_key_rejected() {
    let mut c = deploy();
    install(&mut c, &key(7));
    let (fmsg, fproof) = sign_freeze(&key(9), 1);
    ctx("relayer.testnet", 0, TS);
    c.self_freeze(acc(WALLET), fmsg, fproof);
}

#[test]
#[should_panic(expected = "invalid signature")]
fn self_freeze_rejects_wallet_domain_sig() {
    let mut c = deploy();
    let k = key(7);
    install(&mut c, &k);
    let (fmsg, fproof) = sign_freeze_msg(&k, freeze_msg(1), b"NEAR_WALLET_CONTRACT/V1");
    ctx("relayer.testnet", 0, TS);
    c.self_freeze(acc(WALLET), fmsg, fproof);
}

#[test]
#[should_panic(expected = "nonce already used")]
fn self_freeze_replay_rejected() {
    let mut c = deploy();
    let k = key(7);
    install(&mut c, &k);
    let (fmsg, fproof) = sign_freeze(&k, 1);
    ctx("relayer.testnet", 0, TS);
    c.self_freeze(acc(WALLET), fmsg.clone(), fproof.clone());
    ctx("relayer.testnet", 0, TS);
    c.self_freeze(acc(WALLET), fmsg, fproof);
}

#[test]
fn recovery_swaps_after_self_freeze() {
    let mut c = deploy();
    let k = key(7);
    install(&mut c, &k);
    let (fmsg, fproof) = sign_freeze(&k, 1);
    ctx("relayer.testnet", 0, TS);
    c.self_freeze(acc(WALLET), fmsg, fproof);
    assert_eq!(c.is_frozen(acc(WALLET)), Some(true));
    ctx(RECOVERY, 0, TS);
    c.swap_owner(acc(WALLET), Signer::public_key(&key(8)).to_string(), None);
    assert_eq!(c.is_frozen(acc(WALLET)), Some(false));
    assert_eq!(
        c.signer_of(acc(WALLET)),
        Some(Signer::public_key(&key(8)).to_string())
    );
}

#[test]
#[should_panic(expected = "only recovery")]
fn owner_cannot_unfreeze_self_freeze() {
    let mut c = deploy();
    let k = key(7);
    install(&mut c, &k);
    let (fmsg, fproof) = sign_freeze(&k, 1);
    ctx("relayer.testnet", 0, TS);
    c.self_freeze(acc(WALLET), fmsg, fproof);
    ctx("client.testnet", 0, TS);
    c.unfreeze(acc(WALLET));
}
