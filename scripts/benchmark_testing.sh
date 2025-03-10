#!/bin/bash

# Benchmark script for Bitcoin Indexer performance testing
# This script runs multiple benchmarks with varying configuration parameters

# Default configuration
NETWORK="mainnet"
RPC_HOST="127.0.0.1"
RPC_PORT=8332
RPC_USER="username"
RPC_PASSWORD="password"
SOCKET_PATH="/tmp/network-utxos.sock"
START_HEIGHT=865700
BENCHMARK_BLOCKS=2  # Just 2 blocks for quick testing
OUTPUT_DIR="benchmark_results"

# Create output directory if it doesn't exist
mkdir -p "$OUTPUT_DIR"

# Timestamp for this benchmark run
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
LOG_FILE="$OUTPUT_DIR/benchmark_run_$TIMESTAMP.log"

echo "Starting benchmark run at $TIMESTAMP" | tee -a "$LOG_FILE"
echo "Results will be saved to $OUTPUT_DIR" | tee -a "$LOG_FILE"

# Make sure the database service is running
echo "Ensuring the database service is running..." | tee -a "$LOG_FILE"
if ! docker-compose ps | grep -q "network-utxos"; then
    echo "Starting database service..." | tee -a "$LOG_FILE"
    docker-compose up -d network-utxos

    # Give it a moment to start up properly
    echo "Waiting for database service to initialize..." | tee -a "$LOG_FILE"
    sleep 5
fi

# Function to run a single benchmark with given parameters
run_benchmark() {
    local thread_count=$1
    local max_concurrent_rpc=$2
    local batch_size=$3

    echo "" | tee -a "$LOG_FILE"
    echo "Running benchmark with:" | tee -a "$LOG_FILE"
    echo "- Thread count: $thread_count" | tee -a "$LOG_FILE"
    echo "- Max concurrent RPC: $max_concurrent_rpc" | tee -a "$LOG_FILE"
    echo "- Batch size: $batch_size" | tee -a "$LOG_FILE"

    # Create a unique service name for this benchmark
    local service_name="benchmark_t${thread_count}_rpc${max_concurrent_rpc}_b${batch_size}"

    # Build the docker-compose command - using network-indexer service
    docker-compose run --rm \
        -e START_HEIGHT="$START_HEIGHT" \
        -e THREAD_COUNT="$thread_count" \
        -e MAX_CONCURRENT_RPC="$max_concurrent_rpc" \
        -e MAX_BLOCKS_PER_BATCH="$batch_size" \
        -e BENCHMARK_MODE="true" \
        -e BENCHMARK_BLOCKS="$BENCHMARK_BLOCKS" \
        -e BENCHMARK_BATCH_SIZE="$batch_size" \
        -e RUST_LOG="info" \
        --name "$service_name" \
        network-indexer 2>&1 | tee -a "$LOG_FILE"

    echo "Benchmark complete" | tee -a "$LOG_FILE"
    echo "-----------------------" | tee -a "$LOG_FILE"
}

# Run benchmarks with different configurations

# # Test different thread counts
# echo "Testing different thread counts..." | tee -a "$LOG_FILE"
# for threads in 1 2 4 8 16; do
#     run_benchmark $threads 8 2
# done

# Test different RPC concurrency levels
echo "Testing different RPC concurrency levels..." | tee -a "$LOG_FILE"
for rpc_concurrency in 8 16 32 64; do
    run_benchmark 8 $rpc_concurrency 2
done

# # Test different batch sizes
# echo "Testing different batch sizes..." | tee -a "$LOG_FILE"
# for batch_size in 50 100 200 500; do
#     run_benchmark 8 16 $batch_size
# done

echo "All benchmarks completed. Results saved to $LOG_FILE" | tee -a "$LOG_FILE"
echo "Best configurations are summarized in benchmark result files in $OUTPUT_DIR"

# Ask user if they want to stop the database service
read -p "Do you want to stop the database service now? (y/n): " stop_db
if [[ "$stop_db" == "y" || "$stop_db" == "Y" ]]; then
    echo "Stopping database service..." | tee -a "$LOG_FILE"
    docker-compose stop network-utxos
fi