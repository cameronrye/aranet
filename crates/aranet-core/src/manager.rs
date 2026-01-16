//! Multi-device management.
//!
//! This module provides a manager for handling multiple Aranet devices
//! simultaneously, with connection pooling and concurrent operations.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use tokio::sync::RwLock;
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

use crate::device::Device;
use crate::error::{Error, Result};
use crate::events::{DeviceEvent, DeviceId, DisconnectReason, EventDispatcher};
use crate::reconnect::ReconnectOptions;
use crate::scan::{DiscoveredDevice, ScanOptions, scan_with_options};

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
    /// Whether auto-reconnect is enabled.
    pub auto_reconnect: bool,
    /// Last known reading.
    pub last_reading: Option<CurrentReading>,
    /// Device info.
    pub info: Option<DeviceInfo>,
    /// Reconnection options (if auto-reconnect is enabled).
    pub reconnect_options: ReconnectOptions,
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
        }
    }

    /// Create a managed device with custom reconnect options.
    pub fn with_reconnect_options(id: &str, options: ReconnectOptions) -> Self {
        Self {
            reconnect_options: options,
            ..Self::new(id)
        }
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
    /// Health check interval for auto-reconnect.
    pub health_check_interval: Duration,
}

impl Default for ManagerConfig {
    fn default() -> Self {
        Self {
            scan_options: ScanOptions::default(),
            default_reconnect_options: ReconnectOptions::default(),
            event_capacity: 100,
            health_check_interval: Duration::from_secs(30),
        }
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
    /// The lock is held during the device entry update to prevent race conditions,
    /// but released during the actual BLE connection to avoid blocking other operations.
    pub async fn connect(&self, identifier: &str) -> Result<()> {
        // Check if we need to connect (atomically check and mark as pending)
        let reconnect_options = {
            let mut devices = self.devices.write().await;

            // Get or create the managed device entry
            let managed = devices
                .entry(identifier.to_string())
                .or_insert_with(|| {
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

            // Clone the reconnect options for use after releasing lock
            managed.reconnect_options.clone()
        };
        // Lock is released here - other tasks can now access the device map

        // Perform BLE connection (this may take time)
        // Use the cloned reconnect_options if needed in the future
        let _ = reconnect_options;
        let device = Arc::new(Device::connect(identifier).await?);
        let info = device.read_device_info().await.ok();
        let device_type = device.device_type();
        let name = device.name().map(|s| s.to_string());

        // Update the managed device atomically
        {
            let mut devices = self.devices.write().await;
            if let Some(managed) = devices.get_mut(identifier) {
                // Check if another task connected while we were connecting
                if managed.device.is_some() {
                    // Another task beat us to it - disconnect our connection
                    debug!("Another task connected {} while we were connecting, discarding our connection", identifier);
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

    /// Get the number of connected devices (verified via BLE).
    ///
    /// This method queries each device to verify its connection status.
    /// The lock is released before making BLE calls to avoid contention.
    pub async fn connected_count_verified(&self) -> usize {
        // Collect device handles while holding the lock briefly
        let device_arcs: Vec<Arc<Device>> = {
            let devices = self.devices.read().await;
            devices
                .values()
                .filter_map(|m| m.device_arc())
                .collect()
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
            reading: reading.clone(),
        });

        // Update cached reading
        {
            let mut devices = self.devices.write().await;
            if let Some(managed) = devices.get_mut(identifier) {
                managed.last_reading = Some(reading.clone());
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
                .filter_map(|(id, managed)| {
                    managed.device_arc().map(|d| (id.clone(), d))
                })
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
                    reading: reading.clone(),
                });
            }
        }

        // Update cached readings
        {
            let mut devices = self.devices.write().await;
            for (id, result) in &read_results {
                if let Ok(reading) = result
                    && let Some(managed) = devices.get_mut(id) {
                        managed.last_reading = Some(reading.clone());
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
                .filter_map(|(id, managed)| {
                    managed.device.take().map(|d| (id.clone(), d))
                })
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
            Ok(devices) => {
                Some(devices.get(identifier).map(|m| m.has_device()).unwrap_or(false))
            }
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
        devices.get(identifier).and_then(|m| m.last_reading.clone())
    }

    /// Start a background health check task that monitors connection status.
    ///
    /// This spawns a task that periodically checks device connections and
    /// attempts to reconnect devices that have auto_reconnect enabled.
    ///
    /// The task will run until the provided cancellation token is cancelled.
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
        let interval_duration = manager.config.health_check_interval;

        tokio::spawn(async move {
            let mut check_interval = interval(interval_duration);

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!("Health monitor cancelled, shutting down");
                        break;
                    }
                    _ = check_interval.tick() => {
                        // Get devices that need checking
                        let devices_to_check: Vec<(String, Option<Arc<Device>>, bool, ReconnectOptions)> = {
                            let devices = manager.devices.read().await;
                            devices
                                .iter()
                                .map(|(id, m)| {
                                    (
                                        id.clone(),
                                        m.device_arc(),
                                        m.auto_reconnect,
                                        m.reconnect_options.clone(),
                                    )
                                })
                                .collect()
                        };

                        for (id, device_opt, auto_reconnect, _options) in devices_to_check {
                            let should_reconnect = match device_opt {
                                Some(device) => !device.is_connected().await,
                                None => true,
                            };

                            if should_reconnect && auto_reconnect {
                                debug!("Health monitor: attempting reconnect for {}", id);
                                if let Err(e) = manager.connect(&id).await {
                                    warn!("Health monitor: reconnect failed for {}: {}", id, e);
                                }
                            }
                        }
                    }
                }
            }
        })
    }
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
