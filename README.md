# Privacy Pool Fee Updater

A Starknet WebSocket-based service that monitors network gas prices and automatically updates paymaster contract fees to maintain optimal profit margins.

## Overview

This service connects to Starknet WebSocket endpoints to receive real-time block notifications and automatically adjusts paymaster gas prices based on network conditions to maximize profitability while maintaining competitive rates for users.

## Configuration

### Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `WS_URL` | Starknet WebSocket URL | Yes |
| `API_URL` | Starknet RPC API URL | Yes |
| `PP_ADDRESS` | Privacy Pool contract address | Yes |
| `OWNER_ADDRESS` | Contract owner address | Yes |
| `OWNER_PRIVATE_KEY` | Private key for transactions | Yes |

### Command Line Arguments

```bash
# Using environment variables
export WS_URL="wss://starknet-mainnet.g.alchemy.com/starknet/version/rpc/v0_8/YOUR_KEY"
export API_URL="https://starknet-mainnet.g.alchemy.com/starknet/version/rpc/v0_8/YOUR_KEY"
export PP_ADDRESS="0x123..."
export OWNER_ADDRESS="0x456..."
export OWNER_PRIVATE_KEY="0x789..."
cargo run

# Using command line arguments
cargo run -- --websocket-url "wss://..." --api-url "https://..." --privacy-pool-address "0x123..." --owner-address "0x456..." --owner-private-key "0x789..."
```

## Fee Update Logic

### Asymmetric Thresholds

The service uses different thresholds for upward and downward gas price movements to optimize paymaster profits:

| Direction | Threshold | Margin | Rationale |
|-----------|-----------|---------|-----------|
| **Upward** | +5% | +10% | Quick reaction to capture maximum profits |
| **Downward** | -15% | +10% | Slow reaction to preserve margins |

### Update Conditions

**Gas Price Rising (Profit Opportunity):**
- **Trigger:** Network gas > Contract gas × 105%
- **Action:** Set contract price = Network gas × 110%
- **Example:** Network: 100 → Contract: 95 → Update to 110 (+10% margin)

**Gas Price Falling (Preserve Margins):**
- **Trigger:** Network gas < Contract gas × 85%  
- **Action:** Set contract price = Network gas × 110%
- **Example:** Network: 80 → Contract: 100 → Update to 88 (+10% margin)

**No Update Zone:**
- **Range:** Contract gas × 85% ≤ Network gas ≤ Contract gas × 105%
- **Action:** No update needed
- **Purpose:** Avoid frequent updates for minor fluctuations

### Example Scenarios

**Scenario 1: Gas Price Surge**
```
Network Gas: 1000 → 1100 (+10%)
Contract Gas: 1000
Threshold Check: 1100 > 1000 × 105% = 1050 ✅ UPDATE
New Contract Price: 1100 × 110% = 1210
Paymaster Profit: 1210 - 1100 = 110 per transaction
```

**Scenario 2: Gas Price Drop**
```
Network Gas: 1000 → 800 (-20%)
Contract Gas: 1000  
Threshold Check: 800 < 1000 × 85% = 850 ✅ UPDATE
New Contract Price: 800 × 110% = 880
Paymaster Profit: 880 - 800 = 80 per transaction
```

**Scenario 3: Minor Fluctuation**
```
Network Gas: 1000 → 950 (-5%)
Contract Gas: 1000
Threshold Check: 950 > 1000 × 85% = 850 ❌ NO UPDATE
Reason: Within acceptable range, avoids unnecessary transactions
```

## Logging

The service uses structured logging with different levels:

```bash
# Basic logging (info, warn, error)
cargo run

# Debug logging (all messages)
RUST_LOG=debug cargo run

# Module-specific logging
RUST_LOG=pp_fee_updater::updater=debug cargo run
```

## Transaction Management

- **Pending State Tracking:** Monitors transaction confirmations
- **Auto-Retry:** Clears failed transactions and retries on next block
- **Gas Optimization:** Only updates when economically beneficial
- **Error Handling:** Robust error recovery and logging

## Security Features

- **Input Validation:** Validates all gas prices and contract addresses
- **Private Key Protection:** Environment variable based key management
- **Error Boundaries:** Graceful handling of network and contract errors
- **Audit Trail:** Comprehensive logging of all price updates and decisions

## Building and Running

```bash
# Build
cargo build --release

# Run with logging
RUST_LOG=info cargo run

# Help
cargo run -- --help
```

## Dependencies

- **tokio-tungstenite:** WebSocket client with TLS support
- **starknet:** Starknet protocol integration  
- **tracing:** Structured logging
- **clap:** Command line argument parsing
- **serde_json:** JSON handling for RPC communication
