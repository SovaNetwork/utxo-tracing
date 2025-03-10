# Build stage
FROM rust:1.81-slim-bookworm AS builder

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

# Create a non-root user with specific UID
RUN useradd -m -u 1001 service && \
    mkdir -p /data && \
    chown -R service:service /data
USER service

# Set environment variables
ENV RUST_LOG=info

# Set default values for arguments
ENV SOCKET_PATH="/tmp/network-utxos.sock" \
    NETWORK="regtest" \
    RPC_HOST="bitcoin" \
    RPC_PORT=18443 \
    RPC_USER="user" \
    RPC_PASSWORD="password" \
    START_HEIGHT=0 \
    POLLING_RATE=500 \
    MAX_BLOCKS_PER_BATCH=200 \
    THREAD_COUNT=4 \
    MAX_CONCURRENT_RPC=8 \
    BENCHMARK_MODE="false" \
    BENCHMARK_BLOCKS=1000 \
    BENCHMARK_BATCH_SIZE=""

# Create a directory for benchmark results
WORKDIR /data

# Use shell form to allow environment variable expansion with conditional benchmark mode
CMD if [ "$BENCHMARK_MODE" = "true" ]; then \
    BENCHMARK_ARGS="--benchmark --benchmark-blocks $BENCHMARK_BLOCKS"; \
    if [ -n "$BENCHMARK_BATCH_SIZE" ]; then \
    BENCHMARK_ARGS="$BENCHMARK_ARGS --benchmark-batch-size $BENCHMARK_BATCH_SIZE"; \
    fi; \
    indexer \
    --socket-path "$SOCKET_PATH" \
    --network "$NETWORK" \
    --rpc-host "$RPC_HOST" \
    --rpc-port "$RPC_PORT" \
    --rpc-user "$RPC_USER" \
    --rpc-password "$RPC_PASSWORD" \
    --start-height "$START_HEIGHT" \
    --polling-rate "$POLLING_RATE" \
    --max-blocks-per-batch "$MAX_BLOCKS_PER_BATCH" \
    --thread-count "$THREAD_COUNT" \
    --max-concurrent-rpc "$MAX_CONCURRENT_RPC" \
    $BENCHMARK_ARGS; \
    else \
    indexer \
    --socket-path "$SOCKET_PATH" \
    --network "$NETWORK" \
    --rpc-host "$RPC_HOST" \
    --rpc-port "$RPC_PORT" \
    --rpc-user "$RPC_USER" \
    --rpc-password "$RPC_PASSWORD" \
    --start-height "$START_HEIGHT" \
    --polling-rate "$POLLING_RATE" \
    --max-blocks-per-batch "$MAX_BLOCKS_PER_BATCH" \
    --thread-count "$THREAD_COUNT" \
    --max-concurrent-rpc "$MAX_CONCURRENT_RPC"; \
    fi