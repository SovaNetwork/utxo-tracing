// src/error.rs
use std::fmt;
use std::io;
use serde::{Serialize, Deserialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum StorageError {
    // IO related errors
    IoError(String),
    
    // Database related errors
    DatabaseConnectionFailed(String),
    DatabaseQueryFailed(String),
    MigrationFailed(String),
    
    // Domain specific errors
    BlockNotFound(i32),
    InsufficientFunds { available: i64, required: i64 },
    InvalidAddress(String),
    InvalidBlockHeight(i32),
    InvalidAmount(i64),
    
    // Serialization errors
    SerializationFailed(String),
    DeserializationFailed(String),
    
    // Other errors
    UnexpectedError(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::IoError(s) => write!(f, "I/O error: {}", s),
            StorageError::DatabaseConnectionFailed(s) => write!(f, "Database connection failed: {}", s),
            StorageError::DatabaseQueryFailed(s) => write!(f, "Database query failed: {}", s),
            StorageError::MigrationFailed(s) => write!(f, "Migration failed: {}", s),
            StorageError::BlockNotFound(height) => write!(f, "Block not found: height={}", height),
            StorageError::InsufficientFunds { available, required } => 
                write!(f, "Insufficient funds: available={}, required={}", available, required),
            StorageError::InvalidAddress(addr) => write!(f, "Invalid address: {}", addr),
            StorageError::InvalidBlockHeight(height) => write!(f, "Invalid block height: {}", height),
            StorageError::InvalidAmount(amount) => write!(f, "Invalid amount: {}", amount),
            StorageError::SerializationFailed(s) => write!(f, "Serialization failed: {}", s),
            StorageError::DeserializationFailed(s) => write!(f, "Deserialization failed: {}", s),
            StorageError::UnexpectedError(s) => write!(f, "Unexpected error: {}", s),
        }
    }
}

impl std::error::Error for StorageError {}

// Conversion from io::Error to StorageError
impl From<io::Error> for StorageError {
    fn from(error: io::Error) -> Self {
        StorageError::IoError(error.to_string())
    }
}

// Conversion from rusqlite::Error to StorageError
impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        StorageError::DatabaseQueryFailed(error.to_string())
    }
}

// Conversion from bincode::Error to StorageError
impl From<bincode::Error> for StorageError {
    fn from(error: bincode::Error) -> Self {
        StorageError::DeserializationFailed(error.to_string())
    }
}

// Simplify Result type usage
pub type StorageResult<T> = Result<T, StorageError>;