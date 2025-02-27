#!/bin/bash

# Resources:
# https://github.com/PowVT/satoshi-suite

# Steps:
# 1. Create two Bitcoin wallets
# 2. Mine 1 block to user
# 3. Create a signed btc tx for 49.999 sats and a fee of 0.001 sats
# 4. Broadcast the transaction
# 5. Call and get a response from:
# http://localhost:5557/spendable-utxos/block/$BLOCK_HEIGHT/address/$BITCOIN_RECEIVE_ADDRESS

# Exit on error
set -e

# Configuration
WALLET_1="user"
WALLET_2="miner"
BITCOIN_RECEIVE_ADDRESS="bcrt1q8pw3u88q56mfdqhxyeu0a7fesddq8jwsxxqng8"

# Function to convert BTC to smallest unit (satoshis)
btc_to_sats() {
    echo "$1 * 100000000" | bc | cut -d'.' -f1
}

# Function to extract transaction hex
get_tx_hex() {
    local output=$1
    local hex=$(echo "$output" | grep "Signed transaction:" | sed 's/.*Signed transaction: //')
    echo "$hex"
}

# function to extract the block height
get_block_height() {
    local output=$1
    local height=$(echo "$output" | grep "Current block height:" | sed 's/.*Current block height: //')
    echo "$height"
}

echo "Creating Bitcoin wallets..."
satoshi-suite new-wallet --wallet-name "$WALLET_1"
satoshi-suite new-wallet --wallet-name "$WALLET_2"

echo "Mining initial blocks..."
satoshi-suite mine-blocks --wallet-name "$WALLET_1" --blocks 1
satoshi-suite mine-blocks --wallet-name "$WALLET_2" --blocks 100

echo "Creating Bitcoin transactions..."
# Transaction with 0.001 fee
TX1_OUTPUT=$(satoshi-suite sign-tx --wallet-name "$WALLET_1" --recipient "$BITCOIN_RECEIVE_ADDRESS" --amount 49.999 --fee-amount 0.001)
TX1_HEX=$(get_tx_hex "$TX1_OUTPUT")

# For debugging
echo "TX1 Hex: $TX1_HEX"

# broadcast transaction
satoshi-suite broadcast-tx --tx-hex "$TX1_HEX"

# get current block height
BLOCK_HEIGHT_OUTPUT=$(satoshi-suite get-block-height)
BLOCK_HEIGHT=$(get_block_height "$BLOCK_HEIGHT_OUTPUT")

echo "mining blocks to conifirm btc tx..."
satoshi-suite mine-blocks --wallet-name "$WALLET_2" --blocks 7

sleep 1

curl "http://localhost:5557/spendable-utxos/block/$BLOCK_HEIGHT/address/$BITCOIN_RECEIVE_ADDRESS"