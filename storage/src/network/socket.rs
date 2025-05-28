use std::{fs, io, path::Path, sync::Arc};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

async fn handle_socket_connection(stream: UnixStream, db: Arc<UtxoDatabase>) -> io::Result<()> {
    let mut stream = stream;

    // Read size header
    let mut size_buf = [0u8; 4];
    if let Err(e) = stream.read_exact(&mut size_buf).await {
        // Handle gracefully disconnects or short reads
        return Err(std::io::Error::other(format!(
            "Failed to read message size: {}",
            e
        )));
    }

    let size = u32::from_le_bytes(size_buf) as usize;

    // Read data
    let mut data = vec![0u8; size];
    if let Err(e) = stream.read_exact(&mut data).await {
        return Err(std::io::Error::other(format!(
            "Failed to read message data: {}",
            e
        )));
    }

    // Deserialize the message, handle deserialization errors gracefully
    let message = match bincode::deserialize::<network_shared::NetworkMessage>(&data) {
        Ok(msg) => msg,
        Err(e) => {
            return Err(std::io::Error::other(format!(
                "Failed to deserialize message: {}",
                e
            )));
        }
    };

    match message {
        network_shared::NetworkMessage::BlockUpdate(update) => {
            info!(height = update.height, "Received block update via socket");

            let height = update.height;

            // Store block information
            if let Err(e) = db.store_block(height, &update.hash, update.timestamp) {
                error!("Error storing block info: {}", e);
            }

            // Process the update
            if let Err(e) = db.process_block(update).await {
                error!("Error processing block: {}", e);
            }

            // Send acknowledgment
            let ack = network_shared::NetworkMessage::BlockProcessedAck { height };
            let ack_data = bincode::serialize(&ack)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            let ack_size = ack_data.len() as u32;
            stream.write_all(&ack_size.to_le_bytes()).await?;
            stream.write_all(&ack_data).await?;
        }
        network_shared::NetworkMessage::ReorgEvent { fork_height } => {
            info!(fork_height, "Received reorg event via socket");

            // Handle the reorg
            if let Err(e) = db.revert_to_height(fork_height).await {
                error!("Error handling reorg: {}", e);
            }
        }
        network_shared::NetworkMessage::GetBlockHash { height } => {
            info!(height, "Received block hash request");

            // Get the block hash
            let hash_result = db.get_block_hash(height);

            let response = match hash_result {
                Ok(hash) => network_shared::NetworkMessage::BlockHashResponse { hash },
                Err(_) => network_shared::NetworkMessage::BlockHashResponse {
                    hash: String::new(),
                },
            };

            // Send the response
            let response_data = bincode::serialize(&response)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

            let response_size = response_data.len() as u32;
            stream.write_all(&response_size.to_le_bytes()).await?;
            stream.write_all(&response_data).await?;
        }
        network_shared::NetworkMessage::UpdateFinalityStatus { current_height } => {
            info!(
                "Received finality status update request for height {}",
                current_height
            );

            if let Err(e) = db.update_finality_status(current_height).await {
                error!("Error updating finality status: {}", e);
            }
        }
        _ => {
            error!("Received unknown message type");
        }
    }

    Ok(())
}
