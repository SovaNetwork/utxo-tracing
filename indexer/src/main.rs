mod error;
mod indexer;
mod utils;

use std::{error::Error, time::Duration};

use bitcoincore_rpc::bitcoin::Network;

use clap::Parser;

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
async fn main() -> Result<(), Box<dyn Error>> {
    let env = env_logger::Env::default().filter_or("RUST_LOG", "warn"); // Only show warnings and errors
    env_logger::Builder::from_env(env)
        .format_timestamp_millis()
        .init();

    let args = Args::parse();

    let mut indexer = BitcoinIndexer::new(
        args.parse_network(),
        &args.rpc_user,
        &args.rpc_password,
        &args.rpc_host,
        args.rpc_port,
        &args.socket_path,
        args.start_height,
        args.max_blocks_per_batch,
    )?;

    indexer
        .run(Duration::from_millis(args.polling_rate))
        .await?;

    Ok(())
}
