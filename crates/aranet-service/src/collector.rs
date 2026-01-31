//! Background data collector for polling Aranet devices.
//!
//! This module provides a background collector that polls configured devices at their
//! specified intervals and stores readings in the database.
//!
//! # Concurrency Model
//!
//! The collector uses a "task per device" model:
//!
//! - Each configured device gets its own Tokio task
//! - Tasks run independently with their own polling intervals
//! - Tasks share access to the application state via `Arc<AppState>`
//!
//! ## Lock Acquisition
//!
//! Device polling tasks acquire locks in this order:
//!
//! 1. **`device_stats` write lock** - Brief lock to update polling status
//! 2. **BLE device communication** - No Rust locks, but exclusive Bluetooth access
//! 3. **`store` mutex** - Brief lock to insert the reading
//! 4. **`device_stats` write lock** - Brief lock to update success/failure counts
//!
//! ## Graceful Shutdown
//!
//! The collector uses a `watch` channel for graceful shutdown:
//!
//! - [`Collector::stop()`] sends a stop signal to all tasks
//! - Each task checks for the stop signal between poll cycles
//! - Tasks complete their current operation before stopping
//!
//! ## Error Handling
//!
//! Connection and read errors are tracked per-device and logged with progressive
//! quieting: errors are logged at WARN level for the first 3 failures, then at
//! ERROR level once, then silently retried. This prevents log spam for devices
//! that are temporarily unavailable.
//!
//! # Example
//!
//! ```ignore
//! let collector = Collector::new(Arc::clone(&state));
//! collector.start().await;  // Returns immediately, collection is background
//!
//! // Later...
//! collector.stop();  // Signal all tasks to stop
//! ```

use std::sync::Arc;
use std::time::Duration;

use time::OffsetDateTime;
use tokio::sync::watch;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use aranet_core::Device;
use aranet_store::StoredReading;

use crate::config::DeviceConfig;
use crate::state::{AppState, DeviceCollectionStats, ReadingEvent};

/// Background collector that polls devices on their configured intervals.
pub struct Collector {
    state: Arc<AppState>,
}

impl Collector {
    /// Create a new collector.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Start collecting data from all configured devices.
    ///
    /// This spawns a separate task for each device that polls at the configured interval.
    /// Returns immediately; collection happens in the background.
    pub async fn start(&self) {
        // Reset stop signal if previously stopped
        self.state.collector.reset_stop();

        let config = self.state.config.read().await;
        let devices = config.devices.clone();
        drop(config);

        if devices.is_empty() {
            info!("No devices configured for collection");
            return;
        }

        info!("Starting collector for {} device(s)", devices.len());

        // Initialize device stats
        {
            let mut stats = self.state.collector.device_stats.write().await;
            stats.clear();
            for device in &devices {
                stats.push(DeviceCollectionStats {
                    device_id: device.address.clone(),
                    alias: device.alias.clone(),
                    poll_interval: device.poll_interval,
                    last_poll_at: None,
                    last_error_at: None,
                    last_error: None,
                    success_count: 0,
                    failure_count: 0,
                    polling: false,
                });
            }
        }

        // Mark as running
        self.state.collector.set_running(true);

        for device_config in devices {
            let state = Arc::clone(&self.state);
            let stop_rx = self.state.collector.subscribe_stop();
            tokio::spawn(async move {
                collect_device(state, device_config, stop_rx).await;
            });
        }
    }

    /// Stop the collector.
    pub fn stop(&self) {
        info!("Stopping collector");
        self.state.collector.signal_stop();
    }

    /// Check if the collector is running.
    pub fn is_running(&self) -> bool {
        self.state.collector.is_running()
    }
}

/// Collect readings from a single device.
async fn collect_device(
    state: Arc<AppState>,
    config: DeviceConfig,
    mut stop_rx: watch::Receiver<bool>,
) {
    let device_id = config.address.clone();
    let alias = config.alias.as_deref().unwrap_or(&device_id);
    let poll_interval = Duration::from_secs(config.poll_interval);

    info!(
        "Starting collector for {} (alias: {}, interval: {}s)",
        device_id, alias, config.poll_interval
    );

    let mut interval_timer = interval(poll_interval);
    let mut consecutive_failures = 0u32;

    loop {
        tokio::select! {
            _ = interval_timer.tick() => {
                // Update stats: mark as polling
                update_device_stat(&state, &device_id, |stat| {
                    stat.polling = true;
                }).await;

                match poll_device(&state, &device_id).await {
                    Ok(reading) => {
                        consecutive_failures = 0;
                        debug!("Collected reading from {}: CO2={}", device_id, reading.co2);

                        // Update stats
                        update_device_stat(&state, &device_id, |stat| {
                            stat.last_poll_at = Some(OffsetDateTime::now_utc());
                            stat.success_count += 1;
                            stat.polling = false;
                        }).await;

                        // Broadcast the reading to WebSocket clients
                        let event = ReadingEvent {
                            device_id: device_id.clone(),
                            reading,
                        };
                        let _ = state.readings_tx.send(event);
                    }
                    Err(e) => {
                        consecutive_failures += 1;

                        // Update stats
                        update_device_stat(&state, &device_id, |stat| {
                            stat.last_error_at = Some(OffsetDateTime::now_utc());
                            stat.last_error = Some(e.to_string());
                            stat.failure_count += 1;
                            stat.polling = false;
                        }).await;

                        if consecutive_failures <= 3 {
                            warn!(
                                "Failed to poll {}: {} (attempt {})",
                                device_id, e, consecutive_failures
                            );
                        } else if consecutive_failures == 4 {
                            error!(
                                "Failed to poll {} after {} attempts, will continue trying silently",
                                device_id, consecutive_failures
                            );
                        }
                        // Continue trying - the device may come back online
                    }
                }
            }
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    info!("Collector for {} received stop signal", device_id);
                    break;
                }
            }
        }
    }

    info!("Collector for {} stopped", device_id);
}

/// Update stats for a specific device.
async fn update_device_stat<F>(state: &AppState, device_id: &str, update_fn: F)
where
    F: FnOnce(&mut DeviceCollectionStats),
{
    let mut stats = state.collector.device_stats.write().await;
    if let Some(stat) = stats.iter_mut().find(|s| s.device_id == device_id) {
        update_fn(stat);
    }
}

/// Poll a single device and store the reading.
async fn poll_device(state: &AppState, device_id: &str) -> Result<StoredReading, CollectorError> {
    // Connect to the device
    let device = Device::connect(device_id)
        .await
        .map_err(CollectorError::Connect)?;

    // Read current values
    let reading = device.read_current().await.map_err(CollectorError::Read)?;

    // Disconnect
    let _ = device.disconnect().await;

    // Store the reading
    {
        let store = state.store.lock().await;
        store
            .insert_reading(device_id, &reading)
            .map_err(CollectorError::Store)?;
    }

    // Return the stored reading
    Ok(StoredReading::from_reading(device_id, &reading))
}

/// Collector errors.
#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    #[error("Failed to connect: {0}")]
    Connect(aranet_core::Error),
    #[error("Failed to read: {0}")]
    Read(aranet_core::Error),
    #[error("Failed to store: {0}")]
    Store(aranet_store::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn create_test_state() -> Arc<AppState> {
        let store = aranet_store::Store::open_in_memory().unwrap();
        let config = Config::default();
        AppState::new(store, config)
    }

    #[test]
    fn test_collector_new() {
        let state = create_test_state();
        let collector = Collector::new(Arc::clone(&state));
        assert!(!collector.is_running());
    }

    #[test]
    fn test_collector_is_running_initially_false() {
        let state = create_test_state();
        let collector = Collector::new(state);
        assert!(!collector.is_running());
    }

    #[tokio::test]
    async fn test_collector_start_no_devices() {
        let state = create_test_state();
        let collector = Collector::new(Arc::clone(&state));

        // Start with no devices configured
        collector.start().await;

        // Should not be running since there are no devices
        // (the collector returns early if no devices are configured)
        // But it would still set running=true briefly; let's test the stats are empty
        let stats = state.collector.device_stats.read().await;
        assert!(stats.is_empty());
    }

    #[tokio::test]
    async fn test_collector_start_with_devices_initializes_stats() {
        let state = create_test_state();

        // Add a device to config
        {
            let mut config = state.config.write().await;
            config.devices.push(crate::config::DeviceConfig {
                address: "AA:BB:CC:DD:EE:FF".to_string(),
                alias: Some("Test Device".to_string()),
                poll_interval: 60,
            });
        }

        let collector = Collector::new(Arc::clone(&state));
        collector.start().await;

        // Wait a moment for async initialization
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Check that device stats were initialized
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].device_id, "AA:BB:CC:DD:EE:FF");
        assert_eq!(stats[0].alias, Some("Test Device".to_string()));
        assert_eq!(stats[0].poll_interval, 60);
        // Note: success_count, failure_count, and polling may have changed
        // due to async collector activity, so we only verify initialization happened
    }

    #[test]
    fn test_collector_stop() {
        let state = create_test_state();
        state.collector.set_running(true);

        let collector = Collector::new(Arc::clone(&state));
        assert!(collector.is_running());

        collector.stop();
        assert!(!collector.is_running());
    }

    #[test]
    fn test_collector_error_display_connect() {
        let core_error = aranet_core::Error::NotConnected;
        let error = CollectorError::Connect(core_error);
        let display = format!("{}", error);
        assert!(display.contains("Failed to connect"));
    }

    #[test]
    fn test_collector_error_display_read() {
        let core_error = aranet_core::Error::NotConnected;
        let error = CollectorError::Read(core_error);
        let display = format!("{}", error);
        assert!(display.contains("Failed to read"));
    }

    #[test]
    fn test_collector_error_display_store() {
        let store_error = aranet_store::Error::DeviceNotFound("test".to_string());
        let error = CollectorError::Store(store_error);
        let display = format!("{}", error);
        assert!(display.contains("Failed to store"));
    }

    #[test]
    fn test_collector_error_debug() {
        let core_error = aranet_core::Error::NotConnected;
        let error = CollectorError::Connect(core_error);
        let debug = format!("{:?}", error);
        assert!(debug.contains("Connect"));
    }

    #[tokio::test]
    async fn test_device_collection_stats_initialization() {
        let stats = DeviceCollectionStats {
            device_id: "test-device".to_string(),
            alias: Some("Test Alias".to_string()),
            poll_interval: 120,
            last_poll_at: None,
            last_error_at: None,
            last_error: None,
            success_count: 0,
            failure_count: 0,
            polling: false,
        };

        assert_eq!(stats.device_id, "test-device");
        assert_eq!(stats.alias, Some("Test Alias".to_string()));
        assert_eq!(stats.poll_interval, 120);
        assert!(stats.last_poll_at.is_none());
        assert_eq!(stats.success_count, 0);
        assert_eq!(stats.failure_count, 0);
        assert!(!stats.polling);
    }

    #[tokio::test]
    async fn test_update_device_stat() {
        let state = create_test_state();

        // Initialize stats
        {
            let mut stats = state.collector.device_stats.write().await;
            stats.push(DeviceCollectionStats {
                device_id: "test-device".to_string(),
                alias: None,
                poll_interval: 60,
                last_poll_at: None,
                last_error_at: None,
                last_error: None,
                success_count: 0,
                failure_count: 0,
                polling: false,
            });
        }

        // Update the stat
        update_device_stat(&state, "test-device", |stat| {
            stat.success_count = 5;
            stat.polling = true;
        })
        .await;

        // Verify the update
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats[0].success_count, 5);
        assert!(stats[0].polling);
    }

    #[tokio::test]
    async fn test_update_device_stat_nonexistent_device() {
        let state = create_test_state();

        // Initialize stats with one device
        {
            let mut stats = state.collector.device_stats.write().await;
            stats.push(DeviceCollectionStats {
                device_id: "existing-device".to_string(),
                alias: None,
                poll_interval: 60,
                last_poll_at: None,
                last_error_at: None,
                last_error: None,
                success_count: 0,
                failure_count: 0,
                polling: false,
            });
        }

        // Try to update a nonexistent device - should not panic
        update_device_stat(&state, "nonexistent-device", |stat| {
            stat.success_count = 10;
        })
        .await;

        // Verify existing device wasn't changed
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].success_count, 0);
    }

    #[tokio::test]
    async fn test_collector_multiple_devices() {
        let state = create_test_state();

        // Add multiple devices
        {
            let mut config = state.config.write().await;
            config.devices.push(crate::config::DeviceConfig {
                address: "DEVICE-1".to_string(),
                alias: Some("First".to_string()),
                poll_interval: 30,
            });
            config.devices.push(crate::config::DeviceConfig {
                address: "DEVICE-2".to_string(),
                alias: Some("Second".to_string()),
                poll_interval: 60,
            });
            config.devices.push(crate::config::DeviceConfig {
                address: "DEVICE-3".to_string(),
                alias: None,
                poll_interval: 120,
            });
        }

        let collector = Collector::new(Arc::clone(&state));
        collector.start().await;

        // Wait for initialization
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Check all devices were initialized
        let stats = state.collector.device_stats.read().await;
        assert_eq!(stats.len(), 3);

        // Verify each device
        let device1 = stats.iter().find(|s| s.device_id == "DEVICE-1").unwrap();
        assert_eq!(device1.alias, Some("First".to_string()));
        assert_eq!(device1.poll_interval, 30);

        let device2 = stats.iter().find(|s| s.device_id == "DEVICE-2").unwrap();
        assert_eq!(device2.alias, Some("Second".to_string()));
        assert_eq!(device2.poll_interval, 60);

        let device3 = stats.iter().find(|s| s.device_id == "DEVICE-3").unwrap();
        assert!(device3.alias.is_none());
        assert_eq!(device3.poll_interval, 120);
    }
}
