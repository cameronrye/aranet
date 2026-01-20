//! Error types for aranet-core.
//!
//! This module defines all error types that can occur when communicating with
//! Aranet devices via Bluetooth Low Energy.
//!
//! # Error Recovery Strategies
//!
//! Different errors require different recovery approaches. This guide helps you
//! choose the right strategy for each error type.
//!
//! ## Retry vs Reconnect
//!
//! | Error Type | Strategy | Rationale |
//! |------------|----------|-----------|
//! | [`Error::Timeout`] | Retry (2-3 times) | Transient BLE congestion |
//! | [`Error::Bluetooth`] | Retry, then reconnect | May be transient or connection lost |
//! | [`Error::NotConnected`] | Reconnect | Connection was lost |
//! | [`Error::ConnectionFailed`] | Retry with backoff | Device may be temporarily busy |
//! | [`Error::WriteFailed`] | Retry (1-2 times) | BLE write can fail transiently |
//! | [`Error::InvalidData`] | Do not retry | Data corruption, report to user |
//! | [`Error::DeviceNotFound`] | Do not retry | Device not in range or wrong name |
//! | [`Error::CharacteristicNotFound`] | Do not retry | Firmware incompatibility |
//! | [`Error::InvalidConfig`] | Do not retry | Fix configuration and restart |
//!
//! ## Recommended Timeouts
//!
//! | Operation | Recommended Timeout | Notes |
//! |-----------|---------------------|-------|
//! | Device scan | 10-30 seconds | Aranet4 advertises every ~4s |
//! | Connection | 10-15 seconds | May take longer if device is busy |
//! | Read current | 5 seconds | Usually completes in <1s |
//! | Read device info | 5 seconds | Multiple characteristic reads |
//! | History download | 2-5 minutes | Depends on record count |
//! | Write settings | 5 seconds | Includes verification read |
//!
//! ## Using RetryConfig
//!
//! For transient failures, use [`crate::RetryConfig`] with [`crate::with_retry`]:
//!
//! ```ignore
//! use aranet_core::{RetryConfig, with_retry};
//!
//! // Default: 3 retries with exponential backoff (100ms -> 200ms -> 400ms)
//! let config = RetryConfig::default();
//!
//! // For unreliable connections: 5 retries, more aggressive
//! let aggressive = RetryConfig::aggressive();
//!
//! // Wrap your operation
//! let reading = with_retry(&config, "read_current", || async {
//!     device.read_current().await
//! }).await?;
//! ```
//!
//! ## Using ReconnectingDevice
//!
//! For long-running applications, use [`crate::ReconnectingDevice`] which
//! automatically handles reconnection:
//!
//! ```ignore
//! use aranet_core::{ReconnectingDevice, ReconnectOptions};
//!
//! // Default: 5 attempts with exponential backoff (1s -> 2s -> 4s -> 8s -> 16s)
//! let options = ReconnectOptions::default();
//!
//! // For always-on services: unlimited retries
//! let unlimited = ReconnectOptions::unlimited();
//!
//! // Connect with auto-reconnect
//! let device = ReconnectingDevice::connect("Aranet4 12345", options).await?;
//!
//! // Operations automatically reconnect if connection is lost
//! let reading = device.read_current().await?;
//! ```
//!
//! ## Error Classification
//!
//! The retry module internally classifies errors as retryable or not.
//! The following errors are considered retryable:
//!
//! - [`Error::Timeout`] - BLE operations can time out due to interference
//! - [`Error::Bluetooth`] - Generic BLE errors are often transient
//! - [`Error::NotConnected`] - Connection may have been lost, reconnect and retry
//! - [`Error::WriteFailed`] - Write operations can fail transiently
//! - [`Error::ConnectionFailed`] with `OutOfRange`, `Timeout`, or `BleError` reasons
//! - [`Error::Io`] - I/O errors may be transient
//!
//! The following errors should NOT be retried:
//!
//! - [`Error::InvalidData`] - Data is corrupted, retrying won't help
//! - [`Error::InvalidHistoryData`] - History data format error
//! - [`Error::InvalidReadingFormat`] - Reading format error
//! - [`Error::DeviceNotFound`] - Device is not available
//! - [`Error::CharacteristicNotFound`] - Device doesn't support this feature
//! - [`Error::Cancelled`] - Operation was intentionally cancelled
//! - [`Error::InvalidConfig`] - Configuration error, fix and restart
//!
//! ## Example: Robust Reading Loop
//!
//! ```ignore
//! use aranet_core::{Device, Error, RetryConfig, with_retry};
//! use std::time::Duration;
//!
//! async fn read_with_recovery(device: &Device) -> Result<CurrentReading, Error> {
//!     let config = RetryConfig::new(3);
//!
//!     with_retry(&config, "read_current", || async {
//!         device.read_current().await
//!     }).await
//! }
//!
//! // For long-running monitoring
//! async fn monitoring_loop(identifier: &str) {
//!     let options = ReconnectOptions::default()
//!         .max_attempts(10)
//!         .initial_delay(Duration::from_secs(2));
//!
//!     let device = ReconnectingDevice::connect(identifier, options).await?;
//!
//!     loop {
//!         match device.read_current().await {
//!             Ok(reading) => println!("CO2: {} ppm", reading.co2),
//!             Err(Error::Cancelled) => break, // Graceful shutdown
//!             Err(e) => eprintln!("Error (will retry): {}", e),
//!         }
//!         tokio::time::sleep(Duration::from_secs(60)).await;
//!     }
//! }
//! ```

use std::time::Duration;

use thiserror::Error;

use crate::history::HistoryParam;

/// Errors that can occur when communicating with Aranet devices.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new error variants
/// in future versions without breaking downstream code.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Bluetooth Low Energy error.
    #[error("Bluetooth error: {0}")]
    Bluetooth(#[from] btleplug::Error),

    /// Device not found during scan or connection.
    #[error("Device not found: {0}")]
    DeviceNotFound(DeviceNotFoundReason),

    /// Operation attempted while not connected to device.
    #[error("Not connected to device")]
    NotConnected,

    /// Required BLE characteristic not found on device.
    #[error("Characteristic not found: {uuid} (searched in {service_count} services)")]
    CharacteristicNotFound {
        /// The UUID that was not found.
        uuid: String,
        /// Number of services that were searched.
        service_count: usize,
    },

    /// Failed to parse data received from device.
    #[error("Invalid data: {0}")]
    InvalidData(String),

    /// Invalid history data format.
    #[error(
        "Invalid history data: {message} (param={param:?}, expected {expected} bytes, got {actual})"
    )]
    InvalidHistoryData {
        /// Description of the error.
        message: String,
        /// The history parameter being downloaded.
        param: Option<HistoryParam>,
        /// Expected data size.
        expected: usize,
        /// Actual data size received.
        actual: usize,
    },

    /// Invalid reading format from sensor.
    #[error("Invalid reading format: expected {expected} bytes, got {actual}")]
    InvalidReadingFormat {
        /// Expected data size.
        expected: usize,
        /// Actual data size received.
        actual: usize,
    },

    /// Operation timed out.
    #[error("Operation '{operation}' timed out after {duration:?}")]
    Timeout {
        /// The operation that timed out.
        operation: String,
        /// The timeout duration.
        duration: Duration,
    },

    /// Operation was cancelled.
    #[error("Operation cancelled")]
    Cancelled,

    /// I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Connection failed with specific reason.
    #[error("Connection failed: {reason}")]
    ConnectionFailed {
        /// The device identifier that failed to connect.
        device_id: Option<String>,
        /// The structured reason for the failure.
        reason: ConnectionFailureReason,
    },

    /// Write operation failed.
    #[error("Write failed to characteristic {uuid}: {reason}")]
    WriteFailed {
        /// The characteristic UUID.
        uuid: String,
        /// The reason for the failure.
        reason: String,
    },

    /// Invalid configuration provided.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

/// Structured reasons for connection failures.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new reasons
/// in future versions without breaking downstream code.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionFailureReason {
    /// Bluetooth adapter not available or powered off.
    AdapterUnavailable,
    /// Device is out of range.
    OutOfRange,
    /// Device rejected the connection.
    Rejected,
    /// Connection attempt timed out.
    Timeout,
    /// Already connected to another central.
    AlreadyConnected,
    /// Pairing failed.
    PairingFailed,
    /// Generic BLE error.
    BleError(String),
    /// Other/unknown error.
    Other(String),
}

impl std::fmt::Display for ConnectionFailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AdapterUnavailable => write!(f, "Bluetooth adapter unavailable"),
            Self::OutOfRange => write!(f, "device out of range"),
            Self::Rejected => write!(f, "connection rejected by device"),
            Self::Timeout => write!(f, "connection timed out"),
            Self::AlreadyConnected => write!(f, "device already connected"),
            Self::PairingFailed => write!(f, "pairing failed"),
            Self::BleError(msg) => write!(f, "BLE error: {}", msg),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

/// Reason why a device was not found.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new reasons
/// in future versions without breaking downstream code.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DeviceNotFoundReason {
    /// No devices found during scan.
    NoDevicesInRange,
    /// Device with specified name/address not found.
    NotFound { identifier: String },
    /// Scan timed out before finding device.
    ScanTimeout { duration: Duration },
    /// No Bluetooth adapter available.
    NoAdapter,
}

impl std::fmt::Display for DeviceNotFoundReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDevicesInRange => write!(f, "no devices in range"),
            Self::NotFound { identifier } => write!(f, "device '{}' not found", identifier),
            Self::ScanTimeout { duration } => write!(f, "scan timed out after {:?}", duration),
            Self::NoAdapter => write!(f, "no Bluetooth adapter available"),
        }
    }
}

impl Error {
    /// Create a device not found error for a specific identifier.
    pub fn device_not_found(identifier: impl Into<String>) -> Self {
        Self::DeviceNotFound(DeviceNotFoundReason::NotFound {
            identifier: identifier.into(),
        })
    }

    /// Create a timeout error with operation context.
    pub fn timeout(operation: impl Into<String>, duration: Duration) -> Self {
        Self::Timeout {
            operation: operation.into(),
            duration,
        }
    }

    /// Create a characteristic not found error.
    pub fn characteristic_not_found(uuid: impl Into<String>, service_count: usize) -> Self {
        Self::CharacteristicNotFound {
            uuid: uuid.into(),
            service_count,
        }
    }

    /// Create an invalid reading format error.
    pub fn invalid_reading(expected: usize, actual: usize) -> Self {
        Self::InvalidReadingFormat { expected, actual }
    }

    /// Create a configuration error.
    pub fn invalid_config(message: impl Into<String>) -> Self {
        Self::InvalidConfig(message.into())
    }

    /// Create a connection failure with structured reason.
    pub fn connection_failed(device_id: Option<String>, reason: ConnectionFailureReason) -> Self {
        Self::ConnectionFailed { device_id, reason }
    }

    /// Create a connection failure with a string reason.
    ///
    /// This is a convenience method that wraps the string in `ConnectionFailureReason::Other`.
    pub fn connection_failed_str(device_id: Option<String>, reason: impl Into<String>) -> Self {
        Self::ConnectionFailed {
            device_id,
            reason: ConnectionFailureReason::Other(reason.into()),
        }
    }
}

impl From<aranet_types::ParseError> for Error {
    fn from(err: aranet_types::ParseError) -> Self {
        match err {
            aranet_types::ParseError::InsufficientBytes { expected, actual } => {
                Error::InvalidReadingFormat { expected, actual }
            }
            aranet_types::ParseError::InvalidValue(msg) => Error::InvalidData(msg),
            aranet_types::ParseError::UnknownDeviceType(byte) => {
                Error::InvalidData(format!("Unknown device type: 0x{:02X}", byte))
            }
            // Handle future ParseError variants (non_exhaustive)
            _ => Error::InvalidData(format!("Parse error: {}", err)),
        }
    }
}

/// Result type alias using aranet-core's Error type.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = Error::device_not_found("Aranet4 12345");
        assert!(err.to_string().contains("Aranet4 12345"));

        let err = Error::NotConnected;
        assert_eq!(err.to_string(), "Not connected to device");

        let err = Error::characteristic_not_found("0x2A19", 5);
        assert!(err.to_string().contains("0x2A19"));
        assert!(err.to_string().contains("5 services"));

        let err = Error::InvalidData("bad format".to_string());
        assert_eq!(err.to_string(), "Invalid data: bad format");

        let err = Error::timeout("read_current", Duration::from_secs(10));
        assert!(err.to_string().contains("read_current"));
        assert!(err.to_string().contains("10s"));
    }

    #[test]
    fn test_error_debug() {
        let err = Error::DeviceNotFound(DeviceNotFoundReason::NoDevicesInRange);
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("DeviceNotFound"));
    }

    #[test]
    fn test_device_not_found_reasons() {
        let err = Error::DeviceNotFound(DeviceNotFoundReason::NoAdapter);
        assert!(err.to_string().contains("no Bluetooth adapter"));

        let err = Error::DeviceNotFound(DeviceNotFoundReason::ScanTimeout {
            duration: Duration::from_secs(30),
        });
        assert!(err.to_string().contains("30s"));
    }

    #[test]
    fn test_invalid_reading_format() {
        let err = Error::invalid_reading(13, 7);
        assert!(err.to_string().contains("13"));
        assert!(err.to_string().contains("7"));
    }

    #[test]
    fn test_btleplug_error_conversion() {
        // btleplug::Error doesn't have public constructors for most variants,
        // but we can verify the From impl exists by checking the type compiles
        fn _assert_from_impl<T: From<btleplug::Error>>() {}
        _assert_from_impl::<Error>();
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: Error = io_err.into();
        assert!(matches!(err, Error::Io(_)));
        assert!(err.to_string().contains("file not found"));
    }
}
