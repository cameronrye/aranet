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
use std::time::{Duration, Instant};

use time::OffsetDateTime;
use tokio::sync::watch;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use aranet_core::Device;
use aranet_store::StoredReading;

use crate::config::DeviceConfig;
use crate::state::{AppState, CollectorState, DeviceCollectionStats, ReadingEvent};

/// Per-device stagger interval to avoid BLE adapter contention on startup.
const DEVICE_STAGGER_SECS: u64 = 5;

/// Spawn staggered device-polling tasks into the collector's shared `JoinSet`.
async fn spawn_staggered_device_tasks(
    collector: &CollectorState,
    devices: Vec<DeviceConfig>,
    state: &Arc<AppState>,
) {
    for (index, device_config) in devices.into_iter().enumerate() {
        let state = Arc::clone(state);
        let stop_rx = collector.subscribe_stop();
        let stagger = Duration::from_secs(index as u64 * DEVICE_STAGGER_SECS);
        collector
            .spawn_device_task(async move {
                if !stagger.is_zero() {
                    debug!(
                        "Staggering start for {} by {}s",
                        device_config.address,
                        stagger.as_secs()
                    );
                    tokio::time::sleep(stagger).await;
                }
                collect_device(state, device_config, stop_rx).await;
            })
            .await;
    }
}

/// Initialize per-device collection stats from the current configuration.
async fn initialize_device_stats(state: &AppState, devices: &[DeviceConfig]) {
    let mut stats = state.collector.device_stats.write().await;
    stats.clear();
    for device in devices {
        stats.push(DeviceCollectionStats {
            device_id: device.address.clone(),
            alias: device.alias.clone(),
            poll_interval: device.poll_interval,
            last_poll_at: None,
            last_error_at: None,
            last_error: None,
            last_poll_duration_ms: None,
            success_count: 0,
            failure_count: 0,
            polling: false,
        });
    }
}

/// Result of attempting to start the collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorStartResult {
    Started,
    AlreadyRunning,
    NoDevicesConfigured,
}

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
    ///
    /// Also spawns a reload watcher task that will restart the collector when
    /// the device configuration changes via the API.
    pub async fn start(&self) -> CollectorStartResult {
        if !self.state.collector.try_start() {
            return CollectorStartResult::AlreadyRunning;
        }

        // Reset stop signal if previously stopped
        self.state.collector.reset_stop();

        // Spawn the reload watcher before checking for configured devices so a
        // service started with an empty device list can recover when devices are
        // added later via the API.
        let state = Arc::clone(&self.state);
        self.state
            .collector
            .set_reload_watcher(tokio::spawn(async move {
                watch_for_reload(state).await;
            }))
            .await;

        let config = self.state.config.read().await;
        let devices = config.devices.clone();
        drop(config);

        if devices.is_empty() {
            info!("No devices configured for collection");
            self.state.collector.set_running(false);
            return CollectorStartResult::NoDevicesConfigured;
        }

        info!("Starting collector for {} device(s)", devices.len());

        initialize_device_stats(&self.state, &devices).await;

        // Spawn device tasks into the shared JoinSet on CollectorState
        // This allows the reload watcher to also spawn tasks that are properly tracked
        spawn_staggered_device_tasks(&self.state.collector, devices, &self.state).await;

        CollectorStartResult::Started
    }

    /// Stop the collector and wait for all tasks to complete.
    pub async fn stop(&self) {
        info!("Stopping collector");
        self.state.collector.signal_stop();

        // Wait for device tasks in the shared JoinSet (with timeout)
        let stopped_cleanly = self
            .state
            .collector
            .wait_for_device_tasks(Duration::from_secs(10))
            .await;

        if !stopped_cleanly {
            warn!("Device tasks did not stop within timeout, aborted");
        }

        let watcher_stopped = self
            .state
            .collector
            .wait_for_reload_watcher(Duration::from_secs(2))
            .await;
        if !watcher_stopped {
            warn!("Reload watcher did not stop within timeout, aborting");
        }
    }

    /// Check if the collector is running.
    pub fn is_running(&self) -> bool {
        self.state.collector.is_running()
    }

    /// Get the number of active collection tasks.
    pub fn task_count(&self) -> usize {
        let device_task_count = self
            .state
            .collector
            .device_tasks
            .try_lock()
            .map(|tasks| tasks.len())
            .unwrap_or(0);
        let watcher_count = self
            .state
            .collector
            .reload_watcher
            .try_lock()
            .map(|watcher| usize::from(watcher.is_some()))
            .unwrap_or(0);

        device_task_count + watcher_count
    }
}

/// Watch for configuration reload signals and restart collection tasks.
async fn watch_for_reload(state: Arc<AppState>) {
    let mut reload_rx = state.collector.subscribe_reload();
    let mut stop_rx = state.collector.subscribe_stop();

    loop {
        tokio::select! {
            result = reload_rx.changed() => {
                if result.is_err() {
                    // Sender dropped, exit
                    break;
                }

                info!("Configuration reload requested, restarting device tasks");

                // Signal current tasks to stop and wait for them to finish
                state.collector.signal_stop();
                state
                    .collector
                    .wait_for_device_tasks(Duration::from_secs(5))
                    .await;

                // Reset stop signal now that tasks have drained
                state.collector.reset_stop();

                // Read new config
                let config = state.config.read().await;
                let devices = config.devices.clone();
                drop(config);

                initialize_device_stats(&state, &devices).await;

                if devices.is_empty() {
                    info!("No devices configured after reload");
                    state.collector.set_running(false);
                    continue;
                }

                info!("Restarting collector for {} device(s)", devices.len());
                state.collector.set_running(true);

                // Spawn new device tasks into the shared JoinSet
                spawn_staggered_device_tasks(&state.collector, devices, &state).await;
            }
            _ = stop_rx.changed() => {
                if *stop_rx.borrow() {
                    info!("Reload watcher received stop signal");
                    break;
                }
            }
        }
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

                let poll_start = Instant::now();
                match poll_device(&state, &device_id).await {
                    Ok(reading) => {
                        let poll_duration = poll_start.elapsed();
                        consecutive_failures = 0;
                        debug!(
                            "Collected reading from {}: CO2={} (took {:.1}s)",
                            device_id, reading.co2, poll_duration.as_secs_f64()
                        );

                        // Update stats
                        update_device_stat(&state, &device_id, |stat| {
                            stat.last_poll_at = Some(OffsetDateTime::now_utc());
                            stat.last_error_at = None;
                            stat.last_error = None;
                            stat.last_poll_duration_ms = Some(poll_duration.as_millis() as u64);
                            stat.success_count += 1;
                            stat.polling = false;
                        }).await;

                        // Broadcast the reading to WebSocket clients
                        let event = ReadingEvent {
                            device_id: device_id.clone(),
                            reading,
                        };
                        // Check thresholds and send desktop notification
                        #[cfg(feature = "notifications")]
                        {
                            let config = state.config.read().await;
                            let notif_config = &config.notifications;
                            if notif_config.enabled {
                                check_and_notify(&state, &device_id, alias, &event.reading, notif_config).await;
                            }
                        }
                        if state.readings_tx.send(event).is_err() {
                            debug!("No active WebSocket subscribers for reading broadcast");
                        }
                    }
                    Err(e) => {
                        let poll_duration = poll_start.elapsed();
                        consecutive_failures += 1;

                        // Update stats
                        update_device_stat(&state, &device_id, |stat| {
                            stat.last_error_at = Some(OffsetDateTime::now_utc());
                            stat.last_error = Some(e.to_string());
                            stat.last_poll_duration_ms = Some(poll_duration.as_millis() as u64);
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
                                "Failed to poll {} after {} attempts, reducing log frequency",
                                device_id, consecutive_failures
                            );
                        } else if consecutive_failures.is_multiple_of(100) {
                            error!(
                                "Failed to poll {} ({} consecutive failures): {}",
                                device_id, consecutive_failures, e
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
///
/// Acquires the BLE semaphore to ensure only one device uses the Bluetooth
/// adapter at a time. This prevents BLE contention that causes connection
/// failures and stale data when multiple devices are configured.
async fn poll_device(state: &AppState, device_id: &str) -> Result<StoredReading, CollectorError> {
    // Serialize BLE adapter access — only one device at a time
    let permit = state
        .ble_semaphore
        .acquire()
        .await
        .map_err(|_| CollectorError::BleBusy)?;

    // Connect with moderate timeouts — fail fast and retry rather than blocking
    let config = aranet_core::device::ConnectionConfig::default();
    let device = Device::connect_with_config(device_id, config)
        .await
        .map_err(CollectorError::Connect)?;

    // Read current values
    let reading_result = device.read_current().await;

    // Always disconnect after the read attempt to avoid relying on best-effort Drop cleanup.
    if let Err(e) = device.disconnect().await {
        debug!("Failed to disconnect {} after poll: {}", device_id, e);
    }

    // Drop the BLE permit so other devices can poll
    drop(permit);
    let reading = reading_result.map_err(CollectorError::Read)?;

    // Store the reading
    let row_id = state
        .with_store_write(|store| store.insert_reading(device_id, &reading))
        .await
        .map_err(CollectorError::Store)?;

    // Return the stored reading
    Ok(StoredReading::from_reading_with_id(
        device_id, &reading, row_id,
    ))
}

/// Collector errors.
#[derive(Debug, thiserror::Error)]
pub enum CollectorError {
    #[error("BLE adapter busy (semaphore closed)")]
    BleBusy,
    #[error("Failed to connect: {0}")]
    Connect(aranet_core::Error),
    #[error("Failed to read: {0}")]
    Read(aranet_core::Error),
    #[error("Failed to store: {0}")]
    Store(aranet_store::Error),
}

#[cfg(feature = "notifications")]
mod notifications {
    use std::collections::HashMap;
    use std::sync::LazyLock;
    use std::time::Instant;
    use tokio::sync::Mutex;

    use crate::config::NotificationConfig;

    static LAST_NOTIFICATION: LazyLock<Mutex<HashMap<String, Instant>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    pub async fn check_and_notify(
        _state: &super::AppState,
        device_id: &str,
        alias: &str,
        reading: &aranet_store::StoredReading,
        config: &NotificationConfig,
    ) {
        let cooldown = std::time::Duration::from_secs(config.cooldown_secs);

        // Check cooldown
        {
            let last = LAST_NOTIFICATION.lock().await;
            if let Some(last_time) = last.get(device_id)
                && last_time.elapsed() < cooldown
            {
                return;
            }
        }

        let mut should_notify = false;
        let mut body = String::new();

        if reading.co2 > 0 && reading.co2 >= config.co2_threshold {
            should_notify = true;
            body.push_str(&format!(
                "CO\u{2082}: {} ppm (threshold: {})\n",
                reading.co2, config.co2_threshold
            ));
        }

        if let Some(radon) = reading.radon
            && radon >= config.radon_threshold
        {
            should_notify = true;
            body.push_str(&format!(
                "Radon: {} Bq/m\u{00b3} (threshold: {})\n",
                radon, config.radon_threshold
            ));
        }

        if should_notify {
            let title = format!("Aranet Alert: {}", alias);
            if let Err(e) = notify_rust::Notification::new()
                .summary(&title)
                .body(body.trim())
                .icon("dialog-warning")
                .timeout(notify_rust::Timeout::Milliseconds(10000))
                .show()
            {
                tracing::warn!("Failed to send desktop notification: {}", e);
            } else {
                let mut last = LAST_NOTIFICATION.lock().await;
                last.insert(device_id.to_string(), Instant::now());
            }
        }
    }
}

#[cfg(feature = "notifications")]
use notifications::check_and_notify;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_config_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "aranet-service-collector-test-{}-{}.toml",
            std::process::id(),
            nanos
        ))
    }

    fn create_test_state() -> Arc<AppState> {
        let store = aranet_store::Store::open_in_memory().unwrap();
        let config = Config::default();
        AppState::with_config_path(store, config, test_config_path())
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
        let result = collector.start().await;

        assert_eq!(result, CollectorStartResult::NoDevicesConfigured);
        assert!(!collector.is_running());
        let stats = state.collector.device_stats.read().await;
        assert!(stats.is_empty());

        let watcher = state.collector.reload_watcher.lock().await;
        assert!(
            watcher.is_some(),
            "reload watcher should stay alive for future config changes"
        );
        drop(watcher);

        collector.stop().await;
    }

    #[tokio::test]
    async fn test_devices_changed_signals_reload_when_collector_is_idle() {
        let state = create_test_state();
        let collector = Collector::new(Arc::clone(&state));
        assert_eq!(
            collector.start().await,
            CollectorStartResult::NoDevicesConfigured
        );

        let mut reload_rx = state.collector.subscribe_reload();
        state.on_devices_changed().await;

        tokio::time::timeout(Duration::from_millis(100), reload_rx.changed())
            .await
            .expect("reload notification should be sent")
            .expect("reload channel should stay open");

        collector.stop().await;
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
        let result = collector.start().await;
        assert_eq!(result, CollectorStartResult::Started);

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
        drop(stats);

        collector.stop().await;
    }

    #[tokio::test]
    async fn test_collector_stop() {
        let state = create_test_state();
        state.collector.set_running(true);

        let collector = Collector::new(Arc::clone(&state));
        assert!(collector.is_running());

        collector.stop().await;
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
            last_poll_duration_ms: None,
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
                last_poll_duration_ms: None,
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
                last_poll_duration_ms: None,
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

    #[test]
    fn test_collector_error_ble_busy_display() {
        let err = CollectorError::BleBusy;
        assert_eq!(err.to_string(), "BLE adapter busy (semaphore closed)");
    }

    #[tokio::test]
    async fn test_ble_semaphore_serializes_access() {
        let state = create_test_state();

        // Acquire the single permit
        let permit = state.ble_semaphore.acquire().await.unwrap();

        // A second acquire should not succeed immediately
        let result =
            tokio::time::timeout(Duration::from_millis(50), state.ble_semaphore.acquire()).await;
        assert!(
            result.is_err(),
            "second acquire should timeout while first permit is held"
        );

        // After dropping, the next acquire succeeds
        drop(permit);
        let result =
            tokio::time::timeout(Duration::from_millis(50), state.ble_semaphore.acquire()).await;
        assert!(
            result.is_ok(),
            "acquire should succeed after permit is released"
        );
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
        let result = collector.start().await;
        assert_eq!(result, CollectorStartResult::Started);

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
        drop(stats);

        collector.stop().await;
    }
}
