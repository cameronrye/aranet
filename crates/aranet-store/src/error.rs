//! Error types for aranet-store.

use std::path::PathBuf;

/// Result type for aranet-store operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur in aranet-store.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Database error from SQLite.
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Failed to create database directory.
    #[error("Failed to create database directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Device not found in database.
    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    /// Invalid timestamp.
    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
