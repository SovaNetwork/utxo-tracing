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
