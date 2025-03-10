use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use bitcoincore_rpc::bitcoin::{Block, BlockHash, Network, TxIn, TxOut};
use bitcoincore_rpc::{Auth, Client, RpcApi};
use chrono::{DateTime, Local, Utc};
use log::{debug, error, info, warn};
use network_shared::{BlockUpdate, SocketTransport, UtxoUpdate, FINALITY_CONFIRMATIONS};
use rayon::prelude::*;
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{oneshot, Semaphore};

use crate::benchmark::{get_memory_usage, BenchmarkResult};
use crate::error::{IndexerError, Result};
use crate::utils::{determine_script_type, extract_address, extract_public_key};

trait SocketTransportExt {
    fn get_socket_path(&self) -> &str;
}

impl SocketTransportExt for SocketTransport {
    fn get_socket_path(&self) -> &str {
        &self.socket_path
    }
}

// convenience struct for holding tx data
#[derive(Debug)]
struct TxData {
    txid: String,
    is_coinbase: bool,
    inputs: Vec<(TxIn, usize)>,
    outputs: Vec<(TxOut, usize)>,
}

/// Bitcoin indexer that processes blocks and transactions
pub struct BitcoinIndexer {
    rpc_client: Client,
    network: Network,
    socket_transport: SocketTransport,
    last_processed_height: i32,
    start_height: i32,
    max_blocks_per_batch: i32,
    whitelist_cache: Mutex<HashMap<String, bool>>,
    rpc_semaphore: Arc<Semaphore>,
}

impl BitcoinIndexer {
    /// Creates a new BitcoinIndexer instance
    pub fn new(
        network: Network,
        rpc_user: &str,
        rpc_password: &str,
        rpc_host: &str,
        rpc_port: &u16,
        socket_path: &str,
        mut start_height: i32,
        max_blocks_per_batch: i32,
        rpc_semaphore: Arc<Semaphore>,
    ) -> Result<Self> {
        let rpc_url = format!("http://{}:{}", rpc_host, rpc_port);
        let auth = Auth::UserPass(rpc_user.to_string(), rpc_password.to_string());
        let rpc_client = Client::new(&rpc_url, auth).map_err(IndexerError::BitcoinRPC)?;

        // Try to find the latest state file and use that height if appropriate
        let saved_state = Self::find_latest_state_file();
        if let Some((saved_height, filename)) = saved_state {
            if saved_height >= start_height {
                info!(
                    "Found saved state with height {} from file {}. Using this instead of the provided start height {}.",
                    saved_height, filename, start_height
                );
                start_height = saved_height;
            } else {
                info!(
                    "Found saved state with height {} from file {}, but it's lower than the provided start height {}. Using the provided start height.",
                    saved_height, filename, start_height
                );
            }
        }

        // Validate start block
        let chain_height = rpc_client.get_block_count()? as i32;
        if start_height < 0 || start_height > chain_height {
            return Err(IndexerError::InvalidStartBlock(format!(
                "Start block {} is invalid. Chain height is {}",
                start_height, chain_height
            )));
        }
        info!("Starting indexer from height {}", start_height);

        let socket_transport = SocketTransport::new(socket_path);

        Ok(Self {
            rpc_client,
            network,
            socket_transport,
            last_processed_height: start_height - 1,
            start_height,
            max_blocks_per_batch,
            whitelist_cache: Mutex::new(HashMap::new()),
            rpc_semaphore,
        })
    }

    pub async fn save_state_on_shutdown(&self) -> Result<()> {
        // Create data directory if it doesn't exist
        let data_dir = std::path::Path::new("/data");
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| IndexerError::Network(network_shared::TransportError::IoError(e)))?;

        // Generate filename with timestamp
        let now = Local::now();
        let filename = format!("indexer_state_{}.txt", now.format("%Y-%m-%d_%H-%M-%S"));
        let file_path = data_dir.join(filename);

        // Create and write to file
        let mut file = File::create(&file_path)
            .map_err(|e| IndexerError::Network(network_shared::TransportError::IoError(e)))?;

        // Write state information
        writeln!(&mut file, "Timestamp: {}", now)
            .map_err(|e| IndexerError::Network(network_shared::TransportError::IoError(e)))?;
        writeln!(&mut file, "Network: {:?}", self.network)
            .map_err(|e| IndexerError::Network(network_shared::TransportError::IoError(e)))?;
        writeln!(&mut file, "Start height: {}", self.start_height)
            .map_err(|e| IndexerError::Network(network_shared::TransportError::IoError(e)))?;
        writeln!(
            &mut file,
            "Last processed height: {}",
            self.last_processed_height
        )
        .map_err(|e| IndexerError::Network(network_shared::TransportError::IoError(e)))?;
        writeln!(
            &mut file,
            "Max blocks per batch: {}",
            self.max_blocks_per_batch
        )
        .map_err(|e| IndexerError::Network(network_shared::TransportError::IoError(e)))?;

        info!("Saved indexer state to {}", file_path.display());
        Ok(())
    }

    pub fn find_latest_state_file() -> Option<(i32, String)> {
        let data_dir = std::path::Path::new("/data");
        if !data_dir.exists() {
            return None;
        }

        let mut latest_file = None;
        let mut latest_timestamp = std::time::SystemTime::UNIX_EPOCH;

        if let Ok(entries) = std::fs::read_dir(data_dir) {
            for entry in entries.flatten() {
                let path = entry.path();

                // Check if it's a state file
                if let Some(filename) = path.file_name() {
                    if let Some(filename_str) = filename.to_str() {
                        if filename_str.starts_with("indexer_state_")
                            && filename_str.ends_with(".txt")
                        {
                            if let Ok(metadata) = std::fs::metadata(&path) {
                                if let Ok(modified) = metadata.modified() {
                                    if modified > latest_timestamp {
                                        latest_timestamp = modified;
                                        latest_file = Some(path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Parse the latest file if found
        if let Some(file_path) = latest_file {
            let filename = file_path.to_string_lossy().to_string();

            if let Ok(content) = std::fs::read_to_string(file_path) {
                let mut last_height = None;

                // Extract the last processed height
                for line in content.lines() {
                    if line.starts_with("Last processed height: ") {
                        if let Some(height_str) = line.strip_prefix("Last processed height: ") {
                            if let Ok(height) = height_str.trim().parse::<i32>() {
                                last_height = Some(height);
                            }
                        }
                    }
                }

                if let Some(height) = last_height {
                    return Some((height, filename));
                }
            }
        }

        None
    }

    /// Get a permit from the RPC semaphore for limiting concurrent RPC calls
    async fn get_rpc_permit(&self) -> tokio::sync::SemaphorePermit {
        self.rpc_semaphore.acquire().await.unwrap()
    }

    /// Checks if a reorg has been detected
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
            let _permit = self.get_rpc_permit().await;
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

    /// Get the whitelist status with caching
    async fn is_address_whitelisted(&self, address: &str) -> bool {
        // Check cache first
        if let Some(status) = self.whitelist_cache.lock().unwrap().get(address) {
            return *status;
        }

        // Always include coinbase transactions
        if address == "coinbase" {
            self.whitelist_cache
                .lock()
                .unwrap()
                .insert(address.to_string(), true);
            return true;
        }

        // Query the storage service to check whitelist
        match self.socket_transport.check_whitelist(address).await {
            Ok(is_whitelisted) => {
                // Update cache
                self.whitelist_cache
                    .lock()
                    .unwrap()
                    .insert(address.to_string(), is_whitelisted);
                is_whitelisted
            }
            Err(e) => {
                error!("Failed to check whitelist status: {}", e);
                // Default to true if we can't check the whitelist
                // This is safer than missing transactions
                true
            }
        }
    }

    /// Prefetch whitelist status for all addresses in a block
    async fn prefetch_block_whitelist_status(&self, block: &Block) -> Result<()> {
        let mut addresses = HashSet::new();

        // Extract all unique addresses from the block
        for (tx_index, tx) in block.txdata.iter().enumerate() {
            let is_coinbase = tx_index == 0;

            // Get addresses from inputs (if not coinbase)
            if !is_coinbase {
                for input in &tx.input {
                    if !input.previous_output.is_null() {
                        // Get a permit for this RPC call
                        let _permit = self.get_rpc_permit().await;

                        let prev_tx = self
                            .rpc_client
                            .get_raw_transaction(&input.previous_output.txid, None)?;
                        let prev_output = &prev_tx.output[input.previous_output.vout as usize];

                        if let Ok(address) =
                            extract_address(prev_output.script_pubkey.clone(), self.network)
                        {
                            addresses.insert(address);
                        }
                    }
                }
            }

            // Get addresses from outputs
            for output in &tx.output {
                if let Ok(address) = extract_address(output.script_pubkey.clone(), self.network) {
                    addresses.insert(address);
                }
            }
        }

        // Remove addresses that are already in the cache
        {
            let cache = self.whitelist_cache.lock().unwrap();
            addresses.retain(|addr| !cache.contains_key(addr));
        }

        // Check whitelist status in parallel for remaining addresses
        if !addresses.is_empty() {
            let addresses_vec: Vec<String> = addresses.into_iter().collect();
            let socket_path = self.socket_transport.get_socket_path().to_string();

            // Process addresses in parallel batches using rayon
            let batch_size = 50;
            for chunk in addresses_vec.chunks(batch_size) {
                let mut futures = Vec::new();

                for address in chunk {
                    let addr_clone = address.clone();
                    let socket_path_clone = socket_path.clone();

                    // Use tokio spawn to check whitelist status concurrently
                    // Create a new SocketTransport instance for each task instead of cloning
                    futures.push(tokio::spawn(async move {
                        let transport = SocketTransport::new(&socket_path_clone);
                        let result = transport.check_whitelist(&addr_clone).await;
                        (addr_clone, result)
                    }));
                }

                // Wait for all futures to complete
                for future in futures {
                    if let Ok((address, result)) = future.await {
                        let is_whitelisted = result.unwrap_or(true); // Default to true on error
                        self.whitelist_cache
                            .lock()
                            .unwrap()
                            .insert(address, is_whitelisted);
                    }
                }
            }
        }

        Ok(())
    }

    /// Gets block data for a given block hash and process transactions
    async fn get_block_data(&self, block_hash: &BlockHash) -> Result<BlockUpdate> {
        // Get a permit for the block data RPC call
        let _permit = self.get_rpc_permit().await;

        let block = self.rpc_client.get_block(block_hash)?;
        let block_info = self.rpc_client.get_block_info(block_hash)?;

        let timestamp = DateTime::<Utc>::from_timestamp(block.header.time as i64, 0)
            .ok_or(IndexerError::InvalidTimestamp)?;

        // Prefetch whitelist status for all addresses in the block
        self.prefetch_block_whitelist_status(&block).await?;

        let utxo_updates = self
            .process_transactions(&block, block_info.height as i32, timestamp)
            .await?;

        Ok(BlockUpdate {
            height: block_info.height as i32,
            hash: block_hash.to_string(),
            timestamp,
            utxo_updates,
        })
    }

    /// Processes all transactions in a block
    async fn process_transactions(
        &self,
        block: &Block,
        height: i32,
        block_time: DateTime<Utc>,
    ) -> Result<Vec<UtxoUpdate>> {
        // Step 1: Collect transaction data with parallel preprocessing
        // Preprocess transactions in parallel
        let tx_data: Vec<TxData> = block
            .txdata
            .par_iter()
            .enumerate()
            .map(|(tx_index, tx)| {
                let txid = tx.txid().to_string();
                let is_coinbase = tx_index == 0;

                // Preprocess inputs
                let inputs = tx
                    .input
                    .iter()
                    .enumerate()
                    .map(|(i, input)| (input.clone(), i))
                    .collect();

                // Preprocess outputs
                let outputs = tx
                    .output
                    .iter()
                    .enumerate()
                    .map(|(i, output)| (output.clone(), i))
                    .collect();

                TxData {
                    txid,
                    is_coinbase,
                    inputs,
                    outputs,
                }
            })
            .collect();

        // Step 2: Process transactions sequentially using pre-processed data
        let mut utxo_updates = Vec::new();

        for tx_data in tx_data {
            // Process inputs (spent UTXOs)
            for (input, _) in tx_data.inputs {
                if input.previous_output.is_null() {
                    if !tx_data.is_coinbase {
                        error!("Found null previous output in non-coinbase transaction");
                    } else {
                        debug!("Skipping coinbase transaction input");
                    }
                    continue;
                }

                // Get a permit to limit concurrent RPC calls
                let _permit = self.get_rpc_permit().await;

                let prev_tx = self
                    .rpc_client
                    .get_raw_transaction(&input.previous_output.txid, None)?;
                let prev_output = &prev_tx.output[input.previous_output.vout as usize];

                let address = extract_address(prev_output.script_pubkey.clone(), self.network)?;

                // Skip if address is not whitelisted
                if !self.is_address_whitelisted(&address).await {
                    continue;
                }

                let spent_utxo = UtxoUpdate {
                    id: format!(
                        "{}:{}",
                        input.previous_output.txid, input.previous_output.vout
                    ),
                    address,
                    public_key: extract_public_key(&input.witness),
                    txid: input.previous_output.txid.to_string(),
                    vout: input.previous_output.vout as i32,
                    amount: prev_output.value as i64,
                    script_pub_key: hex::encode(prev_output.script_pubkey.as_bytes()),
                    script_type: determine_script_type(prev_output.script_pubkey.clone()),
                    created_at: block_time,
                    block_height: height,
                    spent_txid: Some(tx_data.txid.clone()),
                    spent_at: Some(block_time),
                    spent_block: Some(height),
                };

                utxo_updates.push(spent_utxo);
            }

            // Process outputs (new UTXOs)
            for (output, vout) in tx_data.outputs {
                // Check if this is a coinbase transaction output
                let (address, script_type) = if tx_data.is_coinbase {
                    ("coinbase".to_string(), "COINBASE".to_string())
                } else {
                    // Regular transaction output
                    (
                        extract_address(output.script_pubkey.clone(), self.network)?,
                        determine_script_type(output.script_pubkey.clone()),
                    )
                };

                // Skip if address is not whitelisted
                if !self.is_address_whitelisted(&address).await {
                    continue;
                }

                let utxo = UtxoUpdate {
                    id: format!("{}:{}", tx_data.txid, vout),
                    address,
                    public_key: None, // Will be filled when the UTXO is spent
                    txid: tx_data.txid.clone(),
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

    /// Process a batch of blocks in parallel where possible
    async fn process_batch_of_blocks(&mut self, start_height: i32, end_height: i32) -> Result<()> {
        debug!(
            "Processing blocks in batch from height {} to {}",
            start_height, end_height
        );

        // Get block hashes sequentially with RPC permits
        // Since rayon is parallel but not async, we can't use the async semaphore directly in par_iter
        let mut block_hashes = Vec::with_capacity((end_height - start_height + 1) as usize);

        for height in start_height..=end_height {
            let _permit = self.get_rpc_permit().await;
            let block_hash = self
                .rpc_client
                .get_block_hash(height as u64)
                .map_err(|e| IndexerError::BitcoinRPC(e))?;
            block_hashes.push(block_hash);
        }

        // Process blocks sequentially (due to async operations)
        for (i, block_hash) in block_hashes.iter().enumerate() {
            let current_height = start_height + i as i32;
            debug!("Processing block at height {}", current_height);
            let block_data = self.get_block_data(block_hash).await?;
            self.send_block_update(&block_data).await?;
        }

        self.last_processed_height = end_height;

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

        // get start time for tracking performance
        let start_time = std::time::Instant::now();

        // Process new blocks
        let start_height = self.last_processed_height + 1;
        let end_height = self.last_processed_height + blocks_to_process;

        self.process_batch_of_blocks(start_height, end_height)
            .await?;

        self.last_processed_height += blocks_to_process;

        // Update finality status after processing new blocks
        let finality_threshold = current_height - FINALITY_CONFIRMATIONS + 1;
        if finality_threshold > 0 {
            debug!(
                "Updating finality status for blocks with threshold {}",
                finality_threshold
            );
            // Send message to update finality status
            self.socket_transport
                .send_update_finality_status(current_height)
                .await?;
        }

        let total_elapsed = start_time.elapsed();
        let total_speed = blocks_to_process as f64 / total_elapsed.as_secs_f64();

        info!(
            "Successfully processed {} blocks. Current height: {}/{}. Speed: {:.2} blocks/sec",
            blocks_to_process, self.last_processed_height, current_height, total_speed
        );

        Ok(blocks_to_process)
    }

    /// Benchmark the indexer by processing a specified number of blocks
    pub async fn benchmark(
        &mut self,
        blocks_to_process: i32,
        batch_size: Option<i32>,
    ) -> Result<BenchmarkResult> {
        info!(
            "Starting benchmark: processing {} blocks",
            blocks_to_process
        );

        // Save original batch size and restore it later
        let original_batch_size = self.max_blocks_per_batch;
        if let Some(batch) = batch_size {
            self.max_blocks_per_batch = batch;
            info!("Using batch size of {} for benchmark", batch);
        }

        let current_height = self.rpc_client.get_block_count()? as i32;

        // Use the current start height if it's set, otherwise use the current height
        let start_height = if self.start_height > 0 {
            self.start_height
        } else {
            std::cmp::max(0, current_height - blocks_to_process)
        };

        let end_height = start_height + blocks_to_process - 1;

        // Validate that we have enough blocks
        if end_height > current_height {
            warn!(
                "Not enough blocks available for benchmark. Requested start: {}, end: {}, current chain height: {}",
                start_height, end_height, current_height
            );
            return Ok(BenchmarkResult {
                blocks_processed: 0,
                duration: Duration::from_secs(0),
                blocks_per_second: 0.0,
                utxos_processed: 0,
                utxos_per_second: 0.0,
                memory_usage: get_memory_usage(),
            });
        }

        // Reset last processed height to start point
        let original_last_processed_height = self.last_processed_height;

        // Start timing
        let start_time = Instant::now();
        let mut total_utxos = 0;

        info!(
            "Benchmark processing from height {} to {}",
            start_height, end_height
        );

        for height in start_height..=end_height {
            // Get a permit for this RPC call
            let _permit = self.get_rpc_permit().await;

            let block_hash = self.rpc_client.get_block_hash(height as u64)?;
            let block = self.rpc_client.get_block(&block_hash)?;

            // Process block data and get UTXO updates
            let block_data = self.get_block_data(&block_hash).await?;
            self.send_block_update(&block_data).await?;

            // Count UTXOs in this block (inputs + outputs)
            for tx in &block.txdata {
                total_utxos += tx.input.len() + tx.output.len();
            }

            info!(
                "Processed block {} with {} transactions",
                height,
                block.txdata.len()
            );
        }

        let duration = start_time.elapsed();
        let blocks_processed = end_height - start_height + 1;
        let blocks_per_second = blocks_processed as f64 / duration.as_secs_f64();
        let utxos_per_second = total_utxos as f64 / duration.as_secs_f64();

        // Restore original settings
        self.last_processed_height = original_last_processed_height;
        self.max_blocks_per_batch = original_batch_size;

        let result = BenchmarkResult {
            blocks_processed,
            duration,
            blocks_per_second,
            utxos_processed: total_utxos as i32,
            utxos_per_second,
            memory_usage: get_memory_usage(),
        };

        info!("Benchmark completed: {}", result);

        Ok(result)
    }

    /// Runs the indexer in a loop, processing new blocks as they are added
    pub async fn run(&mut self, poll_interval: Duration) -> Result<()> {
        info!(
            "Starting indexer with polling interval of {} ms",
            poll_interval.as_millis()
        );

        // Create a shared shutdown flag
        let shutdown = Arc::new(AtomicBool::new(false));

        // Clone for signal handlers
        let shutdown_sigint = shutdown.clone();
        let shutdown_sigterm = shutdown.clone();

        // Create one-shot channel for coordinating the shutdown
        let (shutdown_signal_tx, mut shutdown_signal_rx) = oneshot::channel::<()>();
        let shutdown_signal_tx = Arc::new(std::sync::Mutex::new(Some(shutdown_signal_tx)));

        // Clone for signal handlers
        let sigint_tx = shutdown_signal_tx.clone();

        // Handle SIGINT
        tokio::spawn(async move {
            if let Ok(mut sigint) = signal(SignalKind::interrupt()) {
                sigint.recv().await;
                info!("Received SIGINT, shutting down...");
                shutdown_sigint.store(true, Ordering::SeqCst);
                if let Some(tx) = sigint_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }
        });

        // Handle SIGTERM
        tokio::spawn(async move {
            if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                sigterm.recv().await;
                info!("Received SIGTERM, shutting down...");
                shutdown_sigterm.store(true, Ordering::SeqCst);
                if let Some(tx) = shutdown_signal_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }
        });

        // Main processing loop
        loop {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            if let Err(e) = self.process_new_blocks().await {
                error!("Error in indexer loop: {}", e);
            }

            tokio::select! {
                _ = tokio::time::sleep(poll_interval) => {
                    // Continue with the next iteration
                }
                _ = &mut shutdown_signal_rx => {
                    info!("Shutdown signal received, saving state and exiting...");
                    if let Err(e) = self.save_state_on_shutdown().await {
                        error!("Failed to save state on shutdown: {}", e);
                    }
                    break;
                }
            }
        }

        Ok(())
    }
}
