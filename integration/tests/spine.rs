use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use defuse_wallet::signature::{Deadline, RequestMessage};
use defuse_wallet::{PromiseSingle, Request};
use defuse_wallet_sdk::ed25519::ed25519_dalek::Signer as DalekSigner;
use defuse_wallet_sdk::ed25519::ed25519_dalek::SigningKey;
use defuse_wallet_sdk::Signer;
use near_workspaces::types::{Gas, KeyType, NearToken, PublicKey};
use near_workspaces::{Account, AccountId, Contract, Worker};
use serde_json::json;

const ACTIVE_SIGNER_WASM: &str = "../target/near/active_signer/active_signer.wasm";
const HOS_EXTENSION_WASM: &str = "../target/near/hos_extension/hos_extension.wasm";
const HOS_WALLET_WASM: &str = "../target/near/hos_wallet/hos_wallet.wasm";
const TEST_FT_WASM: &str = "../target/near/test_ft/test_ft.wasm";
const MPC_RECOVERY_WASM: &str = "../target/near/mpc_recovery/mpc_recovery.wasm";

const TIMEOUT_SECS: u32 = 3600;
const SWEEP_ATTACHED: NearToken = NearToken::from_yoctonear(1_250_000_000_000_000_000_000 + 1);

struct Harness {
    worker: Worker<near_workspaces::network::Sandbox>,
    admin: Account,
    registry: Account,
    tla: Account,
    active_signer: Contract,
    hos_extension: Contract,
    mpc_recovery: Contract,
}

fn user_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn raw_base58(key: &SigningKey) -> String {
    Signer::public_key(key).to_string()
}

fn ws_pubkey(key: &SigningKey) -> PublicKey {
    PublicKey::try_from_parts(KeyType::ED25519, key.verifying_key().as_bytes())
        .expect("valid ed25519 public key")
}

async fn deploy_singleton(
    root: &Account,
    name: &str,
    balance: u128,
    wasm: &str,
) -> Result<Contract> {
    let account = root
        .create_subaccount(name)
        .initial_balance(NearToken::from_near(balance))
        .transact()
        .await?
        .into_result()?;
    let bytes = std::fs::read(wasm)?;
    Ok(account.deploy(&bytes).await?.into_result()?)
}

async fn setup() -> Result<Harness> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;

    let admin = root
        .create_subaccount("admin")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let registry = root
        .create_subaccount("registry")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let tla = root
        .create_subaccount("mytla")
        .initial_balance(NearToken::from_near(100))
        .transact()
        .await?
        .into_result()?;

    let active_signer = deploy_singleton(&root, "asigner", 30, ACTIVE_SIGNER_WASM).await?;
    let hos_extension = deploy_singleton(&root, "hosext", 30, HOS_EXTENSION_WASM).await?;
    let mpc_recovery = deploy_singleton(&root, "mpcrec", 20, MPC_RECOVERY_WASM).await?;

    active_signer
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "marketplace_authority": hos_extension.id(),
            "recovery_authority": mpc_recovery.id(),
            "timeout_secs": TIMEOUT_SECS,
        }))
        .transact()
        .await?
        .into_result()?;

    hos_extension
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "registry": registry.id(),
            "active_signer": active_signer.id(),
            "recovery": mpc_recovery.id(),
        }))
        .transact()
        .await?
        .into_result()?;

    mpc_recovery
        .call("new")
        .args_json(json!({
            "owner": admin.id(),
            "signer": admin.id(),
            "transfer_authority": hos_extension.id(),
            "watchers": [ws_pubkey(&user_key(20))],
            "threshold": 1,
        }))
        .transact()
        .await?
        .into_result()?;

    admin
        .call(active_signer.id(), "add_minter")
        .args_json(json!({ "minter": tla.id() }))
        .transact()
        .await?
        .into_result()?;

    Ok(Harness {
        worker,
        admin,
        registry,
        tla,
        active_signer,
        hos_extension,
        mpc_recovery,
    })
}

async fn mint_wallet(h: &Harness, name: &str, owner: &SigningKey) -> Result<Account> {
    let wallet = h
        .tla
        .create_subaccount(name)
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;

    let wasm = std::fs::read(HOS_WALLET_WASM)?;
    let wallet_contract = wallet.deploy(&wasm).await?.into_result()?;
    wallet_contract
        .call("new")
        .args_json(json!({
            "extensions": [h.active_signer.id(), h.hos_extension.id()],
        }))
        .transact()
        .await?
        .into_result()?;

    h.tla
        .call(h.active_signer.id(), "install_signer")
        .args_json(json!({ "wallet": wallet.id(), "public_key": raw_base58(owner) }))
        .transact()
        .await?
        .into_result()?;

    Ok(wallet)
}

fn signed_transfer(
    wallet: &AccountId,
    recipient: &AccountId,
    amount: NearToken,
    nonce: u32,
    key: &SigningKey,
) -> (serde_json::Value, String) {
    let signer_id = near_sdk::AccountId::from_str(wallet.as_str()).unwrap();
    let recipient = near_sdk::AccountId::from_str(recipient.as_str()).unwrap();
    let out = PromiseSingle::new(recipient)
        .transfer(near_sdk::NearToken::from_yoctonear(amount.as_yoctonear()));
    let msg = RequestMessage {
        chain_id: "mainnet".to_string(),
        signer_id,
        nonce,
        created_at: Deadline::now() - Duration::from_secs(60),
        timeout: Duration::from_secs(TIMEOUT_SECS as u64),
        request: Request::new().out(out),
    };
    let proof = Signer::sign(key, &msg).unwrap();
    (serde_json::to_value(&msg).unwrap(), proof)
}

async fn submit(
    h: &Harness,
    wallet: &AccountId,
    msg: &serde_json::Value,
    proof: &str,
) -> Result<near_workspaces::result::ExecutionFinalResult> {
    Ok(h.registry
        .call(h.active_signer.id(), "submit_signed_request")
        .args_json(json!({ "wallet": wallet, "msg": msg, "proof": proof }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(120))
        .transact()
        .await?)
}

#[tokio::test]
async fn no_sign_path_is_rejected() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    let (msg, proof) = signed_transfer(
        wallet.id(),
        h.admin.id(),
        NearToken::from_near(1),
        1,
        &owner,
    );
    let direct = h
        .registry
        .call(wallet.id(), "w_execute_signed")
        .args_json(json!({ "msg": msg, "proof": proof }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?;
    assert!(
        direct.is_failure(),
        "no-sign wallet must reject w_execute_signed"
    );
    Ok(())
}

#[tokio::test]
async fn signed_request_executes_through_active_signer() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    let recipient = h
        .worker
        .root_account()?
        .create_subaccount("recipient")
        .initial_balance(NearToken::from_near(1))
        .transact()
        .await?
        .into_result()?;
    let before = recipient.view_account().await?.balance;

    let (msg, proof) = signed_transfer(
        wallet.id(),
        recipient.id(),
        NearToken::from_near(2),
        1,
        &owner,
    );
    let exec = submit(&h, wallet.id(), &msg, &proof).await?;
    assert!(
        exec.is_success(),
        "signed request should execute: {exec:#?}"
    );

    let after = recipient.view_account().await?.balance;
    assert!(
        after.as_yoctonear() > before.as_yoctonear() + NearToken::from_near(1).as_yoctonear(),
        "recipient should have received the transfer"
    );

    let last_signed: Option<u64> = h
        .active_signer
        .view("last_signed_at")
        .args_json(json!({ "wallet": wallet.id() }))
        .await?
        .json()?;
    assert!(last_signed.unwrap_or(0) > 0, "last_signed_at should update");

    let replay = submit(&h, wallet.id(), &msg, &proof).await?;
    assert!(replay.is_failure(), "replayed nonce must be rejected");
    Ok(())
}

#[tokio::test]
async fn marketplace_rotation_kills_old_key() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let buyer = user_key(8);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    h.registry
        .call(h.hos_extension.id(), "force_transfer")
        .args_json(json!({ "wallet": wallet.id(), "new_public_key": ws_pubkey(&buyer) }))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?
        .into_result()?;

    let signer_now: Option<String> = h
        .active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet.id() }))
        .await?
        .json()?;
    assert_eq!(signer_now.as_deref(), Some(raw_base58(&buyer).as_str()));

    let (old_msg, old_proof) = signed_transfer(
        wallet.id(),
        h.admin.id(),
        NearToken::from_near(1),
        1,
        &owner,
    );
    let old = submit(&h, wallet.id(), &old_msg, &old_proof).await?;
    assert!(
        old.is_failure(),
        "old owner key must be dead after rotation"
    );

    let (new_msg, new_proof) = signed_transfer(
        wallet.id(),
        h.admin.id(),
        NearToken::from_near(1),
        1,
        &buyer,
    );
    let new = submit(&h, wallet.id(), &new_msg, &new_proof).await?;
    assert!(new.is_success(), "new owner key must work: {new:#?}");
    Ok(())
}

#[tokio::test]
async fn reclaim_sweeps_ft_to_destination() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    let token = deploy_singleton(&h.worker.root_account()?, "ft", 20, TEST_FT_WASM).await?;
    token
        .call("new")
        .args_json(json!({ "owner": h.admin.id(), "total_supply": "1000000" }))
        .transact()
        .await?
        .into_result()?;
    let destination = h
        .worker
        .root_account()?
        .create_subaccount("treasury")
        .initial_balance(NearToken::from_near(1))
        .transact()
        .await?
        .into_result()?;
    token
        .call("mint")
        .args_json(json!({ "account_id": wallet.id(), "amount": "5000" }))
        .transact()
        .await?
        .into_result()?;

    h.registry
        .call(h.hos_extension.id(), "sweep_ft")
        .args_json(json!({
            "wallet": wallet.id(),
            "ft": token.id(),
            "destination": destination.id(),
        }))
        .deposit(SWEEP_ATTACHED)
        .gas(Gas::from_tgas(120))
        .transact()
        .await?
        .into_result()?;

    let wallet_bal: near_sdk::json_types::U128 = token
        .view("ft_balance_of")
        .args_json(json!({ "account_id": wallet.id() }))
        .await?
        .json()?;
    let dest_bal: near_sdk::json_types::U128 = token
        .view("ft_balance_of")
        .args_json(json!({ "account_id": destination.id() }))
        .await?
        .json()?;
    assert_eq!(wallet_bal.0, 0, "wallet FT balance should be swept");
    assert_eq!(dest_bal.0, 5000, "destination should receive the swept FT");
    Ok(())
}

#[tokio::test]
async fn sold_recovery_wallet_cannot_be_clawed_back() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    let mother = user_key(11);
    h.admin
        .call(h.mpc_recovery.id(), "install_policy")
        .args_json(json!({
            "account": wallet.id(),
            "target": { "Wallet": {
                "active_signer": h.active_signer.id(),
                "bound_owner": ws_pubkey(&owner),
            }},
            "attestation_key": ws_pubkey(&mother),
            "timelock_secs": 1,
        }))
        .transact()
        .await?
        .into_result()?;

    let round: Option<u64> = h
        .mpc_recovery
        .view("round_of")
        .args_json(json!({ "account": wallet.id() }))
        .await?
        .json()?;
    assert_eq!(round, Some(0), "recovery policy installed");

    let buyer = user_key(8);
    h.registry
        .call(h.hos_extension.id(), "force_transfer")
        .args_json(json!({ "wallet": wallet.id(), "new_public_key": raw_base58(&buyer) }))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?
        .into_result()?;

    let signer: Option<String> = h
        .active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet.id() }))
        .await?
        .json()?;
    assert_eq!(
        signer,
        Some(raw_base58(&buyer)),
        "buyer controls the wallet"
    );

    let round_after: Option<u64> = h
        .mpc_recovery
        .view("round_of")
        .args_json(json!({ "account": wallet.id() }))
        .await?
        .json()?;
    assert!(round_after.is_none(), "recovery policy reset on transfer");

    let attempt = h
        .admin
        .call(h.mpc_recovery.id(), "request_recovery")
        .args_json(json!({
            "account": wallet.id(),
            "new_owner": raw_base58(&owner),
            "round": "0",
            "attestation": "",
        }))
        .transact()
        .await?;
    assert!(
        attempt.is_failure(),
        "seller must not be able to recover a sold wallet"
    );

    Ok(())
}

const ZERO_HASH: &str = "11111111111111111111111111111111";

fn push_len_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

fn ed25519_pubkey_bytes(key: &SigningKey) -> Vec<u8> {
    let mut v = vec![0u8];
    v.extend_from_slice(key.verifying_key().as_bytes());
    v
}

fn request_attestation(
    mother: &SigningKey,
    contract: &AccountId,
    account: &AccountId,
    new_owner: &SigningKey,
    round: u64,
) -> near_sdk::json_types::Base64VecU8 {
    let mut m = vec![1u8];
    push_len_str(&mut m, contract.as_str());
    push_len_str(&mut m, account.as_str());
    m.extend_from_slice(&ed25519_pubkey_bytes(new_owner));
    m.extend_from_slice(&round.to_le_bytes());
    near_sdk::json_types::Base64VecU8::from(DalekSigner::sign(mother, &m).to_bytes().to_vec())
}

fn verdict_signature(
    watcher: &SigningKey,
    contract: &AccountId,
    account: &AccountId,
    round: u64,
    silent: bool,
) -> serde_json::Value {
    let mut m = vec![2u8];
    push_len_str(&mut m, contract.as_str());
    push_len_str(&mut m, account.as_str());
    m.extend_from_slice(&round.to_le_bytes());
    m.push(silent as u8);
    json!({
        "public_key": ws_pubkey(watcher),
        "signature": near_sdk::json_types::Base64VecU8::from(
            DalekSigner::sign(watcher, &m).to_bytes().to_vec()
        ),
    })
}

async fn enroll_recovery(
    h: &Harness,
    wallet: &Account,
    owner: &SigningKey,
    mother: &SigningKey,
    timelock_secs: u32,
) -> Result<()> {
    h.admin
        .call(h.mpc_recovery.id(), "install_policy")
        .args_json(json!({
            "account": wallet.id(),
            "target": { "Wallet": {
                "active_signer": h.active_signer.id(),
                "bound_owner": ws_pubkey(owner),
            }},
            "attestation_key": ws_pubkey(mother),
            "timelock_secs": timelock_secs,
        }))
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

async fn request_recovery(
    h: &Harness,
    wallet: &Account,
    mother: &SigningKey,
    new_owner: &SigningKey,
) -> Result<()> {
    h.registry
        .call(h.mpc_recovery.id(), "request_recovery")
        .args_json(json!({
            "account": wallet.id(),
            "new_owner": ws_pubkey(new_owner),
            "round": "0",
            "attestation": request_attestation(mother, h.mpc_recovery.id(), wallet.id(), new_owner, 0),
        }))
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

#[tokio::test]
async fn recovery_full_lifecycle_swaps_owner() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;
    let mother = user_key(11);
    let new_owner = user_key(33);
    let watcher = user_key(20);

    enroll_recovery(&h, &wallet, &owner, &mother, 1).await?;
    request_recovery(&h, &wallet, &mother, &new_owner).await?;
    h.worker.fast_forward(20).await?;

    h.registry
        .call(h.mpc_recovery.id(), "submit_verdict")
        .args_json(json!({
            "account": wallet.id(),
            "silent": true,
            "signatures": [verdict_signature(&watcher, h.mpc_recovery.id(), wallet.id(), 0, true)],
        }))
        .gas(Gas::from_tgas(100))
        .transact()
        .await?
        .into_result()?;

    h.registry
        .call(h.mpc_recovery.id(), "finalize_recovery")
        .args_json(json!({ "account": wallet.id(), "nonce": "0", "block_hash": ZERO_HASH }))
        .gas(Gas::from_tgas(100))
        .transact()
        .await?
        .into_result()?;

    let signer: Option<String> = h
        .active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet.id() }))
        .await?
        .json()?;
    assert_eq!(
        signer,
        Some(raw_base58(&new_owner)),
        "recovery installed the new owner key"
    );
    let round: Option<u64> = h
        .mpc_recovery
        .view("round_of")
        .args_json(json!({ "account": wallet.id() }))
        .await?
        .json()?;
    assert_eq!(round, Some(1), "recovery complete, round advanced");

    let (msg, proof) = signed_transfer(
        wallet.id(),
        h.admin.id(),
        NearToken::from_near(1),
        1,
        &owner,
    );
    let old = submit(&h, wallet.id(), &msg, &proof).await?;
    assert!(
        old.is_failure(),
        "old owner key must be dead after recovery"
    );
    Ok(())
}

#[tokio::test]
async fn recovery_rejected_before_timelock() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;
    let mother = user_key(11);
    let new_owner = user_key(33);
    let watcher = user_key(20);

    enroll_recovery(&h, &wallet, &owner, &mother, 3600).await?;
    request_recovery(&h, &wallet, &mother, &new_owner).await?;

    let verdict = h
        .registry
        .call(h.mpc_recovery.id(), "submit_verdict")
        .args_json(json!({
            "account": wallet.id(),
            "silent": true,
            "signatures": [verdict_signature(&watcher, h.mpc_recovery.id(), wallet.id(), 0, true)],
        }))
        .gas(Gas::from_tgas(100))
        .transact()
        .await?;
    assert!(verdict.is_failure(), "verdict before timelock must fail");
    Ok(())
}

#[tokio::test]
async fn recovery_rejected_with_non_watcher_signature() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;
    let mother = user_key(11);
    let new_owner = user_key(33);
    let impostor = user_key(77);

    enroll_recovery(&h, &wallet, &owner, &mother, 1).await?;
    request_recovery(&h, &wallet, &mother, &new_owner).await?;
    h.worker.fast_forward(20).await?;

    let verdict = h
        .registry
        .call(h.mpc_recovery.id(), "submit_verdict")
        .args_json(json!({
            "account": wallet.id(),
            "silent": true,
            "signatures": [verdict_signature(&impostor, h.mpc_recovery.id(), wallet.id(), 0, true)],
        }))
        .gas(Gas::from_tgas(100))
        .transact()
        .await?;
    assert!(
        verdict.is_failure(),
        "non-watcher signature must not meet quorum"
    );
    Ok(())
}

#[tokio::test]
async fn recovery_freeze_blocks_owner_then_abort_restores() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;
    let mother = user_key(11);
    let new_owner = user_key(33);
    let watcher = user_key(20);

    enroll_recovery(&h, &wallet, &owner, &mother, 1).await?;
    request_recovery(&h, &wallet, &mother, &new_owner).await?;
    h.worker.fast_forward(20).await?;
    h.registry
        .call(h.mpc_recovery.id(), "submit_verdict")
        .args_json(json!({
            "account": wallet.id(),
            "silent": true,
            "signatures": [verdict_signature(&watcher, h.mpc_recovery.id(), wallet.id(), 0, true)],
        }))
        .gas(Gas::from_tgas(100))
        .transact()
        .await?
        .into_result()?;

    let (msg, proof) = signed_transfer(
        wallet.id(),
        h.admin.id(),
        NearToken::from_near(1),
        1,
        &owner,
    );
    let blocked = submit(&h, wallet.id(), &msg, &proof).await?;
    assert!(
        blocked.is_failure(),
        "frozen wallet must reject signed requests"
    );

    h.admin
        .call(h.mpc_recovery.id(), "abort_recovery")
        .args_json(json!({ "account": wallet.id() }))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?
        .into_result()?;

    let (msg2, proof2) = signed_transfer(
        wallet.id(),
        h.admin.id(),
        NearToken::from_near(1),
        2,
        &owner,
    );
    let restored = submit(&h, wallet.id(), &msg2, &proof2).await?;
    assert!(
        restored.is_success(),
        "wallet must work again after abort unfreezes it"
    );
    Ok(())
}

#[tokio::test]
async fn sale_during_pending_recovery_resets_policy() -> Result<()> {
    let h = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;
    let mother = user_key(11);
    let new_owner = user_key(33);
    let watcher = user_key(20);
    let buyer = user_key(8);

    enroll_recovery(&h, &wallet, &owner, &mother, 1).await?;
    request_recovery(&h, &wallet, &mother, &new_owner).await?;

    h.registry
        .call(h.hos_extension.id(), "force_transfer")
        .args_json(json!({ "wallet": wallet.id(), "new_public_key": raw_base58(&buyer) }))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?
        .into_result()?;

    let round: Option<u64> = h
        .mpc_recovery
        .view("round_of")
        .args_json(json!({ "account": wallet.id() }))
        .await?
        .json()?;
    assert!(round.is_none(), "pending recovery policy reset by the sale");
    let signer: Option<String> = h
        .active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet.id() }))
        .await?
        .json()?;
    assert_eq!(
        signer,
        Some(raw_base58(&buyer)),
        "buyer controls the wallet"
    );

    h.worker.fast_forward(20).await?;
    let verdict = h
        .registry
        .call(h.mpc_recovery.id(), "submit_verdict")
        .args_json(json!({
            "account": wallet.id(),
            "silent": true,
            "signatures": [verdict_signature(&watcher, h.mpc_recovery.id(), wallet.id(), 0, true)],
        }))
        .gas(Gas::from_tgas(100))
        .transact()
        .await?;
    assert!(
        verdict.is_failure(),
        "a late verdict cannot resurrect a reset recovery"
    );
    Ok(())
}
