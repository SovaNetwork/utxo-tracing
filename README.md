# &#8383;itcoin UTXO Tracing :orange_book:

A high-performance Bitcoin UTXO indexer and database for tracking and querying UTXO data with blockchain reorganization handling.

This service is used by [Sova validators](https://github.com/SovaNetwork/sova-reth) to get spendable utxos for network signing and other read operations.

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

*Note: querying data for blocks less than 6 blocks behind the chain tip are subject to change based on the reorg mechanics described below.*

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


## Blockchain Reorganization Handling

The system is designed to handle blockchain reorganizations (reorgs) for up to 6 blocks behind the current block. 

## Performance Considerations

- Binary serialization (bincode) for low-latency communication
- Configurable batch processing
- Supports multiple storage backends
- Optimized for high-throughput UTXO tracking

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in these crates by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
