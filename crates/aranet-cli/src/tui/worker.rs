//! Background worker for BLE sensor operations.
//!
//! This module contains the [`SensorWorker`] which handles all Bluetooth Low Energy
//! operations in a background task, keeping the UI thread responsive. The worker
//! communicates with the UI thread via channels:
//!
//! - Receives [`Command`]s from the UI to perform operations
//! - Sends [`SensorEvent`]s back to report results and status updates
//!
//! # Architecture
//!
//! The worker runs in a separate Tokio task and uses `tokio::select!` to handle:
//! - Incoming commands from the UI
//! - Periodic auto-refresh of sensor readings (when enabled)
//!
//! All BLE operations are performed here to avoid blocking the UI rendering loop.

use std::path::PathBuf;
use std::time::Duration;

use aranet_core::service_client::ServiceClient;
use aranet_core::settings::{DeviceSettings, MeasurementInterval};
use aranet_core::{BluetoothRange, Device, ScanOptions, scan::scan_with_options};
use aranet_store::Store;
use aranet_types::{CurrentReading, DeviceType};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use super::messages::{CachedDevice, Command, SensorEvent};
use aranet_core::messages::ServiceDeviceStats;

/// Maximum time to wait for a BLE connect-and-read operation.
const CONNECT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Background worker that handles BLE operations.
///
/// The worker receives commands from the UI thread and performs
/// Bluetooth operations asynchronously, sending events back to
/// update the UI state.
///
/// Note: The Store is not held directly because rusqlite's Connection
/// is not Send+Sync. Instead, we store the path and open the store
/// when needed.
pub struct SensorWorker {
    /// Receiver for commands from the UI thread.
    command_rx: mpsc::Receiver<Command>,
    /// Sender for events back to the UI thread.
    event_tx: mpsc::Sender<SensorEvent>,
    /// Path to persistent storage.
    store_path: PathBuf,
    /// Service client for aranet-service communication.
    service_client: Option<ServiceClient>,
}

/// Default URL for the aranet-service.
const DEFAULT_SERVICE_URL: &str = "http://localhost:8080";

impl SensorWorker {
    /// Create a new sensor worker.
    ///
    /// # Arguments
    ///
    /// * `command_rx` - Channel receiver for commands from the UI
    /// * `event_tx` - Channel sender for events to the UI
    /// * `store_path` - Path to persistent storage
    pub fn new(
        command_rx: mpsc::Receiver<Command>,
        event_tx: mpsc::Sender<SensorEvent>,
        store_path: PathBuf,
    ) -> Self {
        // Try to create service client with default URL
        let service_client = ServiceClient::new(DEFAULT_SERVICE_URL).ok();

        Self {
            command_rx,
            event_tx,
            store_path,
            service_client,
        }
    }

    /// Open the store, logging a warning on failure.
    ///
    /// This helper centralizes store access and error handling.
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
    ///
    /// This method consumes the worker and runs until a [`Command::Shutdown`]
    /// is received or the command channel is closed.
    pub async fn run(mut self) {
        info!("SensorWorker started");

        loop {
            tokio::select! {
                // Handle incoming commands
                cmd = self.command_rx.recv() => {
                    match cmd {
                        Some(Command::Shutdown) => {
                            info!("SensorWorker received shutdown command");
                            break;
                        }
                        Some(cmd) => {
                            self.handle_command(cmd).await;
                        }
                        None => {
                            info!("Command channel closed, shutting down worker");
                            break;
                        }
                    }
                }
            }
        }

        info!("SensorWorker stopped");
    }

    /// Handle a single command from the UI.
    async fn handle_command(&mut self, cmd: Command) {
        info!(?cmd, "Handling command");

        match cmd {
            Command::LoadCachedData => {
                self.handle_load_cached_data().await;
            }
            Command::Scan { duration } => {
                self.handle_scan(duration).await;
            }
            Command::Connect { device_id } => {
                self.handle_connect(&device_id).await;
            }
            Command::Disconnect { device_id } => {
                self.handle_disconnect(&device_id).await;
            }
            Command::RefreshReading { device_id } => {
                self.handle_refresh_reading(&device_id).await;
            }
            Command::RefreshAll => {
                self.handle_refresh_all().await;
            }
            Command::SyncHistory { device_id } => {
                self.handle_sync_history(&device_id).await;
            }
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
            Command::RefreshServiceStatus => {
                self.handle_refresh_service_status().await;
            }
            Command::StartServiceCollector => {
                self.handle_start_service_collector().await;
            }
            Command::StopServiceCollector => {
                self.handle_stop_service_collector().await;
            }
            Command::SetAlias { device_id, alias } => {
                self.handle_set_alias(&device_id, alias).await;
            }
            Command::ForgetDevice { device_id } => {
                // TUI doesn't fully support forget device yet, but handle the command gracefully
                info!(
                    device_id,
                    "Forget device requested (not implemented in TUI)"
                );
                let _ = self
                    .event_tx
                    .send(SensorEvent::DeviceForgotten { device_id })
                    .await;
            }
            Command::Shutdown => {
                // Handled in run() loop
            }
        }
    }

    /// Load cached devices and readings from the store.
    async fn handle_load_cached_data(&self) {
        info!("Loading cached data from store");

        let Some(store) = self.open_store() else {
            // Send empty cached data
            let _ = self
                .event_tx
                .send(SensorEvent::CachedDataLoaded { devices: vec![] })
                .await;
            return;
        };

        // Load all known devices
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

        // Load latest reading for each device
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
                    interval: 0, // Not stored
                    age: 0,      // Will be calculated below
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
                    debug!("Failed to get latest reading for {}: {}", stored.id, e);
                    None
                }
            };

            // Get sync state for last sync time
            let last_sync = match store.get_sync_state(&stored.id) {
                Ok(Some(state)) => state.last_sync_at,
                Ok(None) => None,
                Err(e) => {
                    debug!("Failed to get sync state for {}: {}", stored.id, e);
                    None
                }
            };

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

        // Load history for each cached device (for sparklines on startup)
        for device_id in device_ids {
            self.load_and_send_history(&device_id).await;
        }
    }

    /// Handle a scan command.
    async fn handle_scan(&self, duration: Duration) {
        info!(?duration, "Starting device scan");

        // Notify UI that scan has started
        if let Err(e) = self.event_tx.send(SensorEvent::ScanStarted).await {
            error!("Failed to send ScanStarted event: {}", e);
            return;
        }

        // Perform the scan
        let options = ScanOptions::default().duration(duration);
        match scan_with_options(options).await {
            Ok(devices) => {
                info!(count = devices.len(), "Scan complete");

                // Save discovered devices to store
                self.save_discovered_devices(&devices);

                if let Err(e) = self
                    .event_tx
                    .send(SensorEvent::ScanComplete { devices })
                    .await
                {
                    error!("Failed to send ScanComplete event: {}", e);
                }
            }
            Err(e) => {
                error!("Scan failed: {}", e);
                if let Err(send_err) = self
                    .event_tx
                    .send(SensorEvent::ScanError {
                        error: e.to_string(),
                    })
                    .await
                {
                    error!("Failed to send ScanError event: {}", send_err);
                }
            }
        }
    }

    /// Handle a connect command.
    async fn handle_connect(&self, device_id: &str) {
        info!(device_id, "Connecting to device");

        // Notify UI that we're connecting
        if let Err(e) = self
            .event_tx
            .send(SensorEvent::DeviceConnecting {
                device_id: device_id.to_string(),
            })
            .await
        {
            error!("Failed to send DeviceConnecting event: {}", e);
            return;
        }

        match timeout(CONNECT_READ_TIMEOUT, self.connect_and_read(device_id)).await {
            Ok(Ok((name, device_type, reading, settings, rssi))) => {
                info!(device_id, ?name, ?device_type, ?rssi, "Device connected");

                // Update device metadata in store
                self.update_device_metadata(device_id, name.as_deref(), device_type);

                // Send connected event
                if let Err(e) = self
                    .event_tx
                    .send(SensorEvent::DeviceConnected {
                        device_id: device_id.to_string(),
                        name,
                        device_type,
                        rssi,
                    })
                    .await
                {
                    error!("Failed to send DeviceConnected event: {}", e);
                }

                // Send settings if we got them
                if let Some(settings) = settings
                    && let Err(e) = self
                        .event_tx
                        .send(SensorEvent::SettingsLoaded {
                            device_id: device_id.to_string(),
                            settings,
                        })
                        .await
                {
                    error!("Failed to send SettingsLoaded event: {}", e);
                }

                // Send reading if we got one and save to store
                if let Some(reading) = reading {
                    // Save to store
                    self.save_reading(device_id, &reading);

                    if let Err(e) = self
                        .event_tx
                        .send(SensorEvent::ReadingUpdated {
                            device_id: device_id.to_string(),
                            reading,
                        })
                        .await
                    {
                        error!("Failed to send ReadingUpdated event: {}", e);
                    }
                }

                // Load history for sparklines
                self.load_and_send_history(device_id).await;
            }
            Ok(Err(e)) => {
                error!(device_id, error = %e, "Failed to connect to device");
                if let Err(send_err) = self
                    .event_tx
                    .send(SensorEvent::ConnectionError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await
                {
                    error!("Failed to send ConnectionError event: {}", send_err);
                }
            }
            Err(_) => {
                // Timeout expired
                error!(device_id, "Connection timed out");
                if let Err(send_err) = self
                    .event_tx
                    .send(SensorEvent::ConnectionError {
                        device_id: device_id.to_string(),
                        error: format!(
                            "Connection timed out after {}s",
                            CONNECT_READ_TIMEOUT.as_secs()
                        ),
                    })
                    .await
                {
                    error!("Failed to send ConnectionError event: {}", send_err);
                }
            }
        }
    }

    /// Handle a disconnect command.
    ///
    /// For TUI purposes, disconnection mostly means updating UI state since
    /// we don't maintain persistent connections (we connect, read, and disconnect).
    /// This sends a DeviceDisconnected event to update the UI.
    async fn handle_disconnect(&self, device_id: &str) {
        info!(device_id, "Disconnecting device");

        // Send disconnected event to update UI state
        if let Err(e) = self
            .event_tx
            .send(SensorEvent::DeviceDisconnected {
                device_id: device_id.to_string(),
            })
            .await
        {
            error!("Failed to send DeviceDisconnected event: {}", e);
        }
    }

    /// Handle a refresh reading command.
    async fn handle_refresh_reading(&self, device_id: &str) {
        info!(device_id, "Refreshing reading for device");

        match timeout(CONNECT_READ_TIMEOUT, self.connect_and_read(device_id)).await {
            Ok(Ok((_, _, reading, settings, _rssi))) => {
                // Send settings if we got them
                if let Some(settings) = settings
                    && let Err(e) = self
                        .event_tx
                        .send(SensorEvent::SettingsLoaded {
                            device_id: device_id.to_string(),
                            settings,
                        })
                        .await
                {
                    error!("Failed to send SettingsLoaded event: {}", e);
                }

                if let Some(reading) = reading {
                    info!(device_id, "Reading refreshed successfully");

                    // Save to store
                    self.save_reading(device_id, &reading);

                    if let Err(e) = self
                        .event_tx
                        .send(SensorEvent::ReadingUpdated {
                            device_id: device_id.to_string(),
                            reading,
                        })
                        .await
                    {
                        error!("Failed to send ReadingUpdated event: {}", e);
                    }
                } else {
                    warn!(device_id, "Connected but failed to read current values");
                    if let Err(e) = self
                        .event_tx
                        .send(SensorEvent::ReadingError {
                            device_id: device_id.to_string(),
                            error: "Failed to read current values".to_string(),
                        })
                        .await
                    {
                        error!("Failed to send ReadingError event: {}", e);
                    }
                }
            }
            Ok(Err(e)) => {
                error!(device_id, error = %e, "Failed to refresh reading");
                if let Err(send_err) = self
                    .event_tx
                    .send(SensorEvent::ReadingError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await
                {
                    error!("Failed to send ReadingError event: {}", send_err);
                }
            }
            Err(_) => {
                // Timeout expired
                error!(device_id, "Refresh reading timed out");
                if let Err(send_err) = self
                    .event_tx
                    .send(SensorEvent::ReadingError {
                        device_id: device_id.to_string(),
                        error: format!(
                            "Refresh timed out after {}s",
                            CONNECT_READ_TIMEOUT.as_secs()
                        ),
                    })
                    .await
                {
                    error!("Failed to send ReadingError event: {}", send_err);
                }
            }
        }
    }

    /// Handle a refresh all command.
    ///
    /// Refreshes readings from all known devices by iterating through
    /// and calling handle_refresh_reading for each device.
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
            self.handle_refresh_reading(&device.id).await;
        }

        info!("Completed refreshing all devices");
    }

    /// Handle a set interval command.
    ///
    /// Connects to the device, sets the measurement interval, and sends
    /// the appropriate event back to the UI.
    async fn handle_set_interval(&self, device_id: &str, interval_secs: u16) {
        info!(device_id, interval_secs, "Setting measurement interval");

        // Validate and convert seconds to MeasurementInterval
        let interval = match MeasurementInterval::from_seconds(interval_secs) {
            Some(i) => i,
            None => {
                let error = format!(
                    "Invalid interval: {} seconds. Must be 60, 120, 300, or 600.",
                    interval_secs
                );
                error!(device_id, %error, "Invalid interval value");
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

        // Connect to the device
        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect for set interval");
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

        // Set the interval
        if let Err(e) = device.set_interval(interval).await {
            error!(device_id, error = %e, "Failed to set interval");
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

        // Disconnect from device
        if let Err(e) = device.disconnect().await {
            warn!(device_id, error = %e, "Failed to disconnect after setting interval");
        }

        info!(
            device_id,
            interval_secs, "Measurement interval set successfully"
        );

        // Send success event
        if let Err(e) = self
            .event_tx
            .send(SensorEvent::IntervalChanged {
                device_id: device_id.to_string(),
                interval_secs,
            })
            .await
        {
            error!("Failed to send IntervalChanged event: {}", e);
        }
    }

    /// Handle a set bluetooth range command.
    async fn handle_set_bluetooth_range(&self, device_id: &str, extended: bool) {
        let range_name = if extended { "Extended" } else { "Standard" };
        info!(device_id, range_name, "Setting Bluetooth range");

        // Connect to the device
        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect for set Bluetooth range");
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

        // Set the Bluetooth range
        let range = if extended {
            BluetoothRange::Extended
        } else {
            BluetoothRange::Standard
        };

        if let Err(e) = device.set_bluetooth_range(range).await {
            error!(device_id, error = %e, "Failed to set Bluetooth range");
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

        // Disconnect from device
        if let Err(e) = device.disconnect().await {
            warn!(device_id, error = %e, "Failed to disconnect after setting Bluetooth range");
        }

        info!(device_id, range_name, "Bluetooth range set successfully");

        // Send success event
        if let Err(e) = self
            .event_tx
            .send(SensorEvent::BluetoothRangeChanged {
                device_id: device_id.to_string(),
                extended,
            })
            .await
        {
            error!("Failed to send BluetoothRangeChanged event: {}", e);
        }
    }

    /// Handle a set smart home command.
    async fn handle_set_smart_home(&self, device_id: &str, enabled: bool) {
        let mode = if enabled { "enabled" } else { "disabled" };
        info!(device_id, mode, "Setting Smart Home");

        // Connect to the device
        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect for set Smart Home");
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

        // Set Smart Home mode
        if let Err(e) = device.set_smart_home(enabled).await {
            error!(device_id, error = %e, "Failed to set Smart Home");
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

        // Disconnect from device
        if let Err(e) = device.disconnect().await {
            warn!(device_id, error = %e, "Failed to disconnect after setting Smart Home");
        }

        info!(device_id, mode, "Smart Home set successfully");

        // Send success event
        if let Err(e) = self
            .event_tx
            .send(SensorEvent::SmartHomeChanged {
                device_id: device_id.to_string(),
                enabled,
            })
            .await
        {
            error!("Failed to send SmartHomeChanged event: {}", e);
        }
    }

    /// Connect to a device and read its current values.
    ///
    /// Returns the device name, type, current reading, settings, and RSSI if successful.
    /// The device is disconnected after reading.
    async fn connect_and_read(
        &self,
        device_id: &str,
    ) -> Result<
        (
            Option<String>,
            Option<DeviceType>,
            Option<CurrentReading>,
            Option<DeviceSettings>,
            Option<i16>,
        ),
        aranet_core::Error,
    > {
        let device = Device::connect(device_id).await?;

        let name = device.name().map(String::from);
        let device_type = device.device_type();

        // Try to read current values
        let reading = match device.read_current().await {
            Ok(reading) => {
                info!(device_id, "Read current values successfully");
                Some(reading)
            }
            Err(e) => {
                warn!(device_id, error = %e, "Failed to read current values");
                None
            }
        };

        // Try to read device settings
        let settings = match device.get_settings().await {
            Ok(settings) => {
                info!(device_id, ?settings, "Read device settings successfully");
                Some(settings)
            }
            Err(e) => {
                warn!(device_id, error = %e, "Failed to read device settings");
                None
            }
        };

        // Try to read RSSI signal strength
        let rssi = device.read_rssi().await.ok();

        // Disconnect from the device
        if let Err(e) = device.disconnect().await {
            warn!(device_id, error = %e, "Failed to disconnect from device");
        }

        Ok((name, device_type, reading, settings, rssi))
    }

    /// Save a reading to the store.
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

    /// Save discovered devices to the store.
    fn save_discovered_devices(&self, devices: &[aranet_core::DiscoveredDevice]) {
        let Some(store) = self.open_store() else {
            return;
        };

        for device in devices {
            let device_id = device.id.to_string();
            // Upsert the device with name
            if let Err(e) = store.upsert_device(&device_id, device.name.as_deref()) {
                warn!(device_id, error = %e, "Failed to upsert device");
                continue;
            }
            // Update device type if known
            if let Some(device_type) = device.device_type
                && let Err(e) = store.update_device_metadata(&device_id, None, Some(device_type))
            {
                warn!(device_id, error = %e, "Failed to update device metadata");
            }
        }

        debug!(count = devices.len(), "Saved discovered devices to store");
    }

    /// Update device metadata in the store.
    fn update_device_metadata(
        &self,
        device_id: &str,
        name: Option<&str>,
        device_type: Option<DeviceType>,
    ) {
        let Some(store) = self.open_store() else {
            return;
        };

        // Ensure device exists
        if let Err(e) = store.upsert_device(device_id, name) {
            warn!(device_id, error = %e, "Failed to upsert device");
            return;
        }

        // Update metadata
        if let Err(e) = store.update_device_metadata(device_id, name, device_type) {
            warn!(device_id, error = %e, "Failed to update device metadata");
        } else {
            debug!(device_id, ?name, ?device_type, "Device metadata updated");
        }
    }

    /// Load history from store and send to UI.
    async fn load_and_send_history(&self, device_id: &str) {
        let Some(store) = self.open_store() else {
            return;
        };

        // Query all history for the device (no limit)
        // The UI will filter by time range and resample for sparkline display
        use aranet_store::HistoryQuery;
        let query = HistoryQuery::new().device(device_id).oldest_first(); // Chronological order for sparkline (oldest to newest)

        match store.query_history(&query) {
            Ok(stored_records) => {
                // Convert StoredHistoryRecord to HistoryRecord
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

        // Send history to UI for sparklines
        self.load_and_send_history(device_id).await;
    }

    /// Handle refreshing the aranet-service status.
    async fn handle_refresh_service_status(&self) {
        info!("Refreshing service status");

        let Some(ref client) = self.service_client else {
            let _ = self
                .event_tx
                .send(SensorEvent::ServiceStatusError {
                    error: "Service client not available".to_string(),
                })
                .await;
            return;
        };

        match client.status().await {
            Ok(status) => {
                // Convert device stats to our message type
                let devices: Vec<ServiceDeviceStats> = status
                    .devices
                    .into_iter()
                    .map(|d| ServiceDeviceStats {
                        device_id: d.device_id,
                        alias: d.alias,
                        poll_interval: d.poll_interval,
                        polling: d.polling,
                        success_count: d.success_count,
                        failure_count: d.failure_count,
                        last_poll_at: d.last_poll_at,
                        last_error: d.last_error,
                    })
                    .collect();

                let _ = self
                    .event_tx
                    .send(SensorEvent::ServiceStatusRefreshed {
                        reachable: true,
                        collector_running: status.collector.running,
                        uptime_seconds: status.collector.uptime_seconds,
                        devices,
                    })
                    .await;
            }
            Err(e) => {
                // Check if it's a connection error (service not running)
                let (reachable, error_msg) = match &e {
                    aranet_core::service_client::ServiceClientError::NotReachable { .. } => {
                        (false, "Service not reachable".to_string())
                    }
                    _ => (false, e.to_string()),
                };

                if reachable {
                    let _ = self
                        .event_tx
                        .send(SensorEvent::ServiceStatusError { error: error_msg })
                        .await;
                } else {
                    // Send status with reachable=false
                    let _ = self
                        .event_tx
                        .send(SensorEvent::ServiceStatusRefreshed {
                            reachable: false,
                            collector_running: false,
                            uptime_seconds: None,
                            devices: vec![],
                        })
                        .await;
                }
            }
        }
    }

    /// Handle starting the aranet-service collector.
    async fn handle_start_service_collector(&self) {
        info!("Starting service collector");

        let Some(ref client) = self.service_client else {
            let _ = self
                .event_tx
                .send(SensorEvent::ServiceCollectorError {
                    error: "Service client not available".to_string(),
                })
                .await;
            return;
        };

        match client.start_collector().await {
            Ok(_) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ServiceCollectorStarted)
                    .await;
                // Refresh status to get updated state
                self.handle_refresh_service_status().await;
            }
            Err(e) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ServiceCollectorError {
                        error: e.to_string(),
                    })
                    .await;
            }
        }
    }

    /// Handle stopping the aranet-service collector.
    async fn handle_stop_service_collector(&self) {
        info!("Stopping service collector");

        let Some(ref client) = self.service_client else {
            let _ = self
                .event_tx
                .send(SensorEvent::ServiceCollectorError {
                    error: "Service client not available".to_string(),
                })
                .await;
            return;
        };

        match client.stop_collector().await {
            Ok(_) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ServiceCollectorStopped)
                    .await;
                // Refresh status to get updated state
                self.handle_refresh_service_status().await;
            }
            Err(e) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::ServiceCollectorError {
                        error: e.to_string(),
                    })
                    .await;
            }
        }
    }

    async fn handle_set_alias(&self, device_id: &str, alias: Option<String>) {
        info!("Setting alias for device {} to {:?}", device_id, alias);

        let Some(store) = self.open_store() else {
            let _ = self
                .event_tx
                .send(SensorEvent::AliasError {
                    device_id: device_id.to_string(),
                    error: "Could not open database".to_string(),
                })
                .await;
            return;
        };

        match store.update_device_metadata(device_id, alias.as_deref(), None) {
            Ok(()) => {
                info!("Alias updated successfully for {}", device_id);
                let _ = self
                    .event_tx
                    .send(SensorEvent::AliasChanged {
                        device_id: device_id.to_string(),
                        alias,
                    })
                    .await;
            }
            Err(e) => {
                let _ = self
                    .event_tx
                    .send(SensorEvent::AliasError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
            }
        }
    }
}
