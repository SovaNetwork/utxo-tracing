# &#8383;itcoin UTXO Tracing :orange_book:

A high-performance Bitcoin UTXO indexer and database for tracking and querying UTXO data with blockchain reorganization handling.

This service is used by [Sova validators](https://github.com/SovaNetwork/sova-reth) to obtain spendable UTXOs for network signing and other read operations.

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
- Hosts an HTTP API for validators
- Proxies transaction signing requests to the configured signing service
- Maintains a vector of watched Bitcoin addresses in memory used to filter incoming blocks

### storage
- Stores UTXO data in SQLite or CSV (configurable)
- Provides REST API for querying UTXOs
- Receives updates via Unix socket
- Supports querying UTXOs at specific block heights

### network-shared
- Shared data models and serialization code
- Common utilities and error handling

## API Endpoints

### UTXO storage service

The storage service provides the following HTTP endpoints:

- `GET /latest-block`: Get the latest processed block height
- `GET /block/{height}/txids`: Get transaction IDs in a specific block
- `GET /utxos/block/{height}/address/{address}`: Get UTXOs for an address at a specific block height
- `GET /spendable-utxos/block/{height}/address/{address}`: Get spendable UTXOs for an address at a specific block height
- `GET /select-utxos/block/{height}/address/{address}/amount/{amount}`: Select UTXOs for a specified amount
- `GET /signed-tx`: List stored signed transactions (optional `?page=N` for 250-record pages)
- `GET /signed-tx/{txid}`: Retrieve a signed transaction by txid
- `GET /signed-tx/caller/{caller}`: Retrieve signed transactions for a caller
- `GET /signed-tx/block/{height}`: Retrieve signed transactions requested at a block height
- `GET /signed-tx/destination/{destination}`: Retrieve signed transactions for a destination address

### Indexer service

The indexer exposes an HTTP API used by validators:

- `POST /watch-address` - add a Bitcoin address to the watch list (requires `X-API-Key` header)
- `GET /watch-address/{address}` - check if an address is watched
- `POST /derive-address` - derive a Bitcoin address from an EVM address (forwarded to the signing service, requires `X-API-Key` header)
- `POST /select-utxos` - select UTXOs across watched addresses for a target amount (requires `X-API-Key` header)
- `POST /prepare-transaction` - build an unsigned transaction skeleton (requires `X-API-Key` header)
- `POST /sign-transaction` - proxy a signing request to the configured signing service (requires `X-API-Key` header)

*Note: querying data for blocks less than 6 blocks behind the chain tip are subject to change based on the reorg mechanics described below.*

All JSON API endpoints enforce a maximum payload size of **32&nbsp;KiB**. Requests exceeding this limit will be rejected with a `413` status code.

### Indexer Address Filtering

The indexer keeps a vector of watched Bitcoin addresses in memory. Every new
block is scanned and only transactions touching these addresses are forwarded
to the UTXO database. It is infeasible to pre-seed filters with all possible
Ethereum addresses because there are 2^160 potential combinations. Instead, the
`derive-address` endpoint automatically inserts the newly converted Bitcoin
address into the in-memory list so it begins receiving UTXO updates
immediately.

## Installation

### Using Docker Compose

> NOTE: be sure to create your own .env file prior to running Docker here.

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
| RPC_HOST | Bitcoin RPC host url | http://bitcoin-regtest:18443 |
| RPC_USER | Bitcoin RPC username | user |
| RPC_PASSWORD | Bitcoin RPC password | password |
| RPC_CONNECTION_TYPE | RPC connection type (bitcoincore, external) | bitcoincore |
| NETWORK | Bitcoin network (mainnet, testnet, regtest, signet) | regtest |
| SOCKET_PATH | Path to Unix socket | /tmp/network-utxos.sock |
| START_HEIGHT | Block height to start from | 0 |
| POLLING_RATE | Polling interval in milliseconds | 500 |
| MAX_BLOCKS_PER_BATCH | Maximum blocks to process in a batch | 200 |
| API_HOST | API server host | 0.0.0.0 |
| API_PORT | API server port | 3031 |
| ENCLAVE_URL | URL of the signing service (enclave) | - |
| UTXO_URL | URL of the UTXO database service | http://network-utxos:5557 |
| ENCLAVE_API_KEY | API key sent when proxying signing requests | - |
| INDEXER_API_KEY | API key required for modifying endpoints | - |

### network-utxos

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| RUST_LOG | Log level | info |
| HOST | Host to bind HTTP server | 0.0.0.0 |
| PORT | HTTP server port | 5557 |
| LOG_LEVEL | Log level | info |
| DATASOURCE | Storage backend (csv/sqlite) | sqlite |
| NETWORK | Bitcoin network (mainnet, testnet, regtest, signet) | regtest |
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
