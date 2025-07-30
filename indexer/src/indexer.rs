use std::time::Duration;

use async_trait::async_trait;
use bitcoincore_rpc::bitcoin::consensus;
use bitcoincore_rpc::bitcoin::{Address, Block, BlockHash, Network, ScriptBuf};
use bitcoincore_rpc::bitcoin::{Transaction, Txid};
use bitcoincore_rpc::json::{GetBlockResult, GetTxOutResult};
use bitcoincore_rpc::{Auth, RpcApi};
use chrono::{DateTime, Utc};
use log::{error, info};
use lru::LruCache;
use network_shared::{BlockUpdate, SocketTransport, UtxoUpdate, FINALITY_CONFIRMATIONS};
use reqwest::Client as HttpClient;
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::error::{IndexerError, Result as IndexerResult};
use crate::utils::{determine_script_type, extract_public_key};

/// Abstraction over Bitcoin RPC clients
#[async_trait]
pub trait BitcoinRpcClient {
    async fn get_block_count(&self) -> IndexerResult<u64>;
    async fn get_block_hash(&self, height: u64) -> IndexerResult<BlockHash>;
    async fn get_block(&self, block_hash: &BlockHash) -> IndexerResult<Block>;
    async fn get_block_info(&self, block_hash: &BlockHash) -> IndexerResult<GetBlockResult>;
    async fn get_tx_out(
        &self,
        txid: &Txid,
        vout: u32,
        include_mempool: Option<bool>,
    ) -> IndexerResult<Option<GetTxOutResult>>;
    async fn get_raw_transaction(
        &self,
        txid: &Txid,
        block_hash: Option<&BlockHash>,
    ) -> IndexerResult<Transaction>;
}

/// RPC client backed by bitcoincore-rpc
pub struct BitcoinCoreRpcClient {
    client: bitcoincore_rpc::Client,
}

impl BitcoinCoreRpcClient {
    pub fn new(rpc_url: &str, auth: Auth) -> Result<Self, bitcoincore_rpc::Error> {
        let client = bitcoincore_rpc::Client::new(rpc_url, auth)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl BitcoinRpcClient for BitcoinCoreRpcClient {
    async fn get_block_count(&self) -> IndexerResult<u64> {
        Ok(self.client.get_block_count()?)
    }

    async fn get_block_hash(&self, height: u64) -> IndexerResult<BlockHash> {
        Ok(self.client.get_block_hash(height)?)
    }

    async fn get_block(&self, block_hash: &BlockHash) -> IndexerResult<Block> {
        Ok(self.client.get_block(block_hash)?)
    }

    async fn get_block_info(&self, block_hash: &BlockHash) -> IndexerResult<GetBlockResult> {
        Ok(self.client.get_block_info(block_hash)?)
    }

    async fn get_tx_out(
        &self,
        txid: &Txid,
        vout: u32,
        include_mempool: Option<bool>,
    ) -> IndexerResult<Option<GetTxOutResult>> {
        Ok(self.client.get_tx_out(txid, vout, include_mempool)?)
    }

    async fn get_raw_transaction(
        &self,
        txid: &Txid,
        block_hash: Option<&BlockHash>,
    ) -> IndexerResult<Transaction> {
        Ok(self.client.get_raw_transaction(txid, block_hash)?)
    }
}

/// RPC client using external HTTP service
pub struct ExternalRpcClient {
    client: reqwest::Client,
    rpc_url: String,
    rpc_user: Option<String>,
    rpc_password: Option<String>,
}

impl ExternalRpcClient {
    pub fn new(rpc_url: String, rpc_user: Option<String>, rpc_password: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_url,
            rpc_user,
            rpc_password,
        }
    }

    async fn make_rpc_call(
        &self,
        method: &str,
        params: Vec<serde_json::Value>,
    ) -> IndexerResult<serde_json::Value> {
        let mut req = self.client.post(&self.rpc_url).json(&serde_json::json!({
            "jsonrpc": "1.0",
            "id": "1",
            "method": method,
            "params": params,
        }));

        if let Some(user) = &self.rpc_user {
            req = req.basic_auth(user, self.rpc_password.as_deref());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| IndexerError::RpcClientError(e.to_string()))?;
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| IndexerError::RpcClientError(e.to_string()))?;
        if let Some(error) = json.get("error") {
            if !error.is_null() {
                return Err(IndexerError::RpcClientError(format!("RPC error: {error}")));
            }
        }
        json
            .get("result")
            .cloned()
            .ok_or_else(|| IndexerError::RpcClientError("missing result".into()))
    }
}

#[async_trait]
impl BitcoinRpcClient for ExternalRpcClient {
    async fn get_block_count(&self) -> IndexerResult<u64> {
        let res = self.make_rpc_call("getblockcount", vec![]).await?;
        Ok(serde_json::from_value(res).map_err(|e| IndexerError::RpcClientError(e.to_string()))?)
    }

    async fn get_block_hash(&self, height: u64) -> IndexerResult<BlockHash> {
        let res = self
            .make_rpc_call("getblockhash", vec![serde_json::json!(height)])
            .await?;
        let hash_str: String =
            serde_json::from_value(res).map_err(|e| IndexerError::RpcClientError(e.to_string()))?;
        Ok(BlockHash::from_str(&hash_str)
            .map_err(|e| IndexerError::RpcClientError(e.to_string()))?)
    }

    async fn get_block(&self, block_hash: &BlockHash) -> IndexerResult<Block> {
        let res = self
            .make_rpc_call(
                "getblock",
                vec![
                    serde_json::json!(block_hash.to_string()),
                    serde_json::json!(0),
                ],
            )
            .await?;
        let hex: String =
            serde_json::from_value(res).map_err(|e| IndexerError::RpcClientError(e.to_string()))?;
        let bytes = hex::decode(hex).map_err(|e| IndexerError::RpcClientError(e.to_string()))?;
        Ok(consensus::deserialize(&bytes)
            .map_err(|e| IndexerError::RpcClientError(e.to_string()))?)
    }

    async fn get_block_info(&self, block_hash: &BlockHash) -> IndexerResult<GetBlockResult> {
        let res = self
            .make_rpc_call(
                "getblock",
                vec![
                    serde_json::json!(block_hash.to_string()),
                    serde_json::json!(1),
                ],
            )
            .await?;
        Ok(serde_json::from_value(res).map_err(|e| IndexerError::RpcClientError(e.to_string()))?)
    }

    async fn get_tx_out(
        &self,
        txid: &Txid,
        vout: u32,
        include_mempool: Option<bool>,
    ) -> IndexerResult<Option<GetTxOutResult>> {
        let mut params = vec![serde_json::json!(txid.to_string()), serde_json::json!(vout)];
        if let Some(include) = include_mempool {
            params.push(serde_json::json!(include));
        }
        let res = self.make_rpc_call("gettxout", params).await?;
        if res.is_null() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(res).map_err(|e| {
            IndexerError::RpcClientError(e.to_string())
        })?))
    }

    async fn get_raw_transaction(
        &self,
        txid: &Txid,
        block_hash: Option<&BlockHash>,
    ) -> IndexerResult<Transaction> {
        let mut params = vec![
            serde_json::json!(txid.to_string()),
            serde_json::json!(false),
        ];
        if let Some(hash) = block_hash {
            params.push(serde_json::json!(hash.to_string()));
        }
        let res = self.make_rpc_call("getrawtransaction", params).await?;
        let hex: String =
            serde_json::from_value(res).map_err(|e| IndexerError::RpcClientError(e.to_string()))?;
        let bytes = hex::decode(hex).map_err(|e| IndexerError::RpcClientError(e.to_string()))?;
        Ok(consensus::deserialize(&bytes)
            .map_err(|e| IndexerError::RpcClientError(e.to_string()))?)
    }
}

/// Configuration for creating a [`BitcoinIndexer`].
pub struct IndexerConfig {
    pub network: Network,
    pub rpc_user: String,
    pub rpc_password: String,
    pub rpc_host: String,
    pub connection_type: String,
    pub socket_path: String,
    pub start_height: i32,
    pub max_blocks_per_batch: i32,
    pub utxo_url: String,
}

/// The main Bitcoin indexer that processes blocks and transactions
pub struct BitcoinIndexer {
    rpc_client: Arc<dyn BitcoinRpcClient + Send + Sync>,
    network: Network,
    socket_transport: SocketTransport,
    last_processed_height: i32,
    start_height: i32,
    max_blocks_per_batch: i32,
    utxo_url: String,
    http_client: HttpClient,
    pub watched_addresses: Arc<RwLock<HashSet<Address>>>,
    tx_cache: Mutex<LruCache<Txid, Transaction>>,
    utxo_cache: Mutex<LruCache<(Txid, u32), Option<UtxoUpdate>>>,
}

impl BitcoinIndexer {
    /// Creates a new BitcoinIndexer instance
    pub async fn new(config: IndexerConfig) -> IndexerResult<Self> {
        let rpc_client: Arc<dyn BitcoinRpcClient + Send + Sync> =
            match config.connection_type.as_str() {
                "bitcoincore" => {
                    let auth = if config.rpc_user.is_empty() && config.rpc_password.is_empty() {
                        Auth::None
                    } else {
                        Auth::UserPass(config.rpc_user.clone(), config.rpc_password.clone())
                    };
                    Arc::new(
                        BitcoinCoreRpcClient::new(&config.rpc_host, auth)
                            .map_err(|e| IndexerError::RpcClientError(e.to_string()))?,
                    )
                }
                "external" => {
                    let user = if config.rpc_user.is_empty() {
                        None
                    } else {
                        Some(config.rpc_user.clone())
                    };
                    let pass = if config.rpc_password.is_empty() {
                        None
                    } else {
                        Some(config.rpc_password.clone())
                    };
                    Arc::new(ExternalRpcClient::new(config.rpc_host.clone(), user, pass))
                }
                other => {
                    return Err(IndexerError::InvalidConfiguration(format!(
                        "Unknown connection type: {}",
                        other
                    )))
                }
            };

        // Validate start block
        let chain_height = rpc_client
            .get_block_count()
            .await
            .map_err(|e| IndexerError::RpcClientError(e.to_string()))?
            as i32;
        if config.start_height < 0 || config.start_height > chain_height {
            return Err(IndexerError::InvalidStartBlock(format!(
                "Start block {} is invalid. Chain height is {}",
                config.start_height, chain_height
            )));
        }

        let socket_transport = SocketTransport::new(&config.socket_path);

        let http_client = HttpClient::new();

        Ok(Self {
            rpc_client,
            network: config.network,
            socket_transport,
            last_processed_height: config.start_height - 1,
            start_height: config.start_height,
            max_blocks_per_batch: config.max_blocks_per_batch,
            utxo_url: config.utxo_url,
            http_client,
            watched_addresses: Arc::new(RwLock::new(HashSet::new())),
            tx_cache: Mutex::new(LruCache::new(NonZeroUsize::new(10_000).unwrap())),
            utxo_cache: Mutex::new(LruCache::new(NonZeroUsize::new(50_000).unwrap())),
        })
    }

    /// Returns a clone of the watched address set
    pub fn watched_addresses(&self) -> Arc<RwLock<HashSet<Address>>> {
        Arc::clone(&self.watched_addresses)
    }

    async fn get_tracked_utxo(&self, txid: &str, vout: u32) -> Option<UtxoUpdate> {
        let url = format!("{}/utxo/{}/{}", self.utxo_url, txid, vout);
        if let Ok(resp) = self.http_client.get(url).send().await {
            if resp.status().is_success() {
                if let Ok(value) = resp.json::<JsonValue>().await {
                    if let Some(utxo_val) = value.get("utxo") {
                        if let Ok(utxo) = serde_json::from_value::<UtxoUpdate>(utxo_val.clone()) {
                            return Some(utxo);
                        }
                    }
                }
            }
        }
        None
    }

    async fn fetch_tracked_utxos_batch(
        &self,
        outpoints: &[(Txid, u32)],
    ) -> HashMap<(Txid, u32), Option<UtxoUpdate>> {
        let mut results = HashMap::new();
        for chunk in outpoints.chunks(50) {
            let mut join_set = tokio::task::JoinSet::new();
            for (txid, vout) in chunk.iter() {
                let client = self.http_client.clone();
                let base = self.utxo_url.clone();
                let txid_c = *txid;
                let vout_c = *vout;
                join_set.spawn(async move {
                    let url = format!("{}/utxo/{}/{}", base, txid_c, vout_c);
                    if let Ok(resp) = client.get(url).send().await {
                        if resp.status().is_success() {
                            if let Ok(value) = resp.json::<JsonValue>().await {
                                if let Some(utxo_val) = value.get("utxo") {
                                    if let Ok(utxo) =
                                        serde_json::from_value::<UtxoUpdate>(utxo_val.clone())
                                    {
                                        return ((txid_c, vout_c), Some(utxo));
                                    }
                                }
                            }
                        }
                    }
                    ((txid_c, vout_c), None)
                });

                if join_set.len() >= 10 {
                    if let Some(Ok((key, val))) = join_set.join_next().await {
                        results.insert(key, val);
                    }
                }
            }
            while let Some(Ok((key, val))) = join_set.join_next().await {
                results.insert(key, val);
            }
        }
        results
    }

    async fn get_transaction_cached(&self, txid: &Txid) -> IndexerResult<Transaction> {
        if let Some(tx) = self.tx_cache.lock().await.get(txid).cloned() {
            return Ok(tx);
        }
        let tx = self.rpc_client.get_raw_transaction(txid, None).await?;
        self.tx_cache.lock().await.put(*txid, tx.clone());
        Ok(tx)
    }

    async fn get_utxo_cached(&self, txid: &Txid, vout: u32) -> IndexerResult<Option<UtxoUpdate>> {
        let key = (*txid, vout);
        if let Some(cached) = self.utxo_cache.lock().await.get(&key).cloned() {
            return Ok(cached);
        }

        if let Some(utxo) = self.get_tracked_utxo(&txid.to_string(), vout).await {
            self.utxo_cache.lock().await.put(key, Some(utxo.clone()));
            return Ok(Some(utxo));
        }

        if let Ok(Some(txout)) = self.rpc_client.get_tx_out(txid, vout, None).await {
            if let Ok(script_bytes) = hex::decode(txout.script_pub_key.hex) {
                let script = ScriptBuf::from_bytes(script_bytes);
                if let Ok(address) = Address::from_script(&script, self.network) {
                    let utxo = UtxoUpdate {
                        id: format!("{}:{}", txid, vout),
                        address: address.to_string(),
                        public_key: None,
                        txid: txid.to_string(),
                        vout: vout as i32,
                        amount: txout.value.to_sat() as i64,
                        script_pub_key: hex::encode(script.as_bytes()),
                        script_type: determine_script_type(script.clone()),
                        created_at: Utc::now(),
                        block_height: 0,
                        spent_txid: None,
                        spent_at: None,
                        spent_block: None,
                    };
                    self.utxo_cache.lock().await.put(key, Some(utxo.clone()));
                    return Ok(Some(utxo));
                }
            }
        }

        if let Ok(prev_tx) = self.get_transaction_cached(txid).await {
            let prev_output = &prev_tx.output[vout as usize];
            if let Ok(address) = Address::from_script(&prev_output.script_pubkey, self.network) {
                let utxo = UtxoUpdate {
                    id: format!("{}:{}", txid, vout),
                    address: address.to_string(),
                    public_key: None,
                    txid: txid.to_string(),
                    vout: vout as i32,
                    amount: prev_output.value.to_sat() as i64,
                    script_pub_key: hex::encode(prev_output.script_pubkey.as_bytes()),
                    script_type: determine_script_type(prev_output.script_pubkey.clone()),
                    created_at: Utc::now(),
                    block_height: 0,
                    spent_txid: None,
                    spent_at: None,
                    spent_block: None,
                };
                self.utxo_cache.lock().await.put(key, Some(utxo.clone()));
                return Ok(Some(utxo));
            }
        }

        self.utxo_cache.lock().await.put(key, None);
        Ok(None)
    }

    /// Gets block data for a given block hash and process transactions
    async fn get_block_data(&self, block_hash: &BlockHash) -> IndexerResult<BlockUpdate> {
        let block = self.rpc_client.get_block(block_hash).await?;
        let block_info = self.rpc_client.get_block_info(block_hash).await?;

        let timestamp = DateTime::<Utc>::from_timestamp(block.header.time as i64, 0)
            .ok_or(IndexerError::InvalidTimestamp)?;

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
    ) -> IndexerResult<Vec<UtxoUpdate>> {
        let start_time = std::time::Instant::now();

        let watch_set = self.watched_addresses.read().await.clone();
        if watch_set.is_empty() {
            return Ok(Vec::new());
        }

        let mut utxo_updates = Vec::new();
        let mut input_count = 0usize;
        let mut output_count = 0usize;
        let mut needed_outpoints: HashSet<(Txid, u32)> = HashSet::new();

        for (tx_index, tx) in block.txdata.iter().enumerate() {
            let is_coinbase = tx_index == 0;

            for (vout, output) in tx.output.iter().enumerate() {
                output_count += 1;
                if let Ok(address) = Address::from_script(&output.script_pubkey, self.network) {
                    if watch_set.contains(&address) {
                        let script_type = determine_script_type(output.script_pubkey.clone());
                        let utxo = UtxoUpdate {
                            id: format!("{}:{}", tx.txid(), vout),
                            address: address.to_string(),
                            public_key: None,
                            txid: tx.txid().to_string(),
                            vout: vout as i32,
                            amount: output.value.to_sat() as i64,
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
            }

            if !is_coinbase {
                for input in tx.input.iter() {
                    if !input.previous_output.is_null() {
                        needed_outpoints
                            .insert((input.previous_output.txid, input.previous_output.vout));
                    }
                }
            }
        }

        let fetched = self
            .fetch_tracked_utxos_batch(&needed_outpoints.iter().cloned().collect::<Vec<_>>())
            .await;

        for (tx_index, tx) in block.txdata.iter().enumerate() {
            let is_coinbase = tx_index == 0;
            for input in tx.input.iter() {
                input_count += 1;
                if input.previous_output.is_null() {
                    if !is_coinbase {
                        error!("Found null previous output in non-coinbase transaction");
                    } else {
                        info!("Skipping coinbase transaction input");
                    }
                    continue;
                }

                let key = (input.previous_output.txid, input.previous_output.vout);
                let mut utxo_opt = fetched.get(&key).cloned().flatten();
                if utxo_opt.is_none() {
                    utxo_opt = self.get_utxo_cached(&key.0, key.1).await?;
                }

                if let Some(utxo) = utxo_opt {
                    if let Ok(addr_unchecked) = Address::from_str(&utxo.address) {
                        if let Ok(address) = addr_unchecked.require_network(self.network) {
                            if !watch_set.contains(&address) {
                                continue;
                            }

                            let spent_utxo = UtxoUpdate {
                                id: format!("{}:{}", input.previous_output.txid, key.1),
                                address: address.to_string(),
                                public_key: extract_public_key(&input.witness),
                                txid: key.0.to_string(),
                                vout: key.1 as i32,
                                amount: utxo.amount,
                                script_pub_key: utxo.script_pub_key,
                                script_type: utxo.script_type,
                                created_at: block_time,
                                block_height: height,
                                spent_txid: Some(tx.txid().to_string()),
                                spent_at: Some(block_time),
                                spent_block: Some(height),
                            };
                            utxo_updates.push(spent_utxo);
                        }
                    }
                }
            }
        }

        info!(
            "Processed block {} with {} transactions ({} inputs, {} outputs) in {:?}",
            height,
            block.txdata.len(),
            input_count,
            output_count,
            start_time.elapsed()
        );

        Ok(utxo_updates)
    }

    /// Sends a block update to the socket transport
    async fn send_block_update(&self, update: &BlockUpdate) -> IndexerResult<()> {
        self.socket_transport.send_update(update).await?;
        Ok(())
    }

    /// Processes new blocks that have been added to the blockchain
    async fn process_new_blocks(&mut self) -> IndexerResult<i32> {
        let current_height = self.rpc_client.get_block_count().await? as i32;
        if current_height <= self.last_processed_height {
            return Ok(0);
        }

        // First, check for reorgs
        if let Ok(true) = self.check_for_reorg().await {
            // A reorg was detected and handled, exit this cycle
            return Ok(0);
        }

        let batch_limit = if self.network == Network::Bitcoin {
            1
        } else {
            self.max_blocks_per_batch
        };

        let blocks_to_process =
            std::cmp::min(current_height - self.last_processed_height, batch_limit);

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
            let block_hash = self.rpc_client.get_block_hash(height as u64).await?;
            let block_data = self.get_block_data(&block_hash).await?;
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

    async fn check_for_reorg(&mut self) -> IndexerResult<bool> {
        let current_height = self.rpc_client.get_block_count().await? as i32;

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
            let chain_hash = self.rpc_client.get_block_hash(height as u64).await?;
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
    async fn find_fork_point(&self, start_height: i32, min_height: i32) -> IndexerResult<i32> {
        let mut height = start_height - 1;

        while height >= min_height {
            let chain_hash = self.rpc_client.get_block_hash(height as u64).await?;
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
    pub async fn run(&mut self, poll_interval: Duration) -> IndexerResult<()> {
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
