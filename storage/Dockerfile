# Build stage
FROM rust:1.87-slim-bookworm AS builder

# Create a new empty shell project
WORKDIR /usr/src/app

# Install system dependencies
RUN apt-get update && \
    apt-get install -y \
    pkg-config \
    libssl-dev \
    clang \
    libsqlite3-dev && \
    rm -rf /var/lib/apt/lists/*

# Copy entire utxo-tracing directory
COPY . .

# Build the application from the workspace root
RUN cargo build --release --package storage

# Final stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y \
    ca-certificates \
    libsqlite3-0 && \
    rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /usr/src/app/target/release/storage /usr/local/bin/
RUN chmod +x /usr/local/bin/storage


# Create a non-root user with specific UID and create data directory
RUN useradd -m -u 1001 service && \
    mkdir -p /data && \
    chown -R service:service /data

# Set environment variables
# Set default values for arguments
ENV HOST=0.0.0.0 \
    PORT=5557 \
    LOG_LEVEL=info \
    DATASOURCE=sqlite \
    SOCKET_PATH=/tmp/network-utxos.sock \
    NETWORK=regtest

# Switch to non-root user
USER service
WORKDIR /data

# Expose the webhook server port
EXPOSE 5557

# Run the binary
CMD storage \
    --host "$HOST" \
    --port "$PORT" \
    --log-level "$LOG_LEVEL" \
    --datasource "$DATASOURCE" \
    --network "$NETWORK" \
    --socket-path "$SOCKET_PATH"
