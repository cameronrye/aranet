//! Multi-device management.
//!
//! This module provides a manager for handling multiple Aranet devices
//! simultaneously, with connection pooling and concurrent operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::future::join_all;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

use crate::device::Device;
use crate::error::{Error, Result};
use crate::events::{DeviceEvent, DeviceId, DisconnectReason, EventDispatcher};
use crate::passive::{PassiveMonitor, PassiveMonitorOptions, PassiveReading};
use crate::reconnect::ReconnectOptions;
use crate::scan::{DiscoveredDevice, ScanOptions, scan_with_options};

/// Device priority levels for connection management.
///
/// When the connection limit is reached, lower priority devices
/// may be disconnected to make room for higher priority devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum DevicePriority {
    /// Low priority - may be disconnected when at capacity.
    Low,
    /// Normal priority (default).
    #[default]
    Normal,
    /// High priority - maintain connection, disconnect lower priorities if needed.
    High,
    /// Critical priority - never disconnect automatically.
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
    /// Whether a connection attempt is currently in progress.
    /// This prevents race conditions where multiple tasks try to connect simultaneously.
    connecting: AtomicBool,
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
            connecting: AtomicBool::new(false),
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
    /// When enabled, health checks will use `device.validate_connection()`
    /// which performs an actual BLE read to verify the connection is alive.
    /// This catches "zombie connections" but uses more power.
    pub use_connection_validation: bool,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        // Use platform-specific defaults if available
        let platform_config = crate::platform::PlatformConfig::for_current_platform();

        Self {
            scan_options: ScanOptions::default(),
            default_reconnect_options: ReconnectOptions::default(),
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

/// Manager for multiple Aranet devices.
pub struct DeviceManager {
    /// Map of device ID to managed device.
    devices: RwLock<HashMap<String, ManagedDevice>>,
    /// Event dispatcher.
    events: EventDispatcher,
    /// Manager configuration.
    config: ManagerConfig,
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
            devices: RwLock::new(HashMap::new()),
            events: EventDispatcher::new(config.event_capacity),
            config,
        }
    }

    /// Get the event dispatcher for subscribing to events.
    pub fn events(&self) -> &EventDispatcher {
        &self.events
    }

    /// Get the manager configuration.
    pub fn config(&self) -> &ManagerConfig {
        &self.config
    }

    /// Scan for available devices.
    pub async fn scan(&self) -> Result<Vec<DiscoveredDevice>> {
        scan_with_options(self.config.scan_options.clone()).await
    }

    /// Scan with custom options.
    pub async fn scan_with_options(&self, options: ScanOptions) -> Result<Vec<DiscoveredDevice>> {
        let devices = scan_with_options(options).await?;

        // Emit discovery events
        for device in &devices {
            self.events.send(DeviceEvent::Discovered {
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
    pub async fn add_device(&self, identifier: &str) -> Result<()> {
        self.add_device_with_options(identifier, self.config.default_reconnect_options.clone())
            .await
    }

    /// Add a device with custom reconnect options.
    pub async fn add_device_with_options(
        &self,
        identifier: &str,
        reconnect_options: ReconnectOptions,
    ) -> Result<()> {
        let mut devices = self.devices.write().await;

        if devices.contains_key(identifier) {
            return Ok(()); // Already exists
        }

        let managed = ManagedDevice::with_reconnect_options(identifier, reconnect_options);
        devices.insert(identifier.to_string(), managed);

        info!("Added device to manager: {}", identifier);
        Ok(())
    }

    /// Connect to a device.
    ///
    /// This method performs an atomic connect-or-skip operation:
    /// - If the device doesn't exist, it's added and connected
    /// - If the device exists but is not connected, it's connected
    /// - If the device is already connected, this is a no-op
    ///
    /// # Connection Limits
    ///
    /// If `max_concurrent_connections` is set in the config and would be exceeded,
    /// this method returns an error. Use `connected_count()` to check the current
    /// number of connections before calling this method.
    ///
    /// The lock is held during the device entry update to prevent race conditions,
    /// but released during the actual BLE connection to avoid blocking other operations.
    pub async fn connect(&self, identifier: &str) -> Result<()> {
        // Check if we need to connect (atomically check and mark as pending)
        let reconnect_options = {
            let mut devices = self.devices.write().await;

            // Check connection limit before doing anything else
            if self.config.max_concurrent_connections > 0 {
                // Check if already connected (doesn't count toward limit)
                let already_connected = devices
                    .get(identifier)
                    .map(|m| m.has_device())
                    .unwrap_or(false);

                if !already_connected {
                    let current_connections = devices.values().filter(|m| m.has_device()).count();
                    if current_connections >= self.config.max_concurrent_connections {
                        warn!(
                            "Connection limit reached ({}/{}), cannot connect to {}",
                            current_connections, self.config.max_concurrent_connections, identifier
                        );
                        return Err(Error::connection_failed(
                            Some(identifier.to_string()),
                            crate::error::ConnectionFailureReason::Other(format!(
                                "Connection limit reached ({}/{})",
                                current_connections, self.config.max_concurrent_connections
                            )),
                        ));
                    }
                }
            }

            // Get or create the managed device entry
            let managed = devices.entry(identifier.to_string()).or_insert_with(|| {
                info!("Adding device to manager: {}", identifier);
                ManagedDevice::with_reconnect_options(
                    identifier,
                    self.config.default_reconnect_options.clone(),
                )
            });

            // If already connected, nothing to do
            if managed.device.is_some() {
                debug!("Device {} already has a connection handle", identifier);
                return Ok(());
            }

            // Try to atomically set the connecting flag to prevent race conditions
            // If another task is already connecting, return early
            if managed
                .connecting
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                debug!(
                    "Another task is already connecting to device {}",
                    identifier
                );
                return Ok(());
            }

            // Clone the reconnect options for use after releasing lock
            managed.reconnect_options.clone()
        };
        // Lock is released here - other tasks can now access the device map

        // Perform BLE connection (this may take time)
        // Use the cloned reconnect_options if needed in the future
        let _ = reconnect_options;
        let connect_result = Device::connect(identifier).await;

        // Handle connection result
        let device = match connect_result {
            Ok(d) => Arc::new(d),
            Err(e) => {
                // Clear the connecting flag on failure
                let devices = self.devices.read().await;
                if let Some(managed) = devices.get(identifier) {
                    managed.connecting.store(false, Ordering::SeqCst);
                }
                return Err(e);
            }
        };

        let info = device.read_device_info().await.ok();
        let device_type = device.device_type();
        let name = device.name().map(|s| s.to_string());

        // Update the managed device atomically
        {
            let mut devices = self.devices.write().await;
            if let Some(managed) = devices.get_mut(identifier) {
                // Clear the connecting flag
                managed.connecting.store(false, Ordering::SeqCst);

                // Check if another task connected while we were connecting
                // (shouldn't happen with the atomic flag, but be defensive)
                if managed.device.is_some() {
                    // Another task beat us to it - disconnect our connection
                    debug!(
                        "Another task connected {} while we were connecting, discarding our connection",
                        identifier
                    );
                    drop(devices); // Release lock before async disconnect
                    let _ = device.disconnect().await;
                    return Ok(());
                }

                managed.device = Some(device);
                managed.info = info.clone();
                managed.device_type = device_type;
                managed.name = name.clone();
            } else {
                // Device was removed while we were connecting - still connect but add it back
                let mut managed = ManagedDevice::new(identifier);
                managed.device = Some(device);
                managed.info = info.clone();
                managed.device_type = device_type;
                managed.name = name.clone();
                devices.insert(identifier.to_string(), managed);
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

        info!("Connected to device: {}", identifier);
        Ok(())
    }

    /// Disconnect from a device.
    pub async fn disconnect(&self, identifier: &str) -> Result<()> {
        let device_arc = {
            let mut devices = self.devices.write().await;
            if let Some(managed) = devices.get_mut(identifier) {
                managed.device.take()
            } else {
                None
            }
        };

        // Disconnect outside the lock
        if let Some(device) = device_arc {
            device.disconnect().await?;
            self.events.send(DeviceEvent::Disconnected {
                device: DeviceId::new(identifier),
                reason: DisconnectReason::UserRequested,
            });
        }

        Ok(())
    }

    /// Remove a device from the manager.
    pub async fn remove_device(&self, identifier: &str) -> Result<()> {
        self.disconnect(identifier).await?;
        self.devices.write().await.remove(identifier);
        info!("Removed device from manager: {}", identifier);
        Ok(())
    }

    /// Get a list of all managed device IDs.
    pub async fn device_ids(&self) -> Vec<String> {
        self.devices.read().await.keys().cloned().collect()
    }

    /// Get the number of managed devices.
    pub async fn device_count(&self) -> usize {
        self.devices.read().await.len()
    }

    /// Get the number of connected devices (fast, doesn't query BLE).
    ///
    /// This returns the number of devices that have an active device handle,
    /// without querying the BLE stack. Use `connected_count_verified` for
    /// an accurate count that queries each device.
    pub async fn connected_count(&self) -> usize {
        let devices = self.devices.read().await;
        devices.values().filter(|m| m.has_device()).count()
    }

    /// Check if a new connection can be made without exceeding the limit.
    ///
    /// Returns `true` if another connection can be made, `false` if at limit.
    /// Always returns `true` if `max_concurrent_connections` is 0 (unlimited).
    pub async fn can_connect(&self) -> bool {
        if self.config.max_concurrent_connections == 0 {
            return true;
        }
        self.connected_count().await < self.config.max_concurrent_connections
    }

    /// Get the connection limit status.
    ///
    /// Returns (current_connections, max_connections). If max is 0, there is no limit.
    pub async fn connection_status(&self) -> (usize, usize) {
        (
            self.connected_count().await,
            self.config.max_concurrent_connections,
        )
    }

    /// Get the number of available connection slots.
    ///
    /// Returns `None` if there is no connection limit (unlimited).
    pub async fn available_connections(&self) -> Option<usize> {
        if self.config.max_concurrent_connections == 0 {
            return None;
        }
        let current = self.connected_count().await;
        Some(
            self.config
                .max_concurrent_connections
                .saturating_sub(current),
        )
    }

    /// Get the number of connected devices (verified via BLE).
    ///
    /// This method queries each device to verify its connection status.
    /// The lock is released before making BLE calls to avoid contention.
    pub async fn connected_count_verified(&self) -> usize {
        // Collect device handles while holding the lock briefly
        let device_arcs: Vec<Arc<Device>> = {
            let devices = self.devices.read().await;
            devices.values().filter_map(|m| m.device_arc()).collect()
        };
        // Lock is released here

        // Check connection status in parallel
        let futures = device_arcs.iter().map(|d| d.is_connected());
        let results = join_all(futures).await;

        results.into_iter().filter(|&connected| connected).count()
    }

    /// Read current values from a specific device.
    pub async fn read_current(&self, identifier: &str) -> Result<CurrentReading> {
        // Get device Arc while holding the lock briefly
        let device = {
            let devices = self.devices.read().await;
            let managed = devices
                .get(identifier)
                .ok_or_else(|| Error::device_not_found(identifier))?;
            managed.device_arc().ok_or(Error::NotConnected)?
        };
        // Lock is released here

        let reading = device.read_current().await?;

        // Emit reading event
        self.events.send(DeviceEvent::Reading {
            device: DeviceId::new(identifier),
            reading,
        });

        // Update cached reading
        {
            let mut devices = self.devices.write().await;
            if let Some(managed) = devices.get_mut(identifier) {
                managed.last_reading = Some(reading);
            }
        }

        Ok(reading)
    }

    /// Read current values from all connected devices (in parallel).
    ///
    /// This method releases the lock before performing async BLE operations,
    /// allowing other tasks to add/remove devices while reads are in progress.
    /// All reads are performed in parallel for maximum performance.
    pub async fn read_all(&self) -> HashMap<String, Result<CurrentReading>> {
        // Collect device handles while holding the lock briefly
        let devices_to_read: Vec<(String, Arc<Device>)> = {
            let devices = self.devices.read().await;
            devices
                .iter()
                .filter_map(|(id, managed)| managed.device_arc().map(|d| (id.clone(), d)))
                .collect()
        };
        // Lock is released here

        // Perform all reads in parallel
        let read_futures = devices_to_read.iter().map(|(id, device)| {
            let id = id.clone();
            let device = Arc::clone(device);
            async move {
                let result = device.read_current().await;
                (id, result)
            }
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
                    && let Some(managed) = devices.get_mut(id)
                {
                    managed.last_reading = Some(*reading);
                }
            }
        }

        read_results.into_iter().collect()
    }

    /// Connect to all known devices (in parallel).
    ///
    /// Returns a map of device IDs to connection results.
    pub async fn connect_all(&self) -> HashMap<String, Result<()>> {
        let ids: Vec<_> = self.devices.read().await.keys().cloned().collect();

        // Note: We can't fully parallelize connect because it modifies state,
        // but we can at least attempt connections concurrently
        let connect_futures = ids.iter().map(|id| {
            let id = id.clone();
            async move {
                let result = self.connect(&id).await;
                (id, result)
            }
        });

        join_all(connect_futures).await.into_iter().collect()
    }

    /// Disconnect from all devices (in parallel).
    ///
    /// Returns a map of device IDs to disconnection results.
    pub async fn disconnect_all(&self) -> HashMap<String, Result<()>> {
        // Collect all device arcs first
        let devices_to_disconnect: Vec<(String, Arc<Device>)> = {
            let mut devices = self.devices.write().await;
            devices
                .iter_mut()
                .filter_map(|(id, managed)| managed.device.take().map(|d| (id.clone(), d)))
                .collect()
        };

        // Disconnect all in parallel
        let disconnect_futures = devices_to_disconnect.iter().map(|(id, device)| {
            let id = id.clone();
            let device = Arc::clone(device);
            async move {
                let result = device.disconnect().await;
                (id, result)
            }
        });

        let results: Vec<(String, Result<()>)> = join_all(disconnect_futures).await;

        // Emit disconnection events
        for (id, result) in &results {
            if result.is_ok() {
                self.events.send(DeviceEvent::Disconnected {
                    device: DeviceId::new(id),
                    reason: DisconnectReason::UserRequested,
                });
            }
        }

        results.into_iter().collect()
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
        // Try to acquire the lock without blocking
        match self.devices.try_read() {
            Ok(devices) => Some(
                devices
                    .get(identifier)
                    .map(|m| m.has_device())
                    .unwrap_or(false),
            ),
            Err(_) => None, // Lock was held, couldn't check
        }
    }

    /// Check if a specific device is connected (verified via BLE).
    ///
    /// The lock is released before making the BLE call.
    pub async fn is_connected(&self, identifier: &str) -> bool {
        let device = {
            let devices = self.devices.read().await;
            devices.get(identifier).and_then(|m| m.device_arc())
        };

        if let Some(device) = device {
            device.is_connected().await
        } else {
            false
        }
    }

    /// Get device info for a specific device.
    pub async fn get_device_info(&self, identifier: &str) -> Option<DeviceInfo> {
        let devices = self.devices.read().await;
        devices.get(identifier).and_then(|m| m.info.clone())
    }

    /// Get the last cached reading for a device.
    pub async fn get_last_reading(&self, identifier: &str) -> Option<CurrentReading> {
        let devices = self.devices.read().await;
        devices.get(identifier).and_then(|m| m.last_reading)
    }

    /// Start a background health check task that monitors connection status.
    ///
    /// This spawns a task that periodically checks device connections and
    /// attempts to reconnect devices that have auto_reconnect enabled.
    ///
    /// The task will run until the provided cancellation token is cancelled.
    ///
    /// # Adaptive Intervals
    ///
    /// If `use_adaptive_interval` is enabled in the config, the health check
    /// interval will automatically adjust based on connection stability:
    /// - When connections are stable, checks become less frequent (up to `max_health_check_interval`)
    /// - When connections are unstable, checks become more frequent (down to `min_health_check_interval`)
    ///
    /// # Connection Validation
    ///
    /// If `use_connection_validation` is enabled, health checks will perform
    /// an actual BLE read (`device.validate_connection()`) to catch "zombie connections"
    /// where the BLE stack thinks it's connected but the device is out of range.
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
        let manager = Arc::clone(self);

        tokio::spawn(async move {
            // Initialize adaptive interval if enabled
            let mut adaptive = if manager.config.use_adaptive_interval {
                Some(AdaptiveInterval::new(
                    manager.config.health_check_interval,
                    manager.config.min_health_check_interval,
                    manager.config.max_health_check_interval,
                ))
            } else {
                None
            };

            loop {
                // Get current interval
                let current_interval = adaptive
                    .as_ref()
                    .map(|a| a.current())
                    .unwrap_or(manager.config.health_check_interval);

                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!("Health monitor cancelled, shutting down");
                        break;
                    }
                    _ = tokio::time::sleep(current_interval) => {
                        let mut any_failures = false;
                        let mut any_successes = false;

                        // Get devices that need checking
                        let devices_to_check: Vec<(String, Option<Arc<Device>>, bool, DevicePriority)> = {
                            let devices = manager.devices.read().await;
                            devices
                                .iter()
                                .map(|(id, m)| {
                                    (
                                        id.clone(),
                                        m.device_arc(),
                                        m.auto_reconnect,
                                        m.priority,
                                    )
                                })
                                .collect()
                        };

                        // Sort by priority (higher priority checked first)
                        let mut sorted_devices = devices_to_check;
                        sorted_devices.sort_by(|a, b| b.3.cmp(&a.3));

                        for (id, device_opt, auto_reconnect, _priority) in sorted_devices {
                            let should_reconnect = match device_opt {
                                Some(device) => {
                                    // Use connection validation if enabled
                                    if manager.config.use_connection_validation {
                                        !device.is_connection_alive().await
                                    } else {
                                        !device.is_connected().await
                                    }
                                }
                                None => true,
                            };

                            if should_reconnect && auto_reconnect {
                                debug!("Health monitor: attempting reconnect for {}", id);
                                any_failures = true;

                                match manager.connect(&id).await {
                                    Ok(()) => {
                                        any_successes = true;
                                        // Update success in managed device
                                        if let Some(m) = manager.devices.write().await.get_mut(&id) {
                                            m.record_success();
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Health monitor: reconnect failed for {}: {}", id, e);
                                        // Update failure in managed device
                                        if let Some(m) = manager.devices.write().await.get_mut(&id) {
                                            m.record_failure();
                                        }
                                    }
                                }
                            } else if !should_reconnect {
                                any_successes = true;
                            }
                        }

                        // Update adaptive interval
                        if let Some(ref mut adaptive) = adaptive {
                            if any_failures && !any_successes {
                                adaptive.on_failure();
                            } else if any_successes && !any_failures {
                                adaptive.on_success();
                            }
                            // Mixed results: don't change interval
                        }
                    }
                }
            }
        })
    }

    /// Add a device with priority.
    pub async fn add_device_with_priority(
        &self,
        identifier: &str,
        priority: DevicePriority,
    ) -> Result<()> {
        let mut devices = self.devices.write().await;

        if devices.contains_key(identifier) {
            // Update priority if device already exists
            if let Some(m) = devices.get_mut(identifier) {
                m.priority = priority;
            }
            return Ok(());
        }

        let mut managed = ManagedDevice::new(identifier);
        managed.priority = priority;
        managed.reconnect_options = self.config.default_reconnect_options.clone();
        devices.insert(identifier.to_string(), managed);

        info!(
            "Added device to manager with priority {:?}: {}",
            priority, identifier
        );
        Ok(())
    }

    /// Get the lowest priority connected device that could be disconnected.
    ///
    /// Returns None if no devices can be disconnected (all are Critical priority or not connected).
    pub async fn lowest_priority_connected(&self) -> Option<String> {
        let devices = self.devices.read().await;
        devices
            .iter()
            .filter(|(_, m)| m.has_device() && m.priority != DevicePriority::Critical)
            .min_by_key(|(_, m)| m.priority)
            .map(|(id, _)| id.clone())
    }

    /// Disconnect the lowest priority device to make room for a new connection.
    ///
    /// Returns Ok(true) if a device was disconnected, Ok(false) if no eligible device found.
    pub async fn evict_lowest_priority(&self) -> Result<bool> {
        if let Some(id) = self.lowest_priority_connected().await {
            info!("Evicting lowest priority device: {}", id);
            self.disconnect(&id).await?;
            Ok(true)
        } else {
            Ok(false)
        }
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
        let manager = Arc::clone(self);
        let options = passive_options.unwrap_or_default();

        tokio::spawn(async move {
            info!("Starting hybrid monitor (passive + active)");

            // Create passive monitor
            let passive_monitor = Arc::new(PassiveMonitor::new(options));
            let mut passive_rx = passive_monitor.subscribe();

            // Start passive monitoring
            let passive_cancel = cancel_token.clone();
            let _passive_handle = passive_monitor.start(passive_cancel);

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!("Hybrid monitor cancelled");
                        break;
                    }
                    result = passive_rx.recv() => {
                        match result {
                            Ok(passive_reading) => {
                                // Convert passive reading to CurrentReading and emit event
                                if let Some(reading) = passive_reading_to_current(&passive_reading) {
                                    // Update last reading in managed device if it exists
                                    if let Some(m) = manager.devices.write().await.get_mut(&passive_reading.device_id) {
                                        m.last_reading = Some(reading);
                                        m.record_success();
                                    }

                                    // Emit reading event
                                    manager.events.send(DeviceEvent::Reading {
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
        let max_age = max_passive_age.unwrap_or(Duration::from_secs(60));

        // Check if we have a recent cached reading
        {
            let devices = self.devices.read().await;
            if let Some(managed) = devices.get(identifier)
                && let Some(reading) = managed.last_reading
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

    /// Check if a device supports passive monitoring (Smart Home enabled).
    ///
    /// This performs a quick scan to check if the device is broadcasting
    /// advertisement data with sensor readings.
    pub async fn supports_passive_monitoring(&self, identifier: &str) -> bool {
        // Create a short-lived passive monitor to check for advertisements
        let options = PassiveMonitorOptions::default()
            .scan_duration(Duration::from_secs(5))
            .filter_devices(vec![identifier.to_string()]);

        let monitor = Arc::new(PassiveMonitor::new(options));
        let mut rx = monitor.subscribe();
        let cancel = CancellationToken::new();

        let _handle = monitor.start(cancel.clone());

        // Wait for a reading or timeout
        let result = tokio::time::timeout(Duration::from_secs(6), rx.recv()).await;
        cancel.cancel();

        matches!(result, Ok(Ok(_)))
    }
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
}
