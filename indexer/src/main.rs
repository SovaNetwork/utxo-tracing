mod benchmark;
mod error;
mod indexer;
mod utils;

use std::{sync::Arc, time::Duration};

use bitcoincore_rpc::bitcoin::Network;

use clap::Parser;
use log::{error, info};
use tokio::sync::Semaphore;

use crate::indexer::BitcoinIndexer;

/// Command line arguments for the Bitcoin indexer
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[arg(long, default_value = "/tmp/network-utxos.sock")]
    pub socket_path: String,

    #[arg(
        long,
        default_value = "regtest",
        help = "Bitcoin network (mainnet, testnet, regtest, signet)"
    )]
    pub network: String,

    #[arg(long, default_value = "user")]
    pub rpc_user: String,

    #[arg(long, default_value = "password")]
    pub rpc_password: String,

    #[arg(long, default_value = "localhost")]
    pub rpc_host: String,

    #[arg(long, default_value = "18443")]
    pub rpc_port: u16,

    #[arg(long, default_value = "0")]
    pub start_height: i32,

    #[arg(long, default_value = "500", help = "Polling interval in milliseconds")]
    pub polling_rate: u64,

    #[arg(
        long,
        default_value = "200",
        help = "Maximum blocks to process in a batch"
    )]
    pub max_blocks_per_batch: i32,

    /// Configure rayon thread pool size
    #[arg(long, help = "Configure the number of threads for rayon thread pool")]
    pub thread_count: Option<usize>,

    /// Configure the number of concurrent RPC calls
    #[arg(
        long,
        default_value = "8",
        help = "Maximum number of concurrent RPC calls"
    )]
    pub max_concurrent_rpc: usize,

    /// Enable benchmark mode
    #[arg(long, help = "Run in benchmark mode")]
    pub benchmark: bool,

    /// Number of blocks to process in benchmark mode
    #[arg(
        long,
        default_value = "1000",
        help = "Number of blocks to process in benchmark mode"
    )]
    pub benchmark_blocks: i32,

    /// Batch size for benchmark mode
    #[arg(
        long,
        help = "Batch size to use in benchmark mode (overrides max_blocks_per_batch)"
    )]
    pub benchmark_batch_size: Option<i32>,
}

impl Args {
    /// Parse the network string into a Bitcoin Network enum
    pub fn parse_network(&self) -> Network {
        match self.network.to_lowercase().as_str() {
            "mainnet" => Network::Bitcoin,
            "regtest" => Network::Regtest,
            "signet" => Network::Signet,
            "testnet" => Network::Testnet,
            _ => panic!("Unsupported network: {}", self.network),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<std::io::Error>> {
    // Set logging level to INFO to see more details, especially for benchmarking
    let env = env_logger::Env::default().filter_or("RUST_LOG", "info");
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .init();

    let args = Args::parse();

    // Configure rayon thread pool if specified
    if let Some(thread_count) = args.thread_count {
        info!(
            "Configuring rayon thread pool with {} threads",
            thread_count
        );
        rayon::ThreadPoolBuilder::new()
            .num_threads(thread_count)
            .build_global()
            .unwrap();
    }

    info!("Initializing Bitcoin UTXO indexer");

    // Create the semaphore with the requested max_concurrent_rpc value
    let rpc_semaphore = Arc::new(Semaphore::new(args.max_concurrent_rpc));
    info!(
        "RPC call limit set to {} concurrent calls",
        args.max_concurrent_rpc
    );

    let mut indexer = match BitcoinIndexer::new(
        args.parse_network(),
        &args.rpc_user,
        &args.rpc_password,
        &args.rpc_host,
        &args.rpc_port,
        &args.socket_path,
        args.start_height,
        args.max_blocks_per_batch,
        rpc_semaphore, // Pass the semaphore to the constructor
    ) {
        Ok(indexer) => indexer,
        Err(e) => {
            error!("Failed to initialize indexer: {}", e);
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to initialize indexer: {}", e),
            )));
        }
    };

    // Run in benchmark mode if specified
    if args.benchmark {
        info!("Running in benchmark mode");
        match indexer
            .benchmark(args.benchmark_blocks, args.benchmark_batch_size)
            .await
        {
            Ok(result) => {
                info!("Benchmark results:");
                info!("{}", result);

                // Save detailed benchmark results to file
                let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
                let filename = format!("benchmark_results_{}.txt", timestamp);

                let result_str = format!(
                    "Benchmark Results\n\
                     -----------------\n\
                     Timestamp: {}\n\
                     Blocks processed: {}\n\
                     Duration: {:.2} seconds\n\
                     Block processing rate: {:.2} blocks/sec\n\
                     UTXOs processed: {}\n\
                     UTXO processing rate: {:.2} UTXOs/sec\n\
                     Memory usage: {:.2} MB\n\
                     Network: {:?}\n\
                     Thread count: {}\n\
                     Batch size: {}\n\
                     Max concurrent RPC: {}\n",
                    timestamp,
                    result.blocks_processed,
                    result.duration.as_secs_f64(),
                    result.blocks_per_second,
                    result.utxos_processed,
                    result.utxos_per_second,
                    result.memory_usage as f64 / (1024.0 * 1024.0),
                    args.network,
                    rayon::current_num_threads(),
                    args.benchmark_batch_size
                        .unwrap_or(args.max_blocks_per_batch),
                    args.max_concurrent_rpc
                );

                if let Err(e) = std::fs::write(&filename, result_str) {
                    error!("Failed to write benchmark results to file: {}", e);
                } else {
                    info!("Benchmark results saved to {}", filename);
                }
            }
            Err(e) => {
                error!("Benchmark failed: {}", e);
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Benchmark error: {}", e),
                )));
            }
        }

        return Ok(());
    }

    if let Err(e) = indexer.run(Duration::from_millis(args.polling_rate)).await {
        error!("Indexer exited with error: {}", e);

        // Try to save state even if there was an error
        if let Err(save_err) = indexer.save_state_on_shutdown().await {
            error!("Failed to save state on error shutdown: {}", save_err);
        } else {
            info!("Successfully saved state during error shutdown");
        }

        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Indexer error: {}", e),
        )));
    }

    info!("Indexer shutdown complete");
    Ok(())
}
