//! Automatic reconnection handling for Aranet devices.
//!
//! This module provides a wrapper around Device that automatically
//! handles reconnection when the connection is lost.
//!
//! [`ReconnectingDevice`] implements the [`AranetDevice`] trait,
//! allowing it to be used interchangeably with regular devices in generic code.
//!
//! # When it reconnects
//!
//! An operation reconnects when the device isn't connected, or when it fails
//! with a connection error: not connected, a timeout, a failed connection or a
//! Bluetooth link error. Other errors, such as a characteristic the device
//! doesn't have, are returned at once. The old connection is closed before the
//! new one is made, because on Linux and macOS closing it later would drop the
//! new connection too.
//!
//! Operations that fail at the same time share one reconnect. Between attempts
//! it waits with the backoff of [`ReconnectOptions`];
//! [`ReconnectingDevice::cancel_reconnect`] and [`ReconnectingDevice::disconnect`]
//! end that wait, or a connect in progress, at once. An operation that waited
//! for a reconnect that gave up or was stopped returns an error instead of
//! starting another one.
//!
//! While a reconnect runs, and after one has failed, there is no connection:
//! `ReconnectingDevice::name()` returns `None` and
//! `ReconnectingDevice::address()` returns the identifier.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use tokio::sync::{Mutex, RwLock};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord};

use crate::connector::{ConnectFn, SensorLink, ble_connector, log_failed_release, release_link};
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
///
/// Lock order: `serial`, then `link`. Operations hold a `link` read guard while
/// they run, so a recovery or a disconnect waits for them before it takes the
/// link out.
///
/// The generation protocol (all `SeqCst`) keeps a recovery from undoing a
/// `cancel()` or a `disconnect()`, and `disconnect()` from waiting for a whole
/// recovery, whatever the interleaving:
///
/// - `disconnect()` bumps `generation` and counts itself in `disconnecting`,
///   and `cancel()` sets `cancelled`, before either cancels the current `wake`
///   token.
/// - `recover` installs a fresh `wake` token before it checks `disconnecting`,
///   `generation` and `cancelled`, so a call that those checks miss cancels its
///   token.
/// - `recover` installs a new link, under the `link` write lock, only if
///   `generation` is still the value it set when it took the old link out and
///   its token isn't cancelled.
/// - A recovery that gives up bumps `generation` too, so the callers queued
///   behind it share its failure instead of starting another one.
pub(crate) struct ReconnectCore<L: SensorLink> {
    identifier: String,
    connect: ConnectFn<L>,
    link: RwLock<Option<Arc<L>>>,
    /// Bumped by every install and removal of a link, by a recovery that gives
    /// up, and by `disconnect()`.
    generation: AtomicU64,
    /// The number of `disconnect()` calls in progress.
    disconnecting: AtomicU32,
    /// One recovery or one disconnect at a time.
    serial: Mutex<()>,
    /// Cancelled by `cancel()` and `disconnect()` to end a backoff wait or a
    /// connect at once. Each recovery starts with a new token.
    wake: std::sync::Mutex<CancellationToken>,
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
            generation: AtomicU64::new(0),
            disconnecting: AtomicU32::new(0),
            serial: Mutex::new(()),
            wake: std::sync::Mutex::new(CancellationToken::new()),
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

    /// Never held across an `.await`.
    fn lock_wake(&self) -> std::sync::MutexGuard<'_, CancellationToken> {
        self.wake.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.lock_wake().cancel();
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
        match self.link().await {
            Some(link) => link.is_connected().await,
            None => false,
        }
    }

    // `run` and `run_owned` can't be one method because their closure bounds
    // differ. `run`'s operations return a `Send` `BoxFuture` that borrows the
    // link (the `AranetDevice` impl), and its closure is `Send + Sync`;
    // `with_device`'s public signature has none of these bounds, and its
    // future can't borrow the link. Neither form fits the other without
    // changing `with_device`'s public signature.

    /// Run `op` on the link. After a connection error, or when there is no
    /// live link, recover once and run `op` again; any other error is returned
    /// as it is.
    pub(crate) async fn run<T, F>(&self, op: F) -> Result<T>
    where
        F: for<'b> Fn(&'b L) -> BoxFuture<'b, Result<T>> + Send + Sync,
        T: Send,
    {
        let seen = {
            let guard = self.link.read().await;
            let seen = self.generation.load(Ordering::SeqCst);
            if let Some(link) = guard.as_ref()
                && link.is_connected().await
            {
                match op(link).await {
                    Ok(value) => return Ok(value),
                    Err(e) if !e.is_connection_error() => return Err(e),
                    Err(e) => warn!("Operation failed with a connection error: {e}; reconnecting"),
                }
            }
            seen
        }; // The read guard is dropped before `recover` takes the write lock.

        self.recover(seen).await?;

        let guard = self.link.read().await;
        match guard.as_ref() {
            Some(link) => op(link).await,
            None => Err(self.stopped_error()),
        }
    }

    /// Same algorithm as `run`, for closures whose future doesn't borrow the
    /// link (`ReconnectingDevice::with_device`).
    pub(crate) async fn run_owned<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: Fn(&L) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let seen = {
            let guard = self.link.read().await;
            let seen = self.generation.load(Ordering::SeqCst);
            if let Some(link) = guard.as_ref()
                && link.is_connected().await
            {
                match op(link).await {
                    Ok(value) => return Ok(value),
                    Err(e) if !e.is_connection_error() => return Err(e),
                    Err(e) => warn!("Operation failed with a connection error: {e}; reconnecting"),
                }
            }
            seen
        };

        self.recover(seen).await?;

        let guard = self.link.read().await;
        match guard.as_ref() {
            Some(link) => op(link).await,
            None => Err(self.stopped_error()),
        }
    }

    /// Close the current link and connect again, even if the link is up. A
    /// call that waited for another recovery shares its result: `Ok` if that
    /// one left a link, `stopped_error()` if it gave up or was stopped.
    pub(crate) async fn reconnect(&self) -> Result<()> {
        self.recover(self.generation.load(Ordering::SeqCst)).await?;
        if self.link.read().await.is_some() {
            Ok(())
        } else {
            Err(self.stopped_error())
        }
    }

    /// What a caller gets when the recovery it waited for left no link, because
    /// it gave up or `cancel()` or `disconnect()` stopped it: `Error::Cancelled`
    /// while the sticky flag is set, `Error::NotConnected` otherwise.
    fn stopped_error(&self) -> Error {
        if self.is_cancelled() {
            Error::Cancelled
        } else {
            Error::NotConnected
        }
    }

    pub(crate) async fn disconnect(&self) -> Result<()> {
        // First the generation and the count, then the token: a recovery in
        // progress either sees them or has its wait or connect cut short.
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _in_progress = DisconnectInProgress::start(&self.disconnecting);
        self.lock_wake().cancel();

        let _serial = self.serial.lock().await;
        let link = {
            let mut link = self.link.write().await;
            self.generation.fetch_add(1, Ordering::SeqCst);
            link.take()
        };
        // Disconnected even if closing the link fails.
        *self.state.write().await = ConnectionState::Disconnected;
        match link {
            Some(link) => release_link(link, ()).await,
            None => Ok(()),
        }
    }

    /// Replace the link that was current at generation `seen`: close it, then
    /// connect with backoff until a connect succeeds, the attempts run out, or
    /// `cancel()` or `disconnect()` stops it.
    async fn recover(&self, seen: u64) -> Result<()> {
        let _serial = self.serial.lock().await;
        // A fresh token first, then the checks (see the protocol above).
        let wake = {
            let mut wake = self.lock_wake();
            *wake = CancellationToken::new();
            wake.clone()
        };
        if self.disconnecting.load(Ordering::SeqCst) > 0 {
            // A `disconnect()` waits for `serial`: don't make it wait for a recovery.
            return Err(self.stopped_error());
        }
        if self.generation.load(Ordering::SeqCst) != seen {
            // While this caller waited, someone reconnected, the user
            // disconnected, or the recovery it waited for gave up.
            return Ok(());
        }
        if self.is_cancelled() {
            return self.cancelled_reconnect().await;
        }

        *self.state.write().await = ConnectionState::Reconnecting;
        self.attempt_count.store(0, Ordering::SeqCst);

        // Close the old link before connecting. A disconnect acts on the sensor,
        // not on the handle, so closing the old handle later (or dropping it
        // unclosed) would take the new link down.
        let (old, mine) = {
            let mut link = self.link.write().await;
            (
                link.take(),
                self.generation.fetch_add(1, Ordering::SeqCst) + 1,
            )
        };
        if let Some(old) = old
            && let Err(e) = release_link(old, ()).await
        {
            log_failed_release("Closing the old link", &self.identifier, &e);
        }

        loop {
            // `disconnect()` sets no flag, only the token; it may have come in
            // while the old link was closing.
            if self.is_cancelled() || wake.is_cancelled() {
                return self.cancelled_reconnect().await;
            }

            let attempt = self.attempt_count.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(max) = self.options.max_attempts
                && attempt > max
            {
                *self.state.write().await = ConnectionState::Failed;
                // The callers queued behind this recovery share its failure.
                self.generation.fetch_add(1, Ordering::SeqCst);
                return Err(Error::Timeout {
                    operation: format!("reconnect to '{}'", self.identifier),
                    duration: self.options.max_delay * max,
                });
            }

            if let Some(sender) = &self.events {
                let _ = sender.send(DeviceEvent::ReconnectStarted {
                    device: DeviceId::new(&self.identifier),
                    attempt,
                });
            }
            info!("Reconnection attempt {} for {}", attempt, self.identifier);

            let delay = self.options.delay_for_attempt(attempt - 1);
            if wake.run_until_cancelled(sleep(delay)).await.is_none() {
                return self.cancelled_reconnect().await;
            }

            // Dropping a connect part-way is safe: `Device::connect` releases the sensor.
            let new = match wake
                .run_until_cancelled((self.connect)(&self.identifier))
                .await
            {
                None => return self.cancelled_reconnect().await,
                Some(Err(e)) => {
                    warn!("Reconnection attempt {} failed: {}", attempt, e);
                    continue;
                }
                Some(Ok(new)) => Arc::new(new),
            };

            let mut link = self.link.write().await;
            if self.generation.load(Ordering::SeqCst) != mine || wake.is_cancelled() {
                // cancel() or disconnect() came in as the connect finished.
                drop(link);
                if let Err(e) = release_link(new, ()).await {
                    log_failed_release("Closing the new link", &self.identifier, &e);
                }
                return self.cancelled_reconnect().await;
            }
            *link = Some(new);
            self.generation.fetch_add(1, Ordering::SeqCst);
            drop(link);
            *self.state.write().await = ConnectionState::Connected;

            if let Some(sender) = &self.events {
                let _ = sender.send(DeviceEvent::ReconnectSucceeded {
                    device: DeviceId::new(&self.identifier),
                    attempts: attempt,
                });
            }
            info!("Reconnected successfully after {} attempts", attempt);
            return Ok(());
        }
    }

    async fn cancelled_reconnect(&self) -> Result<()> {
        *self.state.write().await = ConnectionState::Disconnected;
        info!("Reconnection cancelled for {}", self.identifier);
        Err(Error::Cancelled)
    }
}

/// Counts a `disconnect()` in `ReconnectCore::disconnecting` until it returns
/// or its future is dropped.
struct DisconnectInProgress<'a>(&'a AtomicU32);

impl<'a> DisconnectInProgress<'a> {
    fn start(count: &'a AtomicU32) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for DisconnectInProgress<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
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
    /// A backoff wait or a connect in progress ends at once, and the reconnect
    /// returns `Error::Cancelled`, as do the operations that were waiting for
    /// it. The flag stays set until
    /// [`reset_cancellation()`](Self::reset_cancellation): until then, an
    /// operation that needs to reconnect also returns `Error::Cancelled`.
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
    /// The closure is called with a reference to the device. If the device
    /// isn't connected, this reconnects first. If the closure fails with a
    /// connection error (not connected, a timeout, a failed connection or a
    /// Bluetooth link error), this reconnects once and calls the closure again,
    /// so `f` can run twice. Any other error is returned as it is, without
    /// reconnecting.
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

    /// Close the connection and connect again, waiting between attempts with
    /// the backoff of this device's [`ReconnectOptions`].
    ///
    /// Operations that fail while this runs wait for it instead of starting
    /// their own reconnect. A call made while another reconnect is running
    /// waits for that one instead of starting its own, and shares its result:
    /// `Ok` if it connected, `Error::Cancelled` if
    /// [`cancel_reconnect()`](Self::cancel_reconnect) stopped it, and
    /// `Error::NotConnected` if it gave up or [`disconnect()`](Self::disconnect)
    /// stopped it.
    ///
    /// `cancel_reconnect()` and `disconnect()` end a reconnect at once, and it
    /// returns `Error::Cancelled`. After `max_attempts` failed attempts it
    /// returns `Error::Timeout` and the state is [`ConnectionState::Failed`].
    /// The old connection is closed first, so until a reconnect succeeds
    /// [`name()`](Self::name) returns `None` and [`address()`](Self::address)
    /// returns the identifier.
    ///
    /// If `cancel_reconnect()` was called before this method, it returns
    /// `Error::Cancelled` without connecting. Call
    /// [`reset_cancellation()`](Self::reset_cancellation) first to clear a
    /// previous cancellation.
    pub async fn reconnect(&self) -> Result<()> {
        self.core.reconnect().await
    }

    /// Disconnect from the device.
    ///
    /// Stops a reconnect in progress, which then never installs a new
    /// connection, and closes the connection. It never waits for a reconnect's
    /// backoff or connect: it waits only for operations already running on the
    /// connection to finish, and for a connection that is already being closed
    /// (up to 5 s). The state is [`ConnectionState::Disconnected`] afterwards
    /// even if closing fails; the error is still returned. An operation that
    /// was waiting for the stopped reconnect returns `Error::NotConnected`
    /// (`Error::Cancelled` while `cancel_reconnect()` is in effect). An
    /// operation started after this returns connects again, as before.
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
    use tokio::time::Instant;

    use super::*;
    use crate::test_support::{FakeConn, FakeEvent, FakeRadio, within};

    const LIMIT: Duration = Duration::from_secs(600);

    /// The operation every test runs unless it needs a scripted result.
    fn run_op(link: &FakeConn) -> BoxFuture<'_, Result<()>> {
        Box::pin(link.op())
    }

    fn position(events: &[(Duration, FakeEvent)], wanted: &FakeEvent) -> Option<usize> {
        events.iter().position(|(_, event)| event == wanted)
    }

    fn spawn_run(core: &Arc<ReconnectCore<FakeConn>>) -> tokio::task::JoinHandle<Result<()>> {
        let core = Arc::clone(core);
        tokio::spawn(async move { core.run(run_op).await })
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
    async fn reconnect_disconnects_the_old_link_before_connecting() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = connected_core(&radio, ReconnectOptions::default()).await;
            radio.lose_link("A");

            let result = core.run(run_op).await;

            let events = radio.events();
            assert!(result.is_ok(), "{result:?} after {events:#?}");
            let closed = position(
                &events,
                &FakeEvent::Disconnect {
                    id: "A".into(),
                    handle: 1,
                },
            );
            let opened = position(
                &events,
                &FakeEvent::Connected {
                    id: "A".into(),
                    handle: 2,
                },
            );
            assert!(
                matches!((closed, opened), (Some(closed), Some(opened)) if closed < opened),
                "handle 1 must be disconnected before handle 2 connects: {events:#?}"
            );
            assert_eq!(core.link().await.map(|link| link.handle()), Some(2));
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn reconnected_link_survives_the_old_handle() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = connected_core(&radio, ReconnectOptions::default()).await;
            radio.lose_link("A");

            let first = core.run(run_op).await;
            // A real `Device` tears its link down from a spawned task; let it land.
            sleep(Duration::from_secs(1)).await;
            let second = core.run(run_op).await;

            assert_eq!(radio.connect_count("A"), 2, "{:#?}", radio.events());
            assert!(first.is_ok() && second.is_ok(), "{first:?}, {second:?}");
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn non_connection_error_is_returned_without_reconnecting() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = connected_core(&radio, ReconnectOptions::default()).await;
            let calls = AtomicU32::new(0);
            let start = Instant::now();

            let missing = core
                .run(|_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async {
                        Err::<(), _>(Error::CharacteristicNotFound {
                            uuid: "f0cd1502".into(),
                            service_count: 3,
                        })
                    })
                })
                .await;
            let invalid = core
                .run(|_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Box::pin(async { Err::<(), _>(Error::InvalidData("bad".into())) })
                })
                .await;

            assert!(
                matches!(&missing, Err(Error::CharacteristicNotFound { uuid, service_count: 3 }) if uuid == "f0cd1502"),
                "{missing:?}"
            );
            assert!(matches!(&invalid, Err(Error::InvalidData(msg)) if msg == "bad"), "{invalid:?}");
            assert_eq!(radio.connect_count("A"), 1, "{:#?}", radio.events());
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(start.elapsed(), Duration::ZERO);
            assert_eq!(core.link().await.map(|link| link.handle()), Some(1));
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn connection_error_reruns_the_operation_once() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = connected_core(&radio, ReconnectOptions::default()).await;
            let calls = AtomicU32::new(0);

            let result = core
                .run(|link| {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Box::pin(async { Err(Error::NotConnected) })
                    } else {
                        Box::pin(link.op())
                    }
                })
                .await;

            assert!(result.is_ok(), "{result:?} after {:#?}", radio.events());
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(radio.connect_count("A"), 2);
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_failures_share_one_reconnect() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = connected_core(&radio, ReconnectOptions::default()).await;
            radio.lose_link("A");

            let (first, second) = tokio::join!(core.run(run_op), core.run(run_op));

            assert_eq!(radio.connect_count("A"), 2, "{:#?}", radio.events());
            assert!(first.is_ok() && second.is_ok(), "{first:?}, {second:?}");
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn disconnect_during_backoff_stops_the_loop() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let options = ReconnectOptions::default().initial_delay(Duration::from_secs(30));
            let core = Arc::new(connected_core(&radio, options).await);
            radio.script_connects("A", [false, false, true]);
            radio.lose_link("A");
            let start = Instant::now();

            let recovering = spawn_run(&core);
            sleep(Duration::from_millis(500)).await;
            // Both start during the backoff and wait for the recovery.
            let waiting = spawn_run(&core);
            let reconnecting = tokio::spawn({
                let core = Arc::clone(&core);
                async move { core.reconnect().await }
            });
            sleep(Duration::from_millis(500)).await;
            let disconnected = core.disconnect().await;

            let recovering = recovering.await.expect("join");
            let waiting = waiting.await.expect("join");
            let reconnecting = reconnecting.await.expect("join");
            assert!(
                matches!(recovering, Err(Error::Cancelled)),
                "{recovering:?}"
            );
            assert!(matches!(waiting, Err(Error::NotConnected)), "{waiting:?}");
            assert!(
                matches!(reconnecting, Err(Error::NotConnected)),
                "{reconnecting:?}"
            );
            assert!(disconnected.is_ok(), "{disconnected:?}");
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "{:?}",
                start.elapsed()
            );
            assert_eq!(core.state().await, ConnectionState::Disconnected);

            // Longer than a loop that ignored the disconnect needs (30 + 60 + 60 s of backoff).
            sleep(Duration::from_secs(300)).await;
            let events = radio.events();
            assert!(!radio.link_up("A"), "{events:#?}");
            assert!(
                !events.iter().any(
                    |(_, event)| matches!(event, FakeEvent::Connected { handle, .. } if *handle > 1)
                ),
                "reconnected after disconnect(): {events:#?}"
            );
            assert_eq!(radio.connect_count("A"), 1, "{events:#?}");
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_reconnect_interrupts_the_backoff_sleep() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let options = ReconnectOptions::default().initial_delay(Duration::from_secs(30));
            let core = Arc::new(connected_core(&radio, options).await);

            // An operation is running when the link drops. It fails only after
            // the recovery below has started, and queues behind it.
            let in_flight = tokio::spawn({
                let core = Arc::clone(&core);
                async move {
                    core.run(|link| {
                        Box::pin(async move {
                            sleep(Duration::from_secs(1)).await;
                            link.op().await
                        })
                    })
                    .await
                }
            });
            sleep(Duration::from_millis(500)).await;
            radio.lose_link("A");

            let start = Instant::now();
            let recovering = spawn_run(&core);
            sleep(Duration::from_secs(1)).await;
            core.cancel();
            let result = recovering.await.expect("join");
            assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "cancelled after {:?}",
                start.elapsed()
            );
            let in_flight = in_flight.await.expect("join");
            assert!(matches!(in_flight, Err(Error::Cancelled)), "{in_flight:?}");
            assert_eq!(core.state().await, ConnectionState::Disconnected);

            // After reset_cancellation(), a cancel also ends a connect in progress.
            core.reset_cancellation();
            radio.set_connect_delay("A", Duration::from_secs(60));
            let start = Instant::now();
            let recovering = spawn_run(&core);
            sleep(Duration::from_secs(40)).await; // the 30 s backoff, then 10 s into the connect
            core.cancel();
            let result = recovering.await.expect("join");
            assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
            assert_eq!(start.elapsed(), Duration::from_secs(40));
            assert_eq!(
                radio.connect_count("A"),
                2,
                "the first link and the cancelled connect"
            );
            sleep(Duration::from_secs(120)).await;
            assert!(!radio.link_up("A"), "{:#?}", radio.events());
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn disconnect_error_still_marks_state_disconnected() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = connected_core(&radio, ReconnectOptions::default()).await;
            radio.fail_disconnects("A");

            let result = core.disconnect().await;

            assert!(matches!(result, Err(Error::Timeout { .. })), "{result:?}");
            assert_eq!(core.state().await, ConnectionState::Disconnected);
            assert!(core.link().await.is_none());
            assert!(!radio.link_up("A"));
        })
        .await;
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

    #[tokio::test(start_paused = true)]
    async fn queued_callers_share_a_failed_recovery() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let options = ReconnectOptions::default().max_attempts(2);
            let core = Arc::new(connected_core(&radio, options).await);
            radio.script_connects("A", [false; 4]);
            radio.lose_link("A");
            let start = Instant::now();

            let recovering = spawn_run(&core);
            sleep(Duration::from_millis(500)).await;
            // Both start during the first backoff and wait for the recovery.
            let waiting = spawn_run(&core);
            let reconnecting = tokio::spawn({
                let core = Arc::clone(&core);
                async move { core.reconnect().await }
            });

            let recovering = recovering.await.expect("join");
            let waiting = waiting.await.expect("join");
            let reconnecting = reconnecting.await.expect("join");
            assert!(
                matches!(&recovering, Err(Error::Timeout { operation, .. }) if operation.contains("reconnect to 'A'")),
                "{recovering:?}"
            );
            assert!(matches!(waiting, Err(Error::NotConnected)), "{waiting:?}");
            assert!(
                matches!(reconnecting, Err(Error::NotConnected)),
                "{reconnecting:?}"
            );
            assert_eq!(
                radio.connect_count("A"),
                3,
                "the first link and one recovery's two attempts: {:#?}",
                radio.events()
            );
            assert_eq!(start.elapsed(), Duration::from_secs(3), "1 s + 2 s of backoff");

            // An operation that starts after the failure recovers again.
            let later = core.run(run_op).await;
            assert!(matches!(later, Err(Error::Timeout { .. })), "{later:?}");
            assert_eq!(radio.connect_count("A"), 5);
            assert_eq!(core.state().await, ConnectionState::Failed);
        })
        .await;
    }

    #[tokio::test(start_paused = true)]
    async fn disconnect_while_closing_the_old_link_starts_no_attempt() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = Arc::new(connected_core(&radio, ReconnectOptions::default()).await);
            radio.lose_link("A");

            // The test runtime polls tasks in the order they were spawned or
            // woken. The recovery runs first, takes the old link out and waits
            // for the task that closes it; `disconnect()` runs next, while it
            // waits; then the closing task.
            let recovering = spawn_run(&core);
            let disconnecting = tokio::spawn({
                let core = Arc::clone(&core);
                async move { core.disconnect().await }
            });

            let recovering = recovering.await.expect("join");
            let disconnected = disconnecting.await.expect("join");
            assert!(
                matches!(recovering, Err(Error::Cancelled)),
                "{recovering:?}"
            );
            assert!(disconnected.is_ok(), "{disconnected:?}");
            assert_eq!(core.attempt_count(), 0, "{:#?}", radio.events());
            assert_eq!(core.state().await, ConnectionState::Disconnected);
            assert_eq!(radio.connect_count("A"), 1);
            assert!(!radio.link_up("A"));
        })
        .await;
    }

    /// `with_device`'s path, `run_owned`: after a connection error the
    /// closure runs once more, on the new link; after any other error it
    /// doesn't run again; and a closure that fails with a connection error
    /// both times runs twice in all.
    #[tokio::test(start_paused = true)]
    async fn run_owned_runs_again_only_after_a_connection_error_and_at_most_twice() {
        within(LIMIT, async {
            let radio = FakeRadio::new();
            let core = connected_core(&radio, ReconnectOptions::default()).await;
            let calls = AtomicU32::new(0);

            let handle = core
                .run_owned(|link| {
                    let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
                    let handle = link.handle();
                    async move {
                        if first {
                            Err(Error::NotConnected)
                        } else {
                            Ok(handle)
                        }
                    }
                })
                .await;
            assert_eq!(handle.ok(), Some(2), "{:#?}", radio.events());
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(radio.connect_count("A"), 2);

            calls.store(0, Ordering::SeqCst);
            let invalid = core
                .run_owned(|_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Err::<(), _>(Error::InvalidData("bad".into())) }
                })
                .await;
            assert!(
                matches!(&invalid, Err(Error::InvalidData(msg)) if msg == "bad"),
                "{invalid:?}"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1, "{:#?}", radio.events());
            assert_eq!(radio.connect_count("A"), 2);

            calls.store(0, Ordering::SeqCst);
            let lost = core
                .run_owned(|_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async { Err::<(), _>(Error::NotConnected) }
                })
                .await;
            assert!(matches!(lost, Err(Error::NotConnected)), "{lost:?}");
            assert_eq!(calls.load(Ordering::SeqCst), 2, "{:#?}", radio.events());
            assert_eq!(radio.connect_count("A"), 3, "one reconnect");
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// A stop that comes in after a recovery's connect has returned, but
    /// before the new link is installed, still stops it: the new link is
    /// closed instead of installed, and the recovering call returns
    /// `Cancelled`. An operation's read guard holds the recovery at that
    /// point here, once for `cancel_reconnect()` and once for `disconnect()`,
    /// which also moves the generation on. A stop during the connect itself
    /// ends the connect instead and never gets this far.
    #[tokio::test(start_paused = true)]
    async fn a_stop_after_the_connect_returns_closes_the_new_link() {
        within(LIMIT, async {
            for stop in ["cancel_reconnect", "disconnect"] {
                let radio = FakeRadio::new();
                let core = Arc::new(connected_core(&radio, ReconnectOptions::default()).await);
                radio.set_connect_delay("A", Duration::from_secs(10));
                radio.lose_link("A");

                // 1 s of backoff, then a connect that returns at 11 s, while
                // the read guard taken at 5 s keeps its link from being
                // installed.
                let recovering = spawn_run(&core);
                sleep(Duration::from_secs(5)).await;
                let reading = core.link.read().await;
                sleep(Duration::from_secs(10)).await;
                assert_eq!(radio.connect_count("A"), 2, "{stop}: {:#?}", radio.events());
                let disconnecting = if stop == "disconnect" {
                    let core = Arc::clone(&core);
                    Some(tokio::spawn(async move { core.disconnect().await }))
                } else {
                    core.cancel();
                    None
                };
                // The disconnect runs up to its wait for the recovery.
                sleep(Duration::from_millis(10)).await;
                drop(reading);

                let result = recovering.await.expect("join");
                assert!(
                    matches!(result, Err(Error::Cancelled)),
                    "{stop}: {result:?}"
                );
                if let Some(disconnecting) = disconnecting {
                    let disconnected = disconnecting.await.expect("join");
                    assert!(disconnected.is_ok(), "{disconnected:?}");
                }
                let events = radio.events();
                assert!(
                    core.link().await.is_none(),
                    "{stop}: the new link was installed: {events:#?}"
                );
                assert_eq!(core.state().await, ConnectionState::Disconnected, "{stop}");
                assert!(
                    position(
                        &events,
                        &FakeEvent::Disconnect {
                            id: "A".into(),
                            handle: 2,
                        }
                    )
                    .is_some(),
                    "{stop}: the new link was not closed: {events:#?}"
                );
                assert!(!radio.link_up("A"), "{stop}: {events:#?}");
                radio.assert_no_drop_teardown();
            }
        })
        .await;
    }
}
