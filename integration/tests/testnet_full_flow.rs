use std::str::FromStr;
use std::time::{Duration, SystemTime};

use active_signer::{TxMessage, CHAIN_ID, TX_DOMAIN};
use anyhow::Result;
use defuse_wallet::signature::ed25519::Ed25519Signature;
use defuse_wallet_sdk::ed25519::ed25519_dalek::Signer as DalekSigner;
use defuse_wallet_sdk::ed25519::ed25519_dalek::SigningKey;
use hos_common::tx::TxAction;
use near_crypto::{InMemorySigner, SecretKey};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_jsonrpc_primitives::types::transactions::TransactionInfo;
use near_primitives::account::{AccessKey, AccessKeyPermission};
use near_primitives::action::{
    Action, AddKeyAction, CreateAccountAction, DeleteAccountAction, DeployContractAction,
    FunctionCallAction, TransferAction,
};
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{SignedTransaction, Transaction, TransactionV0};
use near_primitives::types::{AccountId, Balance, BlockReference, Gas};
use near_primitives::views::{
    FinalExecutionOutcomeViewEnum, FinalExecutionStatus, QueryRequest, TxExecutionStatus,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ACTIVE_SIGNER_WASM: &str = "../target/near/active_signer/active_signer.wasm";
const HOS_EXTENSION_WASM: &str = "../target/near/hos_extension/hos_extension.wasm";
const MPC_RECOVERY_WASM: &str = "../target/near/mpc_recovery/mpc_recovery.wasm";
const TLA_MANAGER_WASM: &str = "../target/near/tla_manager/tla_manager.wasm";
const TLA_REGISTRY_WASM: &str = "../target/near/tla_registry/tla_registry.wasm";

const V1_SIGNER: &str = "v1.signer-prod.testnet";
const DEFAULT_RPC: &str = "https://test.rpc.fastnear.com";
const TIMEOUT_SECS: u32 = 3600;
const RECOVERY_TIMELOCK_SECS: u32 = 60;
const GRACE_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

fn user_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn raw_pub(key: &SigningKey) -> String {
    format!(
        "ed25519:{}",
        bs58::encode(key.verifying_key().as_bytes()).into_string()
    )
}

fn near_amount(n: u128) -> Balance {
    Balance::from_yoctonear(n * 1_000_000_000_000_000_000_000_000)
}

fn now_secs() -> u32 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32
}

struct Chain {
    client: JsonRpcClient,
    root: AccountId,
    root_key: SecretKey,
}

impl Chain {
    async fn key_state(
        &self,
        account: &AccountId,
        key: &near_crypto::PublicKey,
    ) -> Result<(u64, CryptoHash)> {
        let access = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::ViewAccessKey {
                    account_id: account.clone(),
                    public_key: key.clone(),
                },
            })
            .await?;
        match access.kind {
            QueryResponseKind::AccessKey(ak) => Ok((ak.nonce, access.block_hash)),
            _ => anyhow::bail!("unexpected access-key response"),
        }
    }

    async fn access_keys(&self, account: &AccountId) -> Result<Vec<String>> {
        let resp = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::ViewAccessKeyList {
                    account_id: account.clone(),
                },
            })
            .await?;
        match resp.kind {
            QueryResponseKind::AccessKeyList(list) => {
                Ok(list.keys.into_iter().map(|k| k.public_key.to_string()).collect())
            }
            _ => anyhow::bail!("unexpected access-key-list response"),
        }
    }

    async fn balance(&self, account: &AccountId) -> Result<Balance> {
        let resp = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::ViewAccount {
                    account_id: account.clone(),
                },
            })
            .await?;
        match resp.kind {
            QueryResponseKind::ViewAccount(a) => Ok(a.amount),
            _ => anyhow::bail!("unexpected account response"),
        }
    }

    async fn view(&self, contract: &AccountId, method: &str, args: Value) -> Result<Value> {
        let resp = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::CallFunction {
                    account_id: contract.clone(),
                    method_name: method.to_string(),
                    args: args.to_string().into_bytes().into(),
                },
            })
            .await?;
        match resp.kind {
            QueryResponseKind::CallResult(r) if r.result.is_empty() => Ok(Value::Null),
            QueryResponseKind::CallResult(r) => Ok(serde_json::from_slice(&r.result)?),
            _ => anyhow::bail!("unexpected call response"),
        }
    }

    async fn send(
        &self,
        signer_id: &AccountId,
        signer_key: &SecretKey,
        receiver: &AccountId,
        actions: Vec<Action>,
    ) -> Result<Value> {
        let (nonce, block_hash) = self.key_state(signer_id, &signer_key.public_key()).await?;
        let signer: near_crypto::Signer =
            InMemorySigner::from_secret_key(signer_id.clone(), signer_key.clone());
        let signed = SignedTransaction::from_actions(
            nonce + 1,
            signer_id.clone(),
            receiver.clone(),
            &signer,
            actions,
            block_hash,
            0,
        );
        self.broadcast(signed).await
    }

    async fn root_send(&self, receiver: &AccountId, actions: Vec<Action>) -> Result<Value> {
        self.send(&self.root, &self.root_key, receiver, actions).await
    }

    async fn broadcast(&self, signed: SignedTransaction) -> Result<Value> {
        let sender = signed.transaction.signer_id().clone();
        let hash = self
            .client
            .call(methods::broadcast_tx_async::RpcBroadcastTxAsyncRequest {
                signed_transaction: signed,
            })
            .await?;
        for _ in 0..400 {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let resp = self
                .client
                .call(methods::tx::RpcTransactionStatusRequest {
                    transaction_info: TransactionInfo::TransactionId {
                        tx_hash: hash,
                        sender_account_id: sender.clone(),
                    },
                    wait_until: TxExecutionStatus::Final,
                })
                .await;
            let Ok(resp) = resp else { continue };
            let Some(outcome) = resp.final_execution_outcome else {
                continue;
            };
            let outcome = match outcome {
                FinalExecutionOutcomeViewEnum::FinalExecutionOutcome(o) => o,
                FinalExecutionOutcomeViewEnum::FinalExecutionOutcomeWithReceipt(o) => o.final_outcome,
            };
            match outcome.status {
                FinalExecutionStatus::SuccessValue(v) => {
                    return Ok(if v.is_empty() {
                        Value::Null
                    } else {
                        serde_json::from_slice(&v).unwrap_or(Value::Null)
                    });
                }
                FinalExecutionStatus::Failure(err) => anyhow::bail!("tx {hash} failed: {err:?}"),
                _ => continue,
            }
        }
        anyhow::bail!("tx {hash} did not finalize")
    }

    async fn create_funded(&self, id: &AccountId, key: &SecretKey, deposit: Balance) -> Result<()> {
        self.root_send(
            id,
            vec![
                Action::CreateAccount(CreateAccountAction {}),
                Action::Transfer(TransferAction { deposit }),
                add_full_access(near_pk(key)),
            ],
        )
        .await?;
        Ok(())
    }

    async fn deploy(&self, id: &AccountId, key: &SecretKey, wasm: &str, init: Value) -> Result<()> {
        let code = std::fs::read(wasm)?;
        let (method, args) = init_call(init);
        self.send(
            id,
            key,
            id,
            vec![
                Action::DeployContract(DeployContractAction { code }),
                Action::FunctionCall(Box::new(FunctionCallAction {
                    method_name: method,
                    args,
                    gas: Gas::from_teragas(50),
                    deposit: Balance::from_yoctonear(0),
                })),
            ],
        )
        .await?;
        Ok(())
    }
}

fn init_call(init: Value) -> (String, Vec<u8>) {
    ("new".to_string(), init.to_string().into_bytes())
}

fn near_pk(key: &SecretKey) -> near_crypto::PublicKey {
    key.public_key()
}

fn add_full_access(public_key: near_crypto::PublicKey) -> Action {
    Action::AddKey(Box::new(AddKeyAction {
        public_key,
        access_key: AccessKey {
            nonce: 0,
            permission: AccessKeyPermission::FullAccess,
        },
    }))
}

fn call(method: &str, args: Value, tgas: u64, deposit: Balance) -> Action {
    Action::FunctionCall(Box::new(FunctionCallAction {
        method_name: method.to_string(),
        args: args.to_string().into_bytes(),
        gas: Gas::from_teragas(tgas),
        deposit,
    }))
}

fn tx_envelope(
    wallet: &AccountId,
    receiver: &AccountId,
    actions: Vec<TxAction>,
    nonce: u32,
    key: &SigningKey,
) -> (Value, String) {
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
    let hash = Sha256::digest([TX_DOMAIN, serialized.as_slice()].concat());
    let proof = Ed25519Signature(DalekSigner::sign(key, hash.as_slice()).to_bytes()).to_string();
    (serde_json::to_value(&msg).unwrap(), proof)
}

fn push_len_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}

fn pubkey_bytes(key: &SigningKey) -> Vec<u8> {
    let mut v = vec![0u8];
    v.extend_from_slice(key.verifying_key().as_bytes());
    v
}

fn b64(bytes: Vec<u8>) -> Value {
    serde_json::to_value(near_sdk::json_types::Base64VecU8::from(bytes)).unwrap()
}

fn attestation(
    mother: &SigningKey,
    contract: &AccountId,
    account: &AccountId,
    new_owner: &SigningKey,
    round: u64,
) -> Value {
    let mut m = vec![1u8];
    push_len_str(&mut m, contract.as_str());
    push_len_str(&mut m, account.as_str());
    m.extend_from_slice(&pubkey_bytes(new_owner));
    m.extend_from_slice(&round.to_le_bytes());
    b64(DalekSigner::sign(mother, &m).to_bytes().to_vec())
}

fn verdict_sig(
    watcher: &SigningKey,
    contract: &AccountId,
    account: &AccountId,
    new_owner: &SigningKey,
    round: u64,
    silent: bool,
) -> Value {
    let mut m = vec![2u8];
    push_len_str(&mut m, contract.as_str());
    push_len_str(&mut m, account.as_str());
    m.extend_from_slice(&pubkey_bytes(new_owner));
    m.extend_from_slice(&round.to_le_bytes());
    m.push(silent as u8);
    json!({ "public_key": raw_pub(watcher), "signature": b64(DalekSigner::sign(watcher, &m).to_bytes().to_vec()) })
}

async fn operate_transfer(
    c: &Chain,
    active_signer: &AccountId,
    relay: &AccountId,
    relay_key: &SecretKey,
    wallet: &AccountId,
    mpc_pub: &near_crypto::PublicKey,
    to: &AccountId,
    amount: Balance,
    nonce: u32,
    owner: &SigningKey,
) -> Result<()> {
    let (tx_nonce, block_hash) = c.key_state(wallet, mpc_pub).await?;
    let (msg, proof) = tx_envelope(
        wallet,
        to,
        vec![TxAction::Transfer {
            deposit: near_sdk::NearToken::from_yoctonear(amount.as_yoctonear()),
        }],
        nonce,
        owner,
    );
    let signed = c
        .send(
            relay,
            relay_key,
            active_signer,
            vec![call(
                "submit_signed_tx",
                json!({
                    "wallet": wallet,
                    "msg": msg,
                    "proof": proof,
                    "tx_nonce": (tx_nonce + 1).to_string(),
                    "block_hash": bs58::encode(block_hash.0).into_string(),
                }),
                150,
                Balance::from_yoctonear(1),
            )],
        )
        .await?;
    assemble_and_broadcast(c, &signed).await
}

async fn assemble_and_broadcast(c: &Chain, signed: &Value) -> Result<()> {
    let hex = signed["unsigned_tx_hex"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no unsigned_tx_hex"))?;
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<_, _>>()?;
    let tx: TransactionV0 = near_sdk::borsh::from_slice(&bytes)?;
    let sig_bytes: Vec<u8> = signed["mpc_signature"]["signature"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no signature"))?
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let signature = near_crypto::Signature::from_parts(near_crypto::KeyType::ED25519, &sig_bytes)?;
    c.broadcast(SignedTransaction::new(signature, Transaction::V0(tx)))
        .await?;
    Ok(())
}

async fn derived_key(c: &Chain, active_signer: &AccountId, wallet: &AccountId) -> Result<String> {
    let v1: AccountId = V1_SIGNER.parse()?;
    let out = c
        .view(
            &v1,
            "derived_public_key",
            json!({
                "path": format!("hos-tla/{wallet}"),
                "predecessor": active_signer,
                "domain_id": 1,
            }),
        )
        .await?;
    Ok(out.as_str().unwrap().to_string())
}

#[tokio::test]
#[ignore]
async fn testnet_full_flow() -> Result<()> {
    let root: AccountId = std::env::var("DEPLOY_ROOT_ID")?.parse()?;
    let root_key = SecretKey::from_str(&std::env::var("DEPLOY_ROOT_SECRET_KEY")?)?;
    let rpc = std::env::var("TESTNET_RPC").unwrap_or_else(|_| DEFAULT_RPC.to_string());
    let c = Chain {
        client: JsonRpcClient::connect(&rpc),
        root: root.clone(),
        root_key: root_key.clone(),
    };

    let status = c.client.call(methods::status::RpcStatusRequest).await?;
    let tag = status.sync_info.latest_block_height % 100_000;

    let admin = root.clone();
    let signer_key = SecretKey::from_random(near_crypto::KeyType::ED25519);
    let ext_key = SecretKey::from_random(near_crypto::KeyType::ED25519);
    let rec_key = SecretKey::from_random(near_crypto::KeyType::ED25519);
    let reg_key = SecretKey::from_random(near_crypto::KeyType::ED25519);
    let tla_key = SecretKey::from_random(near_crypto::KeyType::ED25519);
    let biz_key = SecretKey::from_random(near_crypto::KeyType::ED25519);

    let active_signer: AccountId = format!("as{tag}.{root}").parse()?;
    let hos_extension: AccountId = format!("ext{tag}.{root}").parse()?;
    let recovery: AccountId = format!("rec{tag}.{root}").parse()?;
    let registry: AccountId = format!("reg{tag}.{root}").parse()?;
    let tla: AccountId = format!("open{tag}.{root}").parse()?;
    let biz: AccountId = format!("biz{tag}.{root}").parse()?;

    let watcher = user_key(20);

    println!("== deploying suite under {root} (tag {tag}) ==");
    c.create_funded(&active_signer, &signer_key, near_amount(6)).await?;
    c.create_funded(&hos_extension, &ext_key, near_amount(4)).await?;
    c.create_funded(&recovery, &rec_key, near_amount(4)).await?;
    c.create_funded(&registry, &reg_key, near_amount(7)).await?;
    c.create_funded(&tla, &tla_key, near_amount(5)).await?;
    c.create_funded(&biz, &biz_key, near_amount(5)).await?;

    c.deploy(
        &active_signer,
        &signer_key,
        ACTIVE_SIGNER_WASM,
        json!({
            "admin": admin,
            "marketplace_authority": hos_extension,
            "recovery_authority": recovery,
            "mpc_signer": V1_SIGNER,
            "timeout_secs": TIMEOUT_SECS,
        }),
    )
    .await?;
    c.deploy(
        &hos_extension,
        &ext_key,
        HOS_EXTENSION_WASM,
        json!({ "admin": admin, "registry": registry, "active_signer": active_signer, "recovery": recovery }),
    )
    .await?;
    c.deploy(
        &recovery,
        &rec_key,
        MPC_RECOVERY_WASM,
        json!({
            "owner": admin,
            "signer": admin,
            "transfer_authority": hos_extension,
            "watchers": [raw_pub(&watcher)],
            "threshold": 1,
        }),
    )
    .await?;
    c.deploy(
        &registry,
        &reg_key,
        TLA_REGISTRY_WASM,
        json!({
            "admin": admin,
            "hos_extension": hos_extension,
            "active_signer": active_signer,
            "parked_signer_pubkey": raw_pub(&user_key(99)),
            "grace_period_ns": GRACE_NS.to_string(),
        }),
    )
    .await?;
    c.deploy(
        &tla,
        &tla_key,
        TLA_MANAGER_WASM,
        json!({
            "registry": registry,
            "active_signer": active_signer,
            "mpc_signer": V1_SIGNER,
            "min_balance": near_amount(2),
        }),
    )
    .await?;
    c.deploy(
        &biz,
        &biz_key,
        TLA_MANAGER_WASM,
        json!({
            "registry": registry,
            "active_signer": active_signer,
            "mpc_signer": V1_SIGNER,
            "min_balance": near_amount(2),
        }),
    )
    .await?;
    println!("   deployed active-signer, hos-extension, mpc-recovery, registry, 2 managers");

    c.root_send(
        &active_signer,
        vec![
            call("add_minter", json!({ "minter": tla }), 10, Balance::from_yoctonear(0)),
            call("add_minter", json!({ "minter": biz }), 10, Balance::from_yoctonear(0)),
        ],
    )
    .await?;

    println!("== OPEN TLA: register + activate ==");
    c.root_send(
        &registry,
        vec![call(
            "register_tla",
            json!({ "tla_id": tla, "tla_type": "Open", "premium_category": "Standard", "licensee": null }),
            30,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;
    c.root_send(
        &registry,
        vec![call("activate_open_tla", json!({ "tla_id": tla }), 30, Balance::from_yoctonear(0))],
    )
    .await?;

    let renter_id: AccountId = format!("renter{tag}.{root}").parse()?;
    let renter_key = SecretKey::from_random(near_crypto::KeyType::ED25519);
    c.create_funded(&renter_id, &renter_key, near_amount(15)).await?;
    let owner = user_key(7);

    println!("== MINT / RENT alice ==");
    c.send(
        &renter_id,
        &renter_key,
        &registry,
        vec![call(
            "rent_sub_account",
            json!({ "tla_id": tla, "name": "alice", "owner_key": raw_pub(&owner), "main_wallet": renter_id }),
            300,
            near_amount(10),
        )],
    )
    .await?;
    let wallet: AccountId = format!("alice.{tla}").parse()?;
    let signer_of = c.view(&active_signer, "signer_of", json!({ "wallet": wallet })).await?;
    assert_eq!(signer_of.as_str(), Some(raw_pub(&owner).as_str()), "owner installed");
    let mpc_str = derived_key(&c, &active_signer, &wallet).await?;
    let keys = c.access_keys(&wallet).await?;
    assert_eq!(keys, vec![mpc_str.clone()], "wallet has exactly the MPC FullAccess key");
    println!("   alice minted, sole on-chain key = {mpc_str}");
    let mpc_pub = near_crypto::PublicKey::from_str(&mpc_str)?;

    println!("== OPERATE: renter signs, v1.signer signs, broadcast ==");
    let before = c.balance(&admin).await?;
    operate_transfer(
        &c, &active_signer, &renter_id, &renter_key, &wallet, &mpc_pub, &admin, near_amount(1), 1,
        &owner,
    )
    .await?;
    assert!(c.balance(&admin).await? > before, "operate transfer landed on chain");
    println!("   transfer executed by the wallet's MPC key");

    println!("== LIST + BUY (sell) ==");
    c.send(
        &renter_id,
        &renter_key,
        &registry,
        vec![call(
            "list_sub_account",
            json!({ "tla_id": tla, "name": "alice", "price": near_amount(5).as_yoctonear().to_string(), "owner_key": raw_pub(&owner) }),
            300,
            Balance::from_yoctonear(1),
        )],
    )
    .await?;
    let buyer_id: AccountId = format!("buyer{tag}.{root}").parse()?;
    let buyer_key = SecretKey::from_random(near_crypto::KeyType::ED25519);
    c.create_funded(&buyer_id, &buyer_key, near_amount(10)).await?;
    let buyer_owner = user_key(8);
    c.send(
        &buyer_id,
        &buyer_key,
        &registry,
        vec![call(
            "buy_sub_account",
            json!({ "tla_id": tla, "name": "alice", "new_owner_key": raw_pub(&buyer_owner) }),
            300,
            near_amount(6),
        )],
    )
    .await?;
    let sold_to = c.view(&active_signer, "signer_of", json!({ "wallet": wallet })).await?;
    assert_eq!(sold_to.as_str(), Some(raw_pub(&buyer_owner).as_str()), "buyer controls after sale");
    let keys_after = c.access_keys(&wallet).await?;
    assert_eq!(keys_after, vec![mpc_str.clone()], "MPC key unchanged across sale");
    println!("   alice sold; operating key swapped, MPC key immutable");

    println!("== BUSINESS TLA: register + activate + rent + reclaim ==");
    let licensee_id: AccountId = format!("lic{tag}.{root}").parse()?;
    let licensee_key = SecretKey::from_random(near_crypto::KeyType::ED25519);
    c.create_funded(&licensee_id, &licensee_key, near_amount(15)).await?;
    c.root_send(
        &registry,
        vec![call(
            "update_fee_config",
            json!({ "config": {
                "tla_allocation_fee": near_amount(1).as_yoctonear().to_string(),
                "rent_tier_5": near_amount(1).as_yoctonear().to_string(),
                "rent_tier_8": near_amount(1).as_yoctonear().to_string(),
                "rent_tier_10": near_amount(1).as_yoctonear().to_string(),
                "rent_tier_12plus": near_amount(1).as_yoctonear().to_string(),
                "sub_fee_per_account": near_amount(1).as_yoctonear().to_string(),
                "account_creation_deposit": near_amount(2).as_yoctonear().to_string(),
                "business_max_subs": 1000,
                "retraction_notice_ns": "1",
                "resale_commission_bps": 0,
            }}),
            30,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;
    c.root_send(
        &registry,
        vec![call(
            "register_tla",
            json!({ "tla_id": biz, "tla_type": "Business", "premium_category": "Standard", "licensee": licensee_id }),
            30,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;
    c.send(
        &licensee_id,
        &licensee_key,
        &registry,
        vec![call("activate_tla", json!({ "tla_id": biz }), 60, near_amount(5))],
    )
    .await?;
    c.send(
        &licensee_id,
        &licensee_key,
        &registry,
        vec![call(
            "rent_sub_account",
            json!({ "tla_id": biz, "name": "staff", "owner_key": raw_pub(&user_key(7)), "main_wallet": licensee_id }),
            300,
            near_amount(10),
        )],
    )
    .await?;
    let staff: AccountId = format!("staff.{biz}").parse()?;
    let staff_signer = c.view(&active_signer, "signer_of", json!({ "wallet": staff })).await?;
    assert_eq!(staff_signer.as_str(), Some(raw_pub(&user_key(7)).as_str()), "business mint installed owner");
    println!("   business sub staff minted");

    c.send(
        &licensee_id,
        &licensee_key,
        &registry,
        vec![call(
            "schedule_retraction",
            json!({ "tla_id": biz, "name": "staff" }),
            30,
            Balance::from_yoctonear(1),
        )],
    )
    .await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    c.root_send(
        &registry,
        vec![call("reclaim_finalize", json!({ "tla_id": biz, "name": "staff" }), 300, Balance::from_yoctonear(0))],
    )
    .await?;
    let parked = c.view(&active_signer, "signer_of", json!({ "wallet": staff })).await?;
    assert_eq!(parked.as_str(), Some(raw_pub(&user_key(99)).as_str()), "reclaim parked the staff wallet");
    println!("   staff reclaimed and parked to the parked signer key");

    println!("== RECOVERY: install policy, request, verdict, finalize ==");
    let rec_owner = user_key(7);
    let mother = user_key(11);
    let new_owner = user_key(33);
    c.root_send(
        &recovery,
        vec![call(
            "install_policy",
            json!({
                "account": wallet,
                "target": { "Wallet": { "active_signer": active_signer, "bound_owner": raw_pub(&buyer_owner) } },
                "attestation_key": raw_pub(&mother),
                "timelock_secs": RECOVERY_TIMELOCK_SECS,
            }),
            30,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;
    let _ = rec_owner;
    c.root_send(
        &recovery,
        vec![call(
            "request_recovery",
            json!({
                "account": wallet,
                "new_owner": raw_pub(&new_owner),
                "round": "0",
                "attestation": attestation(&mother, &recovery, &wallet, &new_owner, 0),
            }),
            30,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;
    println!("   recovery requested; waiting out the {RECOVERY_TIMELOCK_SECS}s timelock");
    tokio::time::sleep(Duration::from_secs((RECOVERY_TIMELOCK_SECS + 5) as u64)).await;
    c.root_send(
        &recovery,
        vec![call(
            "submit_verdict",
            json!({
                "account": wallet,
                "silent": true,
                "signatures": [verdict_sig(&watcher, &recovery, &wallet, &new_owner, 0, true)],
            }),
            120,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;
    c.root_send(
        &recovery,
        vec![call(
            "finalize_recovery",
            json!({ "account": wallet, "nonce": "0", "block_hash": "11111111111111111111111111111111" }),
            120,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;
    let recovered = c.view(&active_signer, "signer_of", json!({ "wallet": wallet })).await?;
    assert_eq!(recovered.as_str(), Some(raw_pub(&new_owner).as_str()), "recovery swapped to the new owner");
    let keys_final = c.access_keys(&wallet).await?;
    assert_eq!(keys_final, vec![mpc_str.clone()], "MPC key immutable across recovery");
    println!("   recovery complete; operating key = new owner, MPC key untouched");

    println!("== TEARDOWN ==");
    for (id, key) in [
        (&wallet, None),
        (&renter_id, Some(&renter_key)),
        (&buyer_id, Some(&buyer_key)),
        (&licensee_id, Some(&licensee_key)),
        (&staff, None),
        (&tla, Some(&tla_key)),
        (&biz, Some(&biz_key)),
        (&registry, Some(&reg_key)),
        (&hos_extension, Some(&ext_key)),
        (&recovery, Some(&rec_key)),
        (&active_signer, Some(&signer_key)),
    ] {
        if let Some(k) = key {
            let _ = c
                .send(id, k, id, vec![Action::DeleteAccount(DeleteAccountAction { beneficiary_id: root.clone() })])
                .await;
        }
    }
    println!("== FULL ON-CHAIN FLOW PASSED ==");
    Ok(())
}
