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
use serde_json::json;
use sha2::{Digest, Sha256};

const ACTIVE_SIGNER_WASM: &str = "../target/near/active_signer/active_signer.wasm";
const V1_SIGNER: &str = "v1.signer-prod.testnet";
const DEFAULT_RPC: &str = "https://test.rpc.fastnear.com";

struct Driver {
    client: JsonRpcClient,
    key: SecretKey,
}

impl Driver {
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
            _ => anyhow::bail!("unexpected query response for access key"),
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
            _ => anyhow::bail!("unexpected query response for account"),
        }
    }

    async fn view(
        &self,
        contract: &str,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let resp = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::CallFunction {
                    account_id: contract.parse()?,
                    method_name: method.to_string(),
                    args: args.to_string().into_bytes().into(),
                },
            })
            .await?;
        match resp.kind {
            QueryResponseKind::CallResult(r) => Ok(serde_json::from_slice(&r.result)?),
            _ => anyhow::bail!("unexpected query response for call"),
        }
    }

    async fn send_as(
        &self,
        signer_id: &AccountId,
        receiver: &AccountId,
        actions: Vec<Action>,
    ) -> Result<serde_json::Value> {
        let public_key = self.key.public_key();
        let (nonce, block_hash) = self.key_state(signer_id, &public_key).await?;
        let signer: near_crypto::Signer =
            InMemorySigner::from_secret_key(signer_id.clone(), self.key.clone());
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

    async fn broadcast(&self, signed: SignedTransaction) -> Result<serde_json::Value> {
        let sender = signed.transaction.signer_id().clone();
        let hash = self
            .client
            .call(methods::broadcast_tx_async::RpcBroadcastTxAsyncRequest {
                signed_transaction: signed,
            })
            .await?;
        println!("tx {hash} from {sender}");
        for _ in 0..90 {
            tokio::time::sleep(Duration::from_secs(2)).await;
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
                FinalExecutionOutcomeViewEnum::FinalExecutionOutcomeWithReceipt(o) => {
                    o.final_outcome
                }
            };
            match outcome.status {
                FinalExecutionStatus::SuccessValue(value) => {
                    return Ok(if value.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::from_slice(&value).unwrap_or(serde_json::Value::Null)
                    });
                }
                FinalExecutionStatus::Failure(err) => anyhow::bail!("tx {hash} failed: {err:?}"),
                _ => continue,
            }
        }
        anyhow::bail!("tx {hash} did not finalize in time")
    }
}

fn function_call(method: &str, args: serde_json::Value, tgas: u64, deposit: Balance) -> Action {
    Action::FunctionCall(Box::new(FunctionCallAction {
        method_name: method.to_string(),
        args: args.to_string().into_bytes(),
        gas: Gas::from_teragas(tgas),
        deposit,
    }))
}

fn full_access(public_key: near_crypto::PublicKey) -> Action {
    Action::AddKey(Box::new(AddKeyAction {
        public_key,
        access_key: AccessKey {
            nonce: 0,
            permission: AccessKeyPermission::FullAccess,
        },
    }))
}

fn renter_pubkey(key: &SigningKey) -> String {
    format!(
        "ed25519:{}",
        bs58::encode(key.verifying_key().as_bytes()).into_string()
    )
}

#[tokio::test]
#[ignore]
async fn testnet_mpc_fullaccess_e2e() -> Result<()> {
    let root: AccountId = std::env::var("HOSTLA_ACCOUNT_ID")?.parse()?;
    let key = SecretKey::from_str(&std::env::var("HOSTLA_SECRET_KEY")?)?;
    let rpc = std::env::var("TESTNET_RPC").unwrap_or_else(|_| DEFAULT_RPC.to_string());
    let d = Driver {
        client: JsonRpcClient::connect(&rpc),
        key,
    };

    if let Ok(orphans) = std::env::var("E2E_DELETE_ORPHANS") {
        for name in orphans.split(',').filter(|s| !s.is_empty()) {
            let orphan: AccountId = name.parse()?;
            d.send_as(
                &orphan,
                &orphan,
                vec![Action::DeleteAccount(DeleteAccountAction {
                    beneficiary_id: root.clone(),
                })],
            )
            .await?;
            println!("deleted orphan {orphan}");
        }
    }

    let status = d.client.call(methods::status::RpcStatusRequest).await?;
    let tag = status.sync_info.latest_block_height % 1_000_000;
    let signer_acct: AccountId = format!("as{tag}.{root}").parse()?;
    let wallet_acct: AccountId = format!("w{tag}.{root}").parse()?;
    println!("active-signer: {signer_acct}");
    println!("wallet:        {wallet_acct}");

    let root_pk = d.key.public_key();
    d.send_as(
        &root,
        &signer_acct,
        vec![
            Action::CreateAccount(CreateAccountAction {}),
            Action::Transfer(TransferAction {
                deposit: Balance::from_millinear(3500),
            }),
            full_access(root_pk.clone()),
        ],
    )
    .await?;

    let wasm = std::fs::read(ACTIVE_SIGNER_WASM)?;
    d.send_as(
        &signer_acct,
        &signer_acct,
        vec![
            Action::DeployContract(DeployContractAction { code: wasm }),
            function_call(
                "new",
                json!({
                    "admin": root,
                    "marketplace_authority": root,
                    "recovery_authority": root,
                    "mpc_signer": V1_SIGNER,
                    "timeout_secs": 3600,
                }),
                30,
                Balance::from_yoctonear(0),
            ),
        ],
    )
    .await?;
    d.send_as(
        &root,
        &signer_acct,
        vec![function_call(
            "add_minter",
            json!({ "minter": root }),
            10,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;

    let derived = d
        .view(
            V1_SIGNER,
            "derived_public_key",
            json!({
                "path": format!("hos-tla/{wallet_acct}"),
                "predecessor": signer_acct,
                "domain_id": 1,
            }),
        )
        .await?;
    let derived = derived.as_str().unwrap().to_string();
    println!("derived key:   {derived}");
    let derived_pk = near_crypto::PublicKey::from_str(&derived)?;

    d.send_as(
        &root,
        &wallet_acct,
        vec![
            Action::CreateAccount(CreateAccountAction {}),
            Action::Transfer(TransferAction {
                deposit: Balance::from_millinear(300),
            }),
            full_access(derived_pk.clone()),
        ],
    )
    .await?;

    let renter = SigningKey::from_bytes(&[7u8; 32]);
    d.send_as(
        &root,
        &signer_acct,
        vec![function_call(
            "install_signer",
            json!({
                "wallet": wallet_acct,
                "public_key": renter_pubkey(&renter),
                "mpc_public_key": derived,
            }),
            10,
            Balance::from_yoctonear(0),
        )],
    )
    .await?;

    let msg = TxMessage {
        chain_id: CHAIN_ID.to_string(),
        signer_id: near_sdk::AccountId::from_str(wallet_acct.as_str()).unwrap(),
        nonce: 1,
        created_at_secs: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)?
            .as_secs() as u32
            - 60,
        timeout_secs: 3600,
        receiver_id: near_sdk::AccountId::from_str(root.as_str()).unwrap(),
        actions: vec![TxAction::Transfer {
            deposit: near_sdk::NearToken::from_millinear(100),
        }],
    };
    let serialized = near_sdk::borsh::to_vec(&msg)?;
    let hash = Sha256::digest([TX_DOMAIN, serialized.as_slice()].concat());
    let proof =
        Ed25519Signature(DalekSigner::sign(&renter, hash.as_slice()).to_bytes()).to_string();

    let (wallet_nonce, wallet_block_hash) = d.key_state(&wallet_acct, &derived_pk).await?;
    let root_before = d.balance(&root).await?;
    let wallet_before = d.balance(&wallet_acct).await?;

    let signed = d
        .send_as(
            &root,
            &signer_acct,
            vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: "submit_signed_tx".to_string(),
                args: json!({
                    "wallet": wallet_acct,
                    "msg": msg,
                    "proof": proof,
                    "tx_nonce": (wallet_nonce + 1).to_string(),
                    "block_hash": bs58::encode(wallet_block_hash.0).into_string(),
                })
                .to_string()
                .into_bytes(),
                gas: Gas::from_teragas(150),
                deposit: Balance::from_yoctonear(1),
            }))],
        )
        .await?;
    assert!(
        !signed.is_null(),
        "active-signer must return the MPC-signed payload"
    );
    println!("payload hash:  {}", signed["payload_hash"]);

    let unsigned_hex = signed["unsigned_tx_hex"].as_str().unwrap();
    let bytes: Vec<u8> = (0..unsigned_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&unsigned_hex[i..i + 2], 16))
        .collect::<Result<_, _>>()?;
    let tx: TransactionV0 = near_sdk::borsh::from_slice(&bytes)?;
    let sig_bytes: Vec<u8> = signed["mpc_signature"]["signature"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u8)
        .collect();
    let signature = near_crypto::Signature::from_parts(near_crypto::KeyType::ED25519, &sig_bytes)?;
    d.broadcast(SignedTransaction::new(signature, Transaction::V0(tx)))
        .await?;

    let root_after = d.balance(&root).await?;
    let wallet_after = d.balance(&wallet_acct).await?;
    assert!(
        wallet_before.as_yoctonear() - wallet_after.as_yoctonear()
            >= Balance::from_millinear(100).as_yoctonear(),
        "wallet must have paid the transfer"
    );
    assert!(
        root_after.as_yoctonear() > root_before.as_yoctonear(),
        "treasury must have received the MPC-signed transfer"
    );
    println!(
        "transfer landed: wallet {} -> {}, root {} -> {}",
        wallet_before, wallet_after, root_before, root_after
    );

    d.send_as(
        &signer_acct,
        &signer_acct,
        vec![Action::DeleteAccount(DeleteAccountAction {
            beneficiary_id: root.clone(),
        })],
    )
    .await?;
    println!("cleanup: {signer_acct} deleted, storage refunded to {root}");

    Ok(())
}
