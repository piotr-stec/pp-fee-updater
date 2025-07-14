use starknet::{
    accounts::{Account, ExecutionEncoding, SingleOwnerAccount},
    core::{
        types::{BlockId, BlockTag, Call, Felt, FunctionCall},
        utils::get_selector_from_name,
    },
    providers::{jsonrpc::HttpTransport, JsonRpcClient, Provider, Url},
    signers::{LocalWallet, SigningKey},
};
use thiserror::Error;
use tracing::{debug, error, info, warn};


#[derive(Error, Debug)]
pub enum UpdaterError {
    #[error("Starknet provider error: {0}")]
    Provider(#[from] starknet::providers::ProviderError),
    #[error("Account error: {0}")]
    Account(String),
    #[error("Conversion error: {0}")]
    Conversion(String),
    #[error("Invalid gas price: {0}")]
    InvalidGasPrice(String),
    #[error("Transaction failed or reverted")]
    TransactionFailed,
}

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
    upward_threshold_const: u128,
    downward_threshold_const: u128,
    upward_buffer_const: u128,
    downward_buffer_const: u128,
) -> Result<(bool, Felt), UpdaterError> {
    let provider = JsonRpcClient::new(HttpTransport::new(url));

    // If there's a pending update, first check if it was confirmed or failed
    if let Some(pending) = *pending_update {
        info!("⏳ Checking status of pending transaction: {:?}", pending.tx_hash);

        match check_transaction_status(
            &provider,
            pending.tx_hash,
            contract_address,
            pending.gas_price,
        )
        .await
        {
            Ok(TransactionStatus::Confirmed) => {
                info!("✅ Pending transaction confirmed on contract");
                *pending_update = None;
                // Continue with normal check below
            }
            Ok(TransactionStatus::Failed) => {
                warn!("❌ Pending transaction failed, clearing pending state");
                *pending_update = None;
                // Continue with normal check below
            }
            Ok(TransactionStatus::Pending) => {
                debug!("⏳ Transaction still pending, skipping check");
                return Ok((false, Felt::ZERO));
            }
            Err(e) => {
                error!("❌ Error checking transaction status: {:?}", e);
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
            return Err(UpdaterError::InvalidGasPrice(
                "Cannot get gas price from pending block".to_string(),
            ));
        }
    };

    info!("Current gas price (in fri): {}", current_gas_price);

    let gas_price_on_contract = provider
        .call(
            FunctionCall {
                calldata: vec![],
                contract_address,
                entry_point_selector: get_selector_from_name("get_current_gas_price")
                    .map_err(|e| UpdaterError::Conversion(format!("Invalid selector: {}", e)))?,
            },
            BlockId::Tag(BlockTag::Latest),
        )
        .await?[0];

    info!("Gas price on contract: {}", gas_price_on_contract);

    // Check if current gas price differs by more than 20% from contract gas price
    // Convert Felt to u128 for calculation (Fri values should fit in u128)
    let contract_price_u128: u128 =
        gas_price_on_contract.to_biguint().try_into().map_err(|_| {
            UpdaterError::Conversion("Contract gas price too large for u128".to_string())
        })?;
    let current_price_u128: u128 = current_gas_price.to_biguint().try_into().map_err(|_| {
        UpdaterError::Conversion("Current gas price too large for u128".to_string())
    })?;

    // Asymmetric paymaster thresholds for profit optimization
    let upward_threshold = contract_price_u128 * upward_threshold_const / 100;   // +5% threshold
    let downward_threshold = contract_price_u128 * downward_threshold_const / 100; // -15% threshold

    // Determine update type and direction
    let (should_update, update_direction) = if current_price_u128 > upward_threshold {
        (true, "upward") // Gas price rising - quick reaction for profits
    } else if current_price_u128 < downward_threshold {
        (true, "downward") // Gas price falling - slow reaction to preserve margins
    } else {
        (false, "none") // Within acceptable range
    };

    debug!(
        "Paymaster gas price analysis - Network: {}, Contract: {}",
        current_price_u128, contract_price_u128
    );
    debug!(
        "Thresholds - Upward (+5%): {}, Downward (-15%): {}",
        upward_threshold, downward_threshold
    );
    info!(
        "Fee update required: {} (direction: {}, network vs contract: {}%)", 
        should_update, 
        update_direction,
        if contract_price_u128 > 0 {
            (current_price_u128 as i128 - contract_price_u128 as i128) * 100 / contract_price_u128 as i128
        } else { 0 }
    );

    let new_gas_price = if should_update {
        let (buffered_price, margin_percent) = match update_direction {
            "upward" => {
                // Gas rising: Set higher price with 10% margin for consistent profit
                let price = current_price_u128 * upward_buffer_const / 100;
                (price, 10)
            },
            "downward" => {
                // Gas falling: Set lower price with 10% margin to preserve profits
                let price = current_price_u128 * downward_buffer_const / 100;
                (price, 10)
            },
            _ => (current_price_u128, 0) // Fallback, shouldn't happen
        };
        
        let paymaster_profit = buffered_price.saturating_sub(current_price_u128);
        info!(
            "New gas price for users: {} (network: {} + {}% margin = {} profit per tx)", 
            buffered_price, 
            current_price_u128, 
            margin_percent,
            paymaster_profit
        );
        Felt::from(buffered_price)
    } else {
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
) -> Result<(), UpdaterError> {
    let provider = JsonRpcClient::new(HttpTransport::new(url));

    let paymaster_account = SingleOwnerAccount::new(
        provider.clone(),
        LocalWallet::from(SigningKey::from_secret_scalar(owner_private_key)),
        owner_address,
        provider.chain_id().await?,
        ExecutionEncoding::New,
    );

    let selector = get_selector_from_name("set_current_gas_price")
        .map_err(|e| UpdaterError::Conversion(format!("Invalid selector: {}", e)))?;

    let call = Call {
        to: contract_address,
        selector,
        calldata: [gas_price, Felt::ZERO].to_vec(),
    };

    let invoke_result = paymaster_account.execute_v3(vec![call]).send().await;

    match &invoke_result {
        Ok(result) => {
            info!("✅ Transaction sent: {:?}", result.transaction_hash);
            info!("⏳ Will check transaction status on next block");

            // Set pending update with transaction hash
            *pending_update = Some(PendingUpdate {
                gas_price,
                tx_hash: result.transaction_hash,
            });
        }
        Err(e) => {
            error!("❌ Error sending transaction: {:?}", e);
            *pending_update = None;
            return Err(UpdaterError::Account(format!("{:?}", e)));
        }
    }

    // Result already handled above
    Ok(())
}

// Function to check transaction status
async fn check_transaction_status(
    provider: &JsonRpcClient<HttpTransport>,
    tx_hash: Felt,
    contract_address: Felt,
    expected_gas_price: Felt,
) -> Result<TransactionStatus, UpdaterError> {
    // First try to get transaction receipt
    match provider.get_transaction_receipt(tx_hash).await {
        Ok(_receipt) => {
            // If we got a receipt, the transaction was included in a block
            // Now check if contract was actually updated with expected value
            debug!("Transaction receipt found, checking if contract was updated with expected value: {}", expected_gas_price);
            match check_if_update_completed(provider, contract_address, expected_gas_price).await {
                Ok(true) => {
                    info!("✅ Transaction confirmed - contract updated successfully");
                    Ok(TransactionStatus::Confirmed)
                },
                Ok(false) => {
                    // Transaction was included but contract value doesn't match
                    // Let's see what the actual value is
                    let actual_value = provider
                        .call(
                            FunctionCall {
                                calldata: vec![],
                                contract_address,
                                entry_point_selector: get_selector_from_name(
                                    "get_current_gas_price",
                                )
                                .map_err(|e| {
                                    UpdaterError::Conversion(format!("Invalid selector: {}", e))
                                })?,
                            },
                            BlockId::Tag(BlockTag::Latest),
                        )
                        .await
                        .map(|result| result[0])
                        .unwrap_or(Felt::ZERO);

                    warn!("⚠️ Transaction included but contract value doesn't match expected");
                    warn!("   Expected: {}, Actual: {}", expected_gas_price, actual_value);
                    warn!("   Expected_hex: {:x}, Actual_hex: {:x}", expected_gas_price, actual_value);
                    
                    // Check if values are actually the same (debug false positive)
                    if actual_value == expected_gas_price {
                        error!("🐛 BUG: Values are identical but check_if_update_completed returned false!");
                        return Ok(TransactionStatus::Confirmed); // Force success
                    }
                    
                    Ok(TransactionStatus::Failed)
                }
                Err(e) => {
                    error!("❌ Error checking contract value: {:?}", e);
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
) -> Result<bool, UpdaterError> {
    let current_contract_price = provider
        .call(
            FunctionCall {
                calldata: vec![],
                contract_address,
                entry_point_selector: get_selector_from_name("get_current_gas_price")
                    .map_err(|e| UpdaterError::Conversion(format!("Invalid selector: {}", e)))?,
            },
            BlockId::Tag(BlockTag::Latest),
        )
        .await?[0];

    let is_match = current_contract_price == expected_gas_price;
    debug!(
        "Update completion check - Contract: {}, Expected: {}, Match: {}, Contract_hex: {:x}, Expected_hex: {:x}",
        current_contract_price, expected_gas_price, is_match, current_contract_price, expected_gas_price
    );
    
    Ok(is_match)
}
