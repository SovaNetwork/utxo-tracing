mod database;
mod datasources;
mod error;
mod models;
mod network;

use actix_web::{middleware::Logger, web, App, HttpServer};
use bitcoin::Network;
use clap::Parser;
use tracing::{error, info};

use database::UtxoDatabase;
use datasources::create_datasource;
use network::{socket::run_socket_server, AppState};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Host address to bind to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to listen on
    #[arg(long, default_value = "5557")]
    port: u16,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Datasource type
    #[arg(long, default_value = "sqlite")]
    datasource: String,

    /// Socket path for receiving updates
    #[arg(long, default_value = "/tmp/network-utxos.sock")]
    socket_path: String,

    /// Bitcoin network (mainnet, testnet, regtest, signet)
    #[arg(long, default_value = "regtest")]
    network: String,
}

impl Args {
    /// Parse the network string into a [`Network`] enum
    fn parse_network(&self) -> Network {
        match self.network.to_lowercase().as_str() {
            "mainnet" | "bitcoin" => Network::Bitcoin,
            "testnet" => Network::Testnet,
            "regtest" => Network::Regtest,
            "signet" => Network::Signet,
            other => panic!("Unsupported network: {}", other),
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    // Setup tracing
    tracing_subscriber::fmt()
        .with_env_filter(&args.log_level)
        .init();

    info!("Starting UTXO tracking service");

    // Create datasource based on arguments
    let datasource = match create_datasource(&args.datasource) {
        Ok(ds) => ds,
        Err(e) => {
            error!("Failed to create datasource: {}", e);
            return Err(std::io::Error::other(format!(
                "Failed to create datasource: {}",
                e
            )));
        }
    };

    // Initialize datasource
    if let Err(e) = datasource.setup() {
        error!("Failed to setup datasource: {}", e);
        return Err(std::io::Error::other(format!(
            "Failed to setup datasource: {}",
            e
        )));
    }

    // Create database
    let db = UtxoDatabase::new(datasource);

    // Determine the Bitcoin network this instance should operate on
    let network = args.parse_network();
    let state = web::Data::new(AppState {
        db: db.clone(),
        network,
    });

    // Run Unix socket server in the background
    let socket_path = args.socket_path.clone();
    let db_clone = db.clone();
    tokio::spawn(async move {
        if let Err(e) = run_socket_server(db_clone, &socket_path).await {
            error!("Socket server error: {}", e);
        }
    });

    // Configure and start HTTP server
    let bind_addr = format!("{}:{}", args.host, args.port);
    info!("Starting HTTP server on {}", bind_addr);

    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(state.clone())
            .configure(network::http::configure_routes)
    })
    .bind(bind_addr)?
    .run()
    .await
}
