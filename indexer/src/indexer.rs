use std::time::Duration;

use bitcoincore_rpc::bitcoin::{Block, BlockHash, Network};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use chrono::{DateTime, Utc};
use log::{error, info};
use network_shared::{BlockUpdate, SocketTransport, UtxoUpdate, FINALITY_CONFIRMATIONS};
use tokio;

use crate::error::{IndexerError, Result};
use crate::utils::{determine_script_type, extract_address, extract_public_key};

/// The main Bitcoin indexer that processes blocks and transactions
pub struct BitcoinIndexer {
    rpc_client: Client,
    network: Network,
    socket_transport: SocketTransport,
    last_processed_height: i32,
    start_height: i32,
    max_blocks_per_batch: i32,
}

impl BitcoinIndexer {
    /// Creates a new BitcoinIndexer instance
    pub fn new(
        network: Network,
        rpc_user: &str,
        rpc_password: &str,
        rpc_host: &str,
        rpc_port: u16,
        socket_path: &str,
        start_height: i32,
        max_blocks_per_batch: i32,
    ) -> Result<Self> {
        let rpc_url = format!("http://{}:{}", rpc_host, rpc_port);
        let auth = Auth::UserPass(rpc_user.to_string(), rpc_password.to_string());
        let rpc_client = Client::new(&rpc_url, auth).map_err(IndexerError::BitcoinRPC)?;

        // Validate start block
        let chain_height = rpc_client.get_block_count()? as i32;
        if start_height < 0 || start_height > chain_height {
            return Err(IndexerError::InvalidStartBlock(format!(
                "Start block {} is invalid. Chain height is {}",
                start_height, chain_height
            )));
        }

        let socket_transport = SocketTransport::new(socket_path);

        Ok(Self {
            rpc_client,
            network,
            socket_transport,
            last_processed_height: start_height - 1,
            start_height,
            max_blocks_per_batch,
        })
    }

    /// Gets block data for a given block hash and process transactions
    fn get_block_data(&self, block_hash: &BlockHash) -> Result<BlockUpdate> {
        let block = self.rpc_client.get_block(block_hash)?;
        let block_info = self.rpc_client.get_block_info(block_hash)?;

        let timestamp = DateTime::<Utc>::from_timestamp(block.header.time as i64, 0)
            .ok_or(IndexerError::InvalidTimestamp)?;

        let utxo_updates =
            self.process_transactions(&block, block_info.height as i32, timestamp)?;

        Ok(BlockUpdate {
            height: block_info.height as i32,
            hash: block_hash.to_string(),
            timestamp,
            utxo_updates,
        })
    }

    /// Processes all transactions in a block
    fn process_transactions(
        &self,
        block: &Block,
        height: i32,
        block_time: DateTime<Utc>,
    ) -> Result<Vec<UtxoUpdate>> {
        let mut utxo_updates = Vec::new();

        for (tx_index, tx) in block.txdata.iter().enumerate() {
            // First transaction in a block is always the coinbase, check if it is
            let is_coinbase = tx_index == 0;

            // Process spent UTXOs (inputs)
            for input in tx.input.iter() {
                if input.previous_output.is_null() {
                    if !is_coinbase {
                        error!("Found null previous output in non-coinbase transaction");
                    } else {
                        info!("Skipping coinbase transaction input");
                    }
                    continue;
                }

                let prev_tx = self
                    .rpc_client
                    .get_raw_transaction(&input.previous_output.txid, None)?;
                let prev_output = &prev_tx.output[input.previous_output.vout as usize];

                let spent_utxo = UtxoUpdate {
                    id: format!(
                        "{}:{}",
                        input.previous_output.txid, input.previous_output.vout
                    ),
                    address: extract_address(prev_output.script_pubkey.clone(), self.network)?,
                    public_key: extract_public_key(&input.witness),
                    txid: input.previous_output.txid.to_string(),
                    vout: input.previous_output.vout as i32,
                    amount: prev_output.value as i64,
                    script_pub_key: hex::encode(prev_output.script_pubkey.as_bytes()),
                    script_type: determine_script_type(prev_output.script_pubkey.clone()),
                    created_at: block_time,
                    block_height: height,
                    spent_txid: Some(tx.txid().to_string()),
                    spent_at: Some(block_time),
                    spent_block: Some(height),
                };

                utxo_updates.push(spent_utxo);
            }

            // Process new UTXOs (outputs)
            for (vout, output) in tx.output.iter().enumerate() {
                // Check if this is a coinbase transaction output
                let (address, script_type) = if tx.is_coin_base() {
                    ("coinbase".to_string(), "COINBASE".to_string())
                } else {
                    // Regular transaction output
                    (
                        extract_address(output.script_pubkey.clone(), self.network)?,
                        determine_script_type(output.script_pubkey.clone()),
                    )
                };

                let utxo = UtxoUpdate {
                    id: format!("{}:{}", tx.txid(), vout),
                    address,
                    public_key: None, // Will be filled when the UTXO is spent
                    txid: tx.txid().to_string(),
                    vout: vout as i32,
                    amount: output.value as i64,
                    script_pub_key: hex::encode(output.script_pubkey.as_bytes()),
                    script_type,
                    created_at: block_time,
                    block_height: height,
                    spent_txid: None,
                    spent_at: None,
                    spent_block: None,
                };

                utxo_updates.push(utxo);
            }
        }

        Ok(utxo_updates)
    }

    /// Sends a block update to the socket transport
    async fn send_block_update(&self, update: &BlockUpdate) -> Result<()> {
        self.socket_transport.send_update(update).await?;
        Ok(())
    }

    /// Processes new blocks that have been added to the blockchain
    async fn process_new_blocks(&mut self) -> Result<i32> {
        let current_height = self.rpc_client.get_block_count()? as i32;
        if current_height <= self.last_processed_height {
            return Ok(0);
        }

        // First, check for reorgs
        if let Ok(true) = self.check_for_reorg().await {
            // A reorg was detected and handled, exit this cycle
            return Ok(0);
        }

        let blocks_to_process = std::cmp::min(
            current_height - self.last_processed_height,
            self.max_blocks_per_batch,
        );

        if blocks_to_process == 0 {
            return Ok(0);
        }

        info!(
            "Processing {} new blocks from height {}",
            blocks_to_process,
            self.last_processed_height + 1
        );

        for height in
            self.last_processed_height + 1..=self.last_processed_height + blocks_to_process
        {
            let block_hash = self.rpc_client.get_block_hash(height as u64)?;
            let block_data = self.get_block_data(&block_hash)?;
            self.send_block_update(&block_data).await?;
        }

        self.last_processed_height += blocks_to_process;

        // Update finality status after processing new blocks
        let finality_threshold = current_height - FINALITY_CONFIRMATIONS + 1;
        if finality_threshold > 0 {
            info!(
                "Updating finality status for blocks with threshold {}",
                finality_threshold
            );
            // Send message to update finality status
            self.socket_transport
                .send_update_finality_status(current_height)
                .await?;
        }

        info!(
            "Successfully processed blocks up to height {}",
            self.last_processed_height
        );

        Ok(blocks_to_process)
    }

    async fn check_for_reorg(&mut self) -> Result<bool> {
        let current_height = self.rpc_client.get_block_count()? as i32;

        // Calculate the finality threshold
        let finality_threshold = current_height - FINALITY_CONFIRMATIONS + 1;

        // Only check non-finalized blocks for reorgs
        let start_check_height = std::cmp::max(finality_threshold, 0);

        // If our last processed block is already considered final, no need to check
        if self.last_processed_height < start_check_height {
            return Ok(false);
        }

        // Check each non-final block for hash mismatch
        for height in start_check_height..=self.last_processed_height {
            let chain_hash = self.rpc_client.get_block_hash(height as u64)?;
            let stored_hash = self.socket_transport.get_block_hash(height).await?;

            if chain_hash.to_string() != stored_hash {
                // Reorg detected
                info!("Reorg detected at height {}", height);

                // Find fork point (but don't go below finality threshold)
                let fork_point = self.find_fork_point(height, start_check_height).await?;

                info!("Fork point found at height {}", fork_point);

                // Revert to fork point
                self.socket_transport.send_reorg_event(fork_point).await?;
                self.last_processed_height = fork_point;

                return Ok(true);
            }
        }

        Ok(false)
    }

    // Helper method to find the fork point
    async fn find_fork_point(&self, start_height: i32, min_height: i32) -> Result<i32> {
        let mut height = start_height - 1;

        while height >= min_height {
            let chain_hash = self.rpc_client.get_block_hash(height as u64)?;
            let stored_hash = self.socket_transport.get_block_hash(height).await?;

            if chain_hash.to_string() == stored_hash {
                return Ok(height);
            }

            height -= 1;
        }

        // If we reach here, the fork is at or below the finality threshold
        // We'll use the finality threshold as the safe point
        Ok(min_height - 1)
    }

    /// Runs the indexer in a loop, processing new blocks as they are added
    pub async fn run(&mut self, poll_interval: Duration) -> Result<()> {
        info!(
            "Starting Bitcoin UTXO indexer from block {} with polling interval of {:?}",
            self.start_height, poll_interval
        );

        loop {
            if let Err(e) = self.process_new_blocks().await {
                error!("Error in indexer loop: {}", e);
            }

            tokio::time::sleep(poll_interval).await;
        }
    }
}
