use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use defuse_wallet::signature::{Deadline, RequestMessage};
use defuse_wallet::{PromiseSingle, Request};
use defuse_wallet_sdk::ed25519::ed25519_dalek::SigningKey;
use defuse_wallet_sdk::Signer as WalletSigner;
use near_crypto::{InMemorySigner, SecretKey as NcSecretKey};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::action::{Action, DeployGlobalContractAction, GlobalContractDeployMode};
use near_primitives::transaction::SignedTransaction;
use near_primitives::types::BlockReference;
use near_primitives::views::{FinalExecutionStatus, QueryRequest};
use near_workspaces::types::{Gas, KeyType, NearToken, PublicKey};
use near_workspaces::{Account, AccountId, Contract, Worker};
use serde_json::json;
use sha2::{Digest, Sha256};

const ACTIVE_SIGNER_WASM: &str = "../target/near/active_signer/active_signer.wasm";
const HOS_EXTENSION_WASM: &str = "../target/near/hos_extension/hos_extension.wasm";
const HOS_WALLET_WASM: &str = "../target/near/hos_wallet/hos_wallet.wasm";
const TLA_MANAGER_WASM: &str = "../target/near/tla_manager/tla_manager.wasm";
const TLA_REGISTRY_WASM: &str = "../target/near/tla_registry/tla_registry.wasm";
const TEST_FT_WASM: &str = "../target/near/test_ft/test_ft.wasm";

const TIMEOUT_SECS: u32 = 3600;
const GRACE_NS: u64 = 30 * 24 * 60 * 60 * 1_000_000_000;

fn user_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn raw_base58(key: &SigningKey) -> String {
    WalletSigner::public_key(key).to_string()
}

fn ws_pubkey(key: &SigningKey) -> PublicKey {
    PublicKey::try_from_parts(KeyType::ED25519, key.verifying_key().as_bytes())
        .expect("valid ed25519 public key")
}

async fn deploy_at(root: &Account, name: &str, balance: u128, wasm: &str) -> Result<Contract> {
    let account = root
        .create_subaccount(name)
        .initial_balance(NearToken::from_near(balance))
        .transact()
        .await?
        .into_result()?;
    let bytes = std::fs::read(wasm)?;
    Ok(account.deploy(&bytes).await?.into_result()?)
}

async fn deploy_wallet_global(
    worker: &Worker<near_workspaces::network::Sandbox>,
    deployer: &Account,
) -> Result<[u8; 32]> {
    let wasm = std::fs::read(HOS_WALLET_WASM)?;
    let client = JsonRpcClient::connect(worker.rpc_addr());
    let secret_key = NcSecretKey::from_str(&deployer.secret_key().to_string())?;
    let account_id: near_primitives::types::AccountId = deployer.id().as_str().parse()?;
    let public_key = secret_key.public_key();

    let access = client
        .call(methods::query::RpcQueryRequest {
            block_reference: BlockReference::latest(),
            request: QueryRequest::ViewAccessKey {
                account_id: account_id.clone(),
                public_key: public_key.clone(),
            },
        })
        .await?;
    let (nonce, block_hash) = match access.kind {
        QueryResponseKind::AccessKey(ak) => (ak.nonce, access.block_hash),
        _ => anyhow::bail!("unexpected query response for access key"),
    };

    let signer: near_crypto::Signer =
        InMemorySigner::from_secret_key(account_id.clone(), secret_key);
    let action = Action::DeployGlobalContract(DeployGlobalContractAction {
        code: wasm.clone().into(),
        deploy_mode: GlobalContractDeployMode::CodeHash,
    });
    let signed = SignedTransaction::from_actions(
        nonce + 1,
        account_id.clone(),
        account_id,
        &signer,
        vec![action],
        block_hash,
        0,
    );
    let outcome = client
        .call(methods::broadcast_tx_commit::RpcBroadcastTxCommitRequest {
            signed_transaction: signed,
        })
        .await?;
    match outcome.status {
        FinalExecutionStatus::SuccessValue(_) => {}
        other => anyhow::bail!("global contract deploy failed: {other:?}"),
    }
    Ok(Sha256::digest(&wasm).into())
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
    let proof = WalletSigner::sign(key, &msg).unwrap();
    (serde_json::to_value(&msg).unwrap(), proof)
}

struct Env {
    #[allow(dead_code)]
    worker: Worker<near_workspaces::network::Sandbox>,
    admin: Account,
    registry: Contract,
    active_signer: Contract,
    tla: Account,
    renter: Account,
}

async fn setup(grant_minter: bool) -> Result<Env> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;

    let admin = root
        .create_subaccount("admin")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let recovery = root
        .create_subaccount("recovery")
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;

    let active_signer = deploy_at(&root, "asigner", 30, ACTIVE_SIGNER_WASM).await?;
    let hos_extension = deploy_at(&root, "hosext", 30, HOS_EXTENSION_WASM).await?;
    let registry = deploy_at(&root, "registry", 50, TLA_REGISTRY_WASM).await?;

    active_signer
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "marketplace_authority": hos_extension.id(),
            "recovery_authority": recovery.id(),
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
            "recovery": recovery.id(),
        }))
        .transact()
        .await?
        .into_result()?;
    registry
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "hos_extension": hos_extension.id(),
            "parked_signer_pubkey": ws_pubkey(&user_key(99)),
            "grace_period_ns": GRACE_NS.to_string(),
        }))
        .transact()
        .await?
        .into_result()?;

    let deployer = root
        .create_subaccount("deployer")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let wallet_hash = deploy_wallet_global(&worker, &deployer).await?;

    let tla = root
        .create_subaccount("mytla")
        .initial_balance(NearToken::from_near(100))
        .transact()
        .await?
        .into_result()?;
    let manager_wasm = std::fs::read(TLA_MANAGER_WASM)?;
    let manager = tla.deploy(&manager_wasm).await?.into_result()?;
    manager
        .call("new")
        .args_json(json!({
            "registry": registry.id(),
            "active_signer": active_signer.id(),
            "hos_extension": hos_extension.id(),
            "wallet_code_hash": bs58::encode(wallet_hash).into_string(),
            "min_balance": NearToken::from_near(2),
        }))
        .transact()
        .await?
        .into_result()?;

    if grant_minter {
        admin
            .call(active_signer.id(), "add_minter")
            .args_json(json!({ "minter": tla.id() }))
            .transact()
            .await?
            .into_result()?;
    }
    admin
        .call(registry.id(), "register_tla")
        .args_json(json!({
            "tla_id": tla.id(),
            "tla_type": "Open",
            "premium_category": "Standard",
            "licensee": null,
        }))
        .transact()
        .await?
        .into_result()?;
    admin
        .call(registry.id(), "activate_open_tla")
        .args_json(json!({ "tla_id": tla.id() }))
        .transact()
        .await?
        .into_result()?;

    let renter = root
        .create_subaccount("renter")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;

    Ok(Env {
        worker,
        admin,
        registry,
        active_signer,
        tla,
        renter,
    })
}

#[tokio::test]
async fn signer_install_failure_keeps_name_then_retry_repairs() -> Result<()> {
    let env = setup(false).await?;
    let owner = user_key(7);

    let _ = env
        .renter
        .call(env.registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": env.tla.id(),
            "name": "alice",
            "owner_key": ws_pubkey(&owner),
            "main_wallet": env.renter.id(),
        }))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?;

    let wallet_id: AccountId = format!("alice.{}", env.tla.id()).parse()?;
    let pending: bool = env
        .registry
        .view("is_signer_pending")
        .args_json(json!({ "tla_id": env.tla.id(), "name": "alice" }))
        .await?
        .json()?;
    assert!(
        pending,
        "sub must be flagged signer-pending after install failure"
    );
    let available: bool = env
        .registry
        .view("is_name_available")
        .args_json(json!({ "tla_id": env.tla.id(), "name": "alice" }))
        .await?
        .json()?;
    assert!(
        !available,
        "the name must NOT be freed while the wallet account exists"
    );
    let signer: Option<String> = env
        .active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert!(signer.is_none(), "no signer should be installed yet");

    env.admin
        .call(env.active_signer.id(), "add_minter")
        .args_json(json!({ "minter": env.tla.id() }))
        .transact()
        .await?
        .into_result()?;
    let retry = env
        .renter
        .call(env.registry.id(), "retry_signer_install")
        .args_json(json!({ "tla_id": env.tla.id(), "name": "alice" }))
        .max_gas()
        .transact()
        .await?;
    assert!(
        retry.is_success(),
        "retry_signer_install must repair the pending sub: {retry:#?}"
    );

    let pending_after: bool = env
        .registry
        .view("is_signer_pending")
        .args_json(json!({ "tla_id": env.tla.id(), "name": "alice" }))
        .await?
        .json()?;
    assert!(
        !pending_after,
        "pending flag cleared after successful retry"
    );
    let signer_after: Option<String> = env
        .active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert_eq!(
        signer_after.as_deref(),
        Some(raw_base58(&owner).as_str()),
        "owner key installed on active-signer after retry"
    );

    Ok(())
}

#[tokio::test]
async fn full_registry_mint_flow() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;

    let admin = root
        .create_subaccount("admin")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let recovery = root
        .create_subaccount("recovery")
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;

    let active_signer = deploy_at(&root, "asigner", 30, ACTIVE_SIGNER_WASM).await?;
    let hos_extension = deploy_at(&root, "hosext", 30, HOS_EXTENSION_WASM).await?;
    let registry = deploy_at(&root, "registry", 50, TLA_REGISTRY_WASM).await?;

    active_signer
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "marketplace_authority": hos_extension.id(),
            "recovery_authority": recovery.id(),
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
            "recovery": recovery.id(),
        }))
        .transact()
        .await?
        .into_result()?;
    registry
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "hos_extension": hos_extension.id(),
            "parked_signer_pubkey": ws_pubkey(&user_key(99)),
            "grace_period_ns": GRACE_NS.to_string(),
        }))
        .transact()
        .await?
        .into_result()?;

    let deployer = root
        .create_subaccount("deployer")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let wallet_hash = deploy_wallet_global(&worker, &deployer).await?;

    let tla = root
        .create_subaccount("mytla")
        .initial_balance(NearToken::from_near(100))
        .transact()
        .await?
        .into_result()?;
    let manager_wasm = std::fs::read(TLA_MANAGER_WASM)?;
    let manager = tla.deploy(&manager_wasm).await?.into_result()?;
    manager
        .call("new")
        .args_json(json!({
            "registry": registry.id(),
            "active_signer": active_signer.id(),
            "hos_extension": hos_extension.id(),
            "wallet_code_hash": bs58::encode(wallet_hash).into_string(),
            "min_balance": NearToken::from_near(2),
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
    admin
        .call(registry.id(), "register_tla")
        .args_json(json!({
            "tla_id": tla.id(),
            "tla_type": "Open",
            "premium_category": "Standard",
            "licensee": null,
        }))
        .transact()
        .await?
        .into_result()?;
    admin
        .call(registry.id(), "activate_open_tla")
        .args_json(json!({ "tla_id": tla.id() }))
        .transact()
        .await?
        .into_result()?;

    let renter = root
        .create_subaccount("renter")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let owner = user_key(7);

    let rent = renter
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": tla.id(),
            "name": "alice",
            "owner_key": ws_pubkey(&owner),
            "main_wallet": renter.id(),
        }))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?;
    assert!(rent.is_success(), "rent_sub_account failed: {rent:#?}");

    let wallet_id: AccountId = format!("alice.{}", tla.id()).parse()?;
    let signer_of: Option<String> = active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert_eq!(
        signer_of.as_deref(),
        Some(raw_base58(&owner).as_str()),
        "minted wallet should have the owner installed on active-signer"
    );

    let recipient = root
        .create_subaccount("recipient")
        .initial_balance(NearToken::from_near(1))
        .transact()
        .await?
        .into_result()?;
    let before = recipient.view_account().await?.balance;
    let (msg, proof) = signed_transfer(
        &wallet_id,
        recipient.id(),
        NearToken::from_near(1),
        1,
        &owner,
    );
    let exec = renter
        .call(active_signer.id(), "submit_signed_request")
        .args_json(json!({ "wallet": wallet_id, "msg": msg, "proof": proof }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(120))
        .transact()
        .await?;
    assert!(
        exec.is_success(),
        "minted wallet should execute a signed request: {exec:#?}"
    );
    let after = recipient.view_account().await?.balance;
    assert!(
        after.as_yoctonear() > before.as_yoctonear(),
        "minted wallet should transfer to recipient"
    );

    let mut fee_config: serde_json::Value = registry.view("get_fee_config").await?.json()?;
    fee_config["resale_commission_bps"] = json!(500);
    admin
        .call(registry.id(), "update_fee_config")
        .args_json(json!({ "config": fee_config }))
        .transact()
        .await?
        .into_result()?;

    let revenue_before = registry_total_revenue(&registry).await?;
    let seller_refund_before = pending_refund(&registry, renter.id()).await?;

    renter
        .call(registry.id(), "list_sub_account")
        .args_json(json!({
            "tla_id": tla.id(),
            "name": "alice",
            "price": NearToken::from_near(10).as_yoctonear().to_string(),
            "owner_key": ws_pubkey(&owner),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;

    let buyer = root
        .create_subaccount("buyer")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let buyer_key = user_key(8);
    buyer
        .call(registry.id(), "buy_sub_account")
        .args_json(json!({
            "tla_id": tla.id(),
            "name": "alice",
            "new_owner_key": ws_pubkey(&buyer_key),
        }))
        .deposit(NearToken::from_near(12))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let price = NearToken::from_near(10).as_yoctonear();
    let deposit = NearToken::from_near(12).as_yoctonear();
    let commission = price * 500 / 10_000;
    let seller_proceeds = price - commission;
    let buyer_excess = deposit - price;

    let seller_delta = pending_refund(&registry, renter.id()).await? - seller_refund_before;
    let buyer_refund = pending_refund(&registry, buyer.id()).await?;
    let revenue_delta = registry_total_revenue(&registry).await? - revenue_before;

    assert_eq!(
        seller_delta, seller_proceeds,
        "seller credited price minus commission"
    );
    assert_eq!(buyer_refund, buyer_excess, "buyer refunded the overpayment");
    assert_eq!(revenue_delta, commission, "commission booked to revenue");
    assert_eq!(
        seller_delta + buyer_refund + revenue_delta,
        deposit,
        "every yocto of the buyer deposit must be accounted for"
    );

    let new_owner: Option<String> = active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert_eq!(
        new_owner.as_deref(),
        Some(raw_base58(&buyer_key).as_str()),
        "buyer controls the wallet after sale"
    );

    Ok(())
}

async fn pending_refund(registry: &Contract, account: &AccountId) -> Result<u128> {
    let v: near_sdk::json_types::U128 = registry
        .view("get_pending_refund")
        .args_json(json!({ "account_id": account }))
        .await?
        .json()?;
    Ok(v.0)
}

async fn registry_total_revenue(registry: &Contract) -> Result<u128> {
    let stats: serde_json::Value = registry.view("get_stats").await?.json()?;
    Ok(stats["total_revenue_yocto"].as_str().unwrap().parse()?)
}

#[tokio::test]
async fn asset_gate_fits_gas_at_full_allowlist() -> Result<()> {
    let env = setup(true).await?;
    let root = env.worker.root_account()?;

    for i in 0..16u8 {
        let ft = deploy_at(&root, &format!("ft{i}"), 3, TEST_FT_WASM).await?;
        ft.call("new")
            .args_json(json!({ "owner": ft.id(), "total_supply": "0" }))
            .transact()
            .await?
            .into_result()?;
        env.admin
            .call(env.registry.id(), "add_ft_allowlist")
            .args_json(json!({ "token": ft.id() }))
            .transact()
            .await?
            .into_result()?;
    }

    let owner = user_key(7);
    env.renter
        .call(env.registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": env.tla.id(),
            "name": "alice",
            "owner_key": ws_pubkey(&owner),
            "main_wallet": env.renter.id(),
        }))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    env.renter
        .call(env.registry.id(), "list_sub_account")
        .args_json(json!({
            "tla_id": env.tla.id(),
            "name": "alice",
            "price": NearToken::from_near(5).as_yoctonear().to_string(),
            "owner_key": ws_pubkey(&owner),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;

    let buyer = root
        .create_subaccount("buyer")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let buyer_key = user_key(8);
    let buy = buyer
        .call(env.registry.id(), "buy_sub_account")
        .args_json(json!({
            "tla_id": env.tla.id(),
            "name": "alice",
            "new_owner_key": ws_pubkey(&buyer_key),
        }))
        .deposit(NearToken::from_near(6))
        .max_gas()
        .transact()
        .await?;
    assert!(
        buy.is_success(),
        "buy must fit in gas with a full 16-token asset-gate fan-out: {buy:#?}"
    );

    let wallet_id: AccountId = format!("alice.{}", env.tla.id()).parse()?;
    let new_owner: Option<String> = env
        .active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert_eq!(
        new_owner.as_deref(),
        Some(raw_base58(&buyer_key).as_str()),
        "sale settles after the gate clears all 16 balances within gas"
    );

    Ok(())
}

#[tokio::test]
async fn sale_voids_when_listed_owner_key_is_stale() -> Result<()> {
    let env = setup(true).await?;
    let root = env.worker.root_account()?;
    let owner = user_key(7);

    env.renter
        .call(env.registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": env.tla.id(),
            "name": "alice",
            "owner_key": ws_pubkey(&owner),
            "main_wallet": env.renter.id(),
        }))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let stale = user_key(50);
    env.renter
        .call(env.registry.id(), "list_sub_account")
        .args_json(json!({
            "tla_id": env.tla.id(),
            "name": "alice",
            "price": NearToken::from_near(5).as_yoctonear().to_string(),
            "owner_key": ws_pubkey(&stale),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;

    let buyer = root
        .create_subaccount("buyer")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    buyer
        .call(env.registry.id(), "buy_sub_account")
        .args_json(json!({
            "tla_id": env.tla.id(),
            "name": "alice",
            "new_owner_key": ws_pubkey(&user_key(8)),
        }))
        .deposit(NearToken::from_near(6))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let wallet_id: AccountId = format!("alice.{}", env.tla.id()).parse()?;
    let signer: Option<String> = env
        .active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert_eq!(
        signer.as_deref(),
        Some(raw_base58(&owner).as_str()),
        "a stale-key listing must void the swap and leave the wallet signer unchanged"
    );
    let refund: String = env
        .registry
        .view("get_pending_refund")
        .args_json(json!({ "account_id": buyer.id() }))
        .await?
        .json()?;
    assert_ne!(refund, "0", "buyer must be refunded when the sale voids on a CAS mismatch");

    Ok(())
}

#[tokio::test]
async fn reclaim_rerent_and_asset_gate() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;

    let admin = root
        .create_subaccount("admin")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let recovery = root
        .create_subaccount("recovery")
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;

    let active_signer = deploy_at(&root, "asigner", 30, ACTIVE_SIGNER_WASM).await?;
    let hos_extension = deploy_at(&root, "hosext", 30, HOS_EXTENSION_WASM).await?;
    let registry = deploy_at(&root, "registry", 50, TLA_REGISTRY_WASM).await?;

    active_signer
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "marketplace_authority": hos_extension.id(),
            "recovery_authority": recovery.id(),
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
            "recovery": recovery.id(),
        }))
        .transact()
        .await?
        .into_result()?;
    registry
        .call("new")
        .args_json(json!({
            "admin": admin.id(),
            "hos_extension": hos_extension.id(),
            "parked_signer_pubkey": ws_pubkey(&user_key(99)),
            "grace_period_ns": GRACE_NS.to_string(),
        }))
        .transact()
        .await?
        .into_result()?;

    let deployer = root
        .create_subaccount("deployer")
        .initial_balance(NearToken::from_near(50))
        .transact()
        .await?
        .into_result()?;
    let wallet_hash = deploy_wallet_global(&worker, &deployer).await?;

    let tla = root
        .create_subaccount("biztla")
        .initial_balance(NearToken::from_near(100))
        .transact()
        .await?
        .into_result()?;
    let manager_wasm = std::fs::read(TLA_MANAGER_WASM)?;
    let manager = tla.deploy(&manager_wasm).await?.into_result()?;
    manager
        .call("new")
        .args_json(json!({
            "registry": registry.id(),
            "active_signer": active_signer.id(),
            "hos_extension": hos_extension.id(),
            "wallet_code_hash": bs58::encode(wallet_hash).into_string(),
            "min_balance": NearToken::from_near(2),
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

    let one = NearToken::from_near(1).as_yoctonear().to_string();
    let two = NearToken::from_near(2).as_yoctonear().to_string();
    admin
        .call(registry.id(), "update_fee_config")
        .args_json(json!({ "config": {
            "tla_allocation_fee": one,
            "rent_tier_5": one,
            "rent_tier_8": one,
            "rent_tier_10": one,
            "rent_tier_12plus": one,
            "sub_fee_per_account": one,
            "account_creation_deposit": two,
            "business_max_subs": 1000,
            "retraction_notice_ns": "1",
            "resale_commission_bps": 0,
        }}))
        .transact()
        .await?
        .into_result()?;

    let licensee = root
        .create_subaccount("licensee")
        .initial_balance(NearToken::from_near(100))
        .transact()
        .await?
        .into_result()?;

    admin
        .call(registry.id(), "register_tla")
        .args_json(json!({
            "tla_id": tla.id(),
            "tla_type": "Business",
            "premium_category": "Standard",
            "licensee": licensee.id(),
        }))
        .transact()
        .await?
        .into_result()?;
    licensee
        .call(registry.id(), "activate_tla")
        .args_json(json!({ "tla_id": tla.id() }))
        .deposit(NearToken::from_near(5))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let owner = user_key(7);
    let rent = licensee
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": tla.id(),
            "name": "staff",
            "owner_key": ws_pubkey(&owner),
            "main_wallet": licensee.id(),
        }))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?;
    assert!(rent.is_success(), "business rent failed: {rent:#?}");

    licensee
        .call(registry.id(), "schedule_retraction")
        .args_json(json!({ "tla_id": tla.id(), "name": "staff" }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;

    worker.fast_forward(2).await?;

    let finalize = admin
        .call(registry.id(), "reclaim_finalize")
        .args_json(json!({ "tla_id": tla.id(), "name": "staff" }))
        .max_gas()
        .transact()
        .await?;
    assert!(finalize.is_success(), "reclaim_finalize failed: {finalize:#?}");

    let wallet_id: AccountId = format!("staff.{}", tla.id()).parse()?;
    let parked_owner: Option<String> = active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert_eq!(
        parked_owner.as_deref(),
        Some(raw_base58(&user_key(99)).as_str()),
        "park must rotate the wallet owner to the parked signer key (force_transfer fit gas)"
    );

    let ft = deploy_at(&root, "usdc", 5, TEST_FT_WASM).await?;
    ft.call("new")
        .args_json(json!({ "owner": ft.id(), "total_supply": "0" }))
        .transact()
        .await?
        .into_result()?;
    ft.call("mint")
        .args_json(json!({ "account_id": wallet_id, "amount": "1000000" }))
        .transact()
        .await?
        .into_result()?;
    admin
        .call(registry.id(), "add_ft_allowlist")
        .args_json(json!({ "token": ft.id() }))
        .transact()
        .await?
        .into_result()?;

    let _ = licensee
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": tla.id(),
            "name": "staff",
            "owner_key": ws_pubkey(&user_key(10)),
            "main_wallet": licensee.id(),
        }))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?;
    let still_parked: Option<String> = active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert_eq!(
        still_parked.as_deref(),
        Some(raw_base58(&user_key(99)).as_str()),
        "re-rent must be blocked by the asset gate while the parked wallet holds an allow-listed FT"
    );

    admin
        .call(registry.id(), "remove_ft_allowlist")
        .args_json(json!({ "token": ft.id() }))
        .transact()
        .await?
        .into_result()?;

    let new_owner = user_key(8);
    let re_rent = licensee
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": tla.id(),
            "name": "staff",
            "owner_key": ws_pubkey(&new_owner),
            "main_wallet": licensee.id(),
        }))
        .deposit(NearToken::from_near(10))
        .max_gas()
        .transact()
        .await?;
    assert!(re_rent.is_success(), "re-rent failed: {re_rent:#?}");

    let re_rented_owner: Option<String> = active_signer
        .view("signer_of")
        .args_json(json!({ "wallet": wallet_id }))
        .await?
        .json()?;
    assert_eq!(
        re_rented_owner.as_deref(),
        Some(raw_base58(&new_owner).as_str()),
        "re-rent must rotate the parked wallet to the new renter (force_transfer fit gas)"
    );

    Ok(())
}
