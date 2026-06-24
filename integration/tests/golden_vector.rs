use std::str::FromStr;
use std::time::Duration;

use defuse_wallet::signature::{Deadline, RequestMessage};
use defuse_wallet::{FunctionCallAction, PromiseSingle, Request};
use defuse_wallet_sdk::Signer;
use defuse_wallet_sdk::ed25519::ed25519_dalek::SigningKey;
use near_sdk::{AccountId, Gas, NearToken};

const SIGNER_ID: &str = "alice.acme.testnet";
const RECEIVER_ID: &str = "bob.testnet";
const NONCE: u32 = 1;
const CREATED_AT_SECS: u64 = 1_700_000_000;
const TIMEOUT_SECS: u64 = 3600;
const KEY_SEED: u8 = 7;

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fixed_message(out: PromiseSingle) -> RequestMessage {
    RequestMessage {
        chain_id: "mainnet".to_string(),
        signer_id: AccountId::from_str(SIGNER_ID).unwrap(),
        nonce: NONCE,
        created_at: Deadline::UNIX_EPOCH + Duration::from_secs(CREATED_AT_SECS),
        timeout: Duration::from_secs(TIMEOUT_SECS),
        request: Request::new().out(out),
    }
}

fn emit(label: &str, msg: &RequestMessage) {
    let key = SigningKey::from_bytes(&[KEY_SEED; 32]);
    println!("{label}_BORSH_HEX={}", to_hex(&near_sdk::borsh::to_vec(msg).unwrap()));
    println!("{label}_JSON={}", near_sdk::serde_json::to_string(msg).unwrap());
    println!("{label}_PROOF={}", Signer::sign(&key, msg).unwrap());
    println!("{label}_PUBKEY={}", Signer::public_key(&key));
}

#[test]
fn golden_transfer() {
    let out = PromiseSingle::new(AccountId::from_str(RECEIVER_ID).unwrap())
        .transfer(NearToken::from_yoctonear(1_000_000_000_000_000_000_000_000));
    emit("TRANSFER", &fixed_message(out));
}

#[test]
fn golden_function_call() {
    let action = FunctionCallAction::new("ping")
        .args(vec![1, 2, 3])
        .attached_deposit(NearToken::from_yoctonear(1))
        .min_gas(Gas::from_tgas(5))
        .unused_gas_weight(1);
    let out = PromiseSingle::new(AccountId::from_str(RECEIVER_ID).unwrap()).function_call(action);
    emit("FUNCTION_CALL", &fixed_message(out));
}
