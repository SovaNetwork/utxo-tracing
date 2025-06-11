mod constants;
mod error;

pub use constants::FINALITY_CONFIRMATIONS;
pub use error::TransportError;

use chrono::{DateTime, Utc};
use error::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum NetworkMessage {
    BlockUpdate(BlockUpdate),
    ReorgEvent { fork_height: i32 },
    GetBlockHash { height: i32 },
    BlockHashResponse { hash: String },
    UpdateFinalityStatus { current_height: i32 },
    BlockProcessedAck { height: i32 },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BlockUpdate {
    pub height: i32,
    pub hash: String,
    pub timestamp: DateTime<Utc>,
    pub utxo_updates: Vec<UtxoUpdate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoUpdate {
    pub id: String,                 // Composite of txid:vout
    pub address: String,            // Bitcoin address
    pub public_key: Option<String>, // Optional public key
    pub txid: String,               // Transaction ID
    pub vout: i32,                  // Output index
    pub amount: i64,                // Amount in satoshis
    pub script_pub_key: String,     // The locking script
    pub script_type: String,        // P2PKH, P2SH, P2WPKH, etc.
    pub created_at: DateTime<Utc>,
    pub block_height: i32,
    // Spending information
    pub spent_txid: Option<String>,
    pub spent_at: Option<DateTime<Utc>>,
    pub spent_block: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreSignedTxRequest {
    pub txid: String,
    pub signed_tx: String,
    pub caller: String,
    pub block_height: i32,
    pub amount: i64,
    pub destination: String,
    pub fee: i64,
}

// Socket transport implementation for sender side
pub struct SocketTransport {
    socket_path: String,
}

impl SocketTransport {
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
        }
    }

    pub async fn send_update(&self, update: &BlockUpdate) -> Result<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixStream;

        // Connect to socket
        let mut stream = UnixStream::connect(&self.socket_path).await?;

        let message = NetworkMessage::BlockUpdate(update.clone());
        let data = bincode::serialize(&message)?;

        // Send size header followed by data
        let size = data.len() as u32;
        stream.write_all(&size.to_le_bytes()).await?;
        stream.write_all(&data).await?;

        // Wait for acknowledgment
        let mut size_buf = [0u8; 4];
        stream.read_exact(&mut size_buf).await?;
        let size = u32::from_le_bytes(size_buf) as usize;

        let mut data = vec![0u8; size];
        stream.read_exact(&mut data).await?;

        let response: NetworkMessage = bincode::deserialize(&data)?;

        match response {
            NetworkMessage::BlockProcessedAck { height } if height == update.height => Ok(()),
            _ => Err(TransportError::IoError(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unexpected response type",
            ))),
        }
    }

    pub async fn send_reorg_event(&self, fork_height: i32) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        // Connect to socket
        let mut stream = UnixStream::connect(&self.socket_path).await?;

        let message = NetworkMessage::ReorgEvent { fork_height };
        let data = bincode::serialize(&message)?;

        // Send size header followed by data
        let size = data.len() as u32;
        stream.write_all(&size.to_le_bytes()).await?;
        stream.write_all(&data).await?;

        Ok(())
    }

    pub async fn get_block_hash(&self, height: i32) -> Result<String> {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        // Connect to socket
        let mut stream = UnixStream::connect(&self.socket_path).await?;

        // Create a query message
        let message = NetworkMessage::GetBlockHash { height };

        // Serialize with bincode
        let data = bincode::serialize(&message)?;

        // Send size header followed by data
        let size = data.len() as u32;
        stream.write_all(&size.to_le_bytes()).await?;
        stream.write_all(&data).await?;

        // Read the response size
        let mut size_buf = [0u8; 4];
        stream.read_exact(&mut size_buf).await?;
        let size = u32::from_le_bytes(size_buf) as usize;

        // Read the response data
        let mut data = vec![0u8; size];
        stream.read_exact(&mut data).await?;

        // Deserialize the response
        let response: NetworkMessage = bincode::deserialize(&data)?;

        match response {
            NetworkMessage::BlockHashResponse { hash } => Ok(hash),
            _ => Err(TransportError::IoError(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unexpected response type",
            ))),
        }
    }

    pub async fn send_update_finality_status(&self, current_height: i32) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        let message = NetworkMessage::UpdateFinalityStatus { current_height };

        // Connect to socket
        let mut stream = UnixStream::connect(&self.socket_path).await?;

        // Serialize with bincode
        let data = bincode::serialize(&message)?;

        // Send size header followed by data
        let size = data.len() as u32;
        stream.write_all(&size.to_le_bytes()).await?;
        stream.write_all(&data).await?;

        Ok(())
    }
}
