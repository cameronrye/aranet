//! Error types for data parsing in aranet-types.

use thiserror::Error;

/// Errors that can occur when parsing Aranet sensor data.
///
/// This error type is platform-agnostic and does not include
/// BLE-specific errors (those belong in aranet-core).
///
/// This enum is marked `#[non_exhaustive]` to allow adding new error variants
/// in future versions without breaking downstream code.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ParseError {
    /// Failed to parse data due to insufficient bytes.
    #[error("Invalid data: {0}")]
    InvalidData(String),
}

/// Result type alias using aranet-types' ParseError type.
pub type ParseResult<T> = std::result::Result<T, ParseError>;
