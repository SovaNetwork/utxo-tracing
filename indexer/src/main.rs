mod error;
mod indexer;
mod utils;
mod api;

use std::{error::Error, time::Duration};

use bitcoincore_rpc::bitcoin::Network;

use clap::Parser;
use tokio::task;

use crate::indexer::BitcoinIndexer;
use crate::api::{run_server, ApiState};

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

    #[arg(long, default_value = "0.0.0.0", help = "API server host")]
    pub api_host: String,

    #[arg(long, default_value = "3031", help = "API server port")]
    pub api_port: u16,
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
    env_logger::init();

    let args = Args::parse();

    let network = args.parse_network();
    let mut indexer = BitcoinIndexer::new(
        network,
        &args.rpc_user,
        &args.rpc_password,
        &args.rpc_host,
        args.rpc_port,
        &args.socket_path,
        args.start_height,
        args.max_blocks_per_batch,
    )?;

    let enclave_url = std::env::var("ENCLAVE_URL").expect("ENCLAVE_URL must be set");

    let api_state = ApiState {
        watched_addresses: indexer.watched_addresses(),
        network,
        enclave_url,
    };

    // Run HTTP server in background
    let api_host = args.api_host.clone();
    let api_port = args.api_port;
    task::spawn(async move {
        run_server(&api_host, api_port, api_state).await;
    });

    indexer
        .run(Duration::from_millis(args.polling_rate))
        .await?;

    Ok(())
}
