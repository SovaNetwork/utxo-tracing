mod error;

pub use error::TransportError;

use chrono::{DateTime, Utc};
use error::Result;
use serde::{Deserialize, Serialize};

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
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixStream;

        // Connect to socket
        let mut stream = UnixStream::connect(&self.socket_path).await?;

        // Serialize with bincode
        let data = bincode::serialize(update)?;

        // Send size header followed by data
        let size = data.len() as u32;
        stream.write_all(&size.to_le_bytes()).await?;
        stream.write_all(&data).await?;

        Ok(())
    }
}
