use std::str::FromStr;
use std::time::SystemTime;

use active_signer::{TxMessage, CHAIN_ID, FREEZE_DOMAIN, TX_DOMAIN};
use anyhow::Result;
use defuse_wallet::signature::ed25519::Ed25519Signature;
use defuse_wallet_sdk::ed25519::ed25519_dalek::Signer as DalekSigner;
use defuse_wallet_sdk::ed25519::ed25519_dalek::SigningKey;
use defuse_wallet_sdk::Signer;
use hos_common::tx::TxAction;
use near_workspaces::network::Sandbox;
use near_workspaces::types::{Gas, KeyType, NearToken, PublicKey, SecretKey};
use near_workspaces::{Account, AccountId, Contract, Worker};
use serde_json::json;
use sha2::{Digest, Sha256};

const ACTIVE_SIGNER_WASM: &str = "../target/near/active_signer/active_signer.wasm";
const HOS_EXTENSION_WASM: &str = "../target/near/hos_extension/hos_extension.wasm";
const MPC_RECOVERY_WASM: &str = "../target/near/mpc_recovery/mpc_recovery.wasm";
const TEST_MPC_WASM: &str = "../target/near/test_mpc/test_mpc.wasm";

const TIMEOUT_SECS: u32 = 3600;
const MPC_SECRET: [u8; 32] = [42u8; 32];

struct Harness {
    admin: Account,
    registry: Account,
    tla: Account,
    active_signer: Contract,
    hos_extension: Contract,
}

fn user_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn mpc_key() -> SigningKey {
    SigningKey::from_bytes(&MPC_SECRET)
}

fn raw_base58(key: &SigningKey) -> String {
    Signer::public_key(key).to_string()
}

fn ws_pubkey(key: &SigningKey) -> PublicKey {
    PublicKey::try_from_parts(KeyType::ED25519, key.verifying_key().as_bytes())
        .expect("valid ed25519 public key")
}

fn mpc_secret_key() -> SecretKey {
    let signing = mpc_key();
    let mut bytes = signing.to_bytes().to_vec();
    bytes.extend_from_slice(signing.verifying_key().as_bytes());
    format!("ed25519:{}", bs58::encode(bytes).into_string())
        .parse()
        .expect("valid ed25519 secret key")
}

fn now_secs() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32
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

async fn setup() -> Result<(Worker<Sandbox>, Harness)> {
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
    let test_mpc = deploy_singleton(&root, "mpcsim", 20, TEST_MPC_WASM).await?;

    test_mpc
        .call("new")
        .args_json(json!({ "secret": MPC_SECRET.to_vec() }))
        .transact()
        .await?
        .into_result()?;
    active_signer
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "marketplace_authority": hos_extension.id(),
            "recovery_authority": mpc_recovery.id(),
            "mpc_signer": test_mpc.id(),
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

    Ok((
        worker,
        Harness {
            admin,
            registry,
            tla,
            active_signer,
            hos_extension,
        },
    ))
}

async fn mint_wallet(h: &Harness, name: &str, owner: &SigningKey) -> Result<Account> {
    let wallet = h
        .tla
        .create_subaccount(name)
        .initial_balance(NearToken::from_near(10))
        .keys(mpc_secret_key())
        .transact()
        .await?
        .into_result()?;
    h.tla
        .call(h.active_signer.id(), "install_signer")
        .args_json(json!({
            "wallet": wallet.id(),
            "public_key": raw_base58(owner),
            "mpc_public_key": ws_pubkey(&mpc_key()),
        }))
        .transact()
        .await?
        .into_result()?;
    Ok(wallet)
}

fn tx_envelope(
    wallet: &AccountId,
    receiver: &AccountId,
    actions: Vec<TxAction>,
    nonce: u32,
    key: &SigningKey,
    domain: &[u8],
) -> (TxMessage, String) {
    let msg = TxMessage {
        chain_id: CHAIN_ID.to_string(),
        signer_id: near_sdk::AccountId::from_str(wallet.as_str()).unwrap(),
        nonce,
        created_at_secs: now_secs() - 60,
        timeout_secs: TIMEOUT_SECS,
        receiver_id: near_sdk::AccountId::from_str(receiver.as_str()).unwrap(),
        actions,
    };
    let serialized = near_sdk::borsh::to_vec(&msg).unwrap();
    let hash = Sha256::digest([domain, serialized.as_slice()].concat());
    let proof = Ed25519Signature(DalekSigner::sign(key, hash.as_slice()).to_bytes()).to_string();
    (msg, proof)
}

fn transfer(deposit: u128) -> Vec<TxAction> {
    vec![TxAction::Transfer {
        deposit: near_sdk::NearToken::from_yoctonear(deposit),
    }]
}

async fn submit(
    h: &Harness,
    caller: &Account,
    wallet: &AccountId,
    msg: &TxMessage,
    proof: &str,
    deposit: u128,
) -> Result<near_workspaces::result::ExecutionFinalResult> {
    Ok(caller
        .call(h.active_signer.id(), "submit_signed_tx")
        .args_json(json!({
            "wallet": wallet,
            "msg": msg,
            "proof": proof,
            "tx_nonce": "1",
            "block_hash": bs58::encode([0u8; 32]).into_string(),
        }))
        .deposit(NearToken::from_yoctonear(deposit))
        .gas(Gas::from_tgas(120))
        .transact()
        .await?)
}

async fn access_keys(wallet: &Account) -> Result<Vec<PublicKey>> {
    Ok(wallet
        .view_access_keys()
        .await?
        .into_iter()
        .map(|k| k.public_key)
        .collect())
}

// The account holds exactly one on-chain key, the MPC-derived FullAccess key, and no
// operate/sale/reclaim path ever adds a second key or rotates it. This is what makes a
// seed impossible to export and keeps control a storage swap inside active-signer.
#[tokio::test]
async fn account_key_is_immutable_across_operate_and_sale() -> Result<()> {
    let (_worker, h) = setup().await?;
    let owner = user_key(7);
    let buyer = user_key(8);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    let minted = access_keys(&wallet).await?;
    assert_eq!(minted.len(), 1, "account mints with exactly one key");
    assert_eq!(
        minted[0],
        ws_pubkey(&mpc_key()),
        "the sole key is the MPC key"
    );

    let (msg, proof) = tx_envelope(wallet.id(), h.admin.id(), transfer(1), 1, &owner, TX_DOMAIN);
    submit(&h, &h.registry, wallet.id(), &msg, &proof, 1)
        .await?
        .into_result()?;
    assert_eq!(
        access_keys(&wallet).await?,
        minted,
        "an operate must not change the account key set"
    );

    h.registry
        .call(h.hos_extension.id(), "force_transfer")
        .args_json(json!({ "wallet": wallet.id(), "new_public_key": ws_pubkey(&buyer), "expected_current": null }))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?
        .into_result()?;
    assert_eq!(
        access_keys(&wallet).await?,
        minted,
        "a sale swaps the operating key in active-signer, never the on-chain key"
    );
    Ok(())
}

// A renter can spend and call any contract, but the action set they can sign cannot express
// AddKey, DeleteKey, DeleteAccount, or DeployContract. The escape hatch is closed at the type
// level: TxAction has only FunctionCall and Transfer, so a compromised renter can never seize
// the account away from House of Stake.
#[tokio::test]
async fn renter_can_spend_but_cannot_seize_the_account() -> Result<()> {
    let (_worker, h) = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;
    let before = access_keys(&wallet).await?;

    let (msg, proof) = tx_envelope(wallet.id(), h.admin.id(), transfer(1), 1, &owner, TX_DOMAIN);
    let spend = submit(&h, &h.registry, wallet.id(), &msg, &proof, 1).await?;
    assert!(spend.is_success(), "renter can move funds: {spend:#?}");

    let call = vec![TxAction::FunctionCall {
        method_name: "ft_transfer".to_string(),
        args: near_sdk::json_types::Base64VecU8(
            br#"{"receiver_id":"x.testnet","amount":"1"}"#.to_vec(),
        ),
        gas: Gas::from_tgas(5),
        deposit: near_sdk::NearToken::from_yoctonear(1),
    }];
    let (msg, proof) = tx_envelope(wallet.id(), h.admin.id(), call, 2, &owner, TX_DOMAIN);
    submit(&h, &h.registry, wallet.id(), &msg, &proof, 1)
        .await?
        .into_result()?;

    assert_eq!(
        access_keys(&wallet).await?,
        before,
        "no renter-signable action can add or change an access key"
    );
    Ok(())
}

// authority_sign_tx drives the reclaim sweep and must only ever answer to the marketplace
// authority (hos-extension). A renter or relay calling it directly is rejected by the
// deployed contract, not merely by an off-chain convention.
#[tokio::test]
async fn authority_sign_tx_rejects_non_marketplace_callers() -> Result<()> {
    let (_worker, h) = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    for caller in [&h.registry, &h.admin] {
        let attempt = caller
            .call(h.active_signer.id(), "authority_sign_tx")
            .args_json(json!({
                "wallet": wallet.id(),
                "receiver_id": h.admin.id(),
                "actions": transfer(1),
                "tx_nonce": "1",
                "block_hash": bs58::encode([0u8; 32]).into_string(),
            }))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(60))
            .transact()
            .await?;
        assert!(
            attempt.is_failure(),
            "only the marketplace authority may drive authority_sign_tx, {caller:?} must be rejected"
        );
    }
    Ok(())
}

// A signature made under the freeze domain must not authorize a transaction, and vice versa.
// Domain separation is enforced by the deployed active-signer, so a proof captured for one
// purpose cannot be replayed against another.
#[tokio::test]
async fn tx_path_rejects_a_foreign_domain_signature() -> Result<()> {
    let (_worker, h) = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    let (msg, proof) = tx_envelope(
        wallet.id(),
        h.admin.id(),
        transfer(1),
        1,
        &owner,
        FREEZE_DOMAIN,
    );
    let attempt = submit(&h, &h.registry, wallet.id(), &msg, &proof, 1).await?;
    assert!(
        attempt.is_failure(),
        "a freeze-domain signature must not authorize a transaction"
    );
    Ok(())
}

// submit_signed_tx for an account that active-signer does not manage must fail closed rather
// than reach the MPC signer, and it demands exactly one yoctoNEAR so a caller cannot batch it
// under an unrelated allowance.
#[tokio::test]
async fn tx_path_fails_closed_on_unknown_wallet_and_bad_deposit() -> Result<()> {
    let (_worker, h) = setup().await?;
    let owner = user_key(7);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    let ghost: AccountId = format!("ghost.{}", h.tla.id()).parse()?;
    let (msg, proof) = tx_envelope(&ghost, h.admin.id(), transfer(1), 1, &owner, TX_DOMAIN);
    let unknown = submit(&h, &h.registry, &ghost, &msg, &proof, 1).await?;
    assert!(
        unknown.is_failure(),
        "a wallet with no installed signer must be rejected"
    );

    let (msg, proof) = tx_envelope(wallet.id(), h.admin.id(), transfer(1), 1, &owner, TX_DOMAIN);
    let zero = submit(&h, &h.registry, wallet.id(), &msg, &proof, 0).await?;
    assert!(zero.is_failure(), "a zero-yocto deposit must be rejected");
    let over = submit(&h, &h.registry, wallet.id(), &msg, &proof, 2).await?;
    assert!(
        over.is_failure(),
        "an over-one-yocto deposit must be rejected"
    );
    Ok(())
}

// After a sale the previous renter's operating key is dead and the buyer's works, while the
// account's on-chain MPC key is untouched. Control moved by a storage swap, and the seller
// cannot sign for an account they no longer own.
#[tokio::test]
async fn sold_account_rejects_the_former_owner() -> Result<()> {
    let (_worker, h) = setup().await?;
    let owner = user_key(7);
    let buyer = user_key(8);
    let wallet = mint_wallet(&h, "alice", &owner).await?;

    h.registry
        .call(h.hos_extension.id(), "force_transfer")
        .args_json(json!({ "wallet": wallet.id(), "new_public_key": ws_pubkey(&buyer), "expected_current": null }))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?
        .into_result()?;

    let (msg, proof) = tx_envelope(wallet.id(), h.admin.id(), transfer(1), 1, &owner, TX_DOMAIN);
    let stale = submit(&h, &h.registry, wallet.id(), &msg, &proof, 1).await?;
    assert!(stale.is_failure(), "former owner key must be dead");

    let (msg, proof) = tx_envelope(wallet.id(), h.admin.id(), transfer(1), 1, &buyer, TX_DOMAIN);
    let fresh = submit(&h, &h.registry, wallet.id(), &msg, &proof, 1).await?;
    assert!(fresh.is_success(), "buyer key must work: {fresh:#?}");
    Ok(())
}
