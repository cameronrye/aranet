//! Background worker for BLE sensor operations.
//!
//! This module contains the [`SensorWorker`] which handles all Bluetooth Low Energy
//! operations in a background task, keeping the UI thread responsive.

use std::path::PathBuf;
use std::time::Duration;

use aranet_core::messages::{CachedDevice, Command, SensorEvent};
use aranet_core::scan::scan_with_options;
use aranet_core::settings::{DeviceSettings, MeasurementInterval};
use aranet_core::{BluetoothRange, Device, ScanOptions};
use aranet_store::Store;
use aranet_types::{CurrentReading, DeviceType};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

/// Maximum time to wait for a BLE connect-and-read operation.
const CONNECT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Background worker that handles BLE operations.
pub struct SensorWorker {
    command_rx: mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<SensorEvent>,
    store_path: PathBuf,
}

impl SensorWorker {
    /// Create a new sensor worker with store integration.
    pub fn new(
        command_rx: mpsc::Receiver<Command>,
        event_tx: mpsc::Sender<SensorEvent>,
        store_path: PathBuf,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            store_path,
        }
    }

    /// Open the store, logging a warning on failure.
    fn open_store(&self) -> Option<Store> {
        match Store::open(&self.store_path) {
            Ok(store) => Some(store),
            Err(e) => {
                warn!(error = %e, "Failed to open store");
                None
            }
        }
    }

    /// Run the worker's main loop.
    pub async fn run(mut self) {
        info!("GUI SensorWorker started");
        loop {
            tokio::select! {
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(Command::Shutdown) | None => break,
                        Some(cmd) => self.handle_command(cmd).await,
                    }
                }
            }
        }
        info!("GUI SensorWorker stopped");
    }

    async fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::LoadCachedData => self.handle_load_cached_data().await,
            Command::Scan { duration } => self.handle_scan(duration).await,
            Command::Connect { device_id } => self.handle_connect(&device_id).await,
            Command::Disconnect { device_id } => self.handle_disconnect(&device_id).await,
            Command::RefreshReading { device_id } => self.handle_refresh(&device_id).await,
            Command::RefreshAll => self.handle_refresh_all().await,
            Command::SyncHistory { device_id } => self.handle_sync_history(&device_id).await,
            Command::SetInterval {
                device_id,
                interval_secs,
            } => {
                self.handle_set_interval(&device_id, interval_secs).await;
            }
            Command::SetBluetoothRange {
                device_id,
                extended,
            } => {
                self.handle_set_bluetooth_range(&device_id, extended).await;
            }
            Command::SetSmartHome { device_id, enabled } => {
                self.handle_set_smart_home(&device_id, enabled).await;
            }
            Command::Shutdown => {} // Handled in run() loop
        }
    }

    async fn handle_load_cached_data(&self) {
        info!("Loading cached data from store");
        let Some(store) = self.open_store() else {
            let _ = self
                .event_tx
                .send(SensorEvent::CachedDataLoaded { devices: vec![] })
                .await;
            return;
        };

        let stored_devices = match store.list_devices() {
            Ok(devices) => devices,
            Err(e) => {
                warn!("Failed to list devices: {}", e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::CachedDataLoaded { devices: vec![] })
                    .await;
                return;
            }
        };

        let mut cached_devices = Vec::new();
        for stored in stored_devices {
            let reading = match store.get_latest_reading(&stored.id) {
                Ok(Some(stored_reading)) => Some(CurrentReading {
                    co2: stored_reading.co2,
                    temperature: stored_reading.temperature,
                    pressure: stored_reading.pressure,
                    humidity: stored_reading.humidity,
                    battery: stored_reading.battery,
                    status: stored_reading.status,
                    interval: 0,
                    age: 0,
                    captured_at: Some(stored_reading.captured_at),
                    radon: stored_reading.radon,
                    radiation_rate: stored_reading.radiation_rate,
                    radiation_total: stored_reading.radiation_total,
                    radon_avg_24h: None,
                    radon_avg_7d: None,
                    radon_avg_30d: None,
                }),
                Ok(None) => None,
                Err(e) => {
                    warn!("Failed to get latest reading for {}: {}", stored.id, e);
                    None
                }
            };

            let last_sync = store
                .get_sync_state(&stored.id)
                .ok()
                .flatten()
                .and_then(|s| s.last_sync_at);

            cached_devices.push(CachedDevice {
                id: stored.id,
                name: stored.name,
                device_type: stored.device_type,
                reading,
                last_sync,
            });
        }

        info!(count = cached_devices.len(), "Loaded cached devices");

        // Collect device IDs before sending (we need them for history loading)
        let device_ids: Vec<String> = cached_devices.iter().map(|d| d.id.clone()).collect();

        if let Err(e) = self
            .event_tx
            .send(SensorEvent::CachedDataLoaded {
                devices: cached_devices,
            })
            .await
        {
            error!("Failed to send CachedDataLoaded event: {}", e);
        }

        // Load history for each cached device (so history is visible on startup)
        for device_id in device_ids {
            self.load_and_send_history(&device_id).await;
        }
    }

    /// Load history from store and send to UI.
    async fn load_and_send_history(&self, device_id: &str) {
        let Some(store) = self.open_store() else {
            return;
        };

        // Query all history for the device
        use aranet_store::HistoryQuery;
        let query = HistoryQuery::new().device(device_id).oldest_first();

        match store.query_history(&query) {
            Ok(stored_records) => {
                let records: Vec<aranet_types::HistoryRecord> = stored_records
                    .into_iter()
                    .map(|r| aranet_types::HistoryRecord {
                        timestamp: r.timestamp,
                        co2: r.co2,
                        temperature: r.temperature,
                        pressure: r.pressure,
                        humidity: r.humidity,
                        radon: r.radon,
                        radiation_rate: r.radiation_rate,
                        radiation_total: r.radiation_total,
                    })
                    .collect();

                info!(
                    device_id,
                    count = records.len(),
                    "Loaded history from store"
                );

                if let Err(e) = self
                    .event_tx
                    .send(SensorEvent::HistoryLoaded {
                        device_id: device_id.to_string(),
                        records,
                    })
                    .await
                {
                    error!("Failed to send HistoryLoaded event: {}", e);
                }
            }
            Err(e) => {
                warn!(device_id, error = %e, "Failed to query history from store");
            }
        }
    }

    async fn handle_scan(&self, duration: Duration) {
        let _ = self.event_tx.send(SensorEvent::ScanStarted).await;
        let options = ScanOptions::default().duration(duration);
        match scan_with_options(options).await {
            Ok(devices) => {
                self.save_discovered_devices(&devices);
                let _ = self
                    .event_tx
                    .send(SensorEvent::ScanComplete { devices })
                    .await;
            }
            Err(e) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ScanError {
                        error: e.to_string(),
                    })
                    .await;
            }
        }
    }

    async fn handle_connect(&self, device_id: &str) {
        let _ = self
            .event_tx
            .send(SensorEvent::DeviceConnecting {
                device_id: device_id.to_string(),
            })
            .await;

        match timeout(CONNECT_READ_TIMEOUT, self.connect_and_read(device_id)).await {
            Ok(Ok((name, device_type, reading, settings))) => {
                self.save_device_connection(device_id, name.as_deref(), device_type);
                let _ = self
                    .event_tx
                    .send(SensorEvent::DeviceConnected {
                        device_id: device_id.to_string(),
                        name,
                        device_type,
                        rssi: None,
                    })
                    .await;

                // Send settings if we got them
                if let Some(settings) = settings {
                    let _ = self
                        .event_tx
                        .send(SensorEvent::SettingsLoaded {
                            device_id: device_id.to_string(),
                            settings,
                        })
                        .await;
                }

                // Send reading if we got one and save to store
                if let Some(ref reading) = reading {
                    self.save_reading(device_id, reading);
                    let _ = self
                        .event_tx
                        .send(SensorEvent::ReadingUpdated {
                            device_id: device_id.to_string(),
                            reading: *reading,
                        })
                        .await;
                }

                // Load history for display
                self.load_and_send_history(device_id).await;
            }
            Ok(Err(e)) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ConnectionError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
            }
            Err(_) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ConnectionError {
                        device_id: device_id.to_string(),
                        error: "Connection timed out".to_string(),
                    })
                    .await;
            }
        }
    }

    async fn handle_disconnect(&self, device_id: &str) {
        let _ = self
            .event_tx
            .send(SensorEvent::DeviceDisconnected {
                device_id: device_id.to_string(),
            })
            .await;
    }

    async fn handle_refresh(&self, device_id: &str) {
        match timeout(CONNECT_READ_TIMEOUT, self.connect_and_read(device_id)).await {
            Ok(Ok((_, _, reading, settings))) => {
                // Send settings if we got them
                if let Some(settings) = settings {
                    let _ = self
                        .event_tx
                        .send(SensorEvent::SettingsLoaded {
                            device_id: device_id.to_string(),
                            settings,
                        })
                        .await;
                }

                if let Some(reading) = reading {
                    self.save_reading(device_id, &reading);
                    let _ = self
                        .event_tx
                        .send(SensorEvent::ReadingUpdated {
                            device_id: device_id.to_string(),
                            reading,
                        })
                        .await;
                } else {
                    let _ = self
                        .event_tx
                        .send(SensorEvent::ReadingError {
                            device_id: device_id.to_string(),
                            error: "Failed to read current values".to_string(),
                        })
                        .await;
                }
            }
            Ok(Err(e)) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ReadingError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
            }
            Err(_) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ReadingError {
                        device_id: device_id.to_string(),
                        error: format!(
                            "Refresh timed out after {}s",
                            CONNECT_READ_TIMEOUT.as_secs()
                        ),
                    })
                    .await;
            }
        }
    }

    /// Refresh readings for all known devices.
    ///
    /// Iterates through all devices in the store and refreshes readings for each.
    async fn handle_refresh_all(&self) {
        info!("Refreshing all devices");

        // Open store to get list of known devices
        let Some(store) = self.open_store() else {
            return;
        };

        let devices = match store.list_devices() {
            Ok(devices) => devices,
            Err(e) => {
                warn!("Failed to list devices for refresh all: {}", e);
                return;
            }
        };

        // Refresh each device
        for device in devices {
            self.handle_refresh(&device.id).await;
        }

        info!("Completed refreshing all devices");
    }

    /// Sync history from device (download via BLE and save to store).
    ///
    /// Uses incremental sync - only downloads new records since the last sync.
    async fn handle_sync_history(&self, device_id: &str) {
        use aranet_core::history::HistoryOptions;

        info!(device_id, "Syncing history from device");

        // Notify UI that sync is starting
        if let Err(e) = self
            .event_tx
            .send(SensorEvent::HistorySyncStarted {
                device_id: device_id.to_string(),
            })
            .await
        {
            error!("Failed to send HistorySyncStarted event: {}", e);
            return;
        }

        // Open store first to check sync state
        let Some(store) = self.open_store() else {
            let _ = self
                .event_tx
                .send(SensorEvent::HistorySyncError {
                    device_id: device_id.to_string(),
                    error: "Failed to open store".to_string(),
                })
                .await;
            return;
        };

        // Connect to the device
        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect for history sync");
                let _ = self
                    .event_tx
                    .send(SensorEvent::HistorySyncError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        // Get history info to know how many records are on the device
        let history_info = match device.get_history_info().await {
            Ok(info) => info,
            Err(e) => {
                error!(device_id, error = %e, "Failed to get history info");
                let _ = device.disconnect().await;
                let _ = self
                    .event_tx
                    .send(SensorEvent::HistorySyncError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        let total_on_device = history_info.total_readings;

        // Calculate start index for incremental sync
        let start_index = match store.calculate_sync_start(device_id, total_on_device) {
            Ok(idx) => idx,
            Err(e) => {
                warn!(device_id, error = %e, "Failed to calculate sync start, doing full sync");
                1u16
            }
        };

        // Check if already up to date
        if start_index > total_on_device {
            info!(device_id, "Already up to date, no new readings to sync");
            let _ = device.disconnect().await;
            let _ = self
                .event_tx
                .send(SensorEvent::HistorySynced {
                    device_id: device_id.to_string(),
                    count: 0,
                })
                .await;
            // Still load history from store to update UI
            self.load_and_send_history(device_id).await;
            return;
        }

        let records_to_download = total_on_device.saturating_sub(start_index) + 1;
        info!(
            device_id,
            start_index,
            total_on_device,
            records_to_download,
            "Downloading history (incremental sync)"
        );

        // Download history with start_index for incremental sync
        let history_options = HistoryOptions {
            start_index: Some(start_index),
            end_index: None, // Download to the end
            ..Default::default()
        };

        let records = match device.download_history_with_options(history_options).await {
            Ok(r) => r,
            Err(e) => {
                error!(device_id, error = %e, "Failed to download history");
                let _ = device.disconnect().await;
                let _ = self
                    .event_tx
                    .send(SensorEvent::HistorySyncError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        let record_count = records.len();
        info!(
            device_id,
            count = record_count,
            "Downloaded history from device"
        );

        // Disconnect from device
        let _ = device.disconnect().await;

        // Insert history to store (with deduplication)
        if let Err(e) = store.insert_history(device_id, &records) {
            warn!(device_id, error = %e, "Failed to save history to store");
        } else {
            debug!(device_id, count = record_count, "History saved to store");
        }

        // Update sync state for next incremental sync
        if let Err(e) = store.update_sync_state(device_id, total_on_device, total_on_device) {
            warn!(device_id, error = %e, "Failed to update sync state");
        }

        // Notify UI that sync is complete
        if let Err(e) = self
            .event_tx
            .send(SensorEvent::HistorySynced {
                device_id: device_id.to_string(),
                count: record_count,
            })
            .await
        {
            error!("Failed to send HistorySynced event: {}", e);
        }

        // Send history to UI for display
        self.load_and_send_history(device_id).await;
    }

    async fn connect_and_read(
        &self,
        device_id: &str,
    ) -> Result<
        (
            Option<String>,
            Option<DeviceType>,
            Option<CurrentReading>,
            Option<DeviceSettings>,
        ),
        aranet_core::Error,
    > {
        let device = Device::connect(device_id).await?;
        let name = device.name().map(String::from);
        let device_type = device.device_type();
        let reading = device.read_current().await.ok();
        let settings = device.get_settings().await.ok();
        let _ = device.disconnect().await;
        Ok((name, device_type, reading, settings))
    }

    async fn handle_set_interval(&self, device_id: &str, interval_secs: u16) {
        info!(
            "Setting measurement interval for {} to {} seconds",
            device_id, interval_secs
        );

        let interval = match MeasurementInterval::from_seconds(interval_secs) {
            Some(i) => i,
            None => {
                let error = format!(
                    "Invalid interval: {} seconds. Must be 60, 120, 300, or 600.",
                    interval_secs
                );
                warn!("{}", error);
                let _ = self
                    .event_tx
                    .send(SensorEvent::IntervalError {
                        device_id: device_id.to_string(),
                        error,
                    })
                    .await;
                return;
            }
        };

        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to connect for set interval: {}", e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::IntervalError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        if let Err(e) = device.set_interval(interval).await {
            warn!("Failed to set interval: {}", e);
            let _ = device.disconnect().await;
            let _ = self
                .event_tx
                .send(SensorEvent::IntervalError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                })
                .await;
            return;
        }

        if let Err(e) = device.disconnect().await {
            warn!("Failed to disconnect after setting interval: {}", e);
        }

        info!("Measurement interval set successfully for {}", device_id);
        let _ = self
            .event_tx
            .send(SensorEvent::IntervalChanged {
                device_id: device_id.to_string(),
                interval_secs,
            })
            .await;
    }

    async fn handle_set_bluetooth_range(&self, device_id: &str, extended: bool) {
        let range_name = if extended { "Extended" } else { "Standard" };
        info!(
            "Setting Bluetooth range for {} to {}",
            device_id, range_name
        );

        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to connect for set Bluetooth range: {}", e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::BluetoothRangeError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        let range = if extended {
            BluetoothRange::Extended
        } else {
            BluetoothRange::Standard
        };

        if let Err(e) = device.set_bluetooth_range(range).await {
            warn!("Failed to set Bluetooth range: {}", e);
            let _ = device.disconnect().await;
            let _ = self
                .event_tx
                .send(SensorEvent::BluetoothRangeError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                })
                .await;
            return;
        }

        if let Err(e) = device.disconnect().await {
            warn!("Failed to disconnect after setting Bluetooth range: {}", e);
        }

        info!("Bluetooth range set successfully for {}", device_id);
        let _ = self
            .event_tx
            .send(SensorEvent::BluetoothRangeChanged {
                device_id: device_id.to_string(),
                extended,
            })
            .await;
    }

    async fn handle_set_smart_home(&self, device_id: &str, enabled: bool) {
        let mode = if enabled { "enabled" } else { "disabled" };
        info!("Setting Smart Home for {} to {}", device_id, mode);

        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to connect for set Smart Home: {}", e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::SmartHomeError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        if let Err(e) = device.set_smart_home(enabled).await {
            warn!("Failed to set Smart Home: {}", e);
            let _ = device.disconnect().await;
            let _ = self
                .event_tx
                .send(SensorEvent::SmartHomeError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                })
                .await;
            return;
        }

        if let Err(e) = device.disconnect().await {
            warn!("Failed to disconnect after setting Smart Home: {}", e);
        }

        info!("Smart Home set successfully for {}", device_id);
        let _ = self
            .event_tx
            .send(SensorEvent::SmartHomeChanged {
                device_id: device_id.to_string(),
                enabled,
            })
            .await;
    }

    // -------------------------------------------------------------------------
    // Store Helper Methods
    // -------------------------------------------------------------------------

    fn save_reading(&self, device_id: &str, reading: &CurrentReading) {
        let Some(store) = self.open_store() else {
            return;
        };

        if let Err(e) = store.insert_reading(device_id, reading) {
            warn!(device_id, error = %e, "Failed to save reading to store");
        } else {
            debug!(device_id, "Reading saved to store");
        }
    }

    fn save_discovered_devices(&self, devices: &[aranet_core::DiscoveredDevice]) {
        let Some(store) = self.open_store() else {
            return;
        };

        for device in devices {
            let device_id = device.id.to_string();
            if let Err(e) = store.upsert_device(&device_id, device.name.as_deref()) {
                warn!(device_id, error = %e, "Failed to upsert device");
                continue;
            }
            if let Some(device_type) = device.device_type
                && let Err(e) = store.update_device_metadata(&device_id, None, Some(device_type))
            {
                warn!(device_id, error = %e, "Failed to update device metadata");
            }
        }

        debug!(count = devices.len(), "Saved discovered devices to store");
    }

    fn save_device_connection(
        &self,
        device_id: &str,
        name: Option<&str>,
        device_type: Option<DeviceType>,
    ) {
        let Some(store) = self.open_store() else {
            return;
        };

        if let Err(e) = store.upsert_device(device_id, name) {
            warn!(device_id, error = %e, "Failed to upsert device");
            return;
        }

        if let Err(e) = store.update_device_metadata(device_id, name, device_type) {
            warn!(device_id, error = %e, "Failed to update device metadata");
        } else {
            debug!(device_id, ?name, ?device_type, "Device connection saved");
        }
    }
}
