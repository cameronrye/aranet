//! Error types for data parsing in aranet-types.

use thiserror::Error;

/// Errors that can occur when parsing Aranet sensor data.
///
/// This error type is platform-agnostic and does not include
/// BLE-specific errors (those belong in aranet-core).
///
/// This enum is marked `#[non_exhaustive]` to allow adding new error variants
/// in future versions without breaking downstream code.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ParseError {
    /// Failed to parse data due to insufficient bytes.
    #[error("Insufficient bytes: expected {expected}, got {actual}")]
    InsufficientBytes {
        /// Expected number of bytes.
        expected: usize,
        /// Actual number of bytes received.
        actual: usize,
    },

    /// Invalid or unrecognized value encountered during parsing.
    ///
    /// This variant is used for any value that doesn't meet validation
    /// requirements (e.g., humidity > 100, temperature out of range).
    #[error("Invalid value: {0}")]
    InvalidValue(String),

    /// Unknown device type byte value.
    #[error("Unknown device type: 0x{0:02X}")]
    UnknownDeviceType(u8),
}

impl ParseError {
    /// Create an `InvalidValue` error with a descriptive message.
    ///
    /// This is a convenience constructor for the common case of invalid data.
    #[must_use]
    pub fn invalid_value(message: impl Into<String>) -> Self {
        Self::InvalidValue(message.into())
    }
}

/// Result type alias using aranet-types' [`ParseError`] type.
pub type ParseResult<T> = core::result::Result<T, ParseError>;
