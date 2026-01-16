//! Real-time streaming of sensor readings via BLE notifications.
//!
//! This module provides functionality to subscribe to sensor readings
//! and receive them as an async stream.
//!
//! The stream supports graceful shutdown via the [`ReadingStream::close`] method,
//! which uses a cancellation token to cleanly stop the background polling task.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::stream::Stream;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use aranet_types::CurrentReading;

use crate::device::Device;
use crate::error::Error;

/// Options for reading streams.
///
/// Use the builder pattern for convenient configuration:
///
/// ```ignore
/// let options = StreamOptions::builder()
///     .poll_interval(Duration::from_secs(5))
///     .include_errors(true)
///     .max_consecutive_failures(5)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct StreamOptions {
    /// Polling interval for devices that don't support notifications.
    /// Default: 1 second.
    pub poll_interval: Duration,
    /// Buffer size for the reading channel.
    /// Default: 16 readings.
    pub buffer_size: usize,
    /// Whether to include failed reads in the stream.
    ///
    /// When `false` (default), read errors are logged but not sent to the stream.
    /// When `true`, errors are sent as `Err(Error)` items, allowing the consumer
    /// to detect and handle connection issues.
    ///
    /// **Recommendation:** Set to `true` for applications that need to detect
    /// disconnections or errors in real-time.
    pub include_errors: bool,
    /// Maximum consecutive failures before auto-closing the stream.
    ///
    /// When set to `Some(n)`, the stream will automatically close after `n`
    /// consecutive read failures, indicating a likely disconnection.
    /// When `None` (default), the stream will continue indefinitely regardless
    /// of failures.
    ///
    /// **Recommendation:** Set to `Some(5)` or similar for production use to
    /// prevent indefinite polling of a disconnected device.
    pub max_consecutive_failures: Option<u32>,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            buffer_size: 16,
            include_errors: false,
            max_consecutive_failures: None,
        }
    }
}

impl StreamOptions {
    /// Create a new builder for StreamOptions.
    pub fn builder() -> StreamOptionsBuilder {
        StreamOptionsBuilder::default()
    }

    /// Create options with a specific poll interval.
    pub fn with_interval(interval: Duration) -> Self {
        Self {
            poll_interval: interval,
            ..Default::default()
        }
    }

    /// Validate the options and return an error if invalid.
    ///
    /// Checks that:
    /// - `buffer_size` is > 0
    /// - `poll_interval` is > 0
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.buffer_size == 0 {
            return Err(crate::error::Error::InvalidConfig(
                "buffer_size must be > 0".to_string(),
            ));
        }
        if self.poll_interval.is_zero() {
            return Err(crate::error::Error::InvalidConfig(
                "poll_interval must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}

/// Builder for StreamOptions.
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct StreamOptionsBuilder {
    options: StreamOptions,
}


impl StreamOptionsBuilder {
    /// Set the polling interval.
    #[must_use]
    pub fn poll_interval(mut self, interval: Duration) -> Self {
        self.options.poll_interval = interval;
        self
    }

    /// Set the buffer size.
    #[must_use]
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.options.buffer_size = size;
        self
    }

    /// Set whether to include errors in the stream.
    ///
    /// When `true`, read errors are sent as `Err(Error)` items to the stream,
    /// allowing consumers to detect disconnections and other issues.
    #[must_use]
    pub fn include_errors(mut self, include: bool) -> Self {
        self.options.include_errors = include;
        self
    }

    /// Set the maximum consecutive failures before auto-closing.
    ///
    /// When set, the stream will automatically close after this many
    /// consecutive read failures, indicating a likely disconnection.
    #[must_use]
    pub fn max_consecutive_failures(mut self, max: u32) -> Self {
        self.options.max_consecutive_failures = Some(max);
        self
    }

    /// Build the StreamOptions.
    #[must_use]
    pub fn build(self) -> StreamOptions {
        self.options
    }
}

/// A stream of sensor readings from a device.
///
/// The stream polls the device at a configured interval and sends readings
/// through a channel. It supports graceful shutdown via [`close`](Self::close).
pub struct ReadingStream {
    receiver: mpsc::Receiver<ReadingResult>,
    handle: tokio::task::JoinHandle<()>,
    cancel_token: CancellationToken,
}

/// Result type for stream items.
pub type ReadingResult = std::result::Result<CurrentReading, Error>;

impl ReadingStream {
    /// Create a new reading stream from a connected device (takes Arc).
    ///
    /// This spawns a background task that polls the device at the configured
    /// interval and sends readings to the stream.
    ///
    /// If `max_consecutive_failures` is set, the stream will automatically
    /// close after that many consecutive read failures.
    pub fn new(device: Arc<Device>, options: StreamOptions) -> Self {
        let (tx, rx) = mpsc::channel(options.buffer_size);
        let cancel_token = CancellationToken::new();
        let task_token = cancel_token.clone();
        let max_failures = options.max_consecutive_failures;

        let handle = tokio::spawn(async move {
            let mut interval = interval(options.poll_interval);
            let mut consecutive_failures: u32 = 0;

            loop {
                tokio::select! {
                    _ = task_token.cancelled() => {
                        debug!("Stream cancelled, stopping gracefully");
                        break;
                    }
                    _ = interval.tick() => {
                        match device.read_current().await {
                            Ok(reading) => {
                                // Reset failure counter on success
                                consecutive_failures = 0;
                                if tx.send(Ok(reading)).await.is_err() {
                                    debug!("Stream receiver dropped, stopping");
                                    break;
                                }
                            }
                            Err(e) => {
                                consecutive_failures += 1;
                                warn!(
                                    "Error reading from device (failure {}/{}): {}",
                                    consecutive_failures,
                                    max_failures.map_or("∞".to_string(), |n| n.to_string()),
                                    e
                                );

                                // Check if we've exceeded max consecutive failures
                                if let Some(max) = max_failures
                                    && consecutive_failures >= max {
                                        warn!(
                                            "Max consecutive failures ({}) reached, auto-closing stream",
                                            max
                                        );
                                        // Send final error if configured to include errors
                                        if options.include_errors {
                                            let _ = tx.send(Err(e)).await;
                                        }
                                        break;
                                    }

                                if options.include_errors && tx.send(Err(e)).await.is_err() {
                                    debug!("Stream receiver dropped, stopping");
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });

        Self {
            receiver: rx,
            handle,
            cancel_token,
        }
    }

    /// Close the stream and stop the background polling task gracefully.
    ///
    /// This signals the background task to stop via a cancellation token,
    /// allowing it to complete any in-progress operations before exiting.
    /// This is preferred over aborting the task, which may leave resources
    /// in an inconsistent state.
    pub fn close(self) {
        self.cancel_token.cancel();
        // The handle will complete on its own; we don't need to await it
    }

    /// Get a cancellation token that can be used to cancel the stream externally.
    ///
    /// This allows multiple places to trigger cancellation of the stream.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Check if the stream is still active (background task running).
    pub fn is_active(&self) -> bool {
        !self.handle.is_finished()
    }

    /// Check if the stream has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Check if the stream stopped unexpectedly.
    ///
    /// Returns `true` if the background task has finished but was not explicitly
    /// cancelled via [`close()`](Self::close) or by dropping the stream.
    ///
    /// This can indicate:
    /// - A panic in the background task
    /// - The stream auto-closed due to reaching `max_consecutive_failures`
    /// - The receiver was dropped unexpectedly
    ///
    /// This can be useful for detecting and handling unexpected stream termination:
    ///
    /// ```ignore
    /// if stream.has_unexpectedly_stopped() {
    ///     // Log the event and potentially restart the stream
    ///     log::warn!("Stream stopped unexpectedly - may need restart");
    /// }
    /// ```
    ///
    /// Note: To distinguish between auto-close due to failures vs actual panics,
    /// you may need additional monitoring of the stream's error output.
    pub fn has_unexpectedly_stopped(&self) -> bool {
        self.handle.is_finished() && !self.cancel_token.is_cancelled()
    }

    /// Check if the background task has panicked.
    ///
    /// **Deprecated:** Use [`has_unexpectedly_stopped()`](Self::has_unexpectedly_stopped) instead,
    /// which has clearer semantics. This method may return `true` even when the stream
    /// stopped due to `max_consecutive_failures` being reached, not just panics.
    #[deprecated(since = "0.2.0", note = "Use has_unexpectedly_stopped() instead for clearer semantics")]
    pub fn has_panicked(&self) -> bool {
        self.has_unexpectedly_stopped()
    }
}

impl Drop for ReadingStream {
    fn drop(&mut self) {
        // Ensure the background task is cancelled when the stream is dropped.
        // This prevents resource leaks if the stream is dropped without calling close().
        self.cancel_token.cancel();
    }
}

impl Stream for ReadingStream {
    type Item = ReadingResult;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_recv(cx)
    }
}

/// Extension trait for Device to create reading streams.
///
/// **Note:** This trait requires `Arc<Self>` because the stream's background task
/// needs to hold a reference to the device that outlives the method call.
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use aranet_core::{Device, DeviceStreamExt};
/// use futures::StreamExt;
///
/// // Wrap device in Arc for streaming
/// let device = Arc::new(Device::connect("Aranet4 12345").await?);
///
/// // Create a stream and consume readings
/// let mut stream = device.stream();
/// while let Some(result) = stream.next().await {
///     match result {
///         Ok(reading) => println!("CO2: {} ppm", reading.co2),
///         Err(e) => eprintln!("Error: {}", e),
///     }
/// }
/// ```
pub trait DeviceStreamExt {
    /// Create a reading stream with default options.
    ///
    /// Polls the device every second and buffers up to 16 readings.
    fn stream(self: Arc<Self>) -> ReadingStream;

    /// Create a reading stream with custom options.
    fn stream_with_options(self: Arc<Self>, options: StreamOptions) -> ReadingStream;
}

impl DeviceStreamExt for Device {
    fn stream(self: Arc<Self>) -> ReadingStream {
        ReadingStream::new(self, StreamOptions::default())
    }

    fn stream_with_options(self: Arc<Self>, options: StreamOptions) -> ReadingStream {
        ReadingStream::new(self, options)
    }
}

/// Create a stream from a device without needing the trait import.
///
/// This is a convenience function for creating a polling stream.
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use std::time::Duration;
/// use aranet_core::{Device, streaming};
///
/// let device = Arc::new(Device::connect("Aranet4 12345").await?);
/// let stream = streaming::from_device(device, Duration::from_secs(5));
/// ```
pub fn from_device(device: Arc<Device>, poll_interval: Duration) -> ReadingStream {
    ReadingStream::new(device, StreamOptions::with_interval(poll_interval))
}

/// Create a stream with default options from a device.
///
/// Convenience function that wraps `from_device` with a 1-second interval.
pub fn from_device_default(device: Arc<Device>) -> ReadingStream {
    ReadingStream::new(device, StreamOptions::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_options_default() {
        let opts = StreamOptions::default();
        assert_eq!(opts.poll_interval, Duration::from_secs(1));
        assert_eq!(opts.buffer_size, 16);
        assert!(!opts.include_errors);
    }

    #[test]
    fn test_stream_options_with_interval() {
        let opts = StreamOptions::with_interval(Duration::from_millis(500));
        assert_eq!(opts.poll_interval, Duration::from_millis(500));
    }

    #[test]
    fn test_stream_options_builder() {
        let opts = StreamOptions::builder()
            .poll_interval(Duration::from_secs(5))
            .buffer_size(32)
            .include_errors(true)
            .build();

        assert_eq!(opts.poll_interval, Duration::from_secs(5));
        assert_eq!(opts.buffer_size, 32);
        assert!(opts.include_errors);
    }

    #[test]
    fn test_stream_options_builder_partial() {
        // Only set some options, others should be defaults
        let opts = StreamOptions::builder()
            .include_errors(true)
            .build();

        assert_eq!(opts.poll_interval, Duration::from_secs(1)); // default
        assert_eq!(opts.buffer_size, 16); // default
        assert!(opts.include_errors); // set
    }
}
