use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

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

#[derive(Debug)]
pub enum TransportError {
    IoError(std::io::Error),
    SerializationError(bincode::Error),
}

impl From<std::io::Error> for TransportError {
    fn from(err: std::io::Error) -> Self {
        TransportError::IoError(err)
    }
}

impl From<bincode::Error> for TransportError {
    fn from(err: bincode::Error) -> Self {
        TransportError::SerializationError(err)
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::IoError(e) => write!(f, "IO error: {}", e),
            TransportError::SerializationError(e) => write!(f, "Serialization error: {}", e),
        }
    }
}

impl std::error::Error for TransportError {}

pub type Result<T> = std::result::Result<T, TransportError>;

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