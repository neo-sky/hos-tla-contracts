use anyhow::Result;
use near_workspaces::types::NearToken;
use serde_json::json;

const ACTIVE_SIGNER_WASM: &str = "../target/near/active_signer/active_signer.wasm";

#[tokio::test]
async fn sandbox_boots_and_runs_a_contract() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;
    let account = root
        .create_subaccount("activesigner")
        .initial_balance(NearToken::from_near(30))
        .transact()
        .await?
        .into_result()?;

    let wasm = std::fs::read(ACTIVE_SIGNER_WASM)?;
    let contract = account.deploy(&wasm).await?.into_result()?;

    contract
        .call("new")
        .args_json(json!({
            "admin": root.id(),
            "marketplace_authority": root.id(),
            "recovery_authority": root.id(),
            "timeout_secs": 3600u32,
        }))
        .transact()
        .await?
        .into_result()?;

    let signer: Option<String> = contract
        .view("signer_of")
        .args_json(json!({ "wallet": "alice.tla.testnet" }))
        .await?
        .json()?;
    assert!(signer.is_none());
    Ok(())
}
