use starknet::{
    accounts::{Account, ExecutionEncoding, SingleOwnerAccount},
    core::{
        types::{BlockId, BlockTag, Call, Felt, FunctionCall},
        utils::get_selector_from_name,
    },
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider, Url},
    signers::{LocalWallet, SigningKey},
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

// Structure to track pending update with transaction hash
#[derive(Debug, Clone, Copy)]
pub struct PendingUpdate {
    pub gas_price: Felt,
    pub tx_hash: Felt,
}

// Enum to represent transaction status
#[derive(Debug)]
enum TransactionStatus {
    Confirmed,
    Failed,
    Pending,
}

pub async fn check_fee_update(
    url: Url,
    contract_address: Felt,
    pending_update: &mut Option<PendingUpdate>,
) -> Result<(bool, Felt), BoxError> {
    let provider = JsonRpcClient::new(HttpTransport::new(url));

    // If there's a pending update, first check if it was confirmed or failed
    if let Some(pending) = *pending_update {
        println!(
            "⏳ Checking status of pending transaction: {:?}",
            pending.tx_hash
        );

        match check_transaction_status(
            &provider,
            pending.tx_hash,
            contract_address,
            pending.gas_price,
        )
        .await
        {
            Ok(TransactionStatus::Confirmed) => {
                println!("✅ Pending transaction confirmed on contract");
                *pending_update = None;
                // Continue with normal check below
            }
            Ok(TransactionStatus::Failed) => {
                println!("❌ Pending transaction failed, clearing pending state");
                *pending_update = None;
                // Continue with normal check below
            }
            Ok(TransactionStatus::Pending) => {
                println!("⏳ Transaction still pending, skipping check");
                return Ok((false, Felt::ZERO));
            }
            Err(e) => {
                println!("❌ Error checking transaction status: {}", e);
                // Clear pending to avoid being stuck forever
                *pending_update = None;
                // Continue with normal check below
            }
        }
    }

    let current_block = provider
        .get_block_with_tx_hashes(BlockId::Tag(BlockTag::Latest))
        .await?;

    // Extract the gas price from l1_gas_price field
    let current_gas_price = match current_block {
        starknet::core::types::MaybePendingBlockWithTxHashes::Block(block) => {
            // Access the l1_gas_price field and extract price_in_fri
            let gas_price = block.l1_gas_price.price_in_fri;
            gas_price
        }
        starknet::core::types::MaybePendingBlockWithTxHashes::PendingBlock(_) => {
            return Err("Cannot get gas price from pending block".into());
        }
    };

    println!("Current gas price (in fri): {}", current_gas_price);

    let gas_price_on_contract = provider
        .call(
            FunctionCall {
                calldata: vec![],
                contract_address,
                entry_point_selector: get_selector_from_name("get_current_gas_price").unwrap(),
            },
            BlockId::Tag(BlockTag::Latest),
        )
        .await?[0];

    println!("Gas price on contract: {}", gas_price_on_contract);

    // Check if current gas price differs by more than 20% from contract gas price
    // Convert Felt to u128 for calculation (Fri values should fit in u128)
    let contract_price_u128: u128 = gas_price_on_contract
        .to_biguint()
        .try_into()
        .map_err(|_| "Contract gas price too large for u128")?;
    let current_price_u128: u128 = current_gas_price
        .to_biguint()
        .try_into()
        .map_err(|_| "Current gas price too large for u128")?;

    // Calculate 20% threshold boundaries
    let upper_threshold = contract_price_u128 * 120 / 100; // +20%
    let lower_threshold = contract_price_u128 * 80 / 100; // -20%

    // Update needed if current price is outside the ±20% range
    let should_update =
        current_price_u128 > upper_threshold || current_price_u128 < lower_threshold;

    println!(
        "Current gas price: {}, Contract gas price: {}",
        current_price_u128, contract_price_u128
    );
    println!(
        "Upper threshold (+20%): {}, Lower threshold (-20%): {}",
        upper_threshold, lower_threshold
    );
    println!("Should update: {} (outside ±20% range)", should_update);

    let new_gas_price = if should_update {
        // If update is needed, return the current gas price
        println!(
            "New gas price to set (120% of current): {}",
            current_price_u128 * 120 / 100
        );
        Felt::from(current_price_u128 * 120 / 100) // Adjusted to 120% of current price
    } else {
        // If no update is needed, return the Felt::ZERO
        Felt::ZERO
    };

    Ok((should_update, new_gas_price))
}

pub async fn update_fee(
    url: Url,
    gas_price: Felt,
    contract_address: Felt,
    owner_address: Felt,
    owner_private_key: Felt,
    pending_update: &mut Option<PendingUpdate>,
) -> Result<(), BoxError> {
    let provider = JsonRpcClient::new(HttpTransport::new(url));

    let paymaster_account = SingleOwnerAccount::new(
        provider.clone(),
        LocalWallet::from(SigningKey::from_secret_scalar(owner_private_key)),
        owner_address,
        provider.chain_id().await?,
        ExecutionEncoding::New,
    );

    let selector = get_selector_from_name("set_current_gas_price")?;

    let call = Call {
        to: contract_address,
        selector,
        calldata: [gas_price, Felt::ZERO].to_vec(),
    };

    let invoke_result = paymaster_account.execute_v3(vec![call]).send().await;

    match &invoke_result {
        Ok(result) => {
            println!("✅ Transaction sent: {:?}", result.transaction_hash);
            println!("⏳ Will check transaction status on next block");

            // Set pending update with transaction hash
            *pending_update = Some(PendingUpdate {
                gas_price,
                tx_hash: result.transaction_hash,
            });
        }
        Err(e) => {
            println!("❌ Error sending transaction: {}", e);
            *pending_update = None;
        }
    }

    invoke_result.map_err(|e| Box::new(e) as BoxError)?;
    Ok(())
}

// Function to check transaction status
async fn check_transaction_status(
    provider: &JsonRpcClient<HttpTransport>,
    tx_hash: Felt,
    contract_address: Felt,
    expected_gas_price: Felt,
) -> Result<TransactionStatus, BoxError> {
    // First try to get transaction receipt
    match provider.get_transaction_receipt(tx_hash).await {
        Ok(_receipt) => {
            // If we got a receipt, the transaction was included in a block
            // Now check if contract was actually updated with expected value
            match check_if_update_completed(provider, contract_address, expected_gas_price).await {
                Ok(true) => Ok(TransactionStatus::Confirmed),
                Ok(false) => {
                    // Transaction was included but contract value doesn't match
                    // Let's see what the actual value is
                    let actual_value = provider
                        .call(
                            FunctionCall {
                                calldata: vec![],
                                contract_address,
                                entry_point_selector: get_selector_from_name("get_current_gas_price").unwrap(),
                            },
                            BlockId::Tag(BlockTag::Latest),
                        )
                        .await
                        .map(|result| result[0])
                        .unwrap_or(Felt::ZERO);
                    
                    println!("⚠️ Transaction included but contract value doesn't match expected");
                    println!("   Expected: {}, Actual: {}", expected_gas_price, actual_value);
                    Ok(TransactionStatus::Failed)
                }
                Err(e) => {
                    println!("❌ Error checking contract value: {}", e);
                    Ok(TransactionStatus::Failed)
                }
            }
        }
        Err(_) => {
            // Transaction receipt not found, assume it's still pending
            Ok(TransactionStatus::Pending)
        }
    }
}

// Helper function to check if update was confirmed
async fn check_if_update_completed(
    provider: &JsonRpcClient<HttpTransport>,
    contract_address: Felt,
    expected_gas_price: Felt,
) -> Result<bool, BoxError> {
    let current_contract_price = provider
        .call(
            FunctionCall {
                calldata: vec![],
                contract_address,
                entry_point_selector: get_selector_from_name("get_current_gas_price").unwrap(),
            },
            BlockId::Tag(BlockTag::Latest),
        )
        .await?[0];

    Ok(current_contract_price == expected_gas_price)
}
