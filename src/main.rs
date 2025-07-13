use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use starknet_types_core::felt::Felt;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;

use crate::updater::{check_fee_update, update_fee, PendingUpdate};

pub mod updater;

#[derive(Parser, Debug)]
#[command(name = "pp-fee-updater")]
#[command(about = "A Starknet WebSocket block listener")]
struct Args {
    #[arg(long, short = 'w', env = "WS_URL")]
    websocket_url: Url,
    #[arg(long, short = 'u', env = "API_URL")]
    api_url: Url,
    #[arg(long, short = 'c', env = "PP_ADDRESS")]
    privacy_pool_address: Felt,
    #[arg(long, short = 'o', env = "OWNER_ADDRESS")]
    owner_address: Felt,
    #[arg(long, short = 'p', env = "OWNER_PRIVATE_KEY")]
    owner_private_key: Felt,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let ws_starknet_url = &args.websocket_url;
    let starknet_url = &args.api_url;
    let privacy_pool_address = args.privacy_pool_address;
    let owner_address = args.owner_address;
    let owner_private_key = args.owner_private_key;

    let mut pending_fee_update: Option<PendingUpdate> = None;

    println!("Connecting to Starknet at: {}", ws_starknet_url);

    let (ws_stream, _) = connect_async(ws_starknet_url).await?;
    println!("Connected to Starknet!");

    let (mut write, mut read) = ws_stream.split();

    // Subscribe to new blocks
    let subscribe_msg = json!({
        "jsonrpc": "2.0",
        "method": "starknet_subscribeNewHeads",
        "params": [],
        "id": 1
    });

    println!("Subscribing to new blocks...");
    write.send(Message::Text(subscribe_msg.to_string())).await?;

    // Listen for new blocks
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                // Parse JSON response
                if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(method) = json_value.get("method") {
                        if method == "starknet_subscriptionNewHeads" {
                            if let Some(params) = json_value.get("params") {
                                println!("🔔 New Starknet block received:");
                                if let Some(result) = params.get("result") {
                                    if let Some(block_number) = result.get("block_number") {
                                        println!("🔥 New Starknet block: {}", block_number);
                                    }
                                    if let Some(block_hash) = result.get("block_hash") {
                                        println!("   Block hash: {}", block_hash);
                                    }
                                }
                                let check_fee =
                                    check_fee_update(starknet_url.clone(), privacy_pool_address, &mut pending_fee_update)
                                        .await
                                        .unwrap_or_else(|e| {
                                            println!("Error checking fee update: {}", e);
                                            (false, Felt::ZERO)
                                        });

                                if check_fee.0 {
                                    println!(
                                        "⚠️ Fee update needed! Current gas price: {}",
                                        check_fee.1
                                    );
                                    update_fee(
                                        starknet_url.clone(),
                                        check_fee.1,
                                        privacy_pool_address,
                                        owner_address,
                                        owner_private_key,
                                        &mut pending_fee_update,
                                    )
                                    .await
                                    .unwrap_or_else(|e| {
                                        println!("Error updating fee: {}", e);
                                    });
                                } else {
                                    println!("✅ No fee update needed.");
                                }
                            }
                        }
                    } else if json_value.get("result").is_some() {
                        println!("✅ Subscription confirmed");
                    } else if let Some(error) = json_value.get("error") {
                        println!("❌ Error: {}", error);
                    }
                }
            }
            Ok(Message::Close(_)) => {
                println!("Connection closed by server");
                break;
            }
            Ok(Message::Ping(data)) => {
                write.send(Message::Pong(data)).await?;
            }
            Ok(_) => {}
            Err(e) => {
                println!("WebSocket error: {}", e);
                break;
            }
        }
    }

    println!("Connection closed");
    Ok(())
}
