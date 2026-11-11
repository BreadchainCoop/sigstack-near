mod helpers;

use helpers::*;
use near_api::Data;
use serde_json::{Value, json};
use sigstack_agent::ContractInfo;
use tokio::time::{Duration, sleep};

/// Upgrades the contract to this build's wasm and checks `migrate` keeps the state.
#[tokio::test]
async fn test_update_contract() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let sandbox = near_sandbox::Sandbox::start_sandbox().await?;
    let network_config = create_network_config(&sandbox);
    let (genesis_account_id, genesis_signer) = setup_genesis_account().await;

    let contract_id =
        deploy_contract_default(&network_config, &genesis_account_id, &genesis_signer).await?;

    sleep(Duration::from_millis(200)).await;

    let wasm_bytes = std::fs::read(CONTRACT_WASM_PATH)?;

    // First 8 bytes: migrate gas in TGas as u64 little-endian, then the wasm code
    let gas_value: u64 = 10;
    let mut input = gas_value.to_le_bytes().to_vec();
    input.extend_from_slice(&wasm_bytes);

    call_transaction_raw(
        &contract_id,
        "update_contract",
        input,
        &genesis_account_id,
        &genesis_signer,
        &network_config,
    )
    .await?
    .into_result()
    .expect("update_contract should succeed");

    sleep(Duration::from_millis(200)).await;

    let contract_info: Data<ContractInfo> = call_view(
        &contract_id,
        "get_contract_info",
        json!({}),
        &network_config,
    )
    .await?;
    assert_eq!(contract_info.data.owner_id, genesis_account_id);
    assert!(!contract_info.data.requires_tee);
    assert_eq!(contract_info.data.attestation_expiration_time_ms.0, 100000);

    let measurements: Data<Vec<Value>> = call_view(
        &contract_id,
        "get_approved_measurements",
        json!({}),
        &network_config,
    )
    .await?;
    assert_eq!(
        measurements.data.len(),
        1,
        "Approved measurements should survive the upgrade"
    );

    Ok(())
}
