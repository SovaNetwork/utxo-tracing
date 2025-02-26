use std::error::Error;
use std::fmt;
use std::time::Duration;

use bitcoincore_rpc::bitcoin::{Address, Network};
use bitcoincore_rpc::{bitcoin::Block, bitcoin::BlockHash, Auth, Client, RpcApi};
use chrono::{DateTime, Utc};
use clap::Parser;
use log::{error, info};
use network_shared::{BlockUpdate, SocketTransport, TransportError, UtxoUpdate};
use tokio;

// Custom error types
#[derive(Debug)]
pub enum IndexerError {
    BitcoinRPC(bitcoincore_rpc::Error),
    Network(TransportError),
    InvalidTimestamp,
    ScriptParsing(String),
    InvalidStartBlock(String),
}

impl fmt::Display for IndexerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexerError::BitcoinRPC(e) => write!(f, "Bitcoin RPC error: {}", e),
            IndexerError::Network(e) => write!(f, "Network error: {}", e),
            IndexerError::InvalidTimestamp => write!(f, "Invalid timestamp"),
            IndexerError::ScriptParsing(msg) => write!(f, "Script parsing error: {}", msg),
            IndexerError::InvalidStartBlock(msg) => write!(f, "Invalid start block: {}", msg),
        }
    }
}

impl Error for IndexerError {}

impl From<bitcoincore_rpc::Error> for IndexerError {
    fn from(err: bitcoincore_rpc::Error) -> IndexerError {
        IndexerError::BitcoinRPC(err)
    }
}

impl From<TransportError> for IndexerError {
    fn from(err: TransportError) -> IndexerError {
        IndexerError::Network(err)
    }
}

type Result<T> = std::result::Result<T, IndexerError>;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value = "/tmp/network-utxos.sock")]
    socket_path: String,

    #[arg(long, default_value = "user")]
    rpc_user: String,

    #[arg(long, default_value = "password")]
    rpc_password: String,

    #[arg(long, default_value = "localhost")]
    rpc_host: String,

    #[arg(long, default_value = "18443")]
    rpc_port: u16,

    #[arg(long, default_value = "0")]
    start_height: i32,
    
    #[arg(long, default_value = "500", help = "Polling interval in milliseconds")]
    polling_rate: u64,
    
    #[arg(long, default_value = "200", help = "Maximum blocks to process in a batch")]
    max_blocks_per_batch: i32,
}

struct BitcoinIndexer {
    rpc_client: Client,
    network: Network,
    socket_transport: SocketTransport,
    last_processed_height: i32,
    start_height: i32,
    max_blocks_per_batch: i32,
}

impl BitcoinIndexer {
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

    async fn send_block_update(&self, update: &BlockUpdate) -> Result<()> {
        self.socket_transport.send_update(update).await?;
        Ok(())
    }

    async fn process_new_blocks(&mut self) -> Result<i32> {
        let current_height = self.rpc_client.get_block_count()? as i32;
        if current_height <= self.last_processed_height {
            return Ok(0);
        }

        let blocks_to_process =
            std::cmp::min(current_height - self.last_processed_height, self.max_blocks_per_batch);

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

        info!(
            "Successfully processed blocks up to height {}",
            self.last_processed_height
        );

        Ok(blocks_to_process)
    }

    pub async fn run(&mut self, poll_interval: Duration) -> Result<()> {
        info!(
            "Starting Bitcoin UTXO indexer from block {} with polling interval of {:?}",
            self.start_height,
            poll_interval
        );

        loop {
            if let Err(e) = self.process_new_blocks().await {
                error!("Error in indexer loop: {}", e);
            }

            tokio::time::sleep(poll_interval).await;
        }
    }
}

fn determine_script_type(script: bitcoincore_rpc::bitcoin::ScriptBuf) -> String {
    if script.is_p2pkh() {
        "P2PKH".to_string()
    } else if script.is_p2sh() {
        "P2SH".to_string()
    } else if script.is_v0_p2wpkh() {
        "P2WPKH".to_string()
    } else if script.is_v0_p2wsh() {
        "P2WSH".to_string()
    } else if script.is_op_return() {
        "OP_RETURN".to_string()
    } else if script.is_witness_program() {
        "WITNESS".to_string()
    } else {
        error!("Unknown script type: {}", hex::encode(script.as_bytes()));
        "UNKNOWN".to_string()
    }
}

fn extract_address(
    script: bitcoincore_rpc::bitcoin::ScriptBuf,
    network: Network,
) -> Result<String> {
    Address::from_script(&script, network)
        .map(|addr| addr.to_string())
        .map_err(|_| IndexerError::ScriptParsing("Failed to parse address from script".to_string()))
}

fn extract_public_key(witness: &bitcoincore_rpc::bitcoin::Witness) -> Option<String> {
    if witness.is_empty() {
        return None;
    }
    witness.iter().nth(1).map(|pk| hex::encode(pk))
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn Error>> {
    env_logger::init();

    let args = Args::parse();

    let mut indexer = BitcoinIndexer::new(
        Network::Regtest,
        &args.rpc_user,
        &args.rpc_password,
        &args.rpc_host,
        args.rpc_port,
        &args.socket_path,
        args.start_height,
        args.max_blocks_per_batch,
    )?;

    indexer.run(Duration::from_millis(args.polling_rate)).await?;

    Ok(())
}