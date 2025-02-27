use std::{fs, io, path::Path, sync::Arc};

use tokio::io::AsyncReadExt;
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info};

use crate::database::UtxoDatabase;

// Socket server that listens for Unix socket connections
pub async fn run_socket_server(db: Arc<UtxoDatabase>, socket_path: &str) -> io::Result<()> {
    // Remove socket file if it exists
    let socket_path = Path::new(socket_path);
    if socket_path.exists() {
        fs::remove_file(socket_path)?;
    }

    // Create Unix socket listener
    let listener = UnixListener::bind(socket_path)?;
    info!("Socket server listening on {}", socket_path.display());

    // Accept connections
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let db_clone = db.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_socket_connection(stream, db_clone).await {
                        error!("Error handling socket connection: {}", e);
                    }
                });
            }
            Err(e) => {
                error!("Error accepting connection: {}", e);
            }
        }
    }
}

// Handle incoming Unix socket connections
async fn handle_socket_connection(stream: UnixStream, db: Arc<UtxoDatabase>) -> io::Result<()> {
    let mut stream = stream;

    // Read size header
    let mut size_buf = [0u8; 4];
    stream.read_exact(&mut size_buf).await?;
    let size = u32::from_le_bytes(size_buf) as usize;

    // Read data
    let mut data = vec![0u8; size];
    stream.read_exact(&mut data).await?;

    // Deserialize
    let update: network_shared::BlockUpdate = bincode::deserialize(&data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    info!(height = update.height, "Received block update via socket");

    // Process the update
    if let Err(e) = db.process_block(update).await {
        error!("Error processing block: {}", e);
    }

    Ok(())
}
