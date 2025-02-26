# Build stage for network-shared
FROM rust:1.81-slim-bookworm as builder

# Create a new empty shell project
WORKDIR /usr/src/app

# Install system dependencies
RUN apt-get update && \
    apt-get install -y \
    pkg-config \
    libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Copy manifest
COPY Cargo.toml ./

# Copy source code
COPY src ./src

# Build the library
RUN cargo build --release