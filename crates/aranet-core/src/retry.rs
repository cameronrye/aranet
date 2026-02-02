//! Retry logic for BLE operations.
//!
//! This module provides configurable retry functionality for handling
//! transient BLE failures.
//!
//! # Example
//!
//! ```
//! use aranet_core::{RetryConfig, with_retry, Error};
//!
//! # async fn example() -> Result<(), Error> {
//! // Configure retry behavior (3 retries with default settings)
//! let config = RetryConfig::new(3);
//!
//! // Or use aggressive settings for unreliable connections
//! let aggressive = RetryConfig::aggressive();
//!
//! // Use with_retry to wrap fallible operations
//! let result = with_retry(&config, "read_sensor", || async {
//!     // Your BLE operation here
//!     Ok::<_, Error>(42)
//! }).await?;
//! # Ok(())
//! # }
//! ```

use std::future::Future;
use std::time::Duration;

use rand::Rng;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::error::{Error, Result};

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 means no retries).
    pub max_retries: u32,
    /// Initial delay between retries.
    pub initial_delay: Duration,
    /// Maximum delay between retries (for exponential backoff).
    pub max_delay: Duration,
    /// Backoff multiplier (1.0 = constant delay, 2.0 = double each time).
    pub backoff_multiplier: f64,
    /// Whether to add jitter to delays.
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(5),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryConfig {
    /// Create a new retry config with custom settings.
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }

    /// No retries.
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Conservative retry settings for unreliable connections.
    pub fn aggressive() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 1.5,
            jitter: true,
        }
    }

    // ==================== Per-Operation Presets ====================
    //
    // Different operations have different characteristics and should
    // be retried differently:
    //
    // - Scan: Fast retries, BLE scanning often needs multiple attempts
    // - Connect: Patient retries, device may be busy or waking up
    // - Read: Standard retries, transient BLE errors
    // - Write: Careful retries, writes can fail transiently
    // - History: Persistent retries, long operation, save progress

    /// Retry configuration optimized for device scanning.
    ///
    /// Scanning often requires multiple attempts due to:
    /// - BLE adapter warm-up
    /// - Devices advertising at intervals (Aranet ~4s)
    /// - RF interference
    ///
    /// Uses aggressive, fast retries with short delays.
    pub fn for_scan() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(2),
            backoff_multiplier: 1.5,
            jitter: true,
        }
    }

    /// Retry configuration optimized for device connection.
    ///
    /// Connections may fail due to:
    /// - Device busy with another central
    /// - Device in low-power mode (slower wake-up)
    /// - Signal strength variations
    ///
    /// Uses patient retries with longer delays to allow device recovery.
    pub fn for_connect() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }

    /// Retry configuration optimized for characteristic reads.
    ///
    /// Reads may fail due to:
    /// - Transient BLE errors
    /// - Connection instability
    /// - Device processing delay
    ///
    /// Uses standard retries suitable for most read operations.
    pub fn for_read() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }

    /// Retry configuration optimized for characteristic writes.
    ///
    /// Writes may fail due to:
    /// - BLE transmission errors
    /// - Device busy processing previous write
    /// - Connection instability
    ///
    /// Uses careful retries with moderate delays.
    pub fn for_write() -> Self {
        Self {
            max_retries: 2,
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(3),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }

    /// Retry configuration optimized for history downloads.
    ///
    /// History downloads are long-running operations that may fail due to:
    /// - Connection drops during extended transfer
    /// - Device timeout during large transfers
    /// - BLE congestion from repeated reads
    ///
    /// Uses persistent retries with longer delays, designed to work
    /// with checkpoint-based resumption for large downloads.
    pub fn for_history() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(15),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }

    /// Retry configuration optimized for reconnection attempts.
    ///
    /// After a connection loss, the device may need time to:
    /// - Reset its BLE state
    /// - Complete other operations
    /// - Recover from low-power mode
    ///
    /// Uses very patient retries with long delays.
    pub fn for_reconnect() -> Self {
        Self {
            max_retries: 5,
            initial_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }

    /// Retry configuration for quick, time-sensitive operations.
    ///
    /// For operations where speed is more important than reliability,
    /// uses minimal retries with very short delays.
    pub fn quick() -> Self {
        Self {
            max_retries: 2,
            initial_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(500),
            backoff_multiplier: 2.0,
            jitter: false,
        }
    }

    // ==================== Builder Methods ====================

    /// Set maximum number of retries.
    #[must_use]
    pub fn max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }

    /// Set initial delay.
    #[must_use]
    pub fn initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Set maximum delay.
    #[must_use]
    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Set backoff multiplier.
    #[must_use]
    pub fn backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    /// Enable or disable jitter.
    #[must_use]
    pub fn jitter(mut self, enabled: bool) -> Self {
        self.jitter = enabled;
        self
    }

    /// Calculate delay for a given attempt number.
    fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_delay =
            self.initial_delay.as_secs_f64() * self.backoff_multiplier.powi(attempt as i32);
        let capped_delay = base_delay.min(self.max_delay.as_secs_f64());

        let final_delay = if self.jitter {
            // Add up to 25% jitter using proper random number generation
            let jitter_factor = 1.0 + (rand::rng().random::<f64>() * 0.25);
            capped_delay * jitter_factor
        } else {
            capped_delay
        };

        Duration::from_secs_f64(final_delay)
    }
}

/// Execute an async operation with retry logic.
///
/// # Arguments
///
/// * `config` - Retry configuration
/// * `operation` - The async operation to retry
/// * `operation_name` - Name for logging purposes
///
/// # Returns
///
/// The result of the operation, or the last error if all retries failed.
pub async fn with_retry<F, Fut, T>(
    config: &RetryConfig,
    operation_name: &str,
    operation: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut last_error = None;

    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => {
                if attempt > 0 {
                    debug!("{} succeeded after {} retries", operation_name, attempt);
                }
                return Ok(result);
            }
            Err(e) => {
                if !is_retryable(&e) {
                    return Err(e);
                }

                last_error = Some(e);

                if attempt < config.max_retries {
                    let delay = config.delay_for_attempt(attempt);
                    warn!(
                        "{} failed (attempt {}/{}), retrying in {:?}",
                        operation_name,
                        attempt + 1,
                        config.max_retries + 1,
                        delay
                    );
                    sleep(delay).await;
                }
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| Error::InvalidData("Operation failed with no error".to_string())))
}

/// Check if an error is retryable.
fn is_retryable(error: &Error) -> bool {
    use crate::error::ConnectionFailureReason;

    match error {
        // Timeout errors are usually transient
        Error::Timeout { .. } => true,
        // Bluetooth errors are often transient
        Error::Bluetooth(_) => true,
        // Connection failed - check the reason
        Error::ConnectionFailed { reason, .. } => {
            matches!(
                reason,
                ConnectionFailureReason::OutOfRange
                    | ConnectionFailureReason::Timeout
                    | ConnectionFailureReason::BleError(_)
                    | ConnectionFailureReason::Other(_)
            )
        }
        // Not connected errors might be transient
        Error::NotConnected => true,
        // Write failures might be transient
        Error::WriteFailed { .. } => true,
        // Invalid data is not retryable
        Error::InvalidData(_) => false,
        // Invalid history data is not retryable
        Error::InvalidHistoryData { .. } => false,
        // Invalid reading format is not retryable
        Error::InvalidReadingFormat { .. } => false,
        // Device not found is not retryable
        Error::DeviceNotFound(_) => false,
        // Characteristic not found is not retryable
        Error::CharacteristicNotFound { .. } => false,
        // Cancelled is not retryable
        Error::Cancelled => false,
        // I/O errors might be transient
        Error::Io(_) => true,
        // Invalid configuration is not retryable
        Error::InvalidConfig(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ConnectionFailureReason, DeviceNotFoundReason};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert!(config.jitter);
    }

    #[test]
    fn test_retry_config_none() {
        let config = RetryConfig::none();
        assert_eq!(config.max_retries, 0);
    }

    #[test]
    fn test_delay_calculation() {
        let config = RetryConfig {
            initial_delay: Duration::from_millis(100),
            backoff_multiplier: 2.0,
            max_delay: Duration::from_secs(10),
            jitter: false,
            max_retries: 5,
        };

        assert_eq!(config.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(200));
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(400));
    }

    #[test]
    fn test_is_retryable() {
        assert!(is_retryable(&Error::Timeout {
            operation: "test".to_string(),
            duration: Duration::from_secs(1),
        }));
        assert!(is_retryable(&Error::ConnectionFailed {
            device_id: None,
            reason: ConnectionFailureReason::Other("test".to_string()),
        }));
        assert!(is_retryable(&Error::NotConnected));
        assert!(!is_retryable(&Error::InvalidData("test".to_string())));
        assert!(!is_retryable(&Error::DeviceNotFound(
            DeviceNotFoundReason::NotFound {
                identifier: "test".to_string()
            }
        )));
    }

    #[tokio::test]
    async fn test_with_retry_immediate_success() {
        let config = RetryConfig::new(3);
        let result = with_retry(&config, "test", || async { Ok::<_, Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_with_retry_eventual_success() {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay: Duration::from_millis(1),
            jitter: false,
            ..Default::default()
        };

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result: Result<i32> = with_retry(&config, "test", || {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    Err(Error::ConnectionFailed {
                        device_id: None,
                        reason: ConnectionFailureReason::Other("transient error".to_string()),
                    })
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_with_retry_all_fail() {
        let config = RetryConfig {
            max_retries: 2,
            initial_delay: Duration::from_millis(1),
            jitter: false,
            ..Default::default()
        };

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result: Result<i32> = with_retry(&config, "test", || {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(Error::ConnectionFailed {
                    device_id: None,
                    reason: ConnectionFailureReason::Other("persistent error".to_string()),
                })
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn test_with_retry_non_retryable_error() {
        let config = RetryConfig::new(3);
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = Arc::clone(&attempts);

        let result: Result<i32> = with_retry(&config, "test", || {
            let attempts = Arc::clone(&attempts_clone);
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(Error::InvalidData("not retryable".to_string()))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1); // No retries
    }
}
