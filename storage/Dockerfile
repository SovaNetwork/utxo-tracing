# Build stage
FROM rust:1.81-slim-bookworm as builder

# Create a new empty shell project
WORKDIR /usr/src/app

# Install system dependencies
RUN apt-get update && \
    apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev && \
    rm -rf /var/lib/apt/lists/*

# Copy entire utxo-tracing directory
COPY . .

# Set working directory to storage
WORKDIR /usr/src/app/storage

# Build the application
RUN cargo build --release

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

# Create a non-root user with specific UID and create data directory
RUN useradd -m -u 1001 service && \
    mkdir -p /data && \
    chown -R service:service /data

# Set environment variables
ENV RUST_LOG=info

# Set default values for arguments
ENV HOST=0.0.0.0 \
    PORT=5557 \
    LOG_LEVEL=info \
    DATASOURCE=sqlite \
    SOCKET_PATH=/tmp/network-utxos.sock

# Switch to non-root user
USER service
WORKDIR /data

# Expose the webhook server port
EXPOSE 5557

# Run the binary
CMD network-utxos \
    --host "$HOST" \
    --port "$PORT" \
    --log-level "$LOG_LEVEL" \
    --datasource "$DATASOURCE" \
    --socket-path "$SOCKET_PATH"