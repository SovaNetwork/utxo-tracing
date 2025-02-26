# Build stage
FROM rust:1.81-slim-bookworm as builder

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

# Set working directory to indexer
WORKDIR /usr/src/app/indexer

# Build the application
RUN cargo build --release

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
RUN useradd -m -u 1001 service
USER service

# Set environment variables
ENV RUST_LOG=info

# Set default values for arguments
ENV SOCKET_PATH="/tmp/network-utxos.sock" \
    RPC_HOST="bitcoin" \
    RPC_PORT=18443 \
    RPC_USER="user" \
    RPC_PASSWORD="password" \
    START_HEIGHT=0 \
    POLLING_RATE=500 \
    MAX_BLOCKS_PER_BATCH=200

# Use shell form to allow environment variable expansion
CMD indexer \
    --socket-path "$SOCKET_PATH" \
    --rpc-host "$RPC_HOST" \
    --rpc-port "$RPC_PORT" \
    --rpc-user "$RPC_USER" \
    --rpc-password "$RPC_PASSWORD" \
    --start-height "$START_HEIGHT" \
    --polling-rate "$POLLING_RATE" \
    --max-blocks-per-batch "$MAX_BLOCKS_PER_BATCH"