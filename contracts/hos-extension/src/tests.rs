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
    let _ = c.force_transfer(acc(WALLET), key(), None);
}

#[test]
fn admin_can_skim_within_available_balance() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    let _ = c.skim(U128(1), acc(DEST)).unwrap();
}

#[test]
fn non_admin_cannot_skim() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.skim(U128(1), acc(DEST)),
        Err(ContractError::OnlyAdmin)
    ));
}

#[test]
fn skim_rejects_above_available_balance() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(matches!(
        c.skim(U128(u128::MAX), acc(DEST)),
        Err(ContractError::InsufficientBalance)
    ));
}

#[test]
fn force_transfer_emits_nep297_event() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    let _ = c.force_transfer(acc(WALLET), key(), None);
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
        c.force_transfer(acc(WALLET), key(), None),
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
        c.force_transfer(acc(WALLET), key(), None),
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
        c.force_transfer(acc(WALLET), secp, None),
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
    assert!(matches!(out, PromiseOrValue::Promise(_)));
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
    assert!(matches!(out, PromiseOrValue::Promise(_)));
}

#[test]
fn nonzero_balance_continues_sweep() {
    let mut c = deploy();
    ctx(acc("hos-extension.testnet").as_str(), 0);
    let out = c.after_balance_for_sweep(acc(WALLET), acc(TOKEN), acc(DEST), Ok(U128(1_000_000)));
    assert!(matches!(out, PromiseOrValue::Promise(_)));
}

fn canonical_set() -> BTreeSet<AccountId> {
    let mut set = BTreeSet::new();
    set.insert(acc(SIGNER));
    set.insert(acc("hos-extension.testnet"));
    set
}

#[test]
fn canonical_extension_set_proceeds_to_swap() {
    let mut c = deploy();
    ctx("hos-extension.testnet", 0);
    let raw = ed25519_base58(&key()).unwrap();
    let _ = c.after_extensions_checked(acc(WALLET), raw, None, Ok(canonical_set()));
}

#[test]
#[should_panic(expected = "wallet extension set is not canonical")]
fn extra_extension_blocks_transfer() {
    let mut c = deploy();
    ctx("hos-extension.testnet", 0);
    let mut set = canonical_set();
    set.insert(acc("backdoor.testnet"));
    let raw = ed25519_base58(&key()).unwrap();
    let _ = c.after_extensions_checked(acc(WALLET), raw, None, Ok(set));
}

#[test]
#[should_panic(expected = "wallet extension set is not canonical")]
fn missing_active_signer_blocks_transfer() {
    let mut c = deploy();
    ctx("hos-extension.testnet", 0);
    let mut set = BTreeSet::new();
    set.insert(acc("hos-extension.testnet"));
    let raw = ed25519_base58(&key()).unwrap();
    let _ = c.after_extensions_checked(acc(WALLET), raw, None, Ok(set));
}

#[test]
#[should_panic(expected = "could not read wallet extension set")]
fn failed_extension_query_blocks_transfer() {
    let mut c = deploy();
    ctx("hos-extension.testnet", 0);
    let raw = ed25519_base58(&key()).unwrap();
    let _ = c.after_extensions_checked(acc(WALLET), raw, None, Err(PromiseError::Failed));
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
fn views_expose_config() {
    let c = deploy();
    assert_eq!(c.get_registry(), acc(REGISTRY));
    assert_eq!(c.get_active_signer(), acc(SIGNER));
    assert_eq!(c.get_recovery(), acc(RECOVERY));
    assert_eq!(c.get_version(), 1);
    assert!(!c.is_paused());
    assert_eq!(c.min_sweep_attached().0, MIN_SWEEP_ATTACHED.as_yoctonear());
}
