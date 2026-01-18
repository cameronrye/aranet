//! Automatic reconnection handling for Aranet devices.
//!
//! This module provides a wrapper around Device that automatically
//! handles reconnection when the connection is lost.
//!
//! [`ReconnectingDevice`] implements the [`AranetDevice`] trait,
//! allowing it to be used interchangeably with regular devices in generic code.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{info, warn};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord};

use crate::device::Device;
use crate::error::{Error, Result};
use crate::events::{DeviceEvent, DeviceId, EventSender};
use crate::history::{HistoryInfo, HistoryOptions};
use crate::settings::{CalibrationData, MeasurementInterval};
use crate::traits::AranetDevice;

/// Options for automatic reconnection.
#[derive(Debug, Clone)]
pub struct ReconnectOptions {
    /// Maximum number of reconnection attempts (None = unlimited).
    pub max_attempts: Option<u32>,
    /// Initial delay before first reconnection attempt.
    pub initial_delay: Duration,
    /// Maximum delay between attempts (for exponential backoff).
    pub max_delay: Duration,
    /// Multiplier for exponential backoff.
    pub backoff_multiplier: f64,
    /// Whether to use exponential backoff.
    pub use_exponential_backoff: bool,
}

impl Default for ReconnectOptions {
    fn default() -> Self {
        Self {
            max_attempts: Some(5),
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            use_exponential_backoff: true,
        }
    }
}

impl ReconnectOptions {
    /// Create new reconnect options with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create options with unlimited retry attempts.
    pub fn unlimited() -> Self {
        Self {
            max_attempts: None,
            ..Default::default()
        }
    }

    /// Create options with a fixed delay (no backoff).
    pub fn fixed_delay(delay: Duration) -> Self {
        Self {
            initial_delay: delay,
            use_exponential_backoff: false,
            ..Default::default()
        }
    }

    /// Set maximum number of reconnection attempts.
    pub fn max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = Some(attempts);
        self
    }

    /// Set initial delay before first reconnection attempt.
    pub fn initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Set maximum delay between attempts.
    pub fn max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Set backoff multiplier for exponential backoff.
    pub fn backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    /// Enable or disable exponential backoff.
    pub fn exponential_backoff(mut self, enabled: bool) -> Self {
        self.use_exponential_backoff = enabled;
        self
    }

    /// Calculate delay for a given attempt number.
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if !self.use_exponential_backoff {
            return self.initial_delay;
        }

        let delay_ms =
            self.initial_delay.as_millis() as f64 * self.backoff_multiplier.powi(attempt as i32);
        let delay = Duration::from_millis(delay_ms as u64);

        delay.min(self.max_delay)
    }

    /// Validate the options and return an error if invalid.
    ///
    /// Checks that:
    /// - `backoff_multiplier` is >= 1.0
    /// - `initial_delay` is > 0
    /// - `max_delay` >= `initial_delay`
    pub fn validate(&self) -> Result<()> {
        if self.backoff_multiplier < 1.0 {
            return Err(Error::InvalidConfig(
                "backoff_multiplier must be >= 1.0".to_string(),
            ));
        }
        if self.initial_delay.is_zero() {
            return Err(Error::InvalidConfig(
                "initial_delay must be > 0".to_string(),
            ));
        }
        if self.max_delay < self.initial_delay {
            return Err(Error::InvalidConfig(
                "max_delay must be >= initial_delay".to_string(),
            ));
        }
        Ok(())
    }
}

/// State of the reconnecting device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Device is connected and operational.
    Connected,
    /// Device is disconnected.
    Disconnected,
    /// Attempting to reconnect.
    Reconnecting,
    /// Reconnection failed after max attempts.
    Failed,
}

/// A device wrapper that automatically handles reconnection.
///
/// This wrapper caches the device name and type upon initial connection so they
/// can be accessed synchronously via the [`AranetDevice`] trait, even while
/// reconnecting.
pub struct ReconnectingDevice {
    identifier: String,
    /// The connected device, wrapped in Arc to allow concurrent access.
    device: RwLock<Option<Arc<Device>>>,
    options: ReconnectOptions,
    state: RwLock<ConnectionState>,
    event_sender: Option<EventSender>,
    attempt_count: RwLock<u32>,
    /// Cancellation flag for stopping reconnection attempts.
    cancelled: Arc<AtomicBool>,
    /// Cached device name (populated on first connection).
    cached_name: std::sync::OnceLock<String>,
    /// Cached device type (populated on first connection).
    cached_device_type: std::sync::OnceLock<DeviceType>,
}

impl ReconnectingDevice {
    /// Create a new reconnecting device wrapper.
    pub async fn connect(identifier: &str, options: ReconnectOptions) -> Result<Self> {
        let device = Arc::new(Device::connect(identifier).await?);

        // Cache the name and device type for synchronous access
        let cached_name = std::sync::OnceLock::new();
        if let Some(name) = device.name() {
            let _ = cached_name.set(name.to_string());
        }

        let cached_device_type = std::sync::OnceLock::new();
        if let Some(device_type) = device.device_type() {
            let _ = cached_device_type.set(device_type);
        }

        Ok(Self {
            identifier: identifier.to_string(),
            device: RwLock::new(Some(device)),
            options,
            state: RwLock::new(ConnectionState::Connected),
            event_sender: None,
            attempt_count: RwLock::new(0),
            cancelled: Arc::new(AtomicBool::new(false)),
            cached_name,
            cached_device_type,
        })
    }

    /// Create with an event sender for notifications.
    pub async fn connect_with_events(
        identifier: &str,
        options: ReconnectOptions,
        event_sender: EventSender,
    ) -> Result<Self> {
        let mut this = Self::connect(identifier, options).await?;
        this.event_sender = Some(event_sender);
        Ok(this)
    }

    /// Cancel any ongoing reconnection attempts.
    ///
    /// This will cause the reconnect loop to exit on its next iteration.
    pub fn cancel_reconnect(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check if reconnection has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Reset the cancellation flag.
    ///
    /// Call this before starting a new reconnection attempt if you want to clear
    /// a previous cancellation. The `reconnect()` method will check if cancelled
    /// at the start of each iteration, so this allows re-using a previously
    /// cancelled `ReconnectingDevice`.
    pub fn reset_cancellation(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }

    /// Get the current connection state.
    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    /// Check if currently connected.
    pub async fn is_connected(&self) -> bool {
        let guard = self.device.read().await;
        if let Some(device) = guard.as_ref() {
            device.is_connected().await
        } else {
            false
        }
    }

    /// Get the identifier.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Execute an operation, reconnecting if necessary.
    ///
    /// The closure is called with a reference to the device. If the operation
    /// fails due to a connection issue, the device will attempt to reconnect
    /// and retry the operation.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let reading = device.with_device(|d| async { d.read_current().await }).await?;
    /// ```
    pub async fn with_device<F, Fut, T>(&self, f: F) -> Result<T>
    where
        F: Fn(&Device) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        // Try the operation if already connected
        {
            let guard = self.device.read().await;
            if let Some(device) = guard.as_ref()
                && device.is_connected().await
            {
                match f(device).await {
                    Ok(result) => return Ok(result),
                    Err(e) => {
                        warn!("Operation failed: {}", e);
                        // Fall through to reconnect
                    }
                }
            }
        }

        // Need to reconnect
        self.reconnect().await?;

        // Retry the operation after reconnection
        let guard = self.device.read().await;
        if let Some(device) = guard.as_ref() {
            f(device).await
        } else {
            Err(Error::NotConnected)
        }
    }

    /// Internal helper that executes an operation with automatic reconnection using boxed futures.
    ///
    /// This method uses explicit HRTB (Higher-Rank Trait Bounds) to handle the complex
    /// lifetime requirements when returning futures from closures. It's used internally
    /// by the `AranetDevice` trait implementation.
    ///
    /// Note: We cannot consolidate this with `with_device` due to Rust's async closure
    /// lifetime limitations. The `with_device` method provides a more ergonomic API for
    /// callers, while this method handles the trait implementation requirements.
    async fn run_with_reconnect<'a, T, F>(&'a self, f: F) -> Result<T>
    where
        F: for<'b> Fn(
                &'b Device,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<T>> + Send + 'b>,
            > + Send
            + Sync,
        T: Send,
    {
        // Try the operation if already connected
        {
            let guard = self.device.read().await;
            if let Some(device) = guard.as_ref()
                && device.is_connected().await
            {
                match f(device).await {
                    Ok(result) => return Ok(result),
                    Err(e) => {
                        warn!("Operation failed: {}", e);
                        // Fall through to reconnect
                    }
                }
            }
        }

        // Need to reconnect
        self.reconnect().await?;

        // Retry the operation after reconnection
        let guard = self.device.read().await;
        if let Some(device) = guard.as_ref() {
            f(device).await
        } else {
            Err(Error::NotConnected)
        }
    }

    /// Attempt to reconnect to the device.
    ///
    /// This loop can be cancelled by calling `cancel_reconnect()` from another task.
    /// When cancelled, returns `Error::Cancelled`.
    ///
    /// Note: If `cancel_reconnect()` was called before this method, reconnection
    /// will still proceed. Call `reset_cancellation()` explicitly if you want to
    /// clear a previous cancellation before starting a new reconnection attempt.
    pub async fn reconnect(&self) -> Result<()> {
        // Only reset if not already cancelled - this prevents a race condition
        // where cancel_reconnect() is called just before reconnect() starts
        // and would be immediately cleared.
        if !self.is_cancelled() {
            self.reset_cancellation();
        }

        *self.state.write().await = ConnectionState::Reconnecting;
        *self.attempt_count.write().await = 0;

        loop {
            // Check for cancellation at the start of each iteration
            if self.is_cancelled() {
                *self.state.write().await = ConnectionState::Disconnected;
                info!("Reconnection cancelled for {}", self.identifier);
                return Err(Error::Cancelled);
            }

            let attempt = {
                let mut count = self.attempt_count.write().await;
                *count += 1;
                *count
            };

            // Check if we've exceeded max attempts
            if let Some(max) = self.options.max_attempts
                && attempt > max
            {
                *self.state.write().await = ConnectionState::Failed;
                return Err(Error::Timeout {
                    operation: format!("reconnect to '{}'", self.identifier),
                    duration: self.options.max_delay * max,
                });
            }

            // Send reconnect started event
            if let Some(sender) = &self.event_sender {
                let _ = sender.send(DeviceEvent::ReconnectStarted {
                    device: DeviceId::new(&self.identifier),
                    attempt,
                });
            }

            info!("Reconnection attempt {} for {}", attempt, self.identifier);

            // Wait before attempting (check cancellation during sleep)
            let delay = self.options.delay_for_attempt(attempt - 1);
            sleep(delay).await;

            // Check for cancellation after sleep
            if self.is_cancelled() {
                *self.state.write().await = ConnectionState::Disconnected;
                info!("Reconnection cancelled for {}", self.identifier);
                return Err(Error::Cancelled);
            }

            // Try to connect
            match Device::connect(&self.identifier).await {
                Ok(new_device) => {
                    *self.device.write().await = Some(Arc::new(new_device));
                    *self.state.write().await = ConnectionState::Connected;

                    // Send reconnect succeeded event
                    if let Some(sender) = &self.event_sender {
                        let _ = sender.send(DeviceEvent::ReconnectSucceeded {
                            device: DeviceId::new(&self.identifier),
                            attempts: attempt,
                        });
                    }

                    info!("Reconnected successfully after {} attempts", attempt);
                    return Ok(());
                }
                Err(e) => {
                    warn!("Reconnection attempt {} failed: {}", attempt, e);
                }
            }
        }
    }

    /// Disconnect from the device.
    pub async fn disconnect(&self) -> Result<()> {
        let mut guard = self.device.write().await;
        if let Some(device) = guard.take() {
            device.disconnect().await?;
        }
        *self.state.write().await = ConnectionState::Disconnected;
        Ok(())
    }

    /// Get the number of reconnection attempts made.
    pub async fn attempt_count(&self) -> u32 {
        *self.attempt_count.read().await
    }

    /// Get the device name, if available and connected.
    pub async fn name(&self) -> Option<String> {
        let guard = self.device.read().await;
        guard.as_ref().and_then(|d| d.name().map(|s| s.to_string()))
    }

    /// Get the device address (returns identifier if not connected).
    pub async fn address(&self) -> String {
        let guard = self.device.read().await;
        guard
            .as_ref()
            .map(|d| d.address().to_string())
            .unwrap_or_else(|| self.identifier.clone())
    }

    /// Get the detected device type, if available.
    pub async fn device_type(&self) -> Option<DeviceType> {
        let guard = self.device.read().await;
        guard.as_ref().and_then(|d| d.device_type())
    }
}

// Implement the AranetDevice trait for ReconnectingDevice
#[async_trait]
impl AranetDevice for ReconnectingDevice {
    async fn is_connected(&self) -> bool {
        ReconnectingDevice::is_connected(self).await
    }

    async fn connect(&self) -> Result<()> {
        // If already connected, this is a no-op
        if self.is_connected().await {
            return Ok(());
        }
        // Otherwise, attempt to reconnect
        self.reconnect().await
    }

    async fn disconnect(&self) -> Result<()> {
        ReconnectingDevice::disconnect(self).await
    }

    fn name(&self) -> Option<&str> {
        self.cached_name.get().map(|s| s.as_str())
    }

    fn address(&self) -> &str {
        &self.identifier
    }

    fn device_type(&self) -> Option<DeviceType> {
        self.cached_device_type.get().copied()
    }

    async fn read_current(&self) -> Result<CurrentReading> {
        self.run_with_reconnect(|d| Box::pin(d.read_current()))
            .await
    }

    async fn read_device_info(&self) -> Result<DeviceInfo> {
        self.run_with_reconnect(|d| Box::pin(d.read_device_info()))
            .await
    }

    async fn read_rssi(&self) -> Result<i16> {
        self.run_with_reconnect(|d| Box::pin(d.read_rssi())).await
    }

    async fn read_battery(&self) -> Result<u8> {
        self.run_with_reconnect(|d| Box::pin(d.read_battery()))
            .await
    }

    async fn get_history_info(&self) -> Result<HistoryInfo> {
        self.run_with_reconnect(|d| Box::pin(d.get_history_info()))
            .await
    }

    async fn download_history(&self) -> Result<Vec<HistoryRecord>> {
        self.run_with_reconnect(|d| Box::pin(d.download_history()))
            .await
    }

    async fn download_history_with_options(
        &self,
        options: HistoryOptions,
    ) -> Result<Vec<HistoryRecord>> {
        let opts = options.clone();
        self.run_with_reconnect(move |d| {
            let opts = opts.clone();
            Box::pin(async move { d.download_history_with_options(opts).await })
        })
        .await
    }

    async fn get_interval(&self) -> Result<MeasurementInterval> {
        self.run_with_reconnect(|d| Box::pin(d.get_interval()))
            .await
    }

    async fn set_interval(&self, interval: MeasurementInterval) -> Result<()> {
        self.run_with_reconnect(move |d| Box::pin(d.set_interval(interval)))
            .await
    }

    async fn get_calibration(&self) -> Result<CalibrationData> {
        self.run_with_reconnect(|d| Box::pin(d.get_calibration()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconnect_options_default() {
        let opts = ReconnectOptions::default();
        assert_eq!(opts.max_attempts, Some(5));
        assert!(opts.use_exponential_backoff);
    }

    #[test]
    fn test_reconnect_options_unlimited() {
        let opts = ReconnectOptions::unlimited();
        assert!(opts.max_attempts.is_none());
    }

    #[test]
    fn test_delay_calculation() {
        let opts = ReconnectOptions {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 2.0,
            use_exponential_backoff: true,
            ..Default::default()
        };

        assert_eq!(opts.delay_for_attempt(0), Duration::from_secs(1));
        assert_eq!(opts.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(opts.delay_for_attempt(2), Duration::from_secs(4));
        assert_eq!(opts.delay_for_attempt(3), Duration::from_secs(8));
    }

    #[test]
    fn test_delay_capped_at_max() {
        let opts = ReconnectOptions {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            use_exponential_backoff: true,
            ..Default::default()
        };

        // 2^10 = 1024 seconds, but capped at 10
        assert_eq!(opts.delay_for_attempt(10), Duration::from_secs(10));
    }

    #[test]
    fn test_fixed_delay() {
        let opts = ReconnectOptions::fixed_delay(Duration::from_secs(5));
        assert_eq!(opts.delay_for_attempt(0), Duration::from_secs(5));
        assert_eq!(opts.delay_for_attempt(5), Duration::from_secs(5));
    }
}
