# Build stage
FROM rust:1.87-slim-bookworm AS builder

# Create a new empty shell project
WORKDIR /usr/src/app

# Install system dependencies
RUN apt-get update && \
    apt-get install -y \
    pkg-config \
    libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Copy entire utxo-tracing directory
COPY . .

# Build the application from the workspace root
RUN cargo build --release --package indexer

# Final stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y \
    ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /usr/src/app/target/release/indexer /usr/local/bin/
RUN chmod +x /usr/local/bin/indexer

# Create a non-root user with specific UID and create cache directory
RUN useradd -m -u 1001 service && \
    mkdir -p /app/cache && \
    chown -R service:service /app/cache

USER service

# Set environment variables
ENV RUST_LOG=info

# Set default values for arguments
ENV SOCKET_PATH="/tmp/network-utxos.sock" \
    NETWORK="regtest" \
    RPC_HOST="http://bitcoin:18443" \
    RPC_USER="user" \
    RPC_PASSWORD="password" \
    START_HEIGHT=0 \
    POLLING_RATE=500 \
    MAX_BLOCKS_PER_BATCH=200 \
    API_HOST=0.0.0.0 \
    API_PORT=3031 \
    ENCLAVE_URL="http://network-enclave:5555" \
    UTXO_URL="http://network-utxos:5557" \
    ENCLAVE_API_KEY="" \
    INDEXER_API_KEY=""

# Use shell form to allow environment variable expansion
CMD indexer \
    --socket-path "$SOCKET_PATH" \
    --network "$NETWORK" \
    --rpc-host "$RPC_HOST" \
    --rpc-user "$RPC_USER" \
    --rpc-password "$RPC_PASSWORD" \
    --start-height "$START_HEIGHT" \
    --polling-rate "$POLLING_RATE" \
    --max-blocks-per-batch "$MAX_BLOCKS_PER_BATCH" \
    --api-host "$API_HOST" \
    --api-port "$API_PORT"
