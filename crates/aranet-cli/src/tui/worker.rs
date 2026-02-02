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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aranet_core::device::{ConnectionConfig, SignalQuality};
use aranet_core::messages::{ErrorContext, ServiceDeviceStats};
use aranet_core::service_client::ServiceClient;
use aranet_core::settings::{DeviceSettings, MeasurementInterval};
use aranet_core::{
    BluetoothRange, Device, RetryConfig, ScanOptions, scan::scan_with_options, with_retry,
};
use aranet_store::Store;
use aranet_types::{CurrentReading, DeviceType};
use tokio::sync::{RwLock, mpsc};
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::messages::{CachedDevice, Command, SensorEvent};

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
    /// Connection configuration (platform-optimized timeouts).
    connection_config: ConnectionConfig,
    /// Background polling tasks indexed by device_id.
    /// Each entry holds a cancel token that can be used to stop the polling task.
    background_polling: Arc<RwLock<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
    /// Last known signal quality per device (for adaptive behavior).
    signal_quality_cache: Arc<RwLock<HashMap<String, SignalQuality>>>,
    /// Cancellation token for long-running operations.
    /// Used to cancel scans, connections, and history syncs.
    cancel_token: CancellationToken,
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

        // Use platform-optimized connection configuration
        let connection_config = ConnectionConfig::for_current_platform();
        info!(
            ?connection_config,
            "Using platform-optimized connection config"
        );

        Self {
            command_rx,
            event_tx,
            store_path,
            service_client,
            connection_config,
            background_polling: Arc::new(RwLock::new(HashMap::new())),
            signal_quality_cache: Arc::new(RwLock::new(HashMap::new())),
            cancel_token: CancellationToken::new(),
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
                self.handle_forget_device(&device_id).await;
            }
            Command::CancelOperation => {
                self.handle_cancel_operation().await;
            }
            Command::StartBackgroundPolling {
                device_id,
                interval_secs,
            } => {
                self.handle_start_background_polling(&device_id, interval_secs)
                    .await;
            }
            Command::StopBackgroundPolling { device_id } => {
                self.handle_stop_background_polling(&device_id).await;
            }
            Command::Shutdown => {
                // Handled in run() loop
            }
            // System service commands not supported in TUI
            Command::InstallSystemService { .. }
            | Command::UninstallSystemService { .. }
            | Command::StartSystemService { .. }
            | Command::StopSystemService { .. }
            | Command::CheckSystemServiceStatus { .. }
            | Command::FetchServiceConfig
            | Command::AddServiceDevice { .. }
            | Command::UpdateServiceDevice { .. }
            | Command::RemoveServiceDevice { .. } => {
                info!("System service commands not supported in TUI");
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

        // Clone the cancel token for this operation
        let cancel_token = self.cancel_token.clone();

        // Perform the scan with cancellation support
        let options = ScanOptions::default().duration(duration);
        let scan_result = tokio::select! {
            result = scan_with_options(options) => result,
            _ = cancel_token.cancelled() => {
                info!("Scan cancelled by user");
                let _ = self
                    .event_tx
                    .send(SensorEvent::OperationCancelled {
                        operation: "Device scan".to_string(),
                    })
                    .await;
                return;
            }
        };

        match scan_result {
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

    /// Handle a connect command with retry logic and error context.
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

        // Clone the cancel token for this operation
        let cancel_token = self.cancel_token.clone();

        // Use retry logic for connection (connection can fail due to timing, signal, etc.)
        let retry_config = RetryConfig::for_connect();
        let device_id_owned = device_id.to_string();
        let config = self.connection_config.clone();

        let connect_future = with_retry(&retry_config, "connect_and_read", || {
            let device_id = device_id_owned.clone();
            let config = config.clone();
            async move { Self::connect_and_read_with_config(&device_id, config).await }
        });

        // Wrap in select for cancellation support
        let result = tokio::select! {
            result = connect_future => result,
            _ = cancel_token.cancelled() => {
                info!(device_id, "Connection cancelled by user");
                let _ = self
                    .event_tx
                    .send(SensorEvent::OperationCancelled {
                        operation: format!("Connect to {}", device_id),
                    })
                    .await;
                return;
            }
        };

        match result {
            Ok((name, device_type, reading, settings, rssi, signal_quality)) => {
                info!(
                    device_id,
                    ?name,
                    ?device_type,
                    ?rssi,
                    ?signal_quality,
                    "Device connected"
                );

                // Cache signal quality for adaptive behavior
                if let Some(quality) = signal_quality {
                    self.signal_quality_cache
                        .write()
                        .await
                        .insert(device_id.to_string(), quality);

                    // Send signal strength update
                    if let Some(rssi_val) = rssi {
                        let _ = self
                            .event_tx
                            .send(SensorEvent::SignalStrengthUpdate {
                                device_id: device_id.to_string(),
                                rssi: rssi_val,
                                quality: aranet_core::messages::SignalQuality::from_rssi(rssi_val),
                            })
                            .await;
                    }

                    // Warn about poor signal quality
                    if quality == SignalQuality::Poor {
                        warn!(
                            device_id,
                            "Poor signal quality - connection may be unstable"
                        );
                    }
                }

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
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect to device after retries");
                // Populate error context with user-friendly information
                let context = ErrorContext::from_error(&e);
                if let Err(send_err) = self
                    .event_tx
                    .send(SensorEvent::ConnectionError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                        context: Some(context),
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

    /// Handle a refresh reading command with retry logic and adaptive timing.
    async fn handle_refresh_reading(&self, device_id: &str) {
        info!(device_id, "Refreshing reading for device");

        // Get cached signal quality for adaptive retry configuration
        let signal_quality = self
            .signal_quality_cache
            .read()
            .await
            .get(device_id)
            .copied();

        // Use more aggressive retries for devices with known poor signal
        let retry_config = match signal_quality {
            Some(SignalQuality::Poor) | Some(SignalQuality::Fair) => {
                debug!(
                    device_id,
                    ?signal_quality,
                    "Using aggressive retry config for weak signal"
                );
                RetryConfig::aggressive()
            }
            _ => RetryConfig::for_read(),
        };

        let device_id_owned = device_id.to_string();
        let config = self.connection_config.clone();

        let result = with_retry(&retry_config, "refresh_reading", || {
            let device_id = device_id_owned.clone();
            let config = config.clone();
            async move { Self::connect_and_read_with_config(&device_id, config).await }
        })
        .await;

        match result {
            Ok((_, _, reading, settings, rssi, new_signal_quality)) => {
                // Update cached signal quality
                if let Some(quality) = new_signal_quality {
                    self.signal_quality_cache
                        .write()
                        .await
                        .insert(device_id.to_string(), quality);
                }

                // Send signal strength update if available
                if let Some(rssi_val) = rssi {
                    let _ = self
                        .event_tx
                        .send(SensorEvent::SignalStrengthUpdate {
                            device_id: device_id.to_string(),
                            rssi: rssi_val,
                            quality: aranet_core::messages::SignalQuality::from_rssi(rssi_val),
                        })
                        .await;
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
                    let context = ErrorContext::transient(
                        "Failed to read current values",
                        "Device connected but returned no data. Try again.",
                    );
                    if let Err(e) = self
                        .event_tx
                        .send(SensorEvent::ReadingError {
                            device_id: device_id.to_string(),
                            error: "Failed to read current values".to_string(),
                            context: Some(context),
                        })
                        .await
                    {
                        error!("Failed to send ReadingError event: {}", e);
                    }
                }
            }
            Err(e) => {
                error!(device_id, error = %e, "Failed to refresh reading after retries");
                let context = ErrorContext::from_error(&e);
                if let Err(send_err) = self
                    .event_tx
                    .send(SensorEvent::ReadingError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                        context: Some(context),
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

    /// Handle a set interval command with retry logic and error context.
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
                let context = ErrorContext::permanent(&error);
                let _ = self
                    .event_tx
                    .send(SensorEvent::IntervalError {
                        device_id: device_id.to_string(),
                        error,
                        context: Some(context),
                    })
                    .await;
                return;
            }
        };

        // Connect to the device with retry
        let retry_config = RetryConfig::for_connect();
        let device_id_owned = device_id.to_string();
        let config = self.connection_config.clone();

        let device = match with_retry(&retry_config, "connect_for_interval", || {
            let device_id = device_id_owned.clone();
            let config = config.clone();
            async move { Device::connect_with_config(&device_id, config).await }
        })
        .await
        {
            Ok(d) => d,
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect for set interval");
                let context = ErrorContext::from_error(&e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::IntervalError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                        context: Some(context),
                    })
                    .await;
                return;
            }
        };

        // Set the interval with retry
        let retry_config = RetryConfig::for_write();
        if let Err(e) = with_retry(&retry_config, "set_interval", || async {
            device.set_interval(interval).await
        })
        .await
        {
            error!(device_id, error = %e, "Failed to set interval");
            let _ = device.disconnect().await;
            let context = ErrorContext::from_error(&e);
            let _ = self
                .event_tx
                .send(SensorEvent::IntervalError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                    context: Some(context),
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

    /// Handle a set bluetooth range command with retry logic and error context.
    async fn handle_set_bluetooth_range(&self, device_id: &str, extended: bool) {
        let range_name = if extended { "Extended" } else { "Standard" };
        info!(device_id, range_name, "Setting Bluetooth range");

        // Connect to the device with retry
        let retry_config = RetryConfig::for_connect();
        let device_id_owned = device_id.to_string();
        let config = self.connection_config.clone();

        let device = match with_retry(&retry_config, "connect_for_bt_range", || {
            let device_id = device_id_owned.clone();
            let config = config.clone();
            async move { Device::connect_with_config(&device_id, config).await }
        })
        .await
        {
            Ok(d) => d,
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect for set Bluetooth range");
                let context = ErrorContext::from_error(&e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::BluetoothRangeError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                        context: Some(context),
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

        // Set range with retry
        let retry_config = RetryConfig::for_write();
        if let Err(e) = with_retry(&retry_config, "set_bt_range", || async {
            device.set_bluetooth_range(range).await
        })
        .await
        {
            error!(device_id, error = %e, "Failed to set Bluetooth range");
            let _ = device.disconnect().await;
            let context = ErrorContext::from_error(&e);
            let _ = self
                .event_tx
                .send(SensorEvent::BluetoothRangeError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                    context: Some(context),
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

    /// Handle a set smart home command with retry logic and error context.
    async fn handle_set_smart_home(&self, device_id: &str, enabled: bool) {
        let mode = if enabled { "enabled" } else { "disabled" };
        info!(device_id, mode, "Setting Smart Home");

        // Connect to the device with retry
        let retry_config = RetryConfig::for_connect();
        let device_id_owned = device_id.to_string();
        let config = self.connection_config.clone();

        let device = match with_retry(&retry_config, "connect_for_smart_home", || {
            let device_id = device_id_owned.clone();
            let config = config.clone();
            async move { Device::connect_with_config(&device_id, config).await }
        })
        .await
        {
            Ok(d) => d,
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect for set Smart Home");
                let context = ErrorContext::from_error(&e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::SmartHomeError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                        context: Some(context),
                    })
                    .await;
                return;
            }
        };

        // Set Smart Home mode with retry
        let retry_config = RetryConfig::for_write();
        if let Err(e) = with_retry(&retry_config, "set_smart_home", || async {
            device.set_smart_home(enabled).await
        })
        .await
        {
            error!(device_id, error = %e, "Failed to set Smart Home");
            let _ = device.disconnect().await;
            let context = ErrorContext::from_error(&e);
            let _ = self
                .event_tx
                .send(SensorEvent::SmartHomeError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                    context: Some(context),
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

    /// Connect to a device and read its current values with custom configuration.
    ///
    /// This is a static method that doesn't require `&self`, making it suitable
    /// for use with retry closures.
    ///
    /// Returns the device name, type, current reading, settings, RSSI, and signal quality.
    /// The device is disconnected after reading.
    async fn connect_and_read_with_config(
        device_id: &str,
        config: ConnectionConfig,
    ) -> Result<
        (
            Option<String>,
            Option<DeviceType>,
            Option<CurrentReading>,
            Option<DeviceSettings>,
            Option<i16>,
            Option<SignalQuality>,
        ),
        aranet_core::Error,
    > {
        let device = Device::connect_with_config(device_id, config).await?;

        // Validate connection is truly alive (especially important on macOS)
        if !device.validate_connection().await {
            warn!(
                device_id,
                "Connection validation failed - device may be out of range"
            );
            let _ = device.disconnect().await;
            return Err(aranet_core::Error::NotConnected);
        }
        debug!(device_id, "Connection validated successfully");

        let name = device.name().map(String::from);
        let device_type = device.device_type();

        // Read RSSI and determine signal quality for adaptive behavior
        let rssi = device.read_rssi().await.ok();
        let signal_quality = rssi.map(SignalQuality::from_rssi);

        if let Some(quality) = signal_quality {
            debug!(device_id, ?quality, rssi = ?rssi, "Signal quality assessed");
        }

        // Add adaptive delay for weak signals before reading
        if let Some(quality) = signal_quality {
            let delay = quality.recommended_read_delay();
            if delay > Duration::from_millis(50) {
                debug!(device_id, ?delay, "Adding read delay for signal quality");
                tokio::time::sleep(delay).await;
            }
        }

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

        // Disconnect from the device
        if let Err(e) = device.disconnect().await {
            warn!(device_id, error = %e, "Failed to disconnect from device");
        }

        Ok((name, device_type, reading, settings, rssi, signal_quality))
    }

    /// Connect to a device and read its current values (legacy method).
    ///
    /// Returns the device name, type, current reading, settings, and RSSI if successful.
    /// The device is disconnected after reading.
    #[allow(dead_code)]
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
        let (name, device_type, reading, settings, rssi, _signal_quality) =
            Self::connect_and_read_with_config(device_id, self.connection_config.clone()).await?;
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
    /// Includes retry logic and progress reporting.
    async fn handle_sync_history(&self, device_id: &str) {
        use aranet_core::history::HistoryOptions;

        info!(device_id, "Syncing history from device");

        // Open store first to check sync state
        let Some(store) = self.open_store() else {
            let context = ErrorContext::permanent("Failed to open local database");
            let _ = self
                .event_tx
                .send(SensorEvent::HistorySyncError {
                    device_id: device_id.to_string(),
                    error: "Failed to open store".to_string(),
                    context: Some(context),
                })
                .await;
            return;
        };

        // Connect to the device with retry logic
        let retry_config = RetryConfig::for_connect();
        let device_id_owned = device_id.to_string();
        let config = self.connection_config.clone();

        let device = match with_retry(&retry_config, "connect_for_history", || {
            let device_id = device_id_owned.clone();
            let config = config.clone();
            async move { Device::connect_with_config(&device_id, config).await }
        })
        .await
        {
            Ok(d) => d,
            Err(e) => {
                error!(device_id, error = %e, "Failed to connect for history sync");
                let context = ErrorContext::from_error(&e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::HistorySyncError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                        context: Some(context),
                    })
                    .await;
                return;
            }
        };

        // Validate connection
        if !device.validate_connection().await {
            warn!(device_id, "Connection validation failed for history sync");
            let _ = device.disconnect().await;
            let context = ErrorContext::transient(
                "Connection validation failed",
                "Device connected but is not responding. Try moving closer.",
            );
            let _ = self
                .event_tx
                .send(SensorEvent::HistorySyncError {
                    device_id: device_id.to_string(),
                    error: "Connection validation failed".to_string(),
                    context: Some(context),
                })
                .await;
            return;
        }

        // Get history info to know how many records are on the device
        let history_info = match device.get_history_info().await {
            Ok(info) => info,
            Err(e) => {
                error!(device_id, error = %e, "Failed to get history info");
                let _ = device.disconnect().await;
                let context = ErrorContext::from_error(&e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::HistorySyncError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                        context: Some(context),
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

        // Notify UI that sync is starting with total count
        if let Err(e) = self
            .event_tx
            .send(SensorEvent::HistorySyncStarted {
                device_id: device_id.to_string(),
                total_records: Some(records_to_download),
            })
            .await
        {
            error!("Failed to send HistorySyncStarted event: {}", e);
        }

        // Download history with start_index for incremental sync
        // Use adaptive read delay based on signal quality
        let signal_quality = self
            .signal_quality_cache
            .read()
            .await
            .get(device_id)
            .copied();
        let read_delay = signal_quality
            .map(|q| q.recommended_read_delay())
            .unwrap_or(Duration::from_millis(50));

        let history_options = HistoryOptions {
            start_index: Some(start_index),
            end_index: None, // Download to the end
            read_delay,
            use_adaptive_delay: true, // Use adaptive delay based on signal quality
            ..Default::default()
        };

        // Send periodic progress updates during download
        let event_tx = self.event_tx.clone();
        let device_id_for_progress = device_id.to_string();
        let total = records_to_download as usize;

        // Create a progress callback
        let last_progress_update = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let last_progress_clone = last_progress_update.clone();

        // Spawn a task to send progress updates every 10 records or 500ms
        let progress_task = {
            let event_tx = event_tx.clone();
            let device_id = device_id_for_progress.clone();
            tokio::spawn(async move {
                let mut interval = interval(Duration::from_millis(500));
                loop {
                    interval.tick().await;
                    let downloaded = last_progress_clone.load(std::sync::atomic::Ordering::Relaxed);
                    if downloaded > 0 && downloaded < total {
                        let _ = event_tx
                            .send(SensorEvent::HistorySyncProgress {
                                device_id: device_id.clone(),
                                downloaded,
                                total,
                            })
                            .await;
                    }
                    if downloaded >= total {
                        break;
                    }
                }
            })
        };

        // Clone the cancel token for this operation
        let cancel_token = self.cancel_token.clone();

        // Download with retry for the actual download operation
        let retry_config = RetryConfig::for_history();
        let download_future = with_retry(&retry_config, "download_history", || {
            let options = history_options.clone();
            let progress = last_progress_update.clone();
            let device = &device;
            async move {
                let records = device.download_history_with_options(options).await?;
                progress.store(records.len(), std::sync::atomic::Ordering::Relaxed);
                Ok(records)
            }
        });

        // Wrap download in select for cancellation support
        let download_result = tokio::select! {
            result = download_future => result,
            _ = cancel_token.cancelled() => {
                progress_task.abort();
                info!(device_id, "History sync cancelled by user");
                let _ = device.disconnect().await;
                let _ = self
                    .event_tx
                    .send(SensorEvent::OperationCancelled {
                        operation: format!("History sync for {}", device_id),
                    })
                    .await;
                return;
            }
        };

        let records = match download_result {
            Ok(r) => {
                progress_task.abort();
                r
            }
            Err(e) => {
                progress_task.abort();
                error!(device_id, error = %e, "Failed to download history");
                let _ = device.disconnect().await;
                let context = ErrorContext::from_error(&e);
                let _ = self
                    .event_tx
                    .send(SensorEvent::HistorySyncError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                        context: Some(context),
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

        // Send final progress update
        let _ = self
            .event_tx
            .send(SensorEvent::HistorySyncProgress {
                device_id: device_id.to_string(),
                downloaded: record_count,
                total,
            })
            .await;

        // Disconnect from device
        let _ = device.disconnect().await;

        // Insert history to store (with deduplication)
        // Only update sync state if insert succeeds to avoid data loss on next sync
        match store.insert_history(device_id, &records) {
            Ok(inserted) => {
                debug!(
                    device_id,
                    downloaded = record_count,
                    inserted,
                    "History saved to store"
                );

                // Update sync state for next incremental sync
                if let Err(e) = store.update_sync_state(device_id, total_on_device, total_on_device)
                {
                    warn!(device_id, error = %e, "Failed to update sync state");
                }
            }
            Err(e) => {
                warn!(device_id, error = %e, "Failed to save history to store - sync state not updated");
            }
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

    /// Start background polling for a device.
    ///
    /// Spawns a background task that periodically reads from the device
    /// and sends updates to the UI. The task can be cancelled by calling
    /// `handle_stop_background_polling`.
    async fn handle_start_background_polling(&self, device_id: &str, interval_secs: u64) {
        info!(device_id, interval_secs, "Starting background polling");

        // Check if already polling this device
        {
            let polling = self.background_polling.read().await;
            if polling.contains_key(device_id) {
                warn!(device_id, "Background polling already active for device");
                return;
            }
        }

        // Create a cancel channel
        let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);

        // Store the cancel sender
        {
            let mut polling = self.background_polling.write().await;
            polling.insert(device_id.to_string(), cancel_tx);
        }

        // Clone necessary data for the spawned task
        let device_id_owned = device_id.to_string();
        let event_tx = self.event_tx.clone();
        let config = self.connection_config.clone();
        let signal_quality_cache = self.signal_quality_cache.clone();
        let store_path = self.store_path.clone();
        let polling_interval = Duration::from_secs(interval_secs);

        // Notify UI that polling has started
        let _ = event_tx
            .send(SensorEvent::BackgroundPollingStarted {
                device_id: device_id.to_string(),
                interval_secs,
            })
            .await;

        // Spawn the polling task
        tokio::spawn(async move {
            let mut interval_timer = interval(polling_interval);
            // Skip the first immediate tick
            interval_timer.tick().await;

            loop {
                tokio::select! {
                    _ = cancel_rx.changed() => {
                        if *cancel_rx.borrow() {
                            info!(device_id = %device_id_owned, "Background polling cancelled");
                            break;
                        }
                    }
                    _ = interval_timer.tick() => {
                        debug!(device_id = %device_id_owned, "Background poll tick");

                        // Get cached signal quality for adaptive behavior
                        let signal_quality = signal_quality_cache.read().await.get(&device_id_owned).copied();

                        // Use more aggressive retries for devices with known poor signal
                        let retry_config = match signal_quality {
                            Some(SignalQuality::Poor) | Some(SignalQuality::Fair) => {
                                RetryConfig::aggressive()
                            }
                            _ => RetryConfig::for_read(),
                        };

                        // Attempt to read
                        match with_retry(&retry_config, "background_poll", || {
                            let device_id = device_id_owned.clone();
                            let config = config.clone();
                            async move {
                                Self::connect_and_read_with_config(&device_id, config).await
                            }
                        })
                        .await
                        {
                            Ok((_, _, reading, _, rssi, new_signal_quality)) => {
                                // Update cached signal quality
                                if let Some(quality) = new_signal_quality {
                                    signal_quality_cache
                                        .write()
                                        .await
                                        .insert(device_id_owned.clone(), quality);
                                }

                                // Send signal strength update if available
                                if let Some(rssi_val) = rssi {
                                    let _ = event_tx
                                        .send(SensorEvent::SignalStrengthUpdate {
                                            device_id: device_id_owned.clone(),
                                            rssi: rssi_val,
                                            quality: aranet_core::messages::SignalQuality::from_rssi(rssi_val),
                                        })
                                        .await;
                                }

                                if let Some(reading) = reading {
                                    debug!(device_id = %device_id_owned, "Background poll successful");

                                    // Save to store
                                    if let Ok(store) = Store::open(&store_path) {
                                        if let Err(e) = store.insert_reading(&device_id_owned, &reading) {
                                            warn!(device_id = %device_id_owned, error = %e, "Failed to save background reading to store");
                                        }
                                    }

                                    // Send reading update
                                    let _ = event_tx
                                        .send(SensorEvent::ReadingUpdated {
                                            device_id: device_id_owned.clone(),
                                            reading,
                                        })
                                        .await;
                                }
                            }
                            Err(e) => {
                                warn!(device_id = %device_id_owned, error = %e, "Background poll failed");
                                let context = ErrorContext::from_error(&e);
                                let _ = event_tx
                                    .send(SensorEvent::ReadingError {
                                        device_id: device_id_owned.clone(),
                                        error: e.to_string(),
                                        context: Some(context),
                                    })
                                    .await;
                            }
                        }
                    }
                }
            }

            // Notify UI that polling has stopped
            let _ = event_tx
                .send(SensorEvent::BackgroundPollingStopped {
                    device_id: device_id_owned,
                })
                .await;
        });

        info!(device_id, "Background polling task spawned");
    }

    /// Cancel any currently running long-running operation (scan, connect, history sync).
    ///
    /// This method cancels the current cancellation token and creates a new one
    /// for future operations.
    async fn handle_cancel_operation(&mut self) {
        info!("Cancelling current operation");

        // Cancel the current token
        self.cancel_token.cancel();

        // Create a new token for future operations
        self.cancel_token = CancellationToken::new();

        // Notify the UI that the operation was cancelled
        let _ = self
            .event_tx
            .send(SensorEvent::OperationCancelled {
                operation: "Current operation".to_string(),
            })
            .await;
    }

    /// Forget (remove) a device from the store and stop any associated polling.
    async fn handle_forget_device(&self, device_id: &str) {
        info!(device_id, "Forgetting device");

        // Stop any background polling for this device
        {
            let mut polling = self.background_polling.write().await;
            if let Some(cancel_tx) = polling.remove(device_id) {
                let _ = cancel_tx.send(true);
                info!(device_id, "Stopped background polling for forgotten device");
            }
        }

        // Clear signal quality cache for this device
        {
            let mut cache = self.signal_quality_cache.write().await;
            cache.remove(device_id);
        }

        // Try to delete from the store
        let Some(store) = self.open_store() else {
            let _ = self
                .event_tx
                .send(SensorEvent::ForgetDeviceError {
                    device_id: device_id.to_string(),
                    error: "Could not open database".to_string(),
                })
                .await;
            return;
        };

        match store.delete_device(device_id) {
            Ok(deleted) => {
                if deleted {
                    info!(device_id, "Device forgotten (deleted from store)");
                } else {
                    info!(
                        device_id,
                        "Device not found in store (removing from UI only)"
                    );
                }
                let _ = self
                    .event_tx
                    .send(SensorEvent::DeviceForgotten {
                        device_id: device_id.to_string(),
                    })
                    .await;
            }
            Err(e) => {
                error!(device_id, error = %e, "Failed to forget device");
                let _ = self
                    .event_tx
                    .send(SensorEvent::ForgetDeviceError {
                        device_id: device_id.to_string(),
                        error: e.to_string(),
                    })
                    .await;
            }
        }
    }

    /// Stop background polling for a device.
    async fn handle_stop_background_polling(&self, device_id: &str) {
        info!(device_id, "Stopping background polling");

        let mut polling = self.background_polling.write().await;
        if let Some(cancel_tx) = polling.remove(device_id) {
            // Signal the task to stop
            let _ = cancel_tx.send(true);
            info!(device_id, "Background polling stop signal sent");
        } else {
            warn!(device_id, "No active background polling found for device");
        }
    }
}
