use network_shared::TransportError;
use std::error::Error;
use std::fmt;

/// Custom error types for the Bitcoin indexer
#[derive(Debug)]
pub enum IndexerError {
    BitcoinRPC(bitcoincore_rpc::Error),
    Network(TransportError),
    InvalidTimestamp,
    InvalidStartBlock(String),
}

impl fmt::Display for IndexerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IndexerError::BitcoinRPC(e) => write!(f, "Bitcoin RPC error: {e}"),
            IndexerError::Network(e) => write!(f, "Network error: {e}"),
            IndexerError::InvalidTimestamp => write!(f, "Invalid timestamp"),
            IndexerError::InvalidStartBlock(msg) => write!(f, "Invalid start block: {msg}"),
        }
    }
}

impl Error for IndexerError {}

impl From<bitcoincore_rpc::Error> for IndexerError {
    fn from(err: bitcoincore_rpc::Error) -> IndexerError {
        IndexerError::BitcoinRPC(err)
    }
}

impl From<TransportError> for IndexerError {
    fn from(err: TransportError) -> IndexerError {
        IndexerError::Network(err)
    }
}

/// Result type alias for IndexerError
pub type Result<T> = std::result::Result<T, IndexerError>;
