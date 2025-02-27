# Bitcoin UTXO Tracing

A high-performance Bitcoin UTXO indexer and database for tracking and querying UTXO data with blockchain reorganization handling.

## Overview

This repository contains two main services:

1. **indexer**: A Bitcoin UTXO indexer that monitors the blockchain and sends updates via Unix socket
2. **storage**: A UTXO database that stores and provides query endpoints for UTXO data
3. **network-shared**: Shared code and data models used by both services

These services use a high-performance communication method via Unix sockets with binary serialization (bincode) to minimize latency.

## Architecture

### indexer
- Connects to a Bitcoin node via RPC
- Processes blocks and tracks UTXO creation and spending
- Sends updates to the UTXO database via Unix socket
- Optimized for fast processing of blockchain data

### storage
- Stores UTXO data in SQLite or CSV (configurable)
- Provides REST API for querying UTXOs
- Receives updates via Unix socket
- Supports querying UTXOs at specific block heights

### network-shared
- Shared data models and serialization code
- Common utilities and error handling

## API Endpoints

The UTXO database service provides these HTTP endpoints:

- `GET /latest-block`: Get the latest processed block height
- `GET /block/{height}/txids`: Get transaction IDs in a specific block
- `GET /utxos/block/{height}/address/{address}`: Get UTXOs for an address at a specific block height
- `GET /spendable-utxos/block/{height}/address/{address}`: Get spendable UTXOs for an address at a specific block height
- `GET /select-utxos/block/{height}/address/{address}/amount/{amount}`: Select UTXOs for a specified amount

## Installation

### Using Docker Compose

```bash
# Clone the repository
git clone https://github.com/SovaNetwork/utxo-tracing.git
cd utxo-tracing

# Start the services (includes BTC regtest node)
docker-compose up -d
```

## Configuration

### network-indexer

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| RUST_LOG | Log level | info |
| RPC_HOST | Bitcoin RPC host | bitcoin |
| RPC_PORT | Bitcoin RPC port | 18443 |
| RPC_USER | Bitcoin RPC username | user |
| RPC_PASSWORD | Bitcoin RPC password | password |
| SOCKET_PATH | Path to Unix socket | /tmp/network-utxos.sock |
| START_HEIGHT | Block height to start from | 0 |
| POLLING_RATE | Polling interval in milliseconds | 500 |
| MAX_BLOCKS_PER_BATCH | Maximum blocks to process in a batch | 200 |

### network-utxos

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| RUST_LOG | Log level | info |
| HOST | Host to bind HTTP server | 0.0.0.0 |
| PORT | HTTP server port | 5557 |
| LOG_LEVEL | Log level | info |
| DATASOURCE | Storage backend (csv/sqlite) | sqlite |
| SOCKET_PATH | Path to Unix socket | /tmp/network-utxos.sock |

## Reorg Mechanics

### Blockchain Reorganization Handling

The system is designed to handle blockchain reorganizations (reorgs) through several key mechanisms:

1. **Block Height Tracking**
   - Each UTXO is associated with its creation block height
   - Spending information includes the block where the UTXO was spent
   - This allows for precise tracking of UTXO state at any given block height

2. **Flexible State Management**
    - State transitions are tracked with block-level precision
    - UTXOs can be in 3 potential states:
        #### 1. Created
        - This is the initial state when a new UTXO is generated
        - Occurs when a transaction creates a new output that can potentially be spent
        - Recorded with:
                - Creation transaction ID
                - Output index (vout)
                - Amount
                - Receiving address
                - Block height where it was created
        - At this point, the UTXO is available but not yet marked as spendable

        #### 2. Unspent
        - The primary state for usable funds
        - Indicates the UTXO has not been used as an input in any subsequent transaction
        - Can be selected for future transactions
        - Tracked with all the creation details, plus its current spendable status
        - In the database, this means the `spent_txid`, `spent_at`, and `spent_block` fields are `NULL`/`None`

        #### 3. Spent
        - Occurs when the UTXO is used as an input in another transaction
        - Marked with:
            - The transaction ID that spent it
            - Timestamp of spending
            - Block height where it was spent
        - No longer available for future transactions
        - Maintains historical record of its entire lifecycle

3. **Idempotent Update Mechanism**
   - The `process_block_utxos` method in the database ensures that:
     - New UTXOs can be inserted
     - Existing UTXOs can be updated
     - Spending information can be modified without losing historical data

4. **Rollback Capabilities**
   - Database implementations support atomic updates
   - Transactions are used to ensure data consistency during block processing

### Example Reorg Scenario

Consider a blockchain reorganization:
```
Block 100 (original chain):
- TX1 creates UTXO-A at address X
- TX2 spends UTXO-A

Block 100 (reorganized chain):
- Different transactions
- UTXO-A remains unspent
```

In this scenario, the system will:
- Rollback the spending of UTXO-A
- Restore UTXO-A to an unspent state
- Update block-level metadata accordingly

## Performance Considerations

- Binary serialization (bincode) for low-latency communication
- Configurable batch processing
- Supports multiple storage backends
- Optimized for high-throughput UTXO tracking

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in these crates by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
