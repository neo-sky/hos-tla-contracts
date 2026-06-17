#![cfg(feature = "contract")]

use hos_wallet::{Contract, Request, Wallet};
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::{testing_env, AccountId, NearToken};
use std::str::FromStr;

fn acc(s: &str) -> AccountId {
    AccountId::from_str(s).unwrap()
}

fn signer() -> AccountId {
    acc("active-signer.testnet")
}

fn hos_extension() -> AccountId {
    acc("hos-extension.testnet")
}

fn ctx(predecessor: &str, deposit: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc("alice.tla.testnet"))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .build());
}

fn deploy() -> Contract {
    ctx("tla-manager.testnet", 0);
    Contract::new(vec![signer(), hos_extension()])
}

#[test]
fn init_installs_extensions() {
    let c = deploy();
    assert!(c.w_is_extension_enabled(signer()));
    assert!(c.w_is_extension_enabled(hos_extension()));
    assert!(!c.w_is_extension_enabled(acc("stranger.testnet")));
    assert_eq!(c.w_extensions().len(), 2);
}

#[test]
fn init_disables_signature_path() {
    let c = deploy();
    assert!(!c.w_is_signature_allowed());
}

#[test]
#[should_panic(expected = "extensions must not be empty")]
fn init_rejects_empty_extensions() {
    ctx("tla-manager.testnet", 0);
    let _ = Contract::new(vec![]);
}

#[test]
fn enabled_extension_can_execute() {
    let mut c = deploy();
    ctx("active-signer.testnet", 1);
    c.w_execute_extension(Request::new());
}

#[test]
#[should_panic]
fn non_extension_cannot_execute() {
    let mut c = deploy();
    ctx("stranger.testnet", 1);
    c.w_execute_extension(Request::new());
}

#[test]
#[should_panic]
fn extension_call_requires_deposit() {
    let mut c = deploy();
    ctx("active-signer.testnet", 0);
    c.w_execute_extension(Request::new());
}
