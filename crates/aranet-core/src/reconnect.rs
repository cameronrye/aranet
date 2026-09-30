//! Automatic reconnection handling for Aranet devices.
//!
//! This module provides a wrapper around Device that automatically
//! handles reconnection when the connection is lost.
//!
//! [`ReconnectingDevice`] implements the [`AranetDevice`] trait,
//! allowing it to be used interchangeably with regular devices in generic code.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{info, warn};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord};

use crate::connector::{ConnectFn, SensorLink, ble_connector, release_link};
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

        // Cap attempt count to prevent overflow in exponentiation
        // With multiplier 2.0 and max 32 attempts, 2^32 * base_ms is safe within f64
        let capped_attempt = attempt.min(32);
        let delay_ms = self.initial_delay.as_millis() as f64
            * self.backoff_multiplier.powi(capped_attempt as i32);

        // Guard against overflow/infinity when converting to u64
        let delay = if delay_ms.is_finite() && delay_ms <= u64::MAX as f64 {
            Duration::from_millis(delay_ms as u64)
        } else {
            self.max_delay
        };

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

/// The reconnect logic behind [`ReconnectingDevice`], generic over the link so
/// its races can be tested without Bluetooth (`FakeRadio` in `test_support.rs`).
pub(crate) struct ReconnectCore<L: SensorLink> {
    identifier: String,
    connect: ConnectFn<L>,
    link: RwLock<Option<Arc<L>>>,
    /// The sticky flag set by `cancel_reconnect()`.
    cancelled: AtomicBool,
    state: RwLock<ConnectionState>,
    attempt_count: AtomicU32,
    options: ReconnectOptions,
    events: Option<EventSender>,
}

impl<L: SensorLink> ReconnectCore<L> {
    pub(crate) fn new(
        identifier: impl Into<String>,
        connect: ConnectFn<L>,
        link: Arc<L>,
        options: ReconnectOptions,
    ) -> Self {
        Self {
            identifier: identifier.into(),
            connect,
            link: RwLock::new(Some(link)),
            cancelled: AtomicBool::new(false),
            state: RwLock::new(ConnectionState::Connected),
            attempt_count: AtomicU32::new(0),
            options,
            events: None,
        }
    }

    pub(crate) fn set_events(&mut self, events: EventSender) {
        self.events = Some(events);
    }

    pub(crate) fn identifier(&self) -> &str {
        &self.identifier
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub(crate) fn reset_cancellation(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }

    pub(crate) async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    pub(crate) fn attempt_count(&self) -> u32 {
        self.attempt_count.load(Ordering::SeqCst)
    }

    pub(crate) async fn link(&self) -> Option<Arc<L>> {
        self.link.read().await.clone()
    }

    pub(crate) async fn is_connected(&self) -> bool {
        let guard = self.link.read().await;
        if let Some(link) = guard.as_ref() {
            link.is_connected().await
        } else {
            false
        }
    }

    // `run` and `run_owned` can't be one method because their closure bounds
    // differ. `run`'s operations return a `Send` `BoxFuture` that borrows the
    // link (the `AranetDevice` impl), and its closure is `Send + Sync`;
    // `with_device`'s public signature has none of these bounds, and its
    // future can't borrow the link. Neither form fits the other without
    // changing `with_device`'s public signature.

    /// Run `op` on the link, reconnecting and running it again if it fails.
    pub(crate) async fn run<T, F>(&self, op: F) -> Result<T>
    where
        F: for<'b> Fn(&'b L) -> BoxFuture<'b, Result<T>> + Send + Sync,
        T: Send,
    {
        {
            let guard = self.link.read().await;
            if let Some(link) = guard.as_ref()
                && link.is_connected().await
            {
                match op(link).await {
                    Ok(value) => return Ok(value),
                    Err(e) => warn!("Operation failed: {}", e),
                }
            }
        }

        self.reconnect().await?;

        let guard = self.link.read().await;
        match guard.as_ref() {
            Some(link) => op(link).await,
            None => Err(Error::NotConnected),
        }
    }

    /// Same algorithm as `run`, for closures whose future doesn't borrow the
    /// link (`ReconnectingDevice::with_device`).
    pub(crate) async fn run_owned<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: Fn(&L) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        {
            let guard = self.link.read().await;
            if let Some(link) = guard.as_ref()
                && link.is_connected().await
            {
                match op(link).await {
                    Ok(value) => return Ok(value),
                    Err(e) => warn!("Operation failed: {}", e),
                }
            }
        }

        self.reconnect().await?;

        let guard = self.link.read().await;
        match guard.as_ref() {
            Some(link) => op(link).await,
            None => Err(Error::NotConnected),
        }
    }

    pub(crate) async fn reconnect(&self) -> Result<()> {
        // Do not reset cancellation here — callers must explicitly call
        // reset_cancellation() before reconnect() if they want to clear
        // a previous cancellation. This avoids a race where
        // cancel_reconnect() fires between is_cancelled() and
        // reset_cancellation(), silently discarding the cancel request.

        *self.state.write().await = ConnectionState::Reconnecting;
        self.attempt_count.store(0, Ordering::SeqCst);

        loop {
            // Check for cancellation at the start of each iteration
            if self.is_cancelled() {
                *self.state.write().await = ConnectionState::Disconnected;
                info!("Reconnection cancelled for {}", self.identifier);
                return Err(Error::Cancelled);
            }

            let attempt = self.attempt_count.fetch_add(1, Ordering::SeqCst) + 1;

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
            if let Some(sender) = &self.events {
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
            match (self.connect)(&self.identifier).await {
                Ok(new_link) => {
                    *self.link.write().await = Some(Arc::new(new_link));
                    *self.state.write().await = ConnectionState::Connected;

                    // Send reconnect succeeded event
                    if let Some(sender) = &self.events {
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

    pub(crate) async fn disconnect(&self) -> Result<()> {
        let mut guard = self.link.write().await;
        if let Some(link) = guard.take() {
            release_link(link, ()).await?;
        }
        *self.state.write().await = ConnectionState::Disconnected;
        Ok(())
    }
}

/// A device wrapper that automatically handles reconnection.
///
/// This wrapper caches the device name and type upon initial connection so they
/// can be accessed synchronously via the [`AranetDevice`] trait, even while
/// reconnecting.
pub struct ReconnectingDevice {
    core: ReconnectCore<Device>,
    /// Cached device name (populated on first connection).
    cached_name: OnceLock<String>,
    /// Cached device type (populated on first connection).
    cached_device_type: OnceLock<DeviceType>,
}

impl ReconnectingDevice {
    /// Create a new reconnecting device wrapper.
    pub async fn connect(identifier: &str, options: ReconnectOptions) -> Result<Self> {
        options.validate()?;
        let connect = ble_connector();
        let device = Arc::new(connect(identifier).await?);

        // Cache the name and device type for synchronous access
        let cached_name = OnceLock::new();
        if let Some(name) = device.name() {
            let _ = cached_name.set(name.to_string());
        }

        let cached_device_type = OnceLock::new();
        if let Some(device_type) = device.device_type() {
            let _ = cached_device_type.set(device_type);
        }

        Ok(Self {
            core: ReconnectCore::new(identifier, connect, device, options),
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
        this.core.set_events(event_sender);
        Ok(this)
    }

    /// Cancel any ongoing reconnection attempts.
    ///
    /// This will cause the reconnect loop to exit on its next iteration.
    pub fn cancel_reconnect(&self) {
        self.core.cancel();
    }

    /// Check if reconnection has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.core.is_cancelled()
    }

    /// Reset the cancellation flag.
    ///
    /// Call this before starting a new reconnection attempt if you want to clear
    /// a previous cancellation. The `reconnect()` method will check if cancelled
    /// at the start of each iteration, so this allows re-using a previously
    /// cancelled `ReconnectingDevice`.
    pub fn reset_cancellation(&self) {
        self.core.reset_cancellation();
    }

    /// Get the current connection state.
    pub async fn state(&self) -> ConnectionState {
        self.core.state().await
    }

    /// Check if currently connected.
    pub async fn is_connected(&self) -> bool {
        self.core.is_connected().await
    }

    /// Get the identifier.
    pub fn identifier(&self) -> &str {
        self.core.identifier()
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
        Fut: Future<Output = Result<T>>,
    {
        self.core.run_owned(f).await
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
        self.core.reconnect().await
    }

    /// Disconnect from the device.
    pub async fn disconnect(&self) -> Result<()> {
        self.core.disconnect().await
    }

    /// Get the number of reconnection attempts made.
    pub async fn attempt_count(&self) -> u32 {
        self.core.attempt_count()
    }

    /// Get the device name, if available and connected.
    pub async fn name(&self) -> Option<String> {
        let device = self.core.link().await?;
        device.name().map(str::to_string)
    }

    /// Get the device address (returns identifier if not connected).
    pub async fn address(&self) -> String {
        match self.core.link().await {
            Some(device) => device.address().to_string(),
            None => self.core.identifier().to_string(),
        }
    }

    /// Get the detected device type, if available.
    pub async fn device_type(&self) -> Option<DeviceType> {
        self.core.link().await?.device_type()
    }
}

// Implement the AranetDevice trait for ReconnectingDevice
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
        self.core.identifier()
    }

    fn device_type(&self) -> Option<DeviceType> {
        self.cached_device_type.get().copied()
    }

    async fn read_current(&self) -> Result<CurrentReading> {
        self.core.run(|d| Box::pin(d.read_current())).await
    }

    async fn read_device_info(&self) -> Result<DeviceInfo> {
        self.core.run(|d| Box::pin(d.read_device_info())).await
    }

    async fn read_rssi(&self) -> Result<i16> {
        self.core.run(|d| Box::pin(d.read_rssi())).await
    }

    async fn read_battery(&self) -> Result<u8> {
        self.core.run(|d| Box::pin(d.read_battery())).await
    }

    async fn get_history_info(&self) -> Result<HistoryInfo> {
        self.core.run(|d| Box::pin(d.get_history_info())).await
    }

    async fn download_history(&self) -> Result<Vec<HistoryRecord>> {
        self.core.run(|d| Box::pin(d.download_history())).await
    }

    async fn download_history_with_options(
        &self,
        options: HistoryOptions,
    ) -> Result<Vec<HistoryRecord>> {
        let opts = options.clone();
        self.core
            .run(move |d| {
                let opts = opts.clone();
                Box::pin(async move { d.download_history_with_options(opts).await })
            })
            .await
    }

    async fn get_interval(&self) -> Result<MeasurementInterval> {
        self.core.run(|d| Box::pin(d.get_interval())).await
    }

    async fn set_interval(&self, interval: MeasurementInterval) -> Result<()> {
        self.core
            .run(move |d| Box::pin(d.set_interval(interval)))
            .await
    }

    async fn get_calibration(&self) -> Result<CalibrationData> {
        self.core.run(|d| Box::pin(d.get_calibration())).await
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

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::test_support::{FakeConn, FakeRadio, within};

    const LIMIT: Duration = Duration::from_secs(600);

    /// The operation every test runs unless it needs a scripted result.
    fn run_op(link: &FakeConn) -> BoxFuture<'_, Result<()>> {
        Box::pin(link.op())
    }

    /// A core for sensor "A" whose first link came from the fake radio.
    async fn connected_core(
        radio: &FakeRadio,
        options: ReconnectOptions,
    ) -> ReconnectCore<FakeConn> {
        let connect = radio.connector();
        let first = connect("A").await.expect("first connect");
        ReconnectCore::new("A", connect, Arc::new(first), options)
    }

    #[tokio::test(start_paused = true)]
    async fn gives_up_after_max_attempts() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = connected_core(&radio, ReconnectOptions::default().max_attempts(2)).await;
            radio.script_connects("A", [false; 3]);
            radio.lose_link("A");

            let result = core.run(run_op).await;

            assert!(
                matches!(&result, Err(Error::Timeout { operation, .. }) if operation.contains("reconnect to 'A'")),
                "{result:?}"
            );
            assert_eq!(core.state().await, ConnectionState::Failed);
            assert_eq!(radio.connect_count("A"), 3, "the first link and two attempts");
            radio.assert_no_drop_teardown();
        })
        .await;
    }
}
