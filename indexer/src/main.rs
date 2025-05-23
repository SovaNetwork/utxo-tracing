mod api;
mod error;
mod indexer;
mod utils;

use std::{error::Error, net::IpAddr, time::Duration};

use bitcoincore_rpc::bitcoin::Network;

use clap::Parser;
use reqwest::Url;
use tokio::task;

use crate::api::{run_server, ApiState};
use crate::indexer::{BitcoinIndexer, IndexerConfig};

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

fn validate_enclave_url(url_str: &str) -> Result<(), String> {
    let url = Url::parse(url_str).map_err(|_| "Invalid ENCLAVE_URL".to_string())?;
    let host = url.host_str().ok_or("ENCLAVE_URL missing host")?;

    if host == "localhost" {
        return Ok(());
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip.is_loopback() {
            return Ok(());
        }
        match ip {
            IpAddr::V4(v4) => {
                if v4.is_private() {
                    return Ok(());
                }
            }
            IpAddr::V6(v6) => {
                // Accept IPv6 Unique Local Addresses (fc00::/7) on stable Rust
                let first_octet = v6.octets()[0];
                if (first_octet & 0xFE) == 0xFC {
                    return Ok(());
                }
            }
        }
    }

    Err("ENCLAVE_URL must point to localhost or private network".to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let args = Args::parse();

    let config = IndexerConfig {
        network: args.parse_network(),
        rpc_user: args.rpc_user.clone(),
        rpc_password: args.rpc_password.clone(),
        rpc_host: args.rpc_host.clone(),
        rpc_port: args.rpc_port,
        socket_path: args.socket_path.clone(),
        start_height: args.start_height,
        max_blocks_per_batch: args.max_blocks_per_batch,
    };
    let mut indexer = BitcoinIndexer::new(config)?;

    let enclave_url = std::env::var("ENCLAVE_URL").expect("ENCLAVE_URL must be set");
    validate_enclave_url(&enclave_url).expect("Invalid ENCLAVE_URL");
    let utxo_url =
        std::env::var("UTXO_URL").unwrap_or_else(|_| "http://network-utxos:5557".to_string());
    let enclave_api_key = std::env::var("ENCLAVE_API_KEY").expect("ENCLAVE_API_KEY must be set");
    if enclave_api_key.trim().is_empty() {
        panic!("ENCLAVE_API_KEY must be set");
    }

    let indexer_api_key = std::env::var("INDEXER_API_KEY").expect("INDEXER_API_KEY must be set");
    if indexer_api_key.trim().is_empty() {
        panic!("INDEXER_API_KEY must be set");
    }

    let api_state = ApiState {
        watched_addresses: indexer.watched_addresses(),
        network: args.parse_network(),
        enclave_url,
        utxo_url,
        enclave_api_key,
        indexer_api_key,
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
