use std::str::FromStr;

use active_signer::{MessageRequest, TxMessage, CHAIN_ID, MESSAGE_DOMAIN, TX_DOMAIN};
use defuse_wallet::signature::ed25519::Ed25519Signature;
use defuse_wallet_sdk::ed25519::ed25519_dalek::Signer as DalekSigner;
use defuse_wallet_sdk::ed25519::ed25519_dalek::SigningKey;
use defuse_wallet_sdk::Signer;
use hos_common::tx::TxAction;
use near_sdk::json_types::Base64VecU8;
use near_sdk::{AccountId, Gas, NearToken};
use sha2::{Digest, Sha256};

const SIGNER_ID: &str = "alice.acme.testnet";
const RECEIVER_ID: &str = "bob.testnet";
const NONCE: u32 = 1;
const CREATED_AT_SECS: u32 = 1_700_000_000;
const TIMEOUT_SECS: u32 = 3600;
const KEY_SEED: u8 = 7;

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sign<M: near_sdk::borsh::BorshSerialize>(key: &SigningKey, msg: &M, domain: &[u8]) -> String {
    let serialized = near_sdk::borsh::to_vec(msg).unwrap();
    let hash = Sha256::digest([domain, serialized.as_slice()].concat());
    Ed25519Signature(DalekSigner::sign(key, hash.as_slice()).to_bytes()).to_string()
}

fn emit<M: near_sdk::borsh::BorshSerialize + near_sdk::serde::Serialize>(
    label: &str,
    msg: &M,
    domain: &[u8],
) {
    let key = SigningKey::from_bytes(&[KEY_SEED; 32]);
    println!(
        "{label}_BORSH_HEX={}",
        to_hex(&near_sdk::borsh::to_vec(msg).unwrap())
    );
    println!(
        "{label}_JSON={}",
        near_sdk::serde_json::to_string(msg).unwrap()
    );
    println!("{label}_PROOF={}", sign(&key, msg, domain));
    println!("{label}_PUBKEY={}", Signer::public_key(&key));
}

fn base_tx(actions: Vec<TxAction>) -> TxMessage {
    TxMessage {
        chain_id: CHAIN_ID.to_string(),
        signer_id: AccountId::from_str(SIGNER_ID).unwrap(),
        nonce: NONCE,
        created_at_secs: CREATED_AT_SECS,
        timeout_secs: TIMEOUT_SECS,
        receiver_id: AccountId::from_str(RECEIVER_ID).unwrap(),
        actions,
    }
}

#[test]
fn golden_transfer() {
    let msg = base_tx(vec![TxAction::Transfer {
        deposit: NearToken::from_yoctonear(1_000_000_000_000_000_000_000_000),
    }]);
    emit("TRANSFER", &msg, TX_DOMAIN);
}

#[test]
fn golden_function_call() {
    let msg = base_tx(vec![TxAction::FunctionCall {
        method_name: "ping".to_string(),
        args: Base64VecU8(vec![1, 2, 3]),
        gas: Gas::from_tgas(5),
        deposit: NearToken::from_yoctonear(1),
    }]);
    emit("FUNCTION_CALL", &msg, TX_DOMAIN);
}

#[test]
fn golden_nep413_message() {
    let msg = MessageRequest {
        chain_id: CHAIN_ID.to_string(),
        signer_id: AccountId::from_str(SIGNER_ID).unwrap(),
        nonce: NONCE,
        created_at_secs: CREATED_AT_SECS,
        timeout_secs: TIMEOUT_SECS,
        message: "login".to_string(),
        recipient: "app.example.com".to_string(),
        message_nonce: Base64VecU8(vec![0u8; 32]),
        callback_url: None,
    };
    emit("NEP413_MESSAGE", &msg, MESSAGE_DOMAIN);
}
