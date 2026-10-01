//! Multi-device management.
//!
//! This module provides a manager for handling multiple Aranet devices
//! simultaneously, with connection pooling and concurrent operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

use crate::connector::{ConnectFn, SensorLink, ble_connector, log_failed_release, release_link};
use crate::device::Device;
use crate::error::{ConnectionFailureReason, Error, Result};
use crate::events::{DeviceEvent, DeviceId, DisconnectReason, EventDispatcher};
use crate::passive::{PassiveMonitor, PassiveMonitorOptions, PassiveReading};
use crate::reconnect::ReconnectOptions;
use crate::scan::{DiscoveredDevice, ScanOptions, scan_with_options};

/// Device priority levels for connection management.
///
/// The manager never disconnects a device on its own to make room for
/// another: when the connection limit is reached, `connect()` fails, and
/// [`DeviceManager::evict_lowest_priority`] frees a slot when you call it.
/// The health monitor ([`DeviceManager::start_health_monitor`]) reconnects
/// lost devices highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum DevicePriority {
    /// Low priority: evicted first by `evict_lowest_priority`.
    Low,
    /// Normal priority (default).
    #[default]
    Normal,
    /// High priority: evicted only when no `Low` or `Normal` device is connected.
    High,
    /// Critical priority: never evicted.
    Critical,
}

/// Adaptive interval that adjusts based on connection stability.
///
/// This is used by the health monitor to check connections more frequently
/// when connections are unstable, and less frequently when stable.
#[derive(Debug, Clone)]
pub struct AdaptiveInterval {
    /// Base interval when connections are stable.
    pub base: Duration,
    /// Current interval (may differ from base based on stability).
    current: Duration,
    /// Minimum interval (most frequent checking).
    pub min: Duration,
    /// Maximum interval (least frequent checking).
    pub max: Duration,
    /// Number of consecutive successes.
    consecutive_successes: u32,
    /// Number of consecutive failures.
    consecutive_failures: u32,
    /// Success threshold before increasing interval.
    success_threshold: u32,
    /// Failure threshold before decreasing interval.
    failure_threshold: u32,
}

impl Default for AdaptiveInterval {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(30),
            current: Duration::from_secs(30),
            min: Duration::from_secs(5),
            max: Duration::from_secs(120),
            consecutive_successes: 0,
            consecutive_failures: 0,
            success_threshold: 3,
            failure_threshold: 1,
        }
    }
}

impl AdaptiveInterval {
    /// Create a new adaptive interval with custom settings.
    pub fn new(base: Duration, min: Duration, max: Duration) -> Self {
        Self {
            base,
            current: base,
            min,
            max,
            ..Default::default()
        }
    }

    /// Get the current interval.
    pub fn current(&self) -> Duration {
        self.current
    }

    /// Record a successful health check.
    ///
    /// After enough consecutive successes, the interval will increase
    /// (less frequent checks) up to the maximum.
    pub fn on_success(&mut self) {
        self.consecutive_failures = 0;
        self.consecutive_successes += 1;

        if self.consecutive_successes >= self.success_threshold {
            // Double the interval, capped at max
            let new_interval = self.current.saturating_mul(2);
            self.current = new_interval.min(self.max);
            self.consecutive_successes = 0;
            debug!(
                "Health check stable, increasing interval to {:?}",
                self.current
            );
        }
    }

    /// Record a failed health check (connection lost or reconnect needed).
    ///
    /// After enough consecutive failures, the interval will decrease
    /// (more frequent checks) down to the minimum.
    pub fn on_failure(&mut self) {
        self.consecutive_successes = 0;
        self.consecutive_failures += 1;

        if self.consecutive_failures >= self.failure_threshold {
            // Halve the interval, capped at min
            let new_interval = self.current / 2;
            self.current = new_interval.max(self.min);
            self.consecutive_failures = 0;
            debug!(
                "Health check unstable, decreasing interval to {:?}",
                self.current
            );
        }
    }

    /// Reset to the base interval.
    pub fn reset(&mut self) {
        self.current = self.base;
        self.consecutive_successes = 0;
        self.consecutive_failures = 0;
    }
}

/// Information about a managed device.
///
/// [`DeviceManager`] doesn't store `ManagedDevice` values, and none of its
/// methods return one. The type is kept so that code which names it still
/// compiles.
#[derive(Debug)]
pub struct ManagedDevice {
    /// Device identifier.
    pub id: String,
    /// Device name.
    pub name: Option<String>,
    /// Device type.
    pub device_type: Option<DeviceType>,
    /// The connected device (if connected).
    /// Wrapped in Arc to allow concurrent access without holding the manager lock.
    device: Option<Arc<Device>>,
    /// Whether auto-reconnect is enabled.
    pub auto_reconnect: bool,
    /// Last known reading.
    pub last_reading: Option<CurrentReading>,
    /// Device info.
    pub info: Option<DeviceInfo>,
    /// Reconnection options (if auto-reconnect is enabled).
    pub reconnect_options: ReconnectOptions,
    /// Device priority for connection management.
    pub priority: DevicePriority,
    /// Number of consecutive connection failures.
    pub consecutive_failures: u32,
    /// Last successful connection timestamp (Unix epoch millis).
    pub last_success: Option<u64>,
}

impl ManagedDevice {
    /// Create a new managed device entry.
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            name: None,
            device_type: None,
            device: None,
            auto_reconnect: true,
            last_reading: None,
            info: None,
            reconnect_options: ReconnectOptions::default(),
            priority: DevicePriority::default(),
            consecutive_failures: 0,
            last_success: None,
        }
    }

    /// Create a managed device with custom reconnect options.
    pub fn with_reconnect_options(id: &str, options: ReconnectOptions) -> Self {
        Self {
            reconnect_options: options,
            ..Self::new(id)
        }
    }

    /// Create a managed device with priority.
    pub fn with_priority(id: &str, priority: DevicePriority) -> Self {
        Self {
            priority,
            ..Self::new(id)
        }
    }

    /// Create a managed device with reconnect options and priority.
    pub fn with_options(id: &str, options: ReconnectOptions, priority: DevicePriority) -> Self {
        Self {
            reconnect_options: options,
            priority,
            ..Self::new(id)
        }
    }

    /// Record a successful operation.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_success = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );
    }

    /// Record a failed operation.
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
    }

    /// Check if the device is connected (sync check, doesn't query BLE).
    pub fn has_device(&self) -> bool {
        self.device.is_some()
    }

    /// Check if the device is connected (async, queries BLE).
    pub async fn is_connected(&self) -> bool {
        if let Some(device) = &self.device {
            device.is_connected().await
        } else {
            false
        }
    }

    /// Get a reference to the underlying device.
    pub fn device(&self) -> Option<&Arc<Device>> {
        self.device.as_ref()
    }

    /// Get a clone of the device Arc.
    pub fn device_arc(&self) -> Option<Arc<Device>> {
        self.device.clone()
    }
}

/// Configuration for the device manager.
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// Default scan options.
    pub scan_options: ScanOptions,
    /// Default reconnect options for new devices.
    ///
    /// The health monitor ([`DeviceManager::start_health_monitor`]) waits
    /// between automatic reconnects of a device as these options say, and
    /// stops after `max_attempts` failures in a row, emitting one
    /// [`DeviceEvent::Error`]; [`DeviceManager::connect`] starts over. With
    /// `max_attempts` of 0 it never reconnects a device that has been
    /// connected, but a device that has never been connected, such as one
    /// just added, still gets one attempt.
    ///
    /// The default is [`ReconnectOptions::unlimited`] since 0.3.0. With the
    /// five attempts of `ReconnectOptions::default()`, the monitor would give
    /// up on a device after about a minute.
    pub default_reconnect_options: ReconnectOptions,
    /// Event channel capacity.
    pub event_capacity: usize,
    /// Health check interval for auto-reconnect (base interval).
    pub health_check_interval: Duration,
    /// Maximum number of concurrent BLE connections.
    ///
    /// Most BLE adapters support 5-7 concurrent connections.
    /// Attempting to connect beyond this limit will return an error.
    /// Set to 0 for no limit (not recommended).
    pub max_concurrent_connections: usize,
    /// Whether to use adaptive health check intervals.
    ///
    /// When enabled, the health check interval will automatically adjust:
    /// - Decrease (more frequent) when connections are unstable
    /// - Increase (less frequent) when connections are stable
    pub use_adaptive_interval: bool,
    /// Minimum health check interval (for adaptive mode).
    pub min_health_check_interval: Duration,
    /// Maximum health check interval (for adaptive mode).
    pub max_health_check_interval: Duration,
    /// Default priority for new devices.
    pub default_priority: DevicePriority,
    /// Whether to use connection validation (keepalive checks).
    ///
    /// When enabled, health checks use `device.validate_connection()`, which
    /// reads the current measurements to verify the connection is alive. That
    /// read needs no pairing that reading the sensor doesn't. This catches
    /// "zombie connections" but uses more power.
    ///
    /// When disabled, health checks only ask the Bluetooth stack whether the
    /// device is connected. A zombie connection passes that check, so the
    /// health monitor replaces it only once the stack reports it lost.
    pub use_connection_validation: bool,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        // Use platform-specific defaults if available
        let platform_config = crate::platform::PlatformConfig::for_current_platform();

        Self {
            scan_options: ScanOptions::default(),
            default_reconnect_options: ReconnectOptions::unlimited(),
            event_capacity: 100,
            health_check_interval: Duration::from_secs(30),
            max_concurrent_connections: platform_config.max_concurrent_connections,
            use_adaptive_interval: true,
            min_health_check_interval: Duration::from_secs(5),
            max_health_check_interval: Duration::from_secs(120),
            default_priority: DevicePriority::Normal,
            use_connection_validation: true,
        }
    }
}

impl ManagerConfig {
    /// Create a configuration with a specific connection limit.
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_concurrent_connections = max;
        self
    }

    /// Create a configuration with no connection limit (not recommended).
    pub fn unlimited_connections(mut self) -> Self {
        self.max_concurrent_connections = 0;
        self
    }

    /// Enable or disable adaptive health check intervals.
    pub fn adaptive_interval(mut self, enabled: bool) -> Self {
        self.use_adaptive_interval = enabled;
        self
    }

    /// Set the health check interval (base interval for adaptive mode).
    pub fn health_check_interval(mut self, interval: Duration) -> Self {
        self.health_check_interval = interval;
        self
    }

    /// Set the default device priority.
    pub fn default_priority(mut self, priority: DevicePriority) -> Self {
        self.default_priority = priority;
        self
    }

    /// Enable or disable connection validation in health checks.
    pub fn connection_validation(mut self, enabled: bool) -> Self {
        self.use_connection_validation = enabled;
        self
    }
}

/// Longest wait the health monitor schedules before a device's next automatic
/// reconnect. It only caps `ReconnectOptions::max_delay` values so large that
/// adding them to an `Instant` could overflow.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// A managed device's state inside `ManagerCore`.
struct Entry<L> {
    name: Option<String>,
    device_type: Option<DeviceType>,
    info: Option<DeviceInfo>,
    last_reading: Option<CurrentReading>,
    priority: DevicePriority,
    reconnect_options: ReconnectOptions,
    auto_reconnect: bool,
    /// Consecutive failed health-monitor reconnects.
    failures: u32,
    /// Whether the user wants the device connected. New entries start wanted;
    /// `connect` sets it; `disconnect`, `disconnect_all`,
    /// `evict_lowest_priority` and `remove_device` clear it. `add_device*` on
    /// an existing entry leave it as it is, so only `connect` re-arms a
    /// withdrawn device. The health monitor checks and repairs only wanted
    /// devices.
    wanted: bool,
    /// When the health monitor may next try to reconnect the device (`None`:
    /// at once).
    retry_at: Option<Instant>,
    /// Set when automatic reconnects used up `max_attempts`; cleared by `connect`.
    gave_up: bool,
    /// Whether a link has ever been stored in this entry. Until then, the
    /// health monitor's connect is the device's first connect, not a
    /// reconnect, and `max_attempts` of 0 doesn't stop it.
    ever_connected: bool,
    /// The connection, if connected.
    link: Option<Arc<L>>,
    /// The connection-limit permit, taken before connecting and held until
    /// the link has been disconnected.
    slot: Option<OwnedSemaphorePermit>,
    /// Serialises connect, disconnect and removal of this device. Taken
    /// without holding the device map lock.
    op: Arc<Mutex<()>>,
}

impl<L> Entry<L> {
    fn new(reconnect_options: ReconnectOptions, priority: DevicePriority) -> Self {
        Self {
            name: None,
            device_type: None,
            info: None,
            last_reading: None,
            priority,
            reconnect_options,
            auto_reconnect: true,
            failures: 0,
            wanted: true,
            retry_at: None,
            gave_up: false,
            ever_connected: false,
            link: None,
            slot: None,
            op: Arc::new(Mutex::new(())),
        }
    }

    /// Whether the health monitor should reconnect this device now.
    fn is_due_for_repair(&self) -> bool {
        self.auto_reconnect
            && self.wanted
            && !self.gave_up
            && self.link.is_none()
            && self.retry_at.is_none_or(|at| at <= Instant::now())
    }

    /// Gives up on automatic reconnects if they have used up `max_attempts`,
    /// and then returns the number of failures.
    fn give_up_if_attempts_used_up(&mut self) -> Option<u32> {
        let used_up = self
            .reconnect_options
            .max_attempts
            .is_some_and(|max| self.failures >= max);
        if used_up && !self.gave_up {
            self.gave_up = true;
            return Some(self.failures);
        }
        None
    }

    /// Before an automatic reconnect: gives up without one if `max_attempts`
    /// is already used up, and then returns the number of failures. Only
    /// `max_attempts` of 0 can be used up here, as a failure that uses up a
    /// larger one gives up at once. A device that has never been connected
    /// isn't reconnected but connected for the first time, so it still gets
    /// that attempt.
    fn give_up_before_reconnect(&mut self) -> Option<u32> {
        if self.ever_connected {
            self.give_up_if_attempts_used_up()
        } else {
            None
        }
    }

    /// Records a failed automatic reconnect and schedules the next one.
    /// Returns the number of failures when this one used up `max_attempts`.
    fn record_repair_failure(&mut self, now: Instant) -> Option<u32> {
        self.failures = self.failures.saturating_add(1);
        let wait = self
            .reconnect_options
            .delay_for_attempt(self.failures - 1)
            .min(MAX_RETRY_WAIT);
        self.retry_at = Some(now + wait);
        self.give_up_if_attempts_used_up()
    }

    /// Starts automatic reconnects over, for an explicit `connect` or once a
    /// new link is stored: the next repair is due at once, with all of
    /// `max_attempts` available again.
    fn reset_backoff(&mut self) {
        self.failures = 0;
        self.retry_at = None;
        self.gave_up = false;
    }
}

/// What the lifecycle tests can see of an entry.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntrySnapshot {
    pub(crate) has_link: bool,
    pub(crate) failures: u32,
    pub(crate) wanted: bool,
    pub(crate) gave_up: bool,
    pub(crate) retry_at: Option<Instant>,
}

/// A new link and its slot, not yet stored in the link's entry, with a share
/// of its connect's `op` guard. If it is dropped before `into_parts` (its
/// connect was cancelled while waiting for the device map), it disconnects
/// the link on a spawned task, which keeps the slot and the share of the
/// guard until the link is down.
struct NewLink<L: SensorLink> {
    /// The device the link connects, for the log.
    identifier: String,
    parts: Option<(Arc<L>, Option<OwnedSemaphorePermit>)>,
    held: Arc<tokio::sync::OwnedMutexGuard<()>>,
}

impl<L: SensorLink> NewLink<L> {
    fn new(
        identifier: &str,
        link: Arc<L>,
        slot: Option<OwnedSemaphorePermit>,
        held: &Arc<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Self {
        Self {
            identifier: identifier.to_owned(),
            parts: Some((link, slot)),
            held: Arc::clone(held),
        }
    }

    /// Hands over the link and its slot; dropping `self` then only gives back
    /// its share of the guard, which the connect still holds.
    fn into_parts(mut self) -> (Arc<L>, Option<OwnedSemaphorePermit>) {
        self.parts
            .take()
            .expect("a NewLink is taken apart only once")
    }
}

impl<L: SensorLink> Drop for NewLink<L> {
    fn drop(&mut self) {
        let Some((link, slot)) = self.parts.take() else {
            return;
        };
        // Without a runtime, dropping the link runs its own teardown.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let held = Arc::clone(&self.held);
            let identifier = std::mem::take(&mut self.identifier);
            runtime.spawn(async move {
                if let Err(e) = release_link(link, (slot, held)).await {
                    log_failed_release("Disconnecting an abandoned new link", &identifier, &e);
                }
            });
        }
    }
}

/// Awaits a disconnect or removal task. A task that panicked, or that was
/// dropped when its runtime shut down, gives an `Error::Io`.
async fn finished(task: tokio::task::JoinHandle<Result<()>>) -> Result<()> {
    task.await.map_err(std::io::Error::from)?
}

/// What one health-monitor tick did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TickOutcome {
    /// Connected devices that passed their check.
    pub(crate) healthy: usize,
    /// Devices this tick reconnected.
    pub(crate) repaired: usize,
    /// Reconnects that failed.
    pub(crate) failed: usize,
}

/// The device manager's logic, generic over the connection type so that the
/// lifecycle tests can run it on `test_support::FakeRadio`. `DeviceManager` is
/// this over `Device`.
pub(crate) struct ManagerCore<L: SensorLink> {
    devices: RwLock<HashMap<String, Entry<L>>>,
    events: EventDispatcher,
    config: ManagerConfig,
    connect: ConnectFn<L>,
    /// One permit per allowed connection; `None` when connections are not limited.
    slots: Option<Arc<Semaphore>>,
}

impl<L: SensorLink> ManagerCore<L> {
    pub(crate) fn new(config: ManagerConfig, connect: ConnectFn<L>) -> Self {
        let max = config.max_concurrent_connections;
        let slots = if max == 0 {
            None
        } else if max > Semaphore::MAX_PERMITS {
            warn!(
                "max_concurrent_connections ({max}) is above {}; connections are not limited",
                Semaphore::MAX_PERMITS
            );
            None
        } else {
            Some(Arc::new(Semaphore::new(max)))
        };
        Self {
            devices: RwLock::new(HashMap::new()),
            events: EventDispatcher::new(config.event_capacity),
            config,
            connect,
            slots,
        }
    }

    /// The error for a connect that the connection limit rejects.
    fn limit_error(&self, identifier: &str) -> Error {
        let max = self.config.max_concurrent_connections;
        let free = self
            .slots
            .as_ref()
            .map_or(0, |slots| slots.available_permits());
        let used = max.saturating_sub(free);
        warn!("Connection limit reached ({used}/{max}), cannot connect to {identifier}");
        Error::connection_failed(
            Some(identifier.to_string()),
            ConnectionFailureReason::Other(format!("Connection limit reached ({used}/{max})")),
        )
    }

    /// Takes a connection slot for `identifier`: `None` when connections are
    /// not limited, the limit error when no slot is free.
    fn take_slot(&self, identifier: &str) -> Result<Option<OwnedSemaphorePermit>> {
        match &self.slots {
            Some(slots) => Arc::clone(slots)
                .try_acquire_owned()
                .map(Some)
                .map_err(|_| self.limit_error(identifier)),
            None => Ok(None),
        }
    }

    pub(crate) async fn add_device(&self, identifier: &str) -> Result<()> {
        self.add_device_with_options(identifier, self.config.default_reconnect_options.clone())
            .await
    }

    pub(crate) async fn add_device_with_options(
        &self,
        identifier: &str,
        reconnect_options: ReconnectOptions,
    ) -> Result<()> {
        reconnect_options.validate()?;
        let mut devices = self.devices.write().await;

        if devices.contains_key(identifier) {
            return Ok(()); // Already exists
        }

        devices.insert(
            identifier.to_string(),
            Entry::new(reconnect_options, DevicePriority::default()),
        );

        info!("Added device to manager: {}", identifier);
        Ok(())
    }

    pub(crate) async fn connect(&self, identifier: &str) -> Result<()> {
        let (op, reserved) = {
            let mut devices = self.devices.write().await;
            // A device that the manager doesn't know yet takes its slot here,
            // under the same lock that adds it, so a connect that the limit
            // rejects doesn't add the device.
            let reserved = if devices.contains_key(identifier) {
                None
            } else {
                self.take_slot(identifier)?
            };
            let entry = devices.entry(identifier.to_string()).or_insert_with(|| {
                info!("Adding device to manager: {identifier}");
                Entry::new(
                    self.config.default_reconnect_options.clone(),
                    DevicePriority::default(),
                )
            });
            // An explicit connect: keep the device connected from now on.
            entry.wanted = true;
            (Arc::clone(&entry.op), reserved)
        };
        // The map lock is released before waiting. One connect, disconnect or
        // removal of this device runs at a time. A cancelled caller releases
        // the guard, and a reserved slot, with its future, except that a new
        // link it abandons keeps a share of the guard until it is down.
        let held = Arc::new(Arc::clone(&op).lock_owned().await);
        // An explicit connect starts automatic reconnects over. This runs under
        // the guard, so a repair that failed while this connect waited can't
        // leave its failure count or give-up behind.
        if let Some(entry) = self.devices.write().await.get_mut(identifier)
            && Arc::ptr_eq(&entry.op, &op)
        {
            entry.reset_backoff();
        }
        self.connect_locked(identifier, &held, reserved).await
    }

    /// Connects `identifier` unless it already has a link that the Bluetooth
    /// stack reports as up; a link that is down is closed and replaced. The
    /// caller holds the entry's `op` guard and passes it, shared, as `held`;
    /// a lost link being closed (see `detach`) and a new link that is
    /// abandoned each keep a share of it until that link is down.
    /// `reserved` is the slot that `connect` took when it added the device;
    /// without one, a slot is taken here.
    async fn connect_locked(
        &self,
        identifier: &str,
        held: &Arc<tokio::sync::OwnedMutexGuard<()>>,
        reserved: Option<OwnedSemaphorePermit>,
    ) -> Result<()> {
        let op = tokio::sync::OwnedMutexGuard::mutex(held);
        let existing = {
            let devices = self.devices.read().await;
            match devices.get(identifier) {
                Some(entry) if Arc::ptr_eq(&entry.op, op) && entry.wanted => entry.link.clone(),
                // Disconnected or removed while this connect waited for the guard.
                _ => return Err(Error::Cancelled),
            }
        };
        if let Some(link) = existing {
            if link.is_connected().await {
                debug!("Device {identifier} is already connected");
                return Ok(());
            }
            info!("The connection to {identifier} was lost; reconnecting");
            self.detach(identifier, &link, held, DisconnectReason::Unknown)
                .await;
        }

        // Stored with the link and held until the link has been disconnected.
        // Dropped here if the connect fails or its caller is cancelled.
        let slot = match reserved {
            Some(slot) => Some(slot),
            None => self.take_slot(identifier)?,
        };

        let new_link = NewLink::new(
            identifier,
            Arc::new((self.connect)(identifier).await?),
            slot,
            held,
        );

        // Store the link as soon as the map lock is free, so that a cancel from
        // then on leaves a link that the manager holds. A cancel while waiting
        // for the lock drops `new_link`, which disconnects it.
        let stored = {
            let mut devices = self.devices.write().await;
            match devices.get_mut(identifier) {
                Some(entry) if Arc::ptr_eq(&entry.op, op) && entry.wanted => {
                    let (link, slot) = new_link.into_parts();
                    entry.link = Some(Arc::clone(&link));
                    entry.slot = slot;
                    entry.ever_connected = true;
                    // The device is connected from here on, even if this
                    // connect is cancelled before it returns.
                    entry.reset_backoff();
                    Ok(link)
                }
                _ => Err(new_link),
            }
        };
        let link = match stored {
            Ok(link) => link,
            Err(new_link) => {
                debug!("{identifier} was disconnected or removed; closing the new link");
                let (link, slot) = new_link.into_parts();
                if let Err(e) = release_link(link, (slot, Arc::clone(held))).await {
                    log_failed_release("Disconnecting the unused link", identifier, &e);
                }
                return Err(Error::Cancelled);
            }
        };

        let info = link.read_device_info().await.ok();
        let device_type = link.device_type();
        let name = link.name().map(|s| s.to_string());
        {
            let mut devices = self.devices.write().await;
            match devices.get_mut(identifier) {
                Some(entry) if Arc::ptr_eq(&entry.op, op) => {
                    entry.info = info.clone();
                    entry.device_type = device_type;
                    entry.name = name.clone();
                }
                // Whoever removed the entry released its link.
                _ => return Err(Error::Cancelled),
            }
        }

        // Emit event
        self.events.send(DeviceEvent::Connected {
            device: DeviceId {
                id: identifier.to_string(),
                name,
                device_type,
            },
            info,
        });

        info!("Connected to device: {identifier}");
        Ok(())
    }

    /// Marks the device as not wanted, so that a connect or repair of it
    /// that is running closes its new link instead of installing it, and the
    /// health monitor leaves the device alone until `connect`. Returns the
    /// entry's `op` mutex, or `None` if the device isn't managed.
    async fn withdraw(&self, identifier: &str) -> Option<Arc<Mutex<()>>> {
        let mut devices = self.devices.write().await;
        let entry = devices.get_mut(identifier)?;
        entry.wanted = false;
        Some(Arc::clone(&entry.op))
    }

    pub(crate) async fn disconnect(self: &Arc<Self>, identifier: &str) -> Result<()> {
        let Some(op) = self.withdraw(identifier).await else {
            return Ok(());
        };
        finished(self.spawn_disconnect(identifier, op)).await
    }

    /// Disconnects a withdrawn device (see `withdraw`) on a task, once its
    /// `op` mutex is free. The health monitor leaves a withdrawn device alone,
    /// so nothing else would close its link: the task finishes even if the
    /// caller is dropped, for example while it waits for a connect or a
    /// check of the device to end. A `connect()` that re-arms the device
    /// before the task takes the link supersedes the disconnect, and the task
    /// then does nothing (see `disconnect_locked`).
    fn spawn_disconnect(
        self: &Arc<Self>,
        identifier: &str,
        op: Arc<Mutex<()>>,
    ) -> tokio::task::JoinHandle<Result<()>> {
        let core = Arc::clone(self);
        let identifier = identifier.to_owned();
        tokio::spawn(async move {
            let held = Arc::new(op.lock_owned().await);
            core.disconnect_locked(&identifier, &held, true).await
        })
    }

    /// Disconnects `identifier`'s link, if it has one, and returns the
    /// close's result. It fails only after taking the link out of the entry.
    /// With `unless_rearmed`, it does nothing if the entry is wanted again: a
    /// `connect()` has re-armed the device since it was withdrawn, so that
    /// connect started before this took the link, and supersedes it. The
    /// check and the take share one lock of the device map, so no connect
    /// can come between them.
    /// `held` is the caller's guard of the entry's `op` mutex. The disconnect
    /// keeps a share of that guard, and the slot, until the link is down,
    /// even if this future is dropped: until then no other connect,
    /// disconnect or removal of the device starts, so none can overlap the
    /// old link's disconnect.
    async fn disconnect_locked(
        &self,
        identifier: &str,
        held: &Arc<tokio::sync::OwnedMutexGuard<()>>,
        unless_rearmed: bool,
    ) -> Result<()> {
        let op = tokio::sync::OwnedMutexGuard::mutex(held);
        let taken = {
            let mut devices = self.devices.write().await;
            match devices.get_mut(identifier) {
                Some(entry) if Arc::ptr_eq(&entry.op, op) && !(unless_rearmed && entry.wanted) => {
                    entry.link.take().map(|link| (link, entry.slot.take()))
                }
                _ => None,
            }
        };
        let Some((link, slot)) = taken else {
            return Ok(());
        };

        // The link has left the manager: say so before the close, as `detach`
        // does. The close can fail (on macOS, closing a link that dropped on
        // its own times out) or go on in the background if this future is
        // dropped, and nothing would send the event later.
        self.events.send(DeviceEvent::Disconnected {
            device: DeviceId::new(identifier),
            reason: DisconnectReason::UserRequested,
        });
        release_link(link, (slot, Arc::clone(held))).await
    }

    pub(crate) async fn remove_device(self: &Arc<Self>, identifier: &str) -> Result<()> {
        let Some(op) = self.withdraw(identifier).await else {
            return Ok(());
        };
        // On a task, as in `spawn_disconnect`, so that a dropped caller still
        // removes the device it has withdrawn.
        let core = Arc::clone(self);
        let identifier = identifier.to_owned();
        finished(tokio::spawn(async move {
            let held = Arc::new(Arc::clone(&op).lock_owned().await);
            // Not `disconnect`: the guard is not reentrant. A removal always
            // goes ahead, even if a `connect()` has re-armed the device.
            let result = core.disconnect_locked(&identifier, &held, false).await;
            // Removed even if closing the link failed: the link has left the
            // manager, so a kept entry would have nothing left to close.
            let mut devices = core.devices.write().await;
            if devices
                .get(&identifier)
                .is_some_and(|entry| Arc::ptr_eq(&entry.op, &op))
            {
                devices.remove(&identifier);
                info!("Removed device from manager: {identifier}");
            }
            result
        }))
        .await
    }

    pub(crate) async fn device_ids(&self) -> Vec<String> {
        self.devices.read().await.keys().cloned().collect()
    }

    pub(crate) async fn device_count(&self) -> usize {
        self.devices.read().await.len()
    }

    pub(crate) async fn connected_count(&self) -> usize {
        let devices = self.devices.read().await;
        devices
            .values()
            .filter(|entry| entry.link.is_some())
            .count()
    }

    pub(crate) async fn can_connect(&self) -> bool {
        self.slots
            .as_ref()
            .is_none_or(|slots| slots.available_permits() > 0)
    }

    pub(crate) async fn connection_status(&self) -> (usize, usize) {
        (
            self.connected_count().await,
            self.config.max_concurrent_connections,
        )
    }

    pub(crate) async fn available_connections(&self) -> Option<usize> {
        self.slots.as_ref().map(|slots| slots.available_permits())
    }

    pub(crate) async fn connected_count_verified(&self) -> usize {
        // Collect links while holding the lock briefly
        let links: Vec<Arc<L>> = {
            let devices = self.devices.read().await;
            devices
                .values()
                .filter_map(|entry| entry.link.clone())
                .collect()
        };
        // Lock is released here

        // Check connection status in parallel
        let results = join_all(links.iter().map(|link| link.is_connected())).await;

        results.into_iter().filter(|&connected| connected).count()
    }

    pub(crate) async fn read_current(&self, identifier: &str) -> Result<CurrentReading> {
        // Get the link while holding the lock briefly
        let link = {
            let devices = self.devices.read().await;
            let entry = devices
                .get(identifier)
                .ok_or_else(|| Error::device_not_found(identifier))?;
            entry.link.clone().ok_or(Error::NotConnected)?
        };
        // Lock is released here

        let reading = link.read_current().await?;

        // Emit reading event
        self.events.send(DeviceEvent::Reading {
            device: DeviceId::new(identifier),
            reading,
        });

        // Update cached reading
        {
            let mut devices = self.devices.write().await;
            if let Some(entry) = devices.get_mut(identifier) {
                entry.last_reading = Some(reading);
            }
        }

        Ok(reading)
    }

    pub(crate) async fn read_all(&self) -> HashMap<String, Result<CurrentReading>> {
        // Collect links while holding the lock briefly
        let links: Vec<(String, Arc<L>)> = {
            let devices = self.devices.read().await;
            devices
                .iter()
                .filter_map(|(id, entry)| entry.link.clone().map(|link| (id.clone(), link)))
                .collect()
        };
        // Lock is released here

        // Perform all reads in parallel
        let read_futures = links.into_iter().map(|(id, link)| async move {
            let result = link.read_current().await;
            (id, result)
        });

        let read_results: Vec<(String, Result<CurrentReading>)> = join_all(read_futures).await;

        // Emit events and update cache
        for (id, result) in &read_results {
            if let Ok(reading) = result {
                self.events.send(DeviceEvent::Reading {
                    device: DeviceId::new(id),
                    reading: *reading,
                });
            }
        }

        // Update cached readings
        {
            let mut devices = self.devices.write().await;
            for (id, result) in &read_results {
                if let Ok(reading) = result
                    && let Some(entry) = devices.get_mut(id)
                {
                    entry.last_reading = Some(*reading);
                }
            }
        }

        read_results.into_iter().collect()
    }

    pub(crate) async fn connect_all(&self) -> HashMap<String, Result<()>> {
        let ids: Vec<_> = self.devices.read().await.keys().cloned().collect();

        // Note: We can't fully parallelize connect because it modifies state,
        // but we can at least attempt connections concurrently
        let connect_futures = ids.into_iter().map(|id| async move {
            let result = self.connect(&id).await;
            (id, result)
        });

        join_all(connect_futures).await.into_iter().collect()
    }

    pub(crate) async fn disconnect_all(self: &Arc<Self>) -> HashMap<String, Result<()>> {
        let withdrawn: Vec<(String, Arc<Mutex<()>>)> = {
            let mut devices = self.devices.write().await;
            // Withdraw every device, including those without a link (not
            // connected yet, or lost and waiting for a repair), so the health
            // monitor connects none of them afterwards.
            for entry in devices.values_mut() {
                entry.wanted = false;
            }
            devices
                .iter()
                .filter(|(_, entry)| entry.link.is_some())
                .map(|(id, entry)| (id.clone(), Arc::clone(&entry.op)))
                .collect()
        };
        // Every disconnect starts before anything else is awaited, so that a
        // dropped caller can't leave a device it has withdrawn connected.
        let disconnects: Vec<_> = withdrawn
            .into_iter()
            .map(|(id, op)| {
                let task = self.spawn_disconnect(&id, op);
                async move { (id, finished(task).await) }
            })
            .collect();
        join_all(disconnects).await.into_iter().collect()
    }

    pub(crate) fn try_is_connected(&self, identifier: &str) -> Option<bool> {
        // Try to acquire the lock without blocking
        match self.devices.try_read() {
            Ok(devices) => Some(
                devices
                    .get(identifier)
                    .is_some_and(|entry| entry.link.is_some()),
            ),
            Err(_) => None, // Lock was held, couldn't check
        }
    }

    pub(crate) async fn is_connected(&self, identifier: &str) -> bool {
        let link = {
            let devices = self.devices.read().await;
            devices.get(identifier).and_then(|entry| entry.link.clone())
        };

        if let Some(link) = link {
            link.is_connected().await
        } else {
            false
        }
    }

    pub(crate) async fn get_device_info(&self, identifier: &str) -> Option<DeviceInfo> {
        let devices = self.devices.read().await;
        devices.get(identifier).and_then(|entry| entry.info.clone())
    }

    pub(crate) async fn get_last_reading(&self, identifier: &str) -> Option<CurrentReading> {
        let devices = self.devices.read().await;
        devices.get(identifier).and_then(|entry| entry.last_reading)
    }

    /// One pass of the health monitor.
    ///
    /// Checks every connected device at once and closes the links that fail,
    /// then reconnects the devices that are due, one at a time and highest
    /// priority first. A device whose connect, disconnect or removal is
    /// running is left alone.
    pub(crate) async fn health_tick(&self) -> TickOutcome {
        let mut outcome = TickOutcome::default();

        // First pass: check every connected device at once, so a slow device
        // doesn't hold up the others.
        let targets: Vec<_> = {
            let devices = self.devices.read().await;
            devices
                .iter()
                // A device being disconnected or removed belongs to that call.
                .filter(|(_, entry)| entry.wanted)
                .filter_map(|(id, entry)| {
                    let link = entry.link.as_ref()?;
                    Some((id.clone(), Arc::clone(link), Arc::clone(&entry.op)))
                })
                .collect()
        };
        let mut probes = Vec::with_capacity(targets.len());
        for (id, link, op) in targets {
            probes.push(self.probe(id, link, op));
        }
        outcome.healthy = join_all(probes)
            .await
            .into_iter()
            .filter(|&healthy| healthy)
            .count();

        // Second pass: reconnect one device at a time (a second search would
        // only wait for the scan permit), highest priority first.
        let mut due: Vec<_> = {
            let devices = self.devices.read().await;
            devices
                .iter()
                .filter(|(_, entry)| entry.is_due_for_repair())
                .map(|(id, entry)| (id.clone(), entry.priority, Arc::clone(&entry.op)))
                .collect()
        };
        due.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (id, _priority, op) in due {
            // A connect, disconnect or remove of this device is running.
            let Ok(guard) = Arc::clone(&op).try_lock_owned() else {
                continue;
            };
            let held = Arc::new(guard);
            // Check again now that the device is ours: it may have been
            // connected, disconnected or removed since the list was made.
            // With `max_attempts` of 0 a device that has been connected gets
            // no reconnect: give up.
            let next = match self.devices.write().await.get_mut(&id) {
                Some(entry) if Arc::ptr_eq(&entry.op, &op) && entry.is_due_for_repair() => {
                    match entry.give_up_before_reconnect() {
                        Some(failures) => Err(failures),
                        None => Ok(entry.failures.saturating_add(1)),
                    }
                }
                _ => continue,
            };
            let attempt = match next {
                Ok(attempt) => attempt,
                Err(failures) => {
                    self.report_give_up(&id, failures);
                    continue;
                }
            };
            // With every connection slot taken, the repair would fail at once,
            // without a Bluetooth attempt: wait for a free slot instead, and
            // don't count a failure.
            if self
                .slots
                .as_ref()
                .is_some_and(|slots| slots.available_permits() == 0)
            {
                debug!("Health monitor: no free connection slot for {id}");
                continue;
            }
            debug!("Health monitor: reconnecting {id} (attempt {attempt})");
            self.events.send(DeviceEvent::ReconnectStarted {
                device: DeviceId::new(&id),
                attempt,
            });
            match self.connect_locked(&id, &held, None).await {
                // `connect_locked` started the backoff over when it stored
                // the new link.
                Ok(()) => {
                    info!("Health monitor: reconnected {id}");
                    self.events.send(DeviceEvent::ReconnectSucceeded {
                        device: DeviceId::new(&id),
                        attempts: attempt,
                    });
                    outcome.repaired += 1;
                }
                // The device was disconnected or removed while this repair ran.
                Err(Error::Cancelled) => debug!("Health monitor: reconnect of {id} cancelled"),
                Err(e) => {
                    warn!("Health monitor: reconnect {attempt} of {id} failed: {e}");
                    let gave_up_after = self
                        .devices
                        .write()
                        .await
                        .get_mut(&id)
                        .and_then(|entry| entry.record_repair_failure(Instant::now()));
                    if let Some(failures) = gave_up_after {
                        self.report_give_up(&id, failures);
                    }
                    outcome.failed += 1;
                }
            }
        }
        outcome
    }

    /// Logs that the health monitor stops reconnecting `id` after `failures`
    /// failed attempts, and emits the one `DeviceEvent::Error` that says so.
    fn report_give_up(&self, id: &str, failures: u32) {
        let attempts = if failures == 1 { "attempt" } else { "attempts" };
        warn!(
            "Health monitor: giving up on {id} after {failures} failed {attempts}; \
             connect() starts over"
        );
        self.events.send(DeviceEvent::Error {
            device: DeviceId::new(id),
            error: format!("auto-reconnect gave up after {failures} {attempts}"),
        });
    }

    /// Checks one connected device for `health_tick` and closes its link if
    /// the check fails. Returns whether the device was checked and is healthy.
    async fn probe(&self, id: String, link: Arc<L>, op: Arc<Mutex<()>>) -> bool {
        // A connect, disconnect or remove of this device is running.
        let Ok(guard) = op.try_lock_owned() else {
            return false;
        };
        let held = Arc::new(guard);
        let alive = if self.config.use_connection_validation {
            link.is_alive().await
        } else {
            link.is_connected().await
        };
        if !alive {
            warn!("Health monitor: the connection to {id} is dead; closing it");
            self.detach(&id, &link, &held, DisconnectReason::Unknown)
                .await;
        }
        alive
    }

    /// Closes `link` if the entry for `identifier` still holds it: takes the
    /// link and its slot, emits `Disconnected`, disconnects the link and frees
    /// the slot once the link is down. `held` is the caller's guard of the
    /// entry's `op` mutex. As in `disconnect_locked`, the disconnect keeps a
    /// share of that guard until the link is down, even if this future is
    /// dropped, so no connect of the device can overlap the old link's
    /// disconnect.
    async fn detach(
        &self,
        identifier: &str,
        link: &Arc<L>,
        held: &Arc<tokio::sync::OwnedMutexGuard<()>>,
        reason: DisconnectReason,
    ) {
        let taken = {
            let mut devices = self.devices.write().await;
            match devices.get_mut(identifier) {
                Some(entry)
                    if entry
                        .link
                        .as_ref()
                        .is_some_and(|stored| Arc::ptr_eq(stored, link)) =>
                {
                    // The first repair is due at once.
                    entry.retry_at = Some(Instant::now());
                    entry.link.take().map(|stored| (stored, entry.slot.take()))
                }
                _ => None,
            }
        };
        let Some((link, slot)) = taken else {
            return;
        };
        // The link has left the manager: say so before the close, which can
        // take seconds and goes on in the background if this future is
        // dropped, so a dropped caller can't lose the event.
        self.events.send(DeviceEvent::Disconnected {
            device: DeviceId::new(identifier),
            reason,
        });
        // Close it explicitly: a handle dropped without a disconnect tears
        // down whatever link the sensor has by then, including a new one.
        if let Err(e) = release_link(link, (slot, Arc::clone(held))).await {
            log_failed_release("Closing the dead connection", identifier, &e);
        }
    }

    pub(crate) fn spawn_health_monitor(
        self: Arc<Self>,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut adaptive = self.config.use_adaptive_interval.then(|| {
                AdaptiveInterval::new(
                    self.config.health_check_interval,
                    self.config.min_health_check_interval,
                    self.config.max_health_check_interval,
                )
            });
            loop {
                let interval = adaptive
                    .as_ref()
                    .map_or(self.config.health_check_interval, AdaptiveInterval::current);
                if cancel
                    .run_until_cancelled(tokio::time::sleep(interval))
                    .await
                    .is_none()
                {
                    break;
                }
                // Cancelling mid-tick drops the tick: a connect in progress
                // gives back its slot and `op` guard, and releases the sensor;
                // a dead link being closed, or a new link not stored yet,
                // keeps both until it is down.
                let Some(outcome) = cancel.run_until_cancelled(self.health_tick()).await else {
                    break;
                };
                if let Some(adaptive) = adaptive.as_mut() {
                    if outcome.failed > 0 {
                        adaptive.on_failure();
                    } else if outcome.healthy > 0 && outcome.repaired == 0 {
                        adaptive.on_success();
                    }
                }
            }
            info!("Health monitor cancelled, shutting down");
        })
    }

    pub(crate) async fn add_device_with_priority(
        &self,
        identifier: &str,
        priority: DevicePriority,
    ) -> Result<()> {
        self.config.default_reconnect_options.validate()?;
        let mut devices = self.devices.write().await;

        if let Some(entry) = devices.get_mut(identifier) {
            // Update priority if device already exists
            entry.priority = priority;
            return Ok(());
        }

        devices.insert(
            identifier.to_string(),
            Entry::new(self.config.default_reconnect_options.clone(), priority),
        );

        info!(
            "Added device to manager with priority {:?}: {}",
            priority, identifier
        );
        Ok(())
    }

    pub(crate) async fn lowest_priority_connected(&self) -> Option<String> {
        let devices = self.devices.read().await;
        devices
            .iter()
            .filter(|(_, entry)| entry.link.is_some() && entry.priority != DevicePriority::Critical)
            .min_by_key(|(_, entry)| entry.priority)
            .map(|(id, _)| id.clone())
    }

    pub(crate) async fn evict_lowest_priority(self: &Arc<Self>) -> Result<bool> {
        if let Some(id) = self.lowest_priority_connected().await {
            info!("Evicting lowest priority device: {}", id);
            self.disconnect(&id).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(crate) fn spawn_hybrid_monitor(
        self: Arc<Self>,
        cancel: CancellationToken,
        options: PassiveMonitorOptions,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("Starting hybrid monitor (passive + active)");

            // Create passive monitor
            let passive_monitor = Arc::new(PassiveMonitor::new(options));
            let mut passive_rx = passive_monitor.subscribe();

            // Start passive monitoring
            let passive_cancel = cancel.clone();
            let _passive_handle = passive_monitor.start(passive_cancel);

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("Hybrid monitor cancelled");
                        break;
                    }
                    result = passive_rx.recv() => {
                        match result {
                            Ok(passive_reading) => {
                                // Convert passive reading to CurrentReading and emit event
                                if let Some(reading) = passive_reading_to_current(&passive_reading) {
                                    // Update last reading in the entry if it exists
                                    if let Some(entry) = self.devices.write().await.get_mut(&passive_reading.device_id) {
                                        entry.last_reading = Some(reading);
                                    }

                                    // Emit reading event
                                    self.events.send(DeviceEvent::Reading {
                                        device: DeviceId {
                                            id: passive_reading.device_id.clone(),
                                            name: passive_reading.device_name.clone(),
                                            device_type: Some(passive_reading.data.device_type),
                                        },
                                        reading,
                                    });
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Hybrid monitor lagged {} messages", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                info!("Passive monitor channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        })
    }

    pub(crate) async fn read_hybrid(
        &self,
        identifier: &str,
        max_passive_age: Option<Duration>,
    ) -> Result<CurrentReading> {
        let max_age = max_passive_age.unwrap_or(Duration::from_secs(60));

        // Check if we have a recent cached reading
        {
            let devices = self.devices.read().await;
            if let Some(entry) = devices.get(identifier)
                && let Some(reading) = entry.last_reading
            {
                // Check if the reading has a captured_at timestamp
                if let Some(captured) = reading.captured_at {
                    let age = time::OffsetDateTime::now_utc() - captured;
                    if age
                        < time::Duration::try_from(max_age).unwrap_or(time::Duration::seconds(60))
                    {
                        debug!("Using cached passive reading for {}", identifier);
                        return Ok(reading);
                    }
                }
            }
        }

        // No recent passive reading, use active connection
        debug!(
            "No recent passive reading, using active connection for {}",
            identifier
        );
        self.read_current(identifier).await
    }

    /// A copy of the entry's lifecycle state, for the lifecycle tests.
    #[cfg(test)]
    pub(crate) async fn snapshot(&self, identifier: &str) -> Option<EntrySnapshot> {
        let devices = self.devices.read().await;
        devices.get(identifier).map(|entry| EntrySnapshot {
            has_link: entry.link.is_some(),
            failures: entry.failures,
            wanted: entry.wanted,
            gave_up: entry.gave_up,
            retry_at: entry.retry_at,
        })
    }
}

/// Manager for multiple Aranet devices.
pub struct DeviceManager {
    core: Arc<ManagerCore<Device>>,
}

impl DeviceManager {
    /// Create a new device manager.
    pub fn new() -> Self {
        Self::with_config(ManagerConfig::default())
    }

    /// Create a manager with custom event capacity.
    pub fn with_event_capacity(capacity: usize) -> Self {
        Self::with_config(ManagerConfig {
            event_capacity: capacity,
            ..Default::default()
        })
    }

    /// Create a manager with full configuration.
    pub fn with_config(config: ManagerConfig) -> Self {
        Self {
            core: Arc::new(ManagerCore::new(config, ble_connector())),
        }
    }

    /// Get the event dispatcher for subscribing to events.
    pub fn events(&self) -> &EventDispatcher {
        &self.core.events
    }

    /// Get the manager configuration.
    pub fn config(&self) -> &ManagerConfig {
        &self.core.config
    }

    /// Scan for available devices.
    pub async fn scan(&self) -> Result<Vec<DiscoveredDevice>> {
        scan_with_options(self.core.config.scan_options.clone()).await
    }

    /// Scan with custom options.
    pub async fn scan_with_options(&self, options: ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        let devices = scan_with_options(options).await?;

        // Emit discovery events
        for device in &devices {
            self.core.events.send(DeviceEvent::Discovered {
                device: DeviceId {
                    id: device.identifier.clone(),
                    name: device.name.clone(),
                    device_type: device.device_type,
                },
                rssi: device.rssi,
            });
        }

        Ok(devices)
    }

    /// Add a device to the manager by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if the config's
    /// `default_reconnect_options` are invalid (see [`ReconnectOptions::validate`]).
    pub async fn add_device(&self, identifier: &str) -> Result<()> {
        self.core.add_device(identifier).await
    }

    /// Add a device with custom reconnect options.
    ///
    /// The health monitor waits between automatic reconnects of the device as
    /// `reconnect_options` say. If the device is already managed, nothing
    /// changes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if `reconnect_options` are invalid (see
    /// [`ReconnectOptions::validate`]).
    pub async fn add_device_with_options(
        &self,
        identifier: &str,
        reconnect_options: ReconnectOptions,
    ) -> Result<()> {
        self.core
            .add_device_with_options(identifier, reconnect_options)
            .await
    }

    /// Connect to a device.
    ///
    /// This method performs an atomic connect-or-skip operation:
    /// - If the device doesn't exist, it's added and connected
    /// - If the device exists but is not connected, it's connected
    /// - If the device already has a connection that the Bluetooth stack
    ///   reports as up, this is a no-op; a lost one is closed and replaced
    ///
    /// A second connect of the same device waits for the first and returns its
    /// own real result: `Ok(())` if the first one connected the device,
    /// otherwise the result of its own attempt. A connect that hasn't
    /// connected yet when [`disconnect`](Self::disconnect),
    /// [`disconnect_all`](Self::disconnect_all),
    /// [`evict_lowest_priority`](Self::evict_lowest_priority) or
    /// [`remove_device`](Self::remove_device) is called for the same device
    /// returns [`Error::Cancelled`], closing its connection if one comes up.
    /// A connect that waits for a [`remove_device`](Self::remove_device) of
    /// the same device returns [`Error::Cancelled`] instead of adding the
    /// device back.
    ///
    /// If this future is dropped while a lost connection is being closed,
    /// the close still finishes in the background, and a connect of the same
    /// device waits for it instead of running alongside it.
    ///
    /// If this future is dropped after the connection is made but before the
    /// device information has been read, the device stays connected, but no
    /// [`DeviceEvent::Connected`] is sent and
    /// [`get_device_info`](Self::get_device_info) returns `None` until the
    /// device is reconnected.
    ///
    /// # Connection Limits
    ///
    /// If `max_concurrent_connections` is set in the config and would be exceeded,
    /// this method returns an error. The limit counts connects in progress; use
    /// `can_connect()` or `available_connections()` to check it before calling
    /// this method.
    ///
    /// The device map is locked only while the entry is updated, not during the
    /// BLE connection, so operations on other devices don't wait for it;
    /// operations on the same device do, as described above.
    pub async fn connect(&self, identifier: &str) -> Result<()> {
        self.core.connect(identifier).await
    }

    /// Disconnect from a device.
    ///
    /// A connect of the same device that hasn't connected yet is abandoned:
    /// it returns [`Error::Cancelled`], closing its connection if one comes
    /// up. This waits until that connect's Bluetooth attempt has ended and
    /// such a connection is closed, which can take the connect's whole time
    /// budget: tens of seconds at default settings. The health monitor
    /// doesn't reconnect the device until [`connect`](Self::connect) is
    /// called.
    ///
    /// [`DeviceEvent::Disconnected`], with [`DisconnectReason::UserRequested`],
    /// is sent as soon as the manager lets go of the connection, before the
    /// connection is closed, so it is sent even if closing the connection
    /// fails or this future is dropped. A failed close's error is still
    /// returned.
    ///
    /// If this future is dropped, a disconnect that has started still
    /// finishes in the background, even one still waiting for a connect or a
    /// health check of the device to end. A connect of the same device that
    /// starts meanwhile either waits for the disconnect and then reconnects,
    /// or, if it starts before the disconnect has taken the connection, keeps
    /// the device connected, and the disconnect then does nothing.
    pub async fn disconnect(&self, identifier: &str) -> Result<()> {
        self.core.disconnect(identifier).await
    }

    /// Remove a device from the manager.
    ///
    /// The device is disconnected first, as [`disconnect`](Self::disconnect)
    /// does, and then removed, even if closing its connection fails: that
    /// error is still returned. A connect of the device that hasn't
    /// connected yet is abandoned: it returns [`Error::Cancelled`], closing
    /// its connection if one comes up. As with `disconnect`, this waits until
    /// that connect's Bluetooth attempt has ended and such a connection is
    /// closed, and a removal that has started still finishes in the
    /// background if this future is dropped. A connect of the same device
    /// that starts meanwhile doesn't keep it: the device is removed even when
    /// that connect returns first.
    pub async fn remove_device(&self, identifier: &str) -> Result<()> {
        self.core.remove_device(identifier).await
    }

    /// Get a list of all managed device IDs.
    pub async fn device_ids(&self) -> Vec<String> {
        self.core.device_ids().await
    }

    /// Get the number of managed devices.
    pub async fn device_count(&self) -> usize {
        self.core.device_count().await
    }

    /// Get the number of connected devices (fast, doesn't query BLE).
    ///
    /// This returns the number of devices that have an active device handle,
    /// without querying the BLE stack. Use `connected_count_verified` for
    /// an accurate count that queries each device.
    pub async fn connected_count(&self) -> usize {
        self.core.connected_count().await
    }

    /// Check if a new connection can be made without exceeding the limit.
    ///
    /// Returns `true` if another connection can be made, `false` if at limit.
    /// Connects in progress count against the limit. Always returns `true` if
    /// `max_concurrent_connections` is 0 (unlimited).
    pub async fn can_connect(&self) -> bool {
        self.core.can_connect().await
    }

    /// Get the connection limit status.
    ///
    /// Returns (current_connections, max_connections). If max is 0, there is no limit.
    ///
    /// `current_connections` counts devices with a connection handle; connects in
    /// progress are not included but already hold a slot, so use
    /// `available_connections` or `can_connect` to see whether `connect()` will
    /// pass the limit.
    pub async fn connection_status(&self) -> (usize, usize) {
        self.core.connection_status().await
    }

    /// Get the number of available connection slots.
    ///
    /// Connects in progress hold a slot, and so does a disconnect until the link
    /// is down. Returns `None` if there is no connection limit (unlimited).
    pub async fn available_connections(&self) -> Option<usize> {
        self.core.available_connections().await
    }

    /// Get the number of connected devices (verified via BLE).
    ///
    /// This method queries each device to verify its connection status.
    /// The lock is released before making BLE calls to avoid contention.
    pub async fn connected_count_verified(&self) -> usize {
        self.core.connected_count_verified().await
    }

    /// Read current values from a specific device.
    pub async fn read_current(&self, identifier: &str) -> Result<CurrentReading> {
        self.core.read_current(identifier).await
    }

    /// Read current values from all connected devices (in parallel).
    ///
    /// This method releases the lock before performing async BLE operations,
    /// allowing other tasks to add/remove devices while reads are in progress.
    /// All reads are performed in parallel for maximum performance.
    pub async fn read_all(&self) -> HashMap<String, Result<CurrentReading>> {
        self.core.read_all().await
    }

    /// Connect to all known devices (in parallel).
    ///
    /// Returns a map of device IDs to connection results.
    pub async fn connect_all(&self) -> HashMap<String, Result<()>> {
        self.core.connect_all().await
    }

    /// Disconnect from all devices (in parallel).
    ///
    /// Returns a map of device IDs to disconnection results, with an entry for
    /// each device that had a connection. Every managed device is withdrawn,
    /// connected or not: the health monitor reconnects none of them until
    /// [`connect`](Self::connect) is called. Each connected device is
    /// disconnected as [`disconnect`](Self::disconnect) describes.
    pub async fn disconnect_all(&self) -> HashMap<String, Result<()>> {
        self.core.disconnect_all().await
    }

    /// Check if a specific device is connected (fast, doesn't query BLE).
    ///
    /// This method attempts to check if a device has an active connection handle
    /// without blocking. Returns `None` if the lock couldn't be acquired immediately,
    /// or `Some(bool)` indicating whether the device has a connection handle.
    ///
    /// Note: This only checks if we have a device handle, not whether the actual
    /// BLE connection is still alive. Use [`is_connected`](Self::is_connected) for
    /// a verified check.
    pub fn try_is_connected(&self, identifier: &str) -> Option<bool> {
        self.core.try_is_connected(identifier)
    }

    /// Check if a specific device is connected (verified via BLE).
    ///
    /// The lock is released before making the BLE call.
    pub async fn is_connected(&self, identifier: &str) -> bool {
        self.core.is_connected(identifier).await
    }

    /// Get device info for a specific device.
    pub async fn get_device_info(&self, identifier: &str) -> Option<DeviceInfo> {
        self.core.get_device_info(identifier).await
    }

    /// Get the last cached reading for a device.
    pub async fn get_last_reading(&self, identifier: &str) -> Option<CurrentReading> {
        self.core.get_last_reading(identifier).await
    }

    /// Start a background task that checks the managed devices and repairs
    /// lost connections.
    ///
    /// On every tick the task:
    ///
    /// 1. Checks every connected device at the same time, so a slow device
    ///    doesn't delay the others. A connection that fails its check is
    ///    disconnected explicitly, and [`DeviceEvent::Disconnected`] is emitted.
    /// 2. Reconnects the devices that should be connected but aren't, one at a
    ///    time and highest [`DevicePriority`] first, emitting
    ///    [`DeviceEvent::ReconnectStarted`] and
    ///    [`DeviceEvent::ReconnectSucceeded`].
    ///
    /// Each device waits between reconnect attempts as its [`ReconnectOptions`]
    /// say. After `max_attempts` failures in a row the task stops trying and
    /// emits one [`DeviceEvent::Error`]; [`connect`](Self::connect) starts
    /// over. With `max_attempts` of 0 the task never reconnects a device that
    /// has been connected: it gives up as soon as it finds the device's
    /// connection gone, without trying. A device that has never been
    /// connected, such as one just added, still gets one attempt.
    /// While `max_concurrent_connections` connections are in use, a device
    /// waits for a free one, and that wait isn't counted as a failed attempt.
    ///
    /// Devices added with [`add_device`](Self::add_device) and its variants
    /// are connected by the task. Devices that were disconnected (with
    /// [`disconnect`](Self::disconnect), [`disconnect_all`](Self::disconnect_all)
    /// or [`evict_lowest_priority`](Self::evict_lowest_priority)) or removed
    /// are not reconnected until `connect()` is called for them.
    ///
    /// The task runs until the provided cancellation token is cancelled, and
    /// then stops at once, even in the middle of a check or a reconnect. A
    /// connect it abandons releases the sensor. A lost connection it was
    /// closing is still closed in the background, and a
    /// [`connect`](Self::connect) of that device waits for it.
    ///
    /// # Adaptive Intervals
    ///
    /// If `use_adaptive_interval` is enabled in the config, the time between
    /// ticks adapts to connection stability:
    /// - after a tick with a failed reconnect, it halves (down to
    ///   `min_health_check_interval`);
    /// - after three ticks with a healthy device and no reconnect, without a
    ///   failed reconnect in between, it doubles (up to
    ///   `max_health_check_interval`).
    ///
    /// # Connection Validation
    ///
    /// If `use_connection_validation` is enabled, health checks read the current
    /// measurements (`device.validate_connection()`, which needs no pairing that
    /// a reading doesn't) to catch "zombie connections" where the BLE stack
    /// thinks it's connected but the device is out of range. Otherwise they only
    /// ask the BLE stack, which misses zombie connections.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tokio_util::sync::CancellationToken;
    ///
    /// let manager = Arc::new(DeviceManager::new());
    /// let cancel = CancellationToken::new();
    /// let handle = manager.start_health_monitor(cancel.clone());
    ///
    /// // Later, to stop the health monitor:
    /// cancel.cancel();
    /// handle.await.unwrap();
    /// ```
    pub fn start_health_monitor(
        self: &Arc<Self>,
        cancel_token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        Arc::clone(&self.core).spawn_health_monitor(cancel_token)
    }

    /// Add a device with priority.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if the config's
    /// `default_reconnect_options` are invalid (see [`ReconnectOptions::validate`]).
    pub async fn add_device_with_priority(
        &self,
        identifier: &str,
        priority: DevicePriority,
    ) -> Result<()> {
        self.core
            .add_device_with_priority(identifier, priority)
            .await
    }

    /// Get the lowest priority connected device that could be disconnected.
    ///
    /// Returns None if no devices can be disconnected (all are Critical priority or not connected).
    pub async fn lowest_priority_connected(&self) -> Option<String> {
        self.core.lowest_priority_connected().await
    }

    /// Disconnect the lowest priority device to make room for a new connection.
    ///
    /// Returns Ok(true) if a device was disconnected, Ok(false) if no eligible device found.
    /// The health monitor doesn't reconnect the evicted device until
    /// [`connect`](Self::connect) is called for it. The device is disconnected
    /// as [`disconnect`](Self::disconnect) does it, so this too waits for a
    /// connect of that device that is running to end.
    pub async fn evict_lowest_priority(&self) -> Result<bool> {
        self.core.evict_lowest_priority().await
    }

    /// Start hybrid monitoring using both passive (advertisement) and active connections.
    ///
    /// This is the most efficient way to monitor multiple devices:
    /// - **Passive monitoring**: Uses BLE advertisements to receive real-time readings
    ///   without maintaining connections. Lower power consumption, unlimited devices.
    /// - **Active connections**: Only established when needed (history download, settings changes).
    ///
    /// # Requirements
    ///
    /// Smart Home integration must be enabled on each device for passive monitoring.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use tokio_util::sync::CancellationToken;
    ///
    /// let manager = Arc::new(DeviceManager::new());
    /// let cancel = CancellationToken::new();
    /// let handle = manager.start_hybrid_monitor(cancel.clone(), None);
    ///
    /// // Receive readings via manager events
    /// let mut rx = manager.events().subscribe();
    /// while let Ok(event) = rx.recv().await {
    ///     if let DeviceEvent::Reading { device, reading } = event {
    ///         println!("{}: CO2 = {} ppm", device.id, reading.co2);
    ///     }
    /// }
    /// ```
    pub fn start_hybrid_monitor(
        self: &Arc<Self>,
        cancel_token: CancellationToken,
        passive_options: Option<PassiveMonitorOptions>,
    ) -> tokio::task::JoinHandle<()> {
        Arc::clone(&self.core)
            .spawn_hybrid_monitor(cancel_token, passive_options.unwrap_or_default())
    }

    /// Get a reading using hybrid approach: try passive first, fall back to active.
    ///
    /// This method checks if a recent passive reading is available. If not,
    /// it establishes an active connection to read the value.
    ///
    /// # Arguments
    ///
    /// * `identifier` - Device identifier
    /// * `max_passive_age` - Maximum age of passive reading to accept (default: 60s)
    pub async fn read_hybrid(
        &self,
        identifier: &str,
        max_passive_age: Option<Duration>,
    ) -> Result<CurrentReading> {
        self.core.read_hybrid(identifier, max_passive_age).await
    }

    /// Check if a device supports passive monitoring (Smart Home enabled).
    ///
    /// This performs a quick scan to check if the device is broadcasting
    /// advertisement data with sensor readings.
    ///
    /// Scans in one process run one at a time, in the order they asked to, so
    /// the check's 5 s scan first waits for the scan window that is running
    /// and for every window queued before it. It returns `false` if no reading
    /// arrives within 15 s, so when those windows take more than 10 s in all,
    /// it can miss a device that does advertise.
    ///
    /// The check's scanning stops when it returns, or as soon as this future
    /// is dropped, for example by a caller's timeout.
    pub async fn supports_passive_monitoring(&self, identifier: &str) -> bool {
        // Create a short-lived passive monitor to check for advertisements
        let options = PassiveMonitorOptions::default()
            .scan_duration(Duration::from_secs(5))
            .filter_devices(vec![identifier.to_string()]);

        let monitor = Arc::new(PassiveMonitor::new(options));
        let readings = monitor.subscribe();

        // Wait for a reading or timeout: 5 s of scanning, after up to 10 s of
        // waiting for the scan windows ahead of it.
        receives_within(readings, Duration::from_secs(15), |cancel| {
            monitor.start(cancel)
        })
        .await
    }
}

/// Starts a monitor with `start` and waits up to `limit` for a value on
/// `readings`, which must be subscribed to that monitor before it starts.
/// Returns whether one arrived. The token given to `start` is cancelled,
/// which stops the monitor, when this returns or is dropped.
async fn receives_within<T: Clone>(
    mut readings: tokio::sync::broadcast::Receiver<T>,
    limit: Duration,
    start: impl FnOnce(CancellationToken) -> tokio::task::JoinHandle<()>,
) -> bool {
    let cancel = CancellationToken::new();
    // Dropping this future, as a caller's timeout does, stops the monitor
    // too; otherwise it would scan for the rest of the process.
    let _stop_on_drop = cancel.clone().drop_guard();
    let _monitor = start(cancel);
    matches!(
        tokio::time::timeout(limit, readings.recv()).await,
        Ok(Ok(_))
    )
}

/// Convert a passive advertisement reading to a CurrentReading.
fn passive_reading_to_current(passive: &PassiveReading) -> Option<CurrentReading> {
    let data = &passive.data;

    // We need at least some sensor data to create a reading
    if data.co2.is_none()
        && data.temperature.is_none()
        && data.humidity.is_none()
        && data.radon.is_none()
        && data.radiation_dose_rate.is_none()
    {
        return None;
    }

    Some(CurrentReading {
        co2: data.co2.unwrap_or(0),
        temperature: data.temperature.unwrap_or(0.0),
        pressure: data.pressure.unwrap_or(0.0),
        humidity: data.humidity.unwrap_or(0),
        battery: data.battery,
        status: data.status,
        interval: data.interval,
        age: data.age,
        captured_at: Some(time::OffsetDateTime::now_utc()),
        radon: data.radon,
        radon_avg_24h: None,
        radon_avg_7d: None,
        radon_avg_30d: None,
        radiation_rate: data.radiation_dose_rate,
        radiation_total: None, // Not available in advertisement data
    })
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::within;

    #[tokio::test]
    async fn test_manager_add_device() {
        let manager = DeviceManager::new();
        manager.add_device("test-device").await.unwrap();

        assert_eq!(manager.device_count().await, 1);
        assert!(
            manager
                .device_ids()
                .await
                .contains(&"test-device".to_string())
        );
    }

    #[tokio::test]
    async fn test_manager_remove_device() {
        let manager = DeviceManager::new();
        manager.add_device("test-device").await.unwrap();
        manager.remove_device("test-device").await.unwrap();

        assert_eq!(manager.device_count().await, 0);
    }

    #[tokio::test]
    async fn test_manager_not_connected_by_default() {
        let manager = DeviceManager::new();
        manager.add_device("test-device").await.unwrap();

        assert!(!manager.is_connected("test-device").await);
        assert_eq!(manager.connected_count().await, 0);
    }

    #[tokio::test]
    async fn test_manager_events() {
        let manager = DeviceManager::new();
        let _rx = manager.events().subscribe();

        manager.add_device("test-device").await.unwrap();

        // Events are only emitted for actual device operations
        assert_eq!(manager.events().receiver_count(), 1);
    }

    /// A stand-in for `supports_passive_monitoring`'s monitor: it runs until
    /// its token is cancelled, sending one reading on `sender` 3 s in if
    /// `reading` is set, and then tells `stopped`.
    fn fake_monitor(
        cancel: CancellationToken,
        sender: tokio::sync::broadcast::Sender<()>,
        reading: bool,
        stopped: tokio::sync::oneshot::Sender<()>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if reading {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let _ = sender.send(());
            }
            cancel.cancelled().await;
            let _ = stopped.send(());
        })
    }

    /// The passive check returns whether a reading arrives in time, and
    /// stops its monitor when it returns.
    #[tokio::test(start_paused = true)]
    async fn the_passive_check_stops_its_monitor_when_it_returns() {
        within(Duration::from_secs(600), async {
            for (reading, after) in [(true, 3), (false, 15)] {
                let (sender, readings) = tokio::sync::broadcast::channel(1);
                let (stopped, monitor_stopped) = tokio::sync::oneshot::channel();
                let start = tokio::time::Instant::now();

                let found = receives_within(readings, Duration::from_secs(15), |cancel| {
                    fake_monitor(cancel, sender, reading, stopped)
                })
                .await;

                assert_eq!(found, reading);
                assert_eq!(start.elapsed(), Duration::from_secs(after));
                within(Duration::from_secs(10), monitor_stopped)
                    .await
                    .expect("the monitor ended without being stopped");
            }
        })
        .await;
    }

    /// The passive check's monitor also stops when the check is dropped
    /// before it returns, as by a caller's timeout, instead of scanning for
    /// the rest of the process.
    #[tokio::test(start_paused = true)]
    async fn a_dropped_passive_check_stops_its_monitor() {
        within(Duration::from_secs(600), async {
            let (sender, readings) = tokio::sync::broadcast::channel(1);
            let (stopped, monitor_stopped) = tokio::sync::oneshot::channel();
            let check = receives_within(readings, Duration::from_secs(15), |cancel| {
                fake_monitor(cancel, sender, false, stopped)
            });

            let given_up = tokio::time::timeout(Duration::from_secs(1), check).await;
            assert!(given_up.is_err(), "the check returned {given_up:?}");
            within(Duration::from_secs(10), monitor_stopped)
                .await
                .expect("the monitor ended without being stopped");
        })
        .await;
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::time::timeout;

    use super::{DevicePriority, EntrySnapshot, ManagerConfig, ManagerCore, TickOutcome};
    use crate::error::{ConnectionFailureReason, Error, Result};
    use crate::events::{DeviceEvent, EventReceiver};
    use crate::test_support::{FakeConn, FakeEvent, FakeRadio, within};

    const TEST_LIMIT: Duration = Duration::from_secs(600);

    fn core(radio: &FakeRadio, config: ManagerConfig) -> Arc<ManagerCore<FakeConn>> {
        Arc::new(ManagerCore::new(config, radio.connector()))
    }

    /// The manager events received so far, one line each (`DeviceEvent`
    /// has no `PartialEq`).
    fn drain(events: &mut EventReceiver) -> Vec<String> {
        let mut lines = Vec::new();
        while let Ok(event) = events.try_recv() {
            lines.push(match event {
                DeviceEvent::Connected { device, .. } => format!("Connected {}", device.id),
                DeviceEvent::Disconnected { device, reason } => {
                    format!("Disconnected {} {reason:?}", device.id)
                }
                DeviceEvent::ReconnectStarted { device, attempt } => {
                    format!("ReconnectStarted {} {attempt}", device.id)
                }
                DeviceEvent::ReconnectSucceeded { device, attempts } => {
                    format!("ReconnectSucceeded {} {attempts}", device.id)
                }
                DeviceEvent::Error { device, error } => format!("Error {}: {error}", device.id),
                other => format!("{other:?}"),
            });
        }
        lines
    }

    /// Every sensor whose link is up must be held by the manager.
    async fn assert_no_orphans(core: &ManagerCore<FakeConn>, radio: &FakeRadio) {
        for id in radio.up_ids() {
            assert!(
                core.snapshot(&id).await.is_some_and(|entry| entry.has_link),
                "{id} is connected but the manager holds no link for it: {:#?}",
                radio.events()
            );
        }
    }

    fn is_limit_error(result: &Result<()>) -> bool {
        matches!(
            result,
            Err(Error::ConnectionFailed {
                reason: ConnectionFailureReason::Other(message),
                ..
            }) if message.starts_with("Connection limit reached")
        )
    }

    /// BR-9: a connect dropped by its caller's timeout left the `connecting`
    /// flag set, and every later connect returned Ok without connecting.
    #[tokio::test(start_paused = true)]
    async fn timed_out_connect_does_not_wedge_the_device() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default());

            radio.set_connect_delay("A", Duration::from_secs(10));
            assert!(
                timeout(Duration::from_secs(1), core.connect("A"))
                    .await
                    .is_err()
            );

            radio.set_connect_delay("A", Duration::ZERO);
            core.connect("A").await.expect("second connect");
            assert!(
                radio.link_up("A"),
                "the second connect returned Ok without connecting"
            );
            assert_eq!(radio.connect_count("A"), 2);

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9: a second connect of the same device returned Ok at once while
    /// the first was still running, even when the first then failed.
    #[tokio::test(start_paused = true)]
    async fn concurrent_connects_to_one_device_share_the_real_result() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default());
            radio.set_connect_delay("A", Duration::from_secs(5));
            radio.script_connects("A", [false, false]);

            let (first, second) = tokio::join!(core.connect("A"), core.connect("A"));
            assert!(first.is_err(), "first connect: {first:?}");
            assert!(
                second.is_err(),
                "the second connect returned {second:?} although no connect succeeded"
            );

            // The second attempt starts only after the first has failed.
            let a = || "A".to_string();
            let events: Vec<FakeEvent> =
                radio.events().into_iter().map(|(_, event)| event).collect();
            assert_eq!(
                events,
                [
                    FakeEvent::ConnectStarted { id: a() },
                    FakeEvent::ConnectFailed { id: a() },
                    FakeEvent::ConnectStarted { id: a() },
                    FakeEvent::ConnectFailed { id: a() },
                ]
            );

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9: the limit counted installed links only, so connects that were
    /// still running all passed the check.
    #[tokio::test(start_paused = true)]
    async fn connection_limit_counts_in_flight_connects() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default().with_max_connections(1));
            radio.set_connect_delay("A", Duration::from_secs(5));
            radio.set_connect_delay("B", Duration::from_secs(5));

            let (a, b) = tokio::join!(core.connect("A"), core.connect("B"));
            let results = [a, b];
            assert_eq!(
                results.iter().filter(|result| result.is_ok()).count(),
                1,
                "{results:?}"
            );
            assert_eq!(
                results
                    .iter()
                    .filter(|result| is_limit_error(result))
                    .count(),
                1,
                "{results:?}"
            );
            assert_eq!(core.connected_count().await, 1);
            // A connect that the limit rejects doesn't add the device.
            assert_eq!(core.device_count().await, 1);
            assert_eq!(radio.up_ids().len(), 1, "links up: {:?}", radio.up_ids());

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9: `connect_all` starts every connect at once, so they all passed
    /// the limit check.
    #[tokio::test(start_paused = true)]
    async fn connect_all_respects_the_connection_limit() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default().with_max_connections(2));
            for id in ["A", "B", "C", "D"] {
                radio.set_connect_delay(id, Duration::from_secs(5));
                core.add_device(id).await.expect("add_device");
            }

            let results = core.connect_all().await;
            assert_eq!(results.len(), 4);
            assert_eq!(
                results.values().filter(|result| result.is_ok()).count(),
                2,
                "{results:?}"
            );
            assert_eq!(
                results
                    .values()
                    .filter(|result| is_limit_error(result))
                    .count(),
                2,
                "{results:?}"
            );
            assert_eq!(core.connected_count().await, 2);
            assert_eq!(radio.up_ids().len(), 2, "links up: {:?}", radio.up_ids());
            assert_eq!(core.available_connections().await, Some(0));

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// Guard: a failed or cancelled connect gives its slot back.
    #[tokio::test(start_paused = true)]
    async fn failed_or_cancelled_connect_releases_its_slot() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default().with_max_connections(1));

            radio.script_connects("A", [false]);
            assert!(core.connect("A").await.is_err());
            assert_eq!(core.available_connections().await, Some(1));

            radio.set_connect_delay("B", Duration::from_secs(10));
            assert!(
                timeout(Duration::from_secs(1), core.connect("B"))
                    .await
                    .is_err()
            );
            assert_eq!(core.available_connections().await, Some(1));

            core.connect("C").await.expect("C gets the free slot");
            assert!(radio.link_up("C"));
            assert_eq!(core.available_connections().await, Some(0));
            assert_eq!(
                core.snapshot("C").await,
                Some(EntrySnapshot {
                    has_link: true,
                    failures: 0,
                    wanted: true,
                    gave_up: false,
                    retry_at: None,
                })
            );

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9: the slot was free as soon as the link was taken out of the map,
    /// while the sensor was still connected.
    #[tokio::test(start_paused = true)]
    async fn disconnect_releases_slot_only_after_the_link_is_down() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default().with_max_connections(1));
            core.connect("A").await.expect("connect");
            radio.set_disconnect_delay("A", Duration::from_secs(2));

            let disconnect = tokio::spawn({
                let core = Arc::clone(&core);
                async move { core.disconnect("A").await }
            });
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert!(radio.link_up("A"), "the disconnect is still running");
            assert_eq!(
                core.available_connections().await,
                Some(0),
                "the slot was freed while the sensor was still connected"
            );

            disconnect
                .await
                .expect("disconnect task")
                .expect("disconnect");
            assert!(!radio.link_up("A"));
            assert_eq!(core.available_connections().await, Some(1));

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9: the new link was a local variable until the device-info read
    /// finished, so a cancel during that read dropped a live link.
    #[tokio::test(start_paused = true)]
    async fn cancel_during_device_info_read_leaves_no_dropped_link() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default().with_max_connections(1));
            radio.set_info_delay("A", Duration::from_secs(5));
            let mut events = core.events.subscribe();

            assert!(
                timeout(Duration::from_secs(1), core.connect("A"))
                    .await
                    .is_err()
            );

            assert!(
                core.snapshot("A").await.is_some_and(|entry| entry.has_link),
                "the manager holds no link after the cancel: {:#?}",
                radio.events()
            );
            assert!(radio.link_up("A"));
            assert_eq!(core.available_connections().await, Some(0));
            // As `DeviceManager::connect` documents: the device stays connected,
            // but has no device information, and no `Connected` event was sent.
            assert!(core.get_device_info("A").await.is_none());
            let sent = events.try_recv();
            assert!(sent.is_err(), "unexpected event: {sent:?}");

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9: the limit was checked when `connect` added a new device, but the
    /// slot was taken later. Two connects of new devices that waited for the
    /// device map together both passed the check and both added their device.
    #[tokio::test(start_paused = true)]
    async fn rejected_connect_adds_no_device_while_the_map_is_busy() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default().with_max_connections(1));
            radio.set_connect_delay("A", Duration::from_secs(5));
            radio.set_connect_delay("B", Duration::from_secs(5));

            // Another user of the device map (a read, a snapshot, a monitor)
            // holds it while both connects start.
            let busy = core.devices.read().await;
            let spawn_connect = |id: &'static str| {
                let core = Arc::clone(&core);
                tokio::spawn(async move { core.connect(id).await })
            };
            let a = spawn_connect("A");
            let b = spawn_connect("B");
            tokio::time::sleep(Duration::from_secs(1)).await;
            drop(busy);

            let a = a.await.expect("connect A task");
            let b = b.await.expect("connect B task");
            assert!(a.is_ok(), "A asked first and gets the only slot: {a:?}");
            assert!(is_limit_error(&b), "B is over the limit: {b:?}");
            assert_eq!(
                core.device_ids().await,
                ["A"],
                "a connect that the limit rejects doesn't add the device"
            );
            assert_eq!(radio.up_ids(), ["A"]);

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9: a connect cancelled while it waited for the device map to store
    /// its new link dropped the link, whose `Drop` tore it down, and freed
    /// the slot before the link was down.
    #[tokio::test(start_paused = true)]
    async fn connect_cancelled_while_storing_its_link_disconnects_it() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default().with_max_connections(1));
            radio.set_connect_delay("A", Duration::from_secs(2));
            radio.set_disconnect_delay("A", Duration::from_secs(2));

            let connect = tokio::spawn({
                let core = Arc::clone(&core);
                async move { timeout(Duration::from_secs(3), core.connect("A")).await }
            });
            // Hold the device map from 1 s, while the connect is under way, so
            // that at 2 s the new link waits to be stored until the caller
            // gives up at 3 s.
            tokio::time::sleep(Duration::from_secs(1)).await;
            let busy = core.devices.read().await;
            assert!(connect.await.expect("connect task").is_err());

            assert!(
                radio.link_up("A"),
                "the cancelled connect dropped its new link: {:#?}",
                radio.events()
            );
            assert_eq!(
                core.available_connections().await,
                Some(0),
                "the slot was freed while the sensor was still connected"
            );
            tokio::time::sleep(Duration::from_secs(3)).await;
            assert!(!radio.link_up("A"), "the new link was not disconnected");
            assert_eq!(core.available_connections().await, Some(1));
            drop(busy);

            assert!(
                core.snapshot("A")
                    .await
                    .is_some_and(|entry| !entry.has_link)
            );
            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9, in BR-4's pattern: a disconnect whose caller gave up still ran
    /// to the end (as `Device::disconnect` does), but a connect of the same
    /// device made meanwhile didn't wait for it, so the old link's disconnect
    /// took the new link down while the manager kept the new handle.
    #[tokio::test(start_paused = true)]
    async fn connect_after_a_cancelled_disconnect_waits_for_the_link_to_go_down() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default());
            core.connect("A").await.expect("first connect");
            radio.set_disconnect_delay("A", Duration::from_secs(2));

            assert!(
                timeout(Duration::from_millis(500), core.disconnect("A"))
                    .await
                    .is_err()
            );
            core.connect("A").await.expect("second connect");
            // Until well after the first link's disconnect has finished.
            tokio::time::sleep(Duration::from_secs(3)).await;

            assert!(
                radio.link_up("A"),
                "the old link's disconnect took down the new link: {:#?}",
                radio.events()
            );
            // The second connect started only once the first link was down.
            let a = || "A".to_string();
            let events: Vec<FakeEvent> =
                radio.events().into_iter().map(|(_, event)| event).collect();
            assert_eq!(
                events,
                [
                    FakeEvent::ConnectStarted { id: a() },
                    FakeEvent::Connected { id: a(), handle: 1 },
                    FakeEvent::Disconnect { id: a(), handle: 1 },
                    FakeEvent::ConnectStarted { id: a() },
                    FakeEvent::Connected { id: a(), handle: 2 },
                ]
            );

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// BR-9, in BR-4's pattern: a connect of the same device made after a
    /// connect was cancelled while its new link waited for the device map
    /// didn't wait for that link to go down, so the abandoned link's
    /// disconnect could take the new link down.
    #[tokio::test(start_paused = true)]
    async fn connect_after_a_cancelled_connect_waits_for_its_link_to_go_down() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default());
            radio.set_connect_delay("A", Duration::from_secs(2));
            radio.set_disconnect_delay("A", Duration::from_secs(2));

            // As in test 9: the new link waits for the map from 2 s until the
            // caller gives up at 3 s.
            let connect = tokio::spawn({
                let core = Arc::clone(&core);
                async move { timeout(Duration::from_secs(3), core.connect("A")).await }
            });
            tokio::time::sleep(Duration::from_secs(1)).await;
            let busy = core.devices.read().await;
            assert!(connect.await.expect("connect task").is_err());
            drop(busy);

            radio.set_connect_delay("A", Duration::ZERO);
            core.connect("A").await.expect("second connect");
            // Until well after the abandoned link's disconnect has finished.
            tokio::time::sleep(Duration::from_secs(3)).await;

            assert!(
                radio.link_up("A"),
                "A is down after the second connect: {:#?}",
                radio.events()
            );
            // The second connect started only once the abandoned link was down.
            let a = || "A".to_string();
            let events: Vec<FakeEvent> =
                radio.events().into_iter().map(|(_, event)| event).collect();
            assert_eq!(
                events,
                [
                    FakeEvent::ConnectStarted { id: a() },
                    FakeEvent::Connected { id: a(), handle: 1 },
                    FakeEvent::Disconnect { id: a(), handle: 1 },
                    FakeEvent::ConnectStarted { id: a() },
                    FakeEvent::Connected { id: a(), handle: 2 },
                ]
            );

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// A disconnect sends `Disconnected` once it has taken the link out of
    /// the manager, even if closing the link then fails (as closing a link
    /// that dropped on its own does on macOS): the manager reports the device
    /// as disconnected from then on, and nothing sends the event later. The
    /// error is still returned.
    #[tokio::test(start_paused = true)]
    async fn disconnects_send_disconnected_even_if_the_close_fails() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default());
            core.add_device_with_priority("B", DevicePriority::Low)
                .await
                .expect("add_device_with_priority");
            for id in ["A", "B", "C"] {
                core.connect(id).await.expect("connect");
                radio.fail_disconnects(id);
            }
            let mut events = core.events.subscribe();

            let result = core.disconnect("A").await;
            assert!(matches!(result, Err(Error::Timeout { .. })), "{result:?}");
            assert_eq!(drain(&mut events), ["Disconnected A UserRequested"]);

            // B has the lowest priority.
            let result = core.evict_lowest_priority().await;
            assert!(matches!(result, Err(Error::Timeout { .. })), "{result:?}");
            assert_eq!(drain(&mut events), ["Disconnected B UserRequested"]);

            // C is the only device still connected.
            let results = core.disconnect_all().await;
            assert!(
                results.len() == 1 && matches!(results.get("C"), Some(Err(Error::Timeout { .. }))),
                "{results:?}"
            );
            assert_eq!(drain(&mut events), ["Disconnected C UserRequested"]);

            for id in ["A", "B", "C"] {
                assert_eq!(core.try_is_connected(id), Some(false), "{id}");
            }
            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// `remove_device` disconnects first. When closing the link fails, the
    /// device is still removed, as the manager no longer holds a link that a
    /// second call could close, and the error is returned.
    #[tokio::test(start_paused = true)]
    async fn remove_device_removes_the_device_even_if_its_disconnect_fails() {
        within(TEST_LIMIT, async {
            let radio = FakeRadio::new();
            let core = core(&radio, ManagerConfig::default());
            core.connect("A").await.expect("connect");
            radio.fail_disconnects("A");
            let mut events = core.events.subscribe();

            let result = core.remove_device("A").await;

            assert!(matches!(result, Err(Error::Timeout { .. })), "{result:?}");
            assert_eq!(core.device_count().await, 0);
            assert_eq!(drain(&mut events), ["Disconnected A UserRequested"]);
            // Gone: the health monitor has nothing to reconnect.
            assert_eq!(core.health_tick().await, TickOutcome::default());
            assert_eq!(radio.connect_count("A"), 1);
            assert!(!radio.link_up("A"));

            assert_no_orphans(&core, &radio).await;
            radio.assert_no_drop_teardown();
        })
        .await;
    }

    /// The health monitor: dead links, the user's intent, backoff and
    /// cancellation.
    mod health {
        use std::sync::Arc;
        use std::time::Duration;

        use tokio::time::{Instant, timeout};
        use tokio_util::sync::CancellationToken;

        use super::{assert_no_orphans, core, drain};
        use crate::error::Error;
        use crate::manager::{DevicePriority, EntrySnapshot, ManagerConfig, TickOutcome};
        use crate::reconnect::ReconnectOptions;
        use crate::test_support::{FakeEvent, FakeRadio, within};

        const LIMIT: Duration = Duration::from_secs(600);
        /// For the tests that run an hour or more of paused time.
        const LONG_LIMIT: Duration = Duration::from_secs(2 * 60 * 60);

        /// The radio log without times and without `Op` entries.
        fn radio_log(radio: &FakeRadio) -> Vec<FakeEvent> {
            radio
                .events()
                .into_iter()
                .map(|(_, event)| event)
                .filter(|event| !matches!(event, FakeEvent::Op { .. }))
                .collect()
        }

        /// When each connect to `id` started.
        fn connect_starts(radio: &FakeRadio, id: &str) -> Vec<Duration> {
            radio
                .events()
                .into_iter()
                .filter_map(|(at, event)| {
                    matches!(&event, FakeEvent::ConnectStarted { id: started } if started == id)
                        .then_some(at)
                })
                .collect()
        }

        /// When `id`'s link was first closed with a disconnect.
        fn first_disconnect(radio: &FakeRadio, id: &str) -> Option<Duration> {
            radio.events().into_iter().find_map(|(at, event)| {
                matches!(&event, FakeEvent::Disconnect { id: closed, .. } if closed == id)
                    .then_some(at)
            })
        }

        // ---- Dead links ----

        #[tokio::test(start_paused = true)]
        async fn health_tick_replaces_a_dead_handle() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                radio.lose_link("A");
                let mut events = core.events.subscribe();

                let outcome = core.health_tick().await;

                assert_eq!(
                    outcome,
                    TickOutcome {
                        healthy: 0,
                        repaired: 1,
                        failed: 0
                    }
                );
                let log = radio_log(&radio);
                let [
                    FakeEvent::ConnectStarted { .. },
                    FakeEvent::Connected { handle: first, .. },
                    FakeEvent::Disconnect { handle: closed, .. },
                    FakeEvent::ConnectStarted { .. },
                    FakeEvent::Connected { handle: second, .. },
                ] = log.as_slice()
                else {
                    panic!("unexpected radio log: {log:?}");
                };
                assert_eq!(closed, first, "the dead handle was not the one closed");
                assert_ne!(second, first);
                assert!(radio.link_up("A"));
                assert_eq!(
                    drain(&mut events),
                    [
                        "Disconnected A Unknown",
                        "ReconnectStarted A 1",
                        "Connected A",
                        "ReconnectSucceeded A 1"
                    ]
                );
                core.read_current("A").await.unwrap();

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn connect_replaces_a_dead_link() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                // A link that is up: connecting again changes nothing.
                core.connect("A").await.unwrap();
                assert_eq!(radio.connect_count("A"), 1);

                radio.lose_link("A");
                let mut events = core.events.subscribe();
                core.connect("A").await.unwrap();

                assert_eq!(radio.connect_count("A"), 2, "connect() kept the dead link");
                let log = radio_log(&radio);
                let [
                    FakeEvent::ConnectStarted { .. },
                    FakeEvent::Connected { handle: first, .. },
                    FakeEvent::Disconnect { handle: closed, .. },
                    FakeEvent::ConnectStarted { .. },
                    FakeEvent::Connected { handle: second, .. },
                ] = log.as_slice()
                else {
                    panic!("unexpected radio log: {log:?}");
                };
                assert_eq!(closed, first, "the dead handle was not the one closed");
                assert_ne!(second, first);
                assert!(radio.link_up("A"));
                assert_eq!(
                    drain(&mut events),
                    ["Disconnected A Unknown", "Connected A"]
                );
                core.read_current("A").await.unwrap();

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn health_tick_replaces_a_zombie_handle_when_validation_is_on() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let validating = core(&radio, ManagerConfig::default().connection_validation(true));
                validating.connect("A").await.unwrap();
                radio.make_zombie("A");

                assert_eq!(
                    validating.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 1,
                        failed: 0
                    }
                );
                assert_eq!(radio.connect_count("A"), 2);
                assert!(radio.link_up("A"));
                // The new link answers.
                assert_eq!(
                    validating.health_tick().await,
                    TickOutcome {
                        healthy: 1,
                        repaired: 0,
                        failed: 0
                    }
                );
                validating.read_current("A").await.unwrap();
                assert_no_orphans(&validating, &radio).await;
                radio.assert_no_drop_teardown();

                // Documented limitation: without validation the monitor only
                // asks the stack, which still reports the zombie as connected.
                let radio = FakeRadio::new();
                let asking = core(
                    &radio,
                    ManagerConfig::default().connection_validation(false),
                );
                asking.connect("A").await.unwrap();
                radio.make_zombie("A");
                assert_eq!(
                    asking.health_tick().await,
                    TickOutcome {
                        healthy: 1,
                        repaired: 0,
                        failed: 0
                    }
                );
                assert_eq!(radio.connect_count("A"), 1);
                assert!(
                    asking.read_current("A").await.is_err(),
                    "the zombie's reads should time out"
                );
                assert_no_orphans(&asking, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn health_tick_counts_a_failed_repair_as_failure() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                radio.lose_link("A");
                radio.script_connects("A", [false]);

                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 0,
                        failed: 1
                    }
                );
                let snapshot = core.snapshot("A").await.unwrap();
                assert!(!snapshot.has_link);
                assert_eq!(snapshot.failures, 1);
                assert!(!radio.link_up("A"));

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn health_monitor_tightens_interval_while_repairs_fail() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                // 30 s base interval, adaptive, 5 s minimum.
                let core = core(&radio, ManagerConfig::default());
                core.add_device_with_options(
                    "A",
                    ReconnectOptions {
                        max_attempts: None,
                        ..ReconnectOptions::fixed_delay(Duration::from_millis(100))
                    },
                )
                .await
                .unwrap();
                core.connect("A").await.unwrap();
                // B stays healthy on every tick: a failed repair of A still
                // counts as a failure.
                core.connect("B").await.unwrap();
                radio.lose_link("A");
                radio.script_connects("A", [false; 10]);

                let cancel = CancellationToken::new();
                let monitor = Arc::clone(&core).spawn_health_monitor(cancel.clone());
                tokio::time::sleep(Duration::from_secs(70)).await;
                cancel.cancel();
                monitor.await.unwrap();

                // Each tick with a failed repair halves the interval: 30 s, 15 s,
                // 7.5 s, then the 5 s minimum. The first start is `connect`.
                let starts = connect_starts(&radio, "A");
                assert_eq!(
                    starts[1..],
                    [30_000, 45_000, 52_500, 57_500, 62_500, 67_500].map(Duration::from_millis)
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn probes_run_concurrently_so_a_slow_device_does_not_delay_others() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default().connection_validation(true));
                core.add_device_with_priority("A", DevicePriority::High)
                    .await
                    .unwrap();
                core.add_device_with_priority("B", DevicePriority::Normal)
                    .await
                    .unwrap();
                core.connect("A").await.unwrap();
                core.connect("B").await.unwrap();
                // A's check takes 3 s and fails (a validation read that times
                // out); B's link is simply gone.
                radio.make_zombie("A");
                radio.set_probe_delay("A", Duration::from_secs(3));
                radio.lose_link("B");

                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 2,
                        failed: 0
                    }
                );
                let b = first_disconnect(&radio, "B").expect("B's dead link was not closed");
                assert!(
                    b < Duration::from_secs(1),
                    "B was closed at {b:?}, after A's check"
                );
                let a = first_disconnect(&radio, "A").expect("A's zombie link was not closed");
                assert!(
                    a >= Duration::from_secs(3),
                    "A was closed at {a:?}, before its check ended"
                );
                // Repairs run one at a time, highest priority first.
                let started: Vec<FakeEvent> = radio_log(&radio)
                    .into_iter()
                    .filter(|event| matches!(event, FakeEvent::ConnectStarted { .. }))
                    .collect();
                assert_eq!(
                    started[2..],
                    [
                        FakeEvent::ConnectStarted { id: "A".into() },
                        FakeEvent::ConnectStarted { id: "B".into() }
                    ]
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn health_monitor_stops_promptly_when_cancelled_mid_reconnect() {
            within(LIMIT, async {
                let start = Instant::now();
                let radio = FakeRadio::new();
                let core = core(
                    &radio,
                    ManagerConfig::default()
                        .health_check_interval(Duration::from_secs(5))
                        .adaptive_interval(false),
                );
                // Added but not connected yet: the tick at 5 s connects it,
                // and that connect takes 60 s.
                core.add_device("A").await.unwrap();
                radio.set_connect_delay("A", Duration::from_secs(60));

                let cancel = CancellationToken::new();
                let monitor = Arc::clone(&core).spawn_health_monitor(cancel.clone());
                tokio::time::sleep(Duration::from_secs(6)).await;
                assert_eq!(radio.connect_count("A"), 1, "the tick started no connect");

                cancel.cancel();
                monitor.await.unwrap();
                let stopped = start.elapsed();
                assert!(
                    stopped < Duration::from_secs(10),
                    "the monitor stopped at {stopped:?}"
                );

                // The abandoned connect never completes, and its slot is free.
                tokio::time::sleep(Duration::from_secs(120)).await;
                assert!(!radio.link_up("A"));
                assert_eq!(
                    core.available_connections().await,
                    Some(core.config.max_concurrent_connections)
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// A `connect()` given up on while it closes a dead link keeps the
        /// device until that link is down, as a cancelled `disconnect()` does:
        /// a disconnect acts on the sensor, not on the handle, so a new link
        /// made before it finished would be taken down by it.
        #[tokio::test(start_paused = true)]
        async fn connect_after_a_cancelled_connect_waits_for_the_dead_link_to_go_down() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                radio.set_disconnect_delay("A", Duration::from_secs(2));
                radio.lose_link("A");

                // Given up on 0.5 s into closing the dead link, which takes 2 s.
                let first = timeout(Duration::from_millis(500), core.connect("A")).await;
                assert!(
                    first.is_err(),
                    "connect() returned {first:?} without closing the dead link"
                );
                core.connect("A").await.expect("second connect");
                // Until well after the dead link's disconnect has finished.
                tokio::time::sleep(Duration::from_secs(3)).await;

                assert!(
                    radio.link_up("A"),
                    "the old link's disconnect took down the new link: {:#?}",
                    radio.events()
                );
                // The second connect started only once the dead link was down.
                assert_eq!(
                    connect_starts(&radio, "A"),
                    [Duration::ZERO, Duration::from_secs(2)]
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// The same for a health tick that the monitor's cancel drops while
        /// the tick closes a dead link.
        #[tokio::test(start_paused = true)]
        async fn connect_after_a_cancelled_health_tick_waits_for_the_dead_link_to_go_down() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(
                    &radio,
                    ManagerConfig::default()
                        .health_check_interval(Duration::from_secs(5))
                        .adaptive_interval(false),
                );
                core.connect("A").await.unwrap();
                radio.set_disconnect_delay("A", Duration::from_secs(2));
                radio.lose_link("A");

                // The tick at 5 s finds the link dead and starts closing it,
                // which takes 2 s; the monitor is cancelled 0.5 s later.
                let cancel = CancellationToken::new();
                let monitor = Arc::clone(&core).spawn_health_monitor(cancel.clone());
                tokio::time::sleep(Duration::from_millis(5500)).await;
                cancel.cancel();
                monitor.await.unwrap();
                core.connect("A").await.expect("connect after the cancel");
                assert_eq!(radio.connect_count("A"), 2, "the dead link was kept");
                // Until well after the dead link's disconnect has finished.
                tokio::time::sleep(Duration::from_secs(3)).await;

                assert!(
                    radio.link_up("A"),
                    "the old link's disconnect took down the new link: {:#?}",
                    radio.events()
                );
                // The connect started only once the dead link was down.
                assert_eq!(
                    connect_starts(&radio, "A"),
                    [Duration::ZERO, Duration::from_secs(7)]
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        // ---- The user's intent, backoff and limits ----

        #[tokio::test(start_paused = true)]
        async fn health_tick_skips_user_disconnected_device() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                core.disconnect("A").await.unwrap();
                let mut events = core.events.subscribe();

                assert_eq!(core.health_tick().await, TickOutcome::default());
                assert_eq!(radio.connect_count("A"), 1);
                assert!(!radio.link_up("A"));
                assert!(!core.snapshot("A").await.unwrap().wanted);
                assert_eq!(drain(&mut events), Vec::<String>::new());

                // An added device is connected by the monitor, unless
                // `disconnect_all` withdrew it first, link or no link.
                core.add_device("B").await.unwrap();
                assert!(core.snapshot("B").await.unwrap().wanted);
                assert!(core.disconnect_all().await.is_empty());
                assert_eq!(core.health_tick().await, TickOutcome::default());
                assert_eq!(radio.connect_count("B"), 0);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn health_tick_skips_evicted_device() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default().with_max_connections(2));
                core.add_device_with_priority("A", DevicePriority::Low)
                    .await
                    .unwrap();
                core.add_device_with_priority("B", DevicePriority::High)
                    .await
                    .unwrap();
                core.connect("A").await.unwrap();
                core.connect("B").await.unwrap();

                assert!(core.evict_lowest_priority().await.unwrap());
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 1,
                        repaired: 0,
                        failed: 0
                    }
                );
                assert_eq!(radio.connect_count("A"), 1);
                assert!(!radio.link_up("A"));
                assert!(radio.link_up("B"));

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn health_tick_honours_reconnect_backoff() {
            within(LONG_LIMIT, async {
                let start = Instant::now();
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.add_device_with_options(
                    "A",
                    ReconnectOptions {
                        max_attempts: None,
                        initial_delay: Duration::from_secs(60),
                        max_delay: Duration::from_secs(600),
                        backoff_multiplier: 2.0,
                        use_exponential_backoff: true,
                    },
                )
                .await
                .unwrap();
                core.connect("A").await.unwrap();
                radio.lose_link("A");
                radio.script_connects("A", [false; 20]);

                for _ in 0..800 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    core.health_tick().await;
                }

                // A tick every 5 s. The first repair runs on the tick that
                // finds the link dead; after that the waits follow the
                // options: 60 s, doubling up to 600 s.
                let starts = connect_starts(&radio, "A");
                let gaps: Vec<u64> = starts[1..]
                    .windows(2)
                    .map(|pair| (pair[1] - pair[0]).as_secs())
                    .collect();
                assert_eq!(gaps, [60, 120, 240, 480, 600, 600, 600, 600, 600]);
                let snapshot = core.snapshot("A").await.unwrap();
                assert_eq!(snapshot.failures, 10);
                assert_eq!(
                    snapshot.retry_at,
                    Some(start + *starts.last().unwrap() + Duration::from_secs(600))
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn health_tick_gives_up_after_max_attempts_until_explicit_connect() {
            within(LONG_LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.add_device_with_options(
                    "A",
                    ReconnectOptions::default()
                        .max_attempts(3)
                        .initial_delay(Duration::from_secs(1)),
                )
                .await
                .unwrap();
                core.connect("A").await.unwrap();
                radio.lose_link("A");
                radio.script_connects("A", [false; 3]);
                let mut events = core.events.subscribe();

                // One hour of ticks, 5 s apart.
                for _ in 0..720 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    core.health_tick().await;
                }

                assert_eq!(
                    radio.connect_count("A"),
                    4,
                    "the first connect and exactly 3 repairs"
                );
                let errors: Vec<String> = drain(&mut events)
                    .into_iter()
                    .filter(|line| line.starts_with("Error"))
                    .collect();
                assert_eq!(errors, ["Error A: auto-reconnect gave up after 3 attempts"]);
                let snapshot = core.snapshot("A").await.unwrap();
                assert!(snapshot.gave_up);
                assert_eq!(snapshot.failures, 3);

                // An explicit connect starts over.
                core.connect("A").await.unwrap();
                assert!(radio.link_up("A"));
                let snapshot = core.snapshot("A").await.unwrap();
                assert!(!snapshot.gave_up);
                assert_eq!(snapshot.failures, 0);
                assert!(snapshot.wanted);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn explicit_connect_during_a_failing_repair_starts_over() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.add_device_with_options("A", ReconnectOptions::default().max_attempts(1))
                    .await
                    .unwrap();
                core.connect("A").await.unwrap();
                radio.lose_link("A");
                // The repair's connect and then the user's each take 10 s and
                // fail.
                radio.set_connect_delay("A", Duration::from_secs(10));
                radio.script_connects("A", [false, false]);

                let tick = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.health_tick().await }
                });
                tokio::time::sleep(Duration::from_secs(1)).await;
                // The user connects while the repair runs: the connect waits
                // for the repair, which fails and uses up max_attempts, and
                // then makes its own attempt.
                let connecting = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.connect("A").await }
                });
                assert_eq!(
                    tick.await.unwrap(),
                    TickOutcome {
                        healthy: 0,
                        repaired: 0,
                        failed: 1
                    }
                );
                assert!(connecting.await.unwrap().is_err());
                assert_eq!(radio.connect_count("A"), 3);

                // The explicit connect started over, although it failed too.
                assert_eq!(
                    core.snapshot("A").await,
                    Some(EntrySnapshot {
                        has_link: false,
                        failures: 0,
                        wanted: true,
                        gave_up: false,
                        retry_at: None,
                    })
                );
                // So the monitor keeps repairing the device.
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 1,
                        failed: 0
                    }
                );
                assert!(radio.link_up("A"));

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn remove_device_during_connect_keeps_it_removed() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                radio.set_connect_delay("A", Duration::from_secs(10));
                let connecting = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.connect("A").await }
                });
                tokio::time::sleep(Duration::from_secs(1)).await;

                core.remove_device("A").await.unwrap();

                let result = connecting.await.unwrap();
                assert!(
                    matches!(result, Err(Error::Cancelled)),
                    "connect returned {result:?}"
                );
                assert_eq!(core.device_count().await, 0);
                assert!(!radio.link_up("A"));

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn disconnect_cancels_running_and_waiting_connects() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                radio.set_connect_delay("A", Duration::from_secs(10));
                // One connect runs; the other waits for it.
                let connects: Vec<_> = (0..2)
                    .map(|_| {
                        let core = Arc::clone(&core);
                        tokio::spawn(async move { core.connect("A").await })
                    })
                    .collect();
                tokio::time::sleep(Duration::from_secs(1)).await;

                core.disconnect("A").await.unwrap();

                for connecting in connects {
                    let result = connecting.await.unwrap();
                    assert!(
                        matches!(result, Err(Error::Cancelled)),
                        "connect returned {result:?}"
                    );
                }
                // The waiting connect gave up without a Bluetooth connect.
                assert_eq!(radio.connect_count("A"), 1);
                assert!(!radio.link_up("A"));
                assert!(!core.snapshot("A").await.unwrap().wanted);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// A `disconnect()` or `remove_device()` withdraws the device before
        /// it waits for the device's lock, and from then on the health
        /// monitor leaves the device alone. So one dropped during that wait
        /// (here, for a check that finds the link alive) must still finish
        /// afterwards, or the device stays connected and unchecked.
        #[tokio::test(start_paused = true)]
        async fn a_dropped_disconnect_finishes_after_the_check_it_waited_for() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default().with_max_connections(2));
                for id in ["A", "B"] {
                    core.connect(id).await.unwrap();
                    // Each check takes 3 s and finds the link alive.
                    radio.set_probe_delay(id, Duration::from_secs(3));
                }
                let mut events = core.events.subscribe();

                let tick = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.health_tick().await }
                });
                tokio::time::sleep(Duration::from_secs(1)).await;
                // Both wait for the checks, and are given up on at 2 s.
                let (disconnect, remove) = tokio::join!(
                    timeout(Duration::from_secs(1), core.disconnect("A")),
                    timeout(Duration::from_secs(1), core.remove_device("B")),
                );
                assert!(disconnect.is_err(), "disconnect() returned {disconnect:?}");
                assert!(remove.is_err(), "remove_device() returned {remove:?}");
                assert_eq!(
                    tick.await.unwrap(),
                    TickOutcome {
                        healthy: 2,
                        repaired: 0,
                        failed: 0
                    }
                );
                tokio::time::sleep(Duration::from_secs(1)).await;

                assert_eq!(radio.up_ids(), Vec::<String>::new());
                assert_eq!(core.device_ids().await, ["A"]);
                assert_eq!(
                    core.snapshot("A").await,
                    Some(EntrySnapshot {
                        has_link: false,
                        failures: 0,
                        wanted: false,
                        gave_up: false,
                        retry_at: None,
                    })
                );
                assert_eq!(core.available_connections().await, Some(2));
                let mut sent = drain(&mut events);
                sent.sort();
                assert_eq!(
                    sent,
                    [
                        "Disconnected A UserRequested",
                        "Disconnected B UserRequested"
                    ]
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// `disconnect_all()` withdraws every device at once and then
        /// disconnects each one, so one dropped in between (here, while
        /// another user of the device map holds it) must still disconnect
        /// the devices it withdrew.
        #[tokio::test(start_paused = true)]
        async fn a_dropped_disconnect_all_disconnects_every_device_it_withdrew() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                let mut events = core.events.subscribe();

                // The device map is busy when disconnect_all() starts, and a
                // reader that asks for it next holds it from the moment
                // disconnect_all() has withdrawn the devices until 2 s.
                let busy = core.devices.read().await;
                let disconnect_all = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { timeout(Duration::from_secs(1), core.disconnect_all()).await }
                });
                tokio::time::sleep(Duration::from_millis(1)).await;
                let reader = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move {
                        let _map = core.devices.read().await;
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                });
                tokio::time::sleep(Duration::from_millis(1)).await;
                drop(busy);
                let given_up = disconnect_all.await.unwrap();
                assert!(given_up.is_err(), "disconnect_all() returned {given_up:?}");
                assert!(!core.snapshot("A").await.unwrap().wanted);
                reader.await.unwrap();
                tokio::time::sleep(Duration::from_secs(1)).await;

                assert!(
                    !radio.link_up("A"),
                    "A is still connected, but withdrawn: {:#?}",
                    radio.events()
                );
                assert!(!core.snapshot("A").await.unwrap().has_link);
                assert_eq!(drain(&mut events), ["Disconnected A UserRequested"]);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// A `disconnect()` withdraws the device at once but takes the link
        /// only once its task runs. A `connect()` that starts in between
        /// (here, spawned right after it) re-arms the device, finds the link
        /// up and returns `Ok`, so the disconnect must then do nothing:
        /// taking the link would leave the device wanted but unlinked, which
        /// neither order of the two calls gives.
        #[tokio::test(start_paused = true)]
        async fn a_connect_spawned_right_after_a_disconnect_keeps_the_device_connected() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                let mut events = core.events.subscribe();

                let disconnect = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.disconnect("A").await }
                });
                let connect = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.connect("A").await }
                });
                let disconnect = disconnect.await.unwrap();
                let connect = connect.await.unwrap();

                assert!(disconnect.is_ok(), "disconnect() returned {disconnect:?}");
                assert!(connect.is_ok(), "connect() returned {connect:?}");
                assert!(radio.link_up("A"), "{:#?}", radio.events());
                assert_eq!(
                    core.snapshot("A").await,
                    Some(EntrySnapshot {
                        has_link: true,
                        failures: 0,
                        wanted: true,
                        gave_up: false,
                        retry_at: None,
                    })
                );
                assert_eq!(drain(&mut events), Vec::<String>::new());
                // The disconnect took nothing: the link is still the first one.
                let a = || "A".to_string();
                assert_eq!(
                    radio_log(&radio),
                    [
                        FakeEvent::ConnectStarted { id: a() },
                        FakeEvent::Connected { id: a(), handle: 1 },
                    ]
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// As in the test above, for `disconnect_all()` and
        /// `evict_lowest_priority()`, which disconnect each device the same
        /// way: a device that a `connect()` re-arms first stays connected,
        /// and the others are still disconnected.
        #[tokio::test(start_paused = true)]
        async fn a_connect_spawned_right_after_disconnect_all_or_an_eviction_keeps_its_device() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                for id in ["A", "B"] {
                    core.connect(id).await.unwrap();
                }
                let mut events = core.events.subscribe();
                let connect_a = || {
                    let core = Arc::clone(&core);
                    tokio::spawn(async move { core.connect("A").await })
                };

                let disconnect_all = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.disconnect_all().await }
                });
                let connect = connect_a();
                let results = disconnect_all.await.unwrap();
                let connect = connect.await.unwrap();
                assert!(
                    results.len() == 2 && results.values().all(Result::is_ok),
                    "disconnect_all() returned {results:?}"
                );
                assert!(connect.is_ok(), "connect() returned {connect:?}");
                assert_eq!(radio.up_ids(), ["A"]);
                assert_eq!(drain(&mut events), ["Disconnected B UserRequested"]);

                // A is the only device connected, so it is the one evicted.
                let evict = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.evict_lowest_priority().await }
                });
                let connect = connect_a();
                let evicted = evict.await.unwrap();
                let connect = connect.await.unwrap();
                assert!(
                    matches!(evicted, Ok(true)),
                    "evict_lowest_priority() returned {evicted:?}"
                );
                assert!(connect.is_ok(), "connect() returned {connect:?}");
                assert_eq!(radio.up_ids(), ["A"]);
                assert_eq!(drain(&mut events), Vec::<String>::new());
                assert_eq!(radio.connect_count("A"), 1);
                let a = core.snapshot("A").await.unwrap();
                assert!(a.wanted && a.has_link, "{a:?}");

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// The same race when the `disconnect()` is dropped after its first
        /// poll: it has withdrawn the device and left the rest to its task,
        /// which hasn't run yet when a `connect()` of the device starts.
        #[tokio::test(start_paused = true)]
        async fn a_connect_after_a_disconnect_polled_once_keeps_the_device_connected() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                let mut events = core.events.subscribe();

                let mut disconnect = Box::pin(core.disconnect("A"));
                let first = disconnect
                    .as_mut()
                    .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
                assert!(first.is_pending(), "disconnect() returned {first:?}");
                drop(disconnect);
                assert!(!core.snapshot("A").await.unwrap().wanted, "not withdrawn");

                core.connect("A").await.expect("connect");
                // Until well after the dropped disconnect's task has run.
                tokio::time::sleep(Duration::from_secs(1)).await;

                assert!(radio.link_up("A"), "{:#?}", radio.events());
                assert_eq!(
                    core.snapshot("A").await,
                    Some(EntrySnapshot {
                        has_link: true,
                        failures: 0,
                        wanted: true,
                        gave_up: false,
                        retry_at: None,
                    })
                );
                assert_eq!(drain(&mut events), Vec::<String>::new());

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// A removal always goes ahead: a `connect()` spawned right after a
        /// `remove_device()` of the same device can return first, and the
        /// device is removed after it all the same, as when the two calls run
        /// one after the other.
        #[tokio::test(start_paused = true)]
        async fn a_connect_spawned_right_after_a_removal_does_not_keep_the_device() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.connect("A").await.unwrap();
                let mut events = core.events.subscribe();

                let remove = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.remove_device("A").await }
                });
                let connect = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.connect("A").await }
                });
                let remove = remove.await.unwrap();
                let connect = connect.await.unwrap();

                assert!(remove.is_ok(), "remove_device() returned {remove:?}");
                // Connected, then removed; or removed while the connect waited.
                assert!(
                    matches!(connect, Ok(()) | Err(Error::Cancelled)),
                    "connect() returned {connect:?}"
                );
                assert_eq!(core.device_count().await, 0);
                assert!(!radio.link_up("A"), "{:#?}", radio.events());
                assert_eq!(drain(&mut events), ["Disconnected A UserRequested"]);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn repairs_skip_devices_the_user_changed_during_the_tick() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.add_device_with_priority("A", DevicePriority::High)
                    .await
                    .unwrap();
                for id in ["A", "B", "C", "D"] {
                    core.connect(id).await.unwrap();
                    radio.lose_link(id);
                }
                // All four are due; A goes first, and its connect takes 10 s.
                radio.set_connect_delay("A", Duration::from_secs(10));
                let mut events = core.events.subscribe();
                let tick = tokio::spawn({
                    let core = Arc::clone(&core);
                    async move { core.health_tick().await }
                });
                tokio::time::sleep(Duration::from_secs(1)).await;

                // Meanwhile the user connects B, disconnects C and removes D.
                core.connect("B").await.unwrap();
                core.disconnect("C").await.unwrap();
                core.remove_device("D").await.unwrap();

                assert_eq!(
                    tick.await.unwrap(),
                    TickOutcome {
                        healthy: 0,
                        repaired: 1,
                        failed: 0
                    }
                );
                let repairs: Vec<String> = drain(&mut events)
                    .into_iter()
                    .filter(|line| line.starts_with("Reconnect"))
                    .collect();
                assert_eq!(repairs, ["ReconnectStarted A 1", "ReconnectSucceeded A 1"]);
                assert_eq!(
                    ["A", "B", "C", "D"].map(|id| radio.connect_count(id)),
                    [2, 2, 1, 1]
                );
                assert_eq!(radio.up_ids(), ["A", "B"]);
                assert_eq!(core.device_count().await, 3);
                assert!(!core.snapshot("C").await.unwrap().wanted);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn add_device_with_options_rejects_invalid_options() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let plain = core(&radio, ManagerConfig::default());
                let result = plain
                    .add_device_with_options(
                        "A",
                        ReconnectOptions::default().backoff_multiplier(0.5),
                    )
                    .await;
                assert!(
                    matches!(result, Err(Error::InvalidConfig(_))),
                    "add_device_with_options accepted invalid options: {result:?}"
                );
                assert_eq!(plain.device_count().await, 0);

                // The same options as the manager's default.
                let strict = core(
                    &radio,
                    ManagerConfig {
                        default_reconnect_options: ReconnectOptions::default()
                            .backoff_multiplier(0.5),
                        ..ManagerConfig::default()
                    },
                );
                let result = strict
                    .add_device_with_priority("B", DevicePriority::High)
                    .await;
                assert!(
                    matches!(result, Err(Error::InvalidConfig(_))),
                    "add_device_with_priority accepted invalid options: {result:?}"
                );
                let result = strict.add_device("B").await;
                assert!(
                    matches!(result, Err(Error::InvalidConfig(_))),
                    "add_device accepted invalid options: {result:?}"
                );
                assert_eq!(strict.device_count().await, 0);

                assert_no_orphans(&plain, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        #[tokio::test(start_paused = true)]
        async fn repairs_wait_for_a_free_slot_without_counting_failures() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default().with_max_connections(1));
                core.connect("A").await.unwrap();
                core.add_device_with_options("B", ReconnectOptions::default())
                    .await
                    .unwrap();
                let mut events = core.events.subscribe();

                // A holds the only slot. B waits for it: no connect, no failed
                // repair and no give-up, however many ticks pass.
                for _ in 0..12 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    assert_eq!(
                        core.health_tick().await,
                        TickOutcome {
                            healthy: 1,
                            repaired: 0,
                            failed: 0
                        }
                    );
                }
                assert_eq!(radio.connect_count("B"), 0);
                assert_eq!(drain(&mut events), Vec::<String>::new());
                let waiting = core.snapshot("B").await.unwrap();
                assert_eq!((waiting.failures, waiting.gave_up), (0, false));

                // Once A's slot is free, the next tick connects B.
                core.disconnect("A").await.unwrap();
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 1,
                        failed: 0
                    }
                );
                assert!(radio.link_up("B"));
                assert_eq!(
                    drain(&mut events),
                    [
                        "Disconnected A UserRequested",
                        "ReconnectStarted B 1",
                        "Connected B",
                        "ReconnectSucceeded B 1"
                    ]
                );

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// A repair that succeeds starts the backoff over: the next lost link
        /// is repaired at once, as attempt 1 again, with all of `max_attempts`.
        #[tokio::test(start_paused = true)]
        async fn successful_repair_starts_the_backoff_over() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.add_device_with_options(
                    "A",
                    ReconnectOptions::default()
                        .max_attempts(2)
                        .initial_delay(Duration::from_secs(60)),
                )
                .await
                .unwrap();
                core.connect("A").await.unwrap();
                radio.lose_link("A");
                // The first repair fails; the one 60 s later succeeds.
                radio.script_connects("A", [false]);
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 0,
                        failed: 1
                    }
                );
                tokio::time::sleep(Duration::from_secs(60)).await;
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 1,
                        failed: 0
                    }
                );
                assert_eq!(
                    core.snapshot("A").await,
                    Some(EntrySnapshot {
                        has_link: true,
                        failures: 0,
                        wanted: true,
                        gave_up: false,
                        retry_at: None,
                    })
                );

                // The next loss is repaired as attempt 1 again, and one more
                // failure doesn't use up the two attempts.
                radio.lose_link("A");
                radio.script_connects("A", [false]);
                let mut events = core.events.subscribe();
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 0,
                        failed: 1
                    }
                );
                assert_eq!(
                    drain(&mut events),
                    ["Disconnected A Unknown", "ReconnectStarted A 1"]
                );
                assert!(!core.snapshot("A").await.unwrap().gave_up);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// The same holds for a repair cancelled during the device-info read
        /// after its link was stored: the device stays connected, so its
        /// backoff starts over as soon as the link is stored.
        #[tokio::test(start_paused = true)]
        async fn a_repair_cancelled_during_the_info_read_starts_the_backoff_over() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                core.add_device_with_options(
                    "A",
                    ReconnectOptions::default()
                        .max_attempts(2)
                        .initial_delay(Duration::from_secs(60)),
                )
                .await
                .unwrap();
                core.connect("A").await.unwrap();
                radio.lose_link("A");
                // The first repair fails.
                radio.script_connects("A", [false]);
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 0,
                        failed: 1
                    }
                );
                // The one 60 s later connects, and is cancelled 1 s into the
                // device-info read, which takes 5 s.
                tokio::time::sleep(Duration::from_secs(60)).await;
                radio.set_info_delay("A", Duration::from_secs(5));
                let cancelled = timeout(Duration::from_secs(1), core.health_tick()).await;
                assert!(cancelled.is_err(), "the tick returned {cancelled:?}");
                assert_eq!(
                    core.snapshot("A").await,
                    Some(EntrySnapshot {
                        has_link: true,
                        failures: 0,
                        wanted: true,
                        gave_up: false,
                        retry_at: None,
                    })
                );

                // The next loss is repaired as attempt 1 again, and one more
                // failure doesn't use up the two attempts.
                radio.set_info_delay("A", Duration::ZERO);
                radio.lose_link("A");
                radio.script_connects("A", [false]);
                let mut events = core.events.subscribe();
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 0,
                        failed: 1
                    }
                );
                assert_eq!(
                    drain(&mut events),
                    ["Disconnected A Unknown", "ReconnectStarted A 1"]
                );
                assert!(!core.snapshot("A").await.unwrap().gave_up);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// A wait too long to add to an `Instant` is capped at
        /// `MAX_RETRY_WAIT`. These options pass `validate()`; without the cap
        /// the failed repair would panic and end the monitor for every device.
        #[tokio::test(start_paused = true)]
        async fn an_overlong_reconnect_wait_is_capped_and_the_monitor_keeps_running() {
            within(LIMIT, async {
                let start = Instant::now();
                let radio = FakeRadio::new();
                let core = core(
                    &radio,
                    ManagerConfig::default()
                        .health_check_interval(Duration::from_secs(5))
                        .adaptive_interval(false),
                );
                core.add_device_with_options(
                    "A",
                    ReconnectOptions::fixed_delay(Duration::MAX).max_delay(Duration::MAX),
                )
                .await
                .unwrap();
                core.connect("A").await.unwrap();
                core.connect("B").await.unwrap();
                radio.lose_link("A");
                radio.script_connects("A", [false]);

                let cancel = CancellationToken::new();
                let monitor = Arc::clone(&core).spawn_health_monitor(cancel.clone());
                // The tick at 5 s fails A's repair.
                tokio::time::sleep(Duration::from_secs(6)).await;
                let failed_at = connect_starts(&radio, "A")[1];
                assert_eq!(failed_at, Duration::from_secs(5));
                let snapshot = core.snapshot("A").await.unwrap();
                assert_eq!(snapshot.failures, 1);
                assert_eq!(
                    snapshot.retry_at,
                    Some(start + failed_at + crate::manager::MAX_RETRY_WAIT)
                );

                // The monitor still runs: the tick at 10 s repairs B.
                radio.lose_link("B");
                tokio::time::sleep(Duration::from_secs(5)).await;
                assert!(
                    radio.link_up("B"),
                    "the monitor stopped: {:#?}",
                    radio.events()
                );
                assert!(!monitor.is_finished());
                cancel.cancel();
                monitor.await.expect("the health monitor panicked");

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// A link leaves the manager when a check, a `connect()`, a
        /// `disconnect()` or a `remove_device()` starts closing it, and
        /// `Disconnected` is sent then: a caller dropped during the close,
        /// such as the monitor when it is cancelled or a call under a
        /// timeout, must not lose the event. A removal dropped during the
        /// close still removes the device once the link is down.
        #[tokio::test(start_paused = true)]
        async fn disconnected_is_sent_even_if_the_close_is_cancelled() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(
                    &radio,
                    ManagerConfig::default()
                        .health_check_interval(Duration::from_secs(5))
                        .adaptive_interval(false),
                );
                for id in ["A", "B", "C", "D"] {
                    core.connect(id).await.unwrap();
                    radio.set_disconnect_delay(id, Duration::from_secs(2));
                }
                radio.lose_link("A");
                let mut events = core.events.subscribe();

                // The tick at 5 s starts closing A's dead link, which takes
                // 2 s; the monitor is cancelled 0.5 s later.
                let cancel = CancellationToken::new();
                let monitor = Arc::clone(&core).spawn_health_monitor(cancel.clone());
                tokio::time::sleep(Duration::from_millis(5500)).await;
                cancel.cancel();
                monitor.await.unwrap();
                // A connect() given up on 0.5 s into closing B's dead link.
                radio.lose_link("B");
                let given_up = timeout(Duration::from_millis(500), core.connect("B")).await;
                assert!(given_up.is_err(), "connect() returned {given_up:?}");
                // A disconnect() and a remove_device() given up on 0.5 s into
                // closing C's and D's links.
                let given_up = timeout(Duration::from_millis(500), core.disconnect("C")).await;
                assert!(given_up.is_err(), "disconnect() returned {given_up:?}");
                let given_up = timeout(Duration::from_millis(500), core.remove_device("D")).await;
                assert!(given_up.is_err(), "remove_device() returned {given_up:?}");

                assert_eq!(
                    drain(&mut events),
                    [
                        "Disconnected A Unknown",
                        "Disconnected B Unknown",
                        "Disconnected C UserRequested",
                        "Disconnected D UserRequested"
                    ]
                );
                for id in ["A", "B", "C", "D"] {
                    assert_eq!(core.try_is_connected(id), Some(false), "{id}");
                }

                // Until well after the closes have finished.
                tokio::time::sleep(Duration::from_secs(3)).await;
                let mut ids = core.device_ids().await;
                ids.sort();
                assert_eq!(ids, ["A", "B", "C"], "D wasn't removed");
                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// `max_attempts` counts reconnect attempts: 0 allows none, so the
        /// monitor gives up on the first loss it finds without trying, and 1
        /// allows one.
        #[tokio::test(start_paused = true)]
        async fn max_attempts_counts_from_zero() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                for (id, max) in [("A", 0), ("B", 1)] {
                    core.add_device_with_options(id, ReconnectOptions::default().max_attempts(max))
                        .await
                        .unwrap();
                    core.connect(id).await.unwrap();
                    radio.lose_link(id);
                }
                radio.script_connects("B", [false]);
                let mut events = core.events.subscribe();

                // A minute of ticks, 5 s apart.
                for _ in 0..12 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    core.health_tick().await;
                }

                assert_eq!(radio.connect_count("A"), 1, "A was reconnected");
                assert_eq!(
                    radio.connect_count("B"),
                    2,
                    "the first connect and one repair"
                );
                let errors: Vec<String> = drain(&mut events)
                    .into_iter()
                    .filter(|line| line.starts_with("Error"))
                    .collect();
                assert_eq!(
                    errors,
                    [
                        "Error A: auto-reconnect gave up after 0 attempts",
                        "Error B: auto-reconnect gave up after 1 attempt"
                    ]
                );
                let a = core.snapshot("A").await.unwrap();
                assert_eq!((a.failures, a.gave_up), (0, true));
                assert!(core.snapshot("B").await.unwrap().gave_up);

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }

        /// `max_attempts` limits reconnects, not the monitor's first connect
        /// of a device that has never been connected: with 0, an added device
        /// is still connected, or tried once, and the monitor gives up on it
        /// without trying only once it has been connected and is lost.
        #[tokio::test(start_paused = true)]
        async fn max_attempts_of_zero_still_connects_an_added_device_once() {
            within(LIMIT, async {
                let radio = FakeRadio::new();
                let core = core(&radio, ManagerConfig::default());
                for id in ["A", "B"] {
                    core.add_device_with_options(id, ReconnectOptions::default().max_attempts(0))
                        .await
                        .unwrap();
                }
                radio.script_connects("B", [false]);
                let mut events = core.events.subscribe();

                // The first tick connects A and tries B once.
                assert_eq!(
                    core.health_tick().await,
                    TickOutcome {
                        healthy: 0,
                        repaired: 1,
                        failed: 1
                    }
                );
                assert!(radio.link_up("A"));
                assert_eq!(
                    drain(&mut events),
                    [
                        "ReconnectStarted A 1",
                        "Connected A",
                        "ReconnectSucceeded A 1",
                        "ReconnectStarted B 1",
                        "Error B: auto-reconnect gave up after 1 attempt"
                    ]
                );

                // A minute of ticks, 5 s apart, after A is lost: the monitor
                // gives up on A without trying, and never tries B again.
                radio.lose_link("A");
                for _ in 0..12 {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    core.health_tick().await;
                }
                assert_eq!(radio.connect_count("A"), 1, "A was reconnected");
                assert_eq!(radio.connect_count("B"), 1, "B was tried again");
                assert_eq!(
                    drain(&mut events),
                    [
                        "Disconnected A Unknown",
                        "Error A: auto-reconnect gave up after 0 attempts"
                    ]
                );
                for id in ["A", "B"] {
                    let snapshot = core.snapshot(id).await.unwrap();
                    assert!(snapshot.gave_up && !snapshot.has_link, "{id}: {snapshot:?}");
                }

                assert_no_orphans(&core, &radio).await;
                radio.assert_no_drop_teardown();
            })
            .await;
        }
    }
}
