//! Background worker for BLE sensor operations.
//!
//! This module contains the [`SensorWorker`] which handles all Bluetooth Low Energy
//! operations in a background task, keeping the UI thread responsive.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aranet_core::messages::{
    CachedDevice, Command, ErrorContext, SensorEvent, ServiceDeviceStats, ServiceMonitoredDevice,
    SignalQuality,
};
use aranet_core::retry::{RetryConfig, with_retry};
use aranet_core::scan::scan_with_options;
use aranet_core::service_client::ServiceClient;
use aranet_core::settings::{DeviceSettings, MeasurementInterval};
use aranet_core::{BluetoothRange, Device, ScanOptions};
use aranet_store::Store;
use aranet_types::{CurrentReading, DeviceType};
use futures::future::join_all;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Maximum time to wait for a BLE connect-and-read operation.
const CONNECT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time to wait for history download (5 minutes for large histories).
const HISTORY_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Default URL for the aranet-service.
const DEFAULT_SERVICE_URL: &str = "http://localhost:8080";

/// Retry configuration for BLE operations.
fn default_retry_config() -> RetryConfig {
    RetryConfig {
        max_retries: 2,
        initial_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(5),
        backoff_multiplier: 2.0,
        jitter: true,
    }
}

/// Background polling task handle.
struct PollingTask {
    cancel_token: CancellationToken,
    /// Stored for potential future status reporting.
    #[allow(dead_code)]
    interval_secs: u64,
}

/// Circuit breaker state for service calls.
///
/// Prevents repeated failed calls by "opening" the circuit after too many failures,
/// then periodically testing if the service is available again.
#[derive(Debug)]
struct CircuitBreaker {
    /// Current state of the circuit.
    state: CircuitState,
    /// Number of consecutive failures.
    failure_count: u32,
    /// Number of failures before opening the circuit.
    failure_threshold: u32,
    /// When the circuit entered open state (for timeout tracking).
    opened_at: Option<std::time::Instant>,
    /// How long to wait before trying again (half-open state).
    recovery_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitState {
    /// Circuit is closed - calls go through normally.
    Closed,
    /// Circuit is open - calls are blocked.
    Open,
    /// Circuit is testing - one call allowed to test recovery.
    HalfOpen,
}

impl CircuitBreaker {
    fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            failure_threshold: 3,
            opened_at: None,
            recovery_timeout: Duration::from_secs(30),
        }
    }

    /// Check if a call should be allowed.
    fn should_allow(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check if recovery timeout has passed
                if let Some(opened_at) = self.opened_at {
                    if opened_at.elapsed() >= self.recovery_timeout {
                        self.state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful call.
    fn record_success(&mut self) {
        self.failure_count = 0;
        self.state = CircuitState::Closed;
        self.opened_at = None;
    }

    /// Record a failed call.
    fn record_failure(&mut self) {
        self.failure_count += 1;
        if self.failure_count >= self.failure_threshold {
            self.state = CircuitState::Open;
            self.opened_at = Some(std::time::Instant::now());
        }
    }

    /// Check if the circuit is open (blocking calls).
    fn is_open(&self) -> bool {
        self.state == CircuitState::Open
    }

    /// Get the time until the circuit will try again.
    fn time_until_retry(&self) -> Option<Duration> {
        if self.state == CircuitState::Open {
            if let Some(opened_at) = self.opened_at {
                let elapsed = opened_at.elapsed();
                if elapsed < self.recovery_timeout {
                    return Some(self.recovery_timeout - elapsed);
                }
            }
        }
        None
    }
}

/// Background worker that handles BLE operations.
pub struct SensorWorker {
    command_rx: mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<SensorEvent>,
    store_path: PathBuf,
    /// Cached store connection (opened lazily, kept alive).
    store: Option<Store>,
    /// Service client for aranet-service communication.
    service_client: Option<ServiceClient>,
    /// The service URL being used (for error messages).
    service_url: String,
    /// Cancellation token for long-running operations.
    cancel_token: CancellationToken,
    /// Background polling tasks per device.
    polling_tasks: HashMap<String, PollingTask>,
    /// Circuit breaker for service calls.
    service_circuit_breaker: CircuitBreaker,
}

impl SensorWorker {
    /// Create a new sensor worker with store integration.
    pub fn new(
        command_rx: mpsc::Receiver<Command>,
        event_tx: mpsc::Sender<SensorEvent>,
        store_path: PathBuf,
    ) -> Self {
        Self::with_service_url(command_rx, event_tx, store_path, DEFAULT_SERVICE_URL)
    }

    /// Create a new sensor worker with a custom service URL.
    pub fn with_service_url(
        command_rx: mpsc::Receiver<Command>,
        event_tx: mpsc::Sender<SensorEvent>,
        store_path: PathBuf,
        service_url: &str,
    ) -> Self {
        // Try to create service client, logging any errors
        let service_client = match ServiceClient::new(service_url) {
            Ok(client) => {
                info!(url = service_url, "Service client initialized");
                Some(client)
            }
            Err(e) => {
                warn!(
                    url = service_url,
                    error = %e,
                    "Failed to initialize service client - service features will be unavailable"
                );
                None
            }
        };

        Self {
            command_rx,
            event_tx,
            store_path,
            store: None,
            service_client,
            service_url: service_url.to_string(),
            cancel_token: CancellationToken::new(),
            polling_tasks: HashMap::new(),
            service_circuit_breaker: CircuitBreaker::new(),
        }
    }

    /// Get or open the store connection.
    fn get_store(&mut self) -> Option<&Store> {
        if self.store.is_none() {
            match Store::open(&self.store_path) {
                Ok(store) => {
                    debug!("Opened store connection");
                    self.store = Some(store);
                }
                Err(e) => {
                    warn!(error = %e, "Failed to open store");
                    return None;
                }
            }
        }
        self.store.as_ref()
    }

    /// Get a mutable reference to the store.
    fn get_store_mut(&mut self) -> Option<&mut Store> {
        if self.store.is_none() {
            match Store::open(&self.store_path) {
                Ok(store) => {
                    debug!("Opened store connection");
                    self.store = Some(store);
                }
                Err(e) => {
                    warn!(error = %e, "Failed to open store");
                    return None;
                }
            }
        }
        self.store.as_mut()
    }

    /// Send an event to the UI, logging any send failures.
    async fn send_event(&self, event: SensorEvent) {
        if let Err(e) = self.event_tx.send(event).await {
            error!("Failed to send event to UI: {}", e);
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

        // Clean up: cancel all background polling tasks
        for (device_id, task) in self.polling_tasks.drain() {
            info!(device_id, "Cancelling background polling on shutdown");
            task.cancel_token.cancel();
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
            Command::Shutdown => {} // Handled in run() loop
            Command::InstallSystemService { user_level } => {
                self.handle_install_system_service(user_level).await;
            }
            Command::UninstallSystemService { user_level } => {
                self.handle_uninstall_system_service(user_level).await;
            }
            Command::StartSystemService { user_level } => {
                self.handle_start_system_service(user_level).await;
            }
            Command::StopSystemService { user_level } => {
                self.handle_stop_system_service(user_level).await;
            }
            Command::CheckSystemServiceStatus { user_level } => {
                self.handle_check_system_service_status(user_level).await;
            }
            Command::FetchServiceConfig => {
                self.handle_fetch_service_config().await;
            }
            Command::AddServiceDevice {
                address,
                alias,
                poll_interval,
            } => {
                self.handle_add_service_device(&address, alias, poll_interval)
                    .await;
            }
            Command::UpdateServiceDevice {
                address,
                alias,
                poll_interval,
            } => {
                self.handle_update_service_device(&address, alias, poll_interval)
                    .await;
            }
            Command::RemoveServiceDevice { address } => {
                self.handle_remove_service_device(&address).await;
            }
        }
    }

    /// Cancel any ongoing long-running operation.
    async fn handle_cancel_operation(&mut self) {
        info!("Cancelling current operation");
        self.cancel_token.cancel();
        // Create a new token for future operations
        self.cancel_token = CancellationToken::new();
        self.send_event(SensorEvent::OperationCancelled {
            operation: "Current operation".to_string(),
        })
        .await;
    }

    async fn handle_load_cached_data(&mut self) {
        info!("Loading cached data from store");
        let Some(store) = self.get_store() else {
            self.send_event(SensorEvent::CachedDataLoaded { devices: vec![] })
                .await;
            return;
        };

        let stored_devices = match store.list_devices() {
            Ok(devices) => devices,
            Err(e) => {
                warn!("Failed to list devices: {}", e);
                self.send_event(SensorEvent::CachedDataLoaded { devices: vec![] })
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

        self.send_event(SensorEvent::CachedDataLoaded {
            devices: cached_devices,
        })
        .await;

        // Load history for each cached device (so history is visible on startup)
        for device_id in device_ids {
            self.load_and_send_history(&device_id).await;
        }
    }

    /// Load history from store and send to UI.
    async fn load_and_send_history(&mut self, device_id: &str) {
        let Some(store) = self.get_store() else {
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

                self.send_event(SensorEvent::HistoryLoaded {
                    device_id: device_id.to_string(),
                    records,
                })
                .await;
            }
            Err(e) => {
                warn!(device_id, error = %e, "Failed to query history from store");
            }
        }
    }

    async fn handle_scan(&mut self, duration: Duration) {
        self.send_event(SensorEvent::ScanStarted).await;

        let cancel_token = self.cancel_token.clone();
        let options = ScanOptions::default().duration(duration);

        // Run scan with cancellation support
        let scan_result = tokio::select! {
            result = scan_with_options(options) => result,
            _ = cancel_token.cancelled() => {
                info!("Scan cancelled by user");
                self.send_event(SensorEvent::OperationCancelled {
                    operation: "Device scan".to_string(),
                }).await;
                return;
            }
        };

        match scan_result {
            Ok(devices) => {
                self.save_discovered_devices(&devices);
                self.send_event(SensorEvent::ScanComplete { devices }).await;
            }
            Err(e) => {
                let context = ErrorContext::from_error(&e);
                warn!(error = %e, "Scan failed");
                self.send_event(SensorEvent::ScanError {
                    error: context.message,
                })
                .await;
            }
        }
    }

    async fn handle_connect(&mut self, device_id: &str) {
        self.send_event(SensorEvent::DeviceConnecting {
            device_id: device_id.to_string(),
        })
        .await;

        let cancel_token = self.cancel_token.clone();
        let device_id_owned = device_id.to_string();

        let connect_result = tokio::select! {
            result = timeout(CONNECT_READ_TIMEOUT, self.connect_and_read_with_retry(&device_id_owned)) => result,
            _ = cancel_token.cancelled() => {
                info!(device_id, "Connection cancelled by user");
                self.send_event(SensorEvent::OperationCancelled {
                    operation: format!("Connect to {}", device_id),
                }).await;
                return;
            }
        };

        match connect_result {
            Ok(Ok((name, device_type, reading, settings, rssi))) => {
                self.save_device_connection(device_id, name.as_deref(), device_type);

                // Send signal strength update if available
                if let Some(rssi) = rssi {
                    let quality = SignalQuality::from_rssi(rssi);
                    self.send_event(SensorEvent::SignalStrengthUpdate {
                        device_id: device_id.to_string(),
                        rssi,
                        quality,
                    })
                    .await;
                }

                self.send_event(SensorEvent::DeviceConnected {
                    device_id: device_id.to_string(),
                    name,
                    device_type,
                    rssi,
                })
                .await;

                // Send settings if we got them
                if let Some(settings) = settings {
                    self.send_event(SensorEvent::SettingsLoaded {
                        device_id: device_id.to_string(),
                        settings,
                    })
                    .await;
                }

                // Send reading if we got one and save to store
                if let Some(ref reading) = reading {
                    self.save_reading(device_id, reading);
                    self.send_event(SensorEvent::ReadingUpdated {
                        device_id: device_id.to_string(),
                        reading: *reading,
                    })
                    .await;
                }

                // Load history for display
                self.load_and_send_history(device_id).await;
            }
            Ok(Err(e)) => {
                let context = ErrorContext::from_error(&e);
                warn!(device_id, error = %e, "Connection failed");
                self.send_event(SensorEvent::ConnectionError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
            }
            Err(_) => {
                let context = ErrorContext::transient(
                    "Connection timed out",
                    "The device may be out of range or busy. Try moving closer.",
                );
                self.send_event(SensorEvent::ConnectionError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
            }
        }
    }

    async fn handle_disconnect(&self, device_id: &str) {
        self.send_event(SensorEvent::DeviceDisconnected {
            device_id: device_id.to_string(),
        })
        .await;
    }

    async fn handle_refresh(&mut self, device_id: &str) {
        let cancel_token = self.cancel_token.clone();
        let device_id_owned = device_id.to_string();

        let refresh_result = tokio::select! {
            result = timeout(CONNECT_READ_TIMEOUT, self.connect_and_read_with_retry(&device_id_owned)) => result,
            _ = cancel_token.cancelled() => {
                info!(device_id, "Refresh cancelled by user");
                self.send_event(SensorEvent::OperationCancelled {
                    operation: format!("Refresh {}", device_id),
                }).await;
                return;
            }
        };

        match refresh_result {
            Ok(Ok((_, _, reading, settings, rssi))) => {
                // Send signal strength update if available
                if let Some(rssi) = rssi {
                    let quality = SignalQuality::from_rssi(rssi);
                    self.send_event(SensorEvent::SignalStrengthUpdate {
                        device_id: device_id.to_string(),
                        rssi,
                        quality,
                    })
                    .await;
                }

                // Send settings if we got them
                if let Some(settings) = settings {
                    self.send_event(SensorEvent::SettingsLoaded {
                        device_id: device_id.to_string(),
                        settings,
                    })
                    .await;
                }

                if let Some(reading) = reading {
                    self.save_reading(device_id, &reading);
                    self.send_event(SensorEvent::ReadingUpdated {
                        device_id: device_id.to_string(),
                        reading,
                    })
                    .await;
                } else {
                    let context = ErrorContext::transient(
                        "Failed to read current values",
                        "The device responded but didn't send valid data. Try again.",
                    );
                    self.send_event(SensorEvent::ReadingError {
                        device_id: device_id.to_string(),
                        error: context.message.clone(),
                        context: Some(context),
                    })
                    .await;
                }
            }
            Ok(Err(e)) => {
                let context = ErrorContext::from_error(&e);
                warn!(device_id, error = %e, "Refresh failed");
                self.send_event(SensorEvent::ReadingError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
            }
            Err(_) => {
                let context = ErrorContext::transient(
                    format!(
                        "Refresh timed out after {}s",
                        CONNECT_READ_TIMEOUT.as_secs()
                    ),
                    "The device may be out of range. Try moving closer.",
                );
                self.send_event(SensorEvent::ReadingError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
            }
        }
    }

    /// Refresh readings for all known devices in parallel.
    async fn handle_refresh_all(&mut self) {
        info!("Refreshing all devices in parallel");

        // Get list of known devices
        let device_ids: Vec<String> = {
            let Some(store) = self.get_store() else {
                return;
            };

            match store.list_devices() {
                Ok(devices) => devices.into_iter().map(|d| d.id).collect(),
                Err(e) => {
                    warn!("Failed to list devices for refresh all: {}", e);
                    return;
                }
            }
        };

        if device_ids.is_empty() {
            info!("No devices to refresh");
            return;
        }

        // Create futures for all device refreshes
        let event_tx = self.event_tx.clone();
        let cancel_token = self.cancel_token.clone();
        let store_path = self.store_path.clone();

        let refresh_futures: Vec<_> = device_ids
            .into_iter()
            .map(|device_id| {
                let event_tx = event_tx.clone();
                let cancel_token = cancel_token.clone();
                let store_path = store_path.clone();
                async move {
                    refresh_single_device(&device_id, &event_tx, &cancel_token, &store_path).await;
                }
            })
            .collect();

        // Execute all refreshes in parallel
        join_all(refresh_futures).await;

        info!("Completed refreshing all devices");
    }

    /// Sync history from device (download via BLE and save to store).
    ///
    /// Uses incremental sync - only downloads new records since the last sync.
    async fn handle_sync_history(&mut self, device_id: &str) {
        use aranet_core::history::HistoryOptions;

        info!(device_id, "Syncing history from device");

        let cancel_token = self.cancel_token.clone();

        // Verify store is accessible before starting sync
        if self.get_store().is_none() {
            self.send_event(SensorEvent::HistorySyncError {
                device_id: device_id.to_string(),
                error: "Failed to open store".to_string(),
                context: Some(ErrorContext::permanent("Failed to open store")),
            })
            .await;
            return;
        }

        // Notify UI that sync is starting
        self.send_event(SensorEvent::HistorySyncStarted {
            device_id: device_id.to_string(),
            total_records: None, // Will be updated after connecting
        })
        .await;

        // Connect to the device with retry
        let device = match with_retry(&default_retry_config(), "connect for history", || async {
            Device::connect(device_id).await
        })
        .await
        {
            Ok(d) => d,
            Err(e) => {
                let context = ErrorContext::from_error(&e);
                error!(device_id, error = %e, "Failed to connect for history sync");
                self.send_event(SensorEvent::HistorySyncError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
                return;
            }
        };

        // Get history info to know how many records are on the device
        let history_info = match device.get_history_info().await {
            Ok(info) => info,
            Err(e) => {
                let context = ErrorContext::from_error(&e);
                error!(device_id, error = %e, "Failed to get history info");
                let _ = device.disconnect().await;
                self.send_event(SensorEvent::HistorySyncError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
                return;
            }
        };

        let total_on_device = history_info.total_readings;

        // Calculate start index for incremental sync
        let start_index = {
            let Some(store) = self.get_store() else {
                let _ = device.disconnect().await;
                return;
            };

            match store.calculate_sync_start(device_id, total_on_device) {
                Ok(idx) => idx,
                Err(e) => {
                    warn!(device_id, error = %e, "Failed to calculate sync start, doing full sync");
                    1u16
                }
            }
        };

        // Check if already up to date
        if start_index > total_on_device {
            info!(device_id, "Already up to date, no new readings to sync");
            let _ = device.disconnect().await;
            self.send_event(SensorEvent::HistorySynced {
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

        // Update UI with total records to download
        self.send_event(SensorEvent::HistorySyncStarted {
            device_id: device_id.to_string(),
            total_records: Some(records_to_download),
        })
        .await;

        // Create progress callback
        let event_tx = self.event_tx.clone();
        let device_id_for_progress = device_id.to_string();
        let total_for_progress = records_to_download as usize;

        let progress_callback = Arc::new(move |progress: aranet_core::history::HistoryProgress| {
            let event_tx = event_tx.clone();
            let device_id = device_id_for_progress.clone();
            let downloaded = progress.values_downloaded;
            // Send progress event (fire and forget since we're in a sync callback)
            tokio::spawn(async move {
                let _ = event_tx
                    .send(SensorEvent::HistorySyncProgress {
                        device_id,
                        downloaded,
                        total: total_for_progress,
                    })
                    .await;
            });
        });

        // Download history with start_index for incremental sync
        let history_options = HistoryOptions {
            start_index: Some(start_index),
            end_index: None, // Download to the end
            progress_callback: Some(progress_callback),
            ..Default::default()
        };

        // Download with timeout and cancellation support
        let download_result = tokio::select! {
            result = timeout(HISTORY_DOWNLOAD_TIMEOUT, device.download_history_with_options(history_options)) => result,
            _ = cancel_token.cancelled() => {
                info!(device_id, "History sync cancelled by user");
                let _ = device.disconnect().await;
                self.send_event(SensorEvent::OperationCancelled {
                    operation: format!("History sync for {}", device_id),
                }).await;
                return;
            }
        };

        let records = match download_result {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                let context = ErrorContext::from_error(&e);
                error!(device_id, error = %e, "Failed to download history");
                let _ = device.disconnect().await;
                self.send_event(SensorEvent::HistorySyncError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
                return;
            }
            Err(_) => {
                let context = ErrorContext::transient(
                    format!(
                        "History download timed out after {}s",
                        HISTORY_DOWNLOAD_TIMEOUT.as_secs()
                    ),
                    "Large history downloads can take time. Try again or sync in smaller batches.",
                );
                let _ = device.disconnect().await;
                self.send_event(SensorEvent::HistorySyncError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
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

        // Disconnect from device
        if let Err(e) = device.disconnect().await {
            warn!(device_id, error = %e, "Failed to disconnect after history sync");
        }

        // Insert history to store (with deduplication)
        {
            let Some(store) = self.get_store_mut() else {
                return;
            };

            match store.insert_history(device_id, &records) {
                Ok(inserted) => {
                    debug!(
                        device_id,
                        downloaded = record_count,
                        inserted,
                        "History saved to store"
                    );

                    // Update sync state for next incremental sync
                    if let Err(e) =
                        store.update_sync_state(device_id, total_on_device, total_on_device)
                    {
                        warn!(device_id, error = %e, "Failed to update sync state");
                    }
                }
                Err(e) => {
                    warn!(device_id, error = %e, "Failed to save history to store - sync state not updated");
                }
            }
        }

        // Notify UI that sync is complete
        self.send_event(SensorEvent::HistorySynced {
            device_id: device_id.to_string(),
            count: record_count,
        })
        .await;

        // Send history to UI for display
        self.load_and_send_history(device_id).await;
    }

    /// Connect to device and read data with automatic retry on transient failures.
    async fn connect_and_read_with_retry(
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
        with_retry(&default_retry_config(), "connect_and_read", || async {
            self.connect_and_read(device_id).await
        })
        .await
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
            Option<i16>,
        ),
        aranet_core::Error,
    > {
        let device = Device::connect(device_id).await?;
        let name = device.name().map(String::from);
        let device_type = device.device_type();

        // Read current values, logging any failures
        let reading = match device.read_current().await {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(device_id, error = %e, "Failed to read current values");
                None
            }
        };

        // Read settings, logging any failures
        let settings = match device.get_settings().await {
            Ok(s) => Some(s),
            Err(e) => {
                debug!(device_id, error = %e, "Failed to read settings (non-critical)");
                None
            }
        };

        // Read RSSI, logging any failures
        let rssi = match device.read_rssi().await {
            Ok(r) => Some(r),
            Err(e) => {
                debug!(device_id, error = %e, "Failed to read RSSI (non-critical)");
                None
            }
        };

        if let Err(e) = device.disconnect().await {
            warn!(device_id, error = %e, "Failed to disconnect");
        }

        Ok((name, device_type, reading, settings, rssi))
    }

    async fn handle_set_interval(&mut self, device_id: &str, interval_secs: u16) {
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
                let context = aranet_core::messages::ErrorContext::permanent(&error);
                self.send_event(SensorEvent::IntervalError {
                    device_id: device_id.to_string(),
                    error,
                    context: Some(context),
                })
                .await;
                return;
            }
        };

        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to connect for set interval: {}", e);
                let context = aranet_core::messages::ErrorContext::from_error(&e);
                self.send_event(SensorEvent::IntervalError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                    context: Some(context),
                })
                .await;
                return;
            }
        };

        if let Err(e) = device.set_interval(interval).await {
            warn!("Failed to set interval: {}", e);
            if let Err(disconnect_err) = device.disconnect().await {
                warn!(
                    "Failed to disconnect after interval error: {}",
                    disconnect_err
                );
            }
            let context = aranet_core::messages::ErrorContext::from_error(&e);
            self.send_event(SensorEvent::IntervalError {
                device_id: device_id.to_string(),
                error: e.to_string(),
                context: Some(context),
            })
            .await;
            return;
        }

        if let Err(e) = device.disconnect().await {
            warn!("Failed to disconnect after setting interval: {}", e);
        }

        info!("Measurement interval set successfully for {}", device_id);
        self.send_event(SensorEvent::IntervalChanged {
            device_id: device_id.to_string(),
            interval_secs,
        })
        .await;
    }

    async fn handle_set_bluetooth_range(&mut self, device_id: &str, extended: bool) {
        let range_name = if extended { "Extended" } else { "Standard" };
        info!(
            "Setting Bluetooth range for {} to {}",
            device_id, range_name
        );

        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to connect for set Bluetooth range: {}", e);
                let context = aranet_core::messages::ErrorContext::from_error(&e);
                self.send_event(SensorEvent::BluetoothRangeError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                    context: Some(context),
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
            if let Err(disconnect_err) = device.disconnect().await {
                warn!("Failed to disconnect after range error: {}", disconnect_err);
            }
            let context = aranet_core::messages::ErrorContext::from_error(&e);
            self.send_event(SensorEvent::BluetoothRangeError {
                device_id: device_id.to_string(),
                error: e.to_string(),
                context: Some(context),
            })
            .await;
            return;
        }

        if let Err(e) = device.disconnect().await {
            warn!("Failed to disconnect after setting Bluetooth range: {}", e);
        }

        info!("Bluetooth range set successfully for {}", device_id);
        self.send_event(SensorEvent::BluetoothRangeChanged {
            device_id: device_id.to_string(),
            extended,
        })
        .await;
    }

    async fn handle_set_smart_home(&mut self, device_id: &str, enabled: bool) {
        let mode = if enabled { "enabled" } else { "disabled" };
        info!("Setting Smart Home for {} to {}", device_id, mode);

        let device = match Device::connect(device_id).await {
            Ok(d) => d,
            Err(e) => {
                warn!("Failed to connect for set Smart Home: {}", e);
                let context = aranet_core::messages::ErrorContext::from_error(&e);
                self.send_event(SensorEvent::SmartHomeError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                    context: Some(context),
                })
                .await;
                return;
            }
        };

        if let Err(e) = device.set_smart_home(enabled).await {
            warn!("Failed to set Smart Home: {}", e);
            if let Err(disconnect_err) = device.disconnect().await {
                warn!(
                    "Failed to disconnect after Smart Home error: {}",
                    disconnect_err
                );
            }
            let context = aranet_core::messages::ErrorContext::from_error(&e);
            self.send_event(SensorEvent::SmartHomeError {
                device_id: device_id.to_string(),
                error: e.to_string(),
                context: Some(context),
            })
            .await;
            return;
        }

        if let Err(e) = device.disconnect().await {
            warn!("Failed to disconnect after setting Smart Home: {}", e);
        }

        info!("Smart Home set successfully for {}", device_id);
        self.send_event(SensorEvent::SmartHomeChanged {
            device_id: device_id.to_string(),
            enabled,
        })
        .await;
    }

    // -------------------------------------------------------------------------
    // Background Polling Methods
    // -------------------------------------------------------------------------

    async fn handle_start_background_polling(&mut self, device_id: &str, interval_secs: u64) {
        // Stop any existing polling for this device
        if let Some(existing) = self.polling_tasks.remove(device_id) {
            existing.cancel_token.cancel();
        }

        let cancel_token = CancellationToken::new();
        let task_token = cancel_token.clone();
        let event_tx = self.event_tx.clone();
        let store_path = self.store_path.clone();
        let device_id_owned = device_id.to_string();

        // Spawn background polling task
        tokio::spawn(async move {
            background_polling_task(
                device_id_owned,
                interval_secs,
                event_tx,
                store_path,
                task_token,
            )
            .await;
        });

        self.polling_tasks.insert(
            device_id.to_string(),
            PollingTask {
                cancel_token,
                interval_secs,
            },
        );

        info!(device_id, interval_secs, "Started background polling");
        self.send_event(SensorEvent::BackgroundPollingStarted {
            device_id: device_id.to_string(),
            interval_secs,
        })
        .await;
    }

    async fn handle_stop_background_polling(&mut self, device_id: &str) {
        if let Some(task) = self.polling_tasks.remove(device_id) {
            task.cancel_token.cancel();
            info!(device_id, "Stopped background polling");
            self.send_event(SensorEvent::BackgroundPollingStopped {
                device_id: device_id.to_string(),
            })
            .await;
        } else {
            debug!(device_id, "No background polling task to stop");
        }
    }

    // -------------------------------------------------------------------------
    // Store Helper Methods
    // -------------------------------------------------------------------------

    fn save_reading(&mut self, device_id: &str, reading: &CurrentReading) {
        let Some(store) = self.get_store_mut() else {
            return;
        };

        if let Err(e) = store.insert_reading(device_id, reading) {
            warn!(device_id, error = %e, "Failed to save reading to store");
        } else {
            debug!(device_id, "Reading saved to store");
        }
    }

    fn save_discovered_devices(&mut self, devices: &[aranet_core::DiscoveredDevice]) {
        let Some(store) = self.get_store_mut() else {
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
        &mut self,
        device_id: &str,
        name: Option<&str>,
        device_type: Option<DeviceType>,
    ) {
        let Some(store) = self.get_store_mut() else {
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

    // -------------------------------------------------------------------------
    // Service Handler Methods
    // -------------------------------------------------------------------------

    /// Handle refreshing the aranet-service status.
    async fn handle_refresh_service_status(&mut self) {
        info!("Refreshing service status");

        // Check circuit breaker
        if !self.service_circuit_breaker.should_allow() {
            if let Some(retry_in) = self.service_circuit_breaker.time_until_retry() {
                self.send_event(SensorEvent::ServiceStatusRefreshed {
                    reachable: false,
                    collector_running: false,
                    uptime_seconds: None,
                    devices: vec![],
                })
                .await;
                debug!(
                    "Circuit breaker open, skipping service call (retry in {:?})",
                    retry_in
                );
                return;
            }
        }

        let Some(ref client) = self.service_client else {
            self.send_event(SensorEvent::ServiceStatusError {
                error: format!(
                    "Service client not initialized. Check if the service URL '{}' is valid.",
                    self.service_url
                ),
            })
            .await;
            return;
        };

        match client.status().await {
            Ok(status) => {
                // Record success - close circuit if it was open
                self.service_circuit_breaker.record_success();

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

                self.send_event(SensorEvent::ServiceStatusRefreshed {
                    reachable: true,
                    collector_running: status.collector.running,
                    uptime_seconds: status.collector.uptime_seconds,
                    devices,
                })
                .await;
            }
            Err(e) => {
                // Record failure - may open circuit
                self.service_circuit_breaker.record_failure();

                use aranet_core::service_client::ServiceClientError;

                // Provide user-friendly error messages based on error type
                let error_msg = match &e {
                    ServiceClientError::NotReachable { url, .. } => {
                        format!(
                            "Service not reachable at {}. Run 'aranet-service run' to start it.",
                            url
                        )
                    }
                    ServiceClientError::InvalidUrl(url) => {
                        format!("Invalid service URL: '{}'. Check your configuration.", url)
                    }
                    ServiceClientError::ApiError { status, message } => match *status {
                        401 => "Authentication required. Check your API key.".to_string(),
                        403 => "Access denied. Check your API key permissions.".to_string(),
                        404 => "Service endpoint not found. The service may be an older version."
                            .to_string(),
                        500..=599 => format!("Service error ({}): {}", status, message),
                        _ => format!("API error ({}): {}", status, message),
                    },
                    ServiceClientError::Request(req_err) => {
                        if req_err.is_timeout() {
                            "Request timed out. The service may be overloaded.".to_string()
                        } else if req_err.is_connect() {
                            "Connection failed. The service may not be running.".to_string()
                        } else {
                            format!("Request failed: {}", req_err)
                        }
                    }
                };

                warn!(error = %error_msg, "Failed to refresh service status");

                // Log circuit breaker state
                if self.service_circuit_breaker.is_open() {
                    warn!(
                        "Circuit breaker opened after {} failures - service calls will be blocked for {:?}",
                        self.service_circuit_breaker.failure_count,
                        self.service_circuit_breaker.recovery_timeout
                    );
                }

                // Send partial status with reachable=false so UI can show "not running"
                self.send_event(SensorEvent::ServiceStatusRefreshed {
                    reachable: false,
                    collector_running: false,
                    uptime_seconds: None,
                    devices: vec![],
                })
                .await;
            }
        }
    }

    /// Handle starting the aranet-service collector.
    async fn handle_start_service_collector(&mut self) {
        use std::time::Duration;

        info!("Starting service collector");

        let Some(ref client) = self.service_client else {
            self.send_event(SensorEvent::ServiceCollectorError {
                error: "Service client not initialized. Run 'aranet-service run' first, then restart the GUI.".to_string(),
            })
            .await;
            return;
        };

        // Add timeout to prevent hanging indefinitely
        const SERVICE_CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

        match tokio::time::timeout(SERVICE_CONTROL_TIMEOUT, client.start_collector()).await {
            Ok(Ok(_)) => {
                self.send_event(SensorEvent::ServiceCollectorStarted).await;
                // Refresh status to get updated state
                self.handle_refresh_service_status().await;
            }
            Ok(Err(e)) => {
                self.send_event(SensorEvent::ServiceCollectorError {
                    error: Self::format_service_error(&e),
                })
                .await;
            }
            Err(_) => {
                self.send_event(SensorEvent::ServiceCollectorError {
                    error: "Operation timed out. The service may be unresponsive.".to_string(),
                })
                .await;
            }
        }
    }

    /// Handle stopping the aranet-service collector.
    async fn handle_stop_service_collector(&mut self) {
        use std::time::Duration;

        info!("Stopping service collector");

        let Some(ref client) = self.service_client else {
            self.send_event(SensorEvent::ServiceCollectorError {
                error: "Service client not initialized. Run 'aranet-service run' first, then restart the GUI.".to_string(),
            })
            .await;
            return;
        };

        // Add timeout to prevent hanging indefinitely
        const SERVICE_CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

        match tokio::time::timeout(SERVICE_CONTROL_TIMEOUT, client.stop_collector()).await {
            Ok(Ok(_)) => {
                self.send_event(SensorEvent::ServiceCollectorStopped).await;
                // Refresh status to get updated state
                self.handle_refresh_service_status().await;
            }
            Ok(Err(e)) => {
                self.send_event(SensorEvent::ServiceCollectorError {
                    error: Self::format_service_error(&e),
                })
                .await;
            }
            Err(_) => {
                self.send_event(SensorEvent::ServiceCollectorError {
                    error: "Operation timed out. The service may be unresponsive.".to_string(),
                })
                .await;
            }
        }
    }

    /// Format a service client error into a user-friendly message.
    fn format_service_error(e: &aranet_core::service_client::ServiceClientError) -> String {
        use aranet_core::service_client::ServiceClientError;

        match e {
            ServiceClientError::NotReachable { url, .. } => {
                format!(
                    "Service not reachable at {}. Run 'aranet-service run' to start it.",
                    url
                )
            }
            ServiceClientError::InvalidUrl(url) => {
                format!("Invalid service URL: '{}'. Check your configuration.", url)
            }
            ServiceClientError::ApiError { status, message } => match *status {
                401 => "Authentication required. Check your API key.".to_string(),
                403 => "Access denied. Check your API key permissions.".to_string(),
                404 => "Endpoint not found. The service may be an older version.".to_string(),
                500..=599 => format!("Service error ({}): {}", status, message),
                _ => format!("API error ({}): {}", status, message),
            },
            ServiceClientError::Request(req_err) => {
                if req_err.is_timeout() {
                    "Request timed out. The service may be overloaded.".to_string()
                } else if req_err.is_connect() {
                    "Connection failed. The service may not be running.".to_string()
                } else {
                    format!("Request failed: {}", req_err)
                }
            }
        }
    }

    /// Handle installing aranet-service as a system service.
    async fn handle_install_system_service(&self, user_level: bool) {
        info!("Installing system service (user_level={})", user_level);

        let result = tokio::task::spawn_blocking(move || {
            Self::run_service_command(&["service", "install"], user_level)
        })
        .await;

        match result {
            Ok(Ok(())) => {
                self.send_event(SensorEvent::SystemServiceInstalled).await;
            }
            Ok(Err(e)) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "install".to_string(),
                    error: e,
                })
                .await;
            }
            Err(e) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "install".to_string(),
                    error: format!("Task failed: {}", e),
                })
                .await;
            }
        }
    }

    /// Handle uninstalling aranet-service system service.
    async fn handle_uninstall_system_service(&self, user_level: bool) {
        info!("Uninstalling system service (user_level={})", user_level);

        let result = tokio::task::spawn_blocking(move || {
            Self::run_service_command(&["service", "uninstall"], user_level)
        })
        .await;

        match result {
            Ok(Ok(())) => {
                self.send_event(SensorEvent::SystemServiceUninstalled).await;
            }
            Ok(Err(e)) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "uninstall".to_string(),
                    error: e,
                })
                .await;
            }
            Err(e) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "uninstall".to_string(),
                    error: format!("Task failed: {}", e),
                })
                .await;
            }
        }
    }

    /// Handle starting aranet-service system service.
    async fn handle_start_system_service(&mut self, user_level: bool) {
        info!("Starting system service (user_level={})", user_level);

        let result = tokio::task::spawn_blocking(move || {
            Self::run_service_command(&["service", "start"], user_level)
        })
        .await;

        match result {
            Ok(Ok(())) => {
                self.send_event(SensorEvent::SystemServiceStarted).await;
                // Also refresh the HTTP API status since the service should now be running
                self.handle_refresh_service_status().await;
            }
            Ok(Err(e)) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "start".to_string(),
                    error: e,
                })
                .await;
            }
            Err(e) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "start".to_string(),
                    error: format!("Task failed: {}", e),
                })
                .await;
            }
        }
    }

    /// Handle stopping aranet-service system service.
    async fn handle_stop_system_service(&mut self, user_level: bool) {
        info!("Stopping system service (user_level={})", user_level);

        let result = tokio::task::spawn_blocking(move || {
            Self::run_service_command(&["service", "stop"], user_level)
        })
        .await;

        match result {
            Ok(Ok(())) => {
                self.send_event(SensorEvent::SystemServiceStopped).await;
                // Also refresh the HTTP API status
                self.handle_refresh_service_status().await;
            }
            Ok(Err(e)) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "stop".to_string(),
                    error: e,
                })
                .await;
            }
            Err(e) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "stop".to_string(),
                    error: format!("Task failed: {}", e),
                })
                .await;
            }
        }
    }

    /// Handle checking aranet-service system service status.
    async fn handle_check_system_service_status(&self, user_level: bool) {
        info!("Checking system service status (user_level={})", user_level);

        let result =
            tokio::task::spawn_blocking(move || Self::check_service_status(user_level)).await;

        match result {
            Ok((installed, running)) => {
                self.send_event(SensorEvent::SystemServiceStatus { installed, running })
                    .await;
            }
            Err(e) => {
                self.send_event(SensorEvent::SystemServiceError {
                    operation: "status".to_string(),
                    error: format!("Task failed: {}", e),
                })
                .await;
            }
        }
    }

    /// Run an aranet-service CLI command.
    fn run_service_command(args: &[&str], user_level: bool) -> Result<(), String> {
        use std::process::Command;

        let exe = Self::find_aranet_service_exe()?;

        let mut cmd = Command::new(&exe);
        cmd.args(args);
        if user_level {
            cmd.arg("--user");
        }

        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run aranet-service: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(format!(
                "{}{}",
                if stderr.is_empty() { "" } else { &stderr },
                if stdout.is_empty() { "" } else { &stdout }
            )
            .trim()
            .to_string())
        }
    }

    /// Check if the system service is installed and running.
    fn check_service_status(user_level: bool) -> (bool, bool) {
        use std::process::Command;

        let Ok(exe) = Self::find_aranet_service_exe() else {
            return (false, false);
        };

        let mut cmd = Command::new(&exe);
        cmd.args(["service", "status"]);
        if user_level {
            cmd.arg("--user");
        }

        match cmd.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let running = stdout.contains("running");
                // If we got any output without error, it's installed
                let installed = output.status.success() || stdout.contains("stopped");
                (installed, running)
            }
            Err(_) => (false, false),
        }
    }

    /// Find the aranet-service executable.
    fn find_aranet_service_exe() -> Result<std::path::PathBuf, String> {
        // Check common install locations
        #[cfg(target_os = "macos")]
        let candidates = [
            "/usr/local/bin/aranet-service",
            "/opt/homebrew/bin/aranet-service",
        ];

        #[cfg(target_os = "linux")]
        let candidates = ["/usr/local/bin/aranet-service", "/usr/bin/aranet-service"];

        #[cfg(target_os = "windows")]
        let candidates: [&str; 0] = [];

        for path in candidates {
            let p = std::path::PathBuf::from(path);
            if p.is_file() {
                return Ok(p);
            }
        }

        // Check cargo install location
        if let Some(home) = dirs::home_dir() {
            #[cfg(not(target_os = "windows"))]
            let cargo_bin = home.join(".cargo/bin/aranet-service");

            #[cfg(target_os = "windows")]
            let cargo_bin = home.join(".cargo/bin/aranet-service.exe");

            if cargo_bin.is_file() {
                return Ok(cargo_bin);
            }
        }

        // Windows: Check Program Files
        #[cfg(target_os = "windows")]
        {
            if let Ok(pf) = std::env::var("ProgramFiles") {
                let path = std::path::PathBuf::from(pf)
                    .join("aranet-service")
                    .join("aranet-service.exe");
                if path.is_file() {
                    return Ok(path);
                }
            }
        }

        Err(
            "aranet-service executable not found. Install it with 'cargo install aranet-service'."
                .to_string(),
        )
    }

    /// Handle fetching the service configuration.
    async fn handle_fetch_service_config(&self) {
        info!("Fetching service configuration");

        let Some(ref client) = self.service_client else {
            self.send_event(SensorEvent::ServiceConfigError {
                error: "Service client not initialized".to_string(),
            })
            .await;
            return;
        };

        match client.config().await {
            Ok(config) => {
                let devices: Vec<ServiceMonitoredDevice> = config
                    .devices
                    .into_iter()
                    .map(|d| ServiceMonitoredDevice {
                        address: d.address,
                        alias: d.alias,
                        poll_interval: d.poll_interval,
                    })
                    .collect();
                self.send_event(SensorEvent::ServiceConfigFetched { devices })
                    .await;
            }
            Err(e) => {
                self.send_event(SensorEvent::ServiceConfigError {
                    error: Self::format_service_error(&e),
                })
                .await;
            }
        }
    }

    /// Handle adding a device to service monitoring.
    async fn handle_add_service_device(
        &mut self,
        address: &str,
        alias: Option<String>,
        poll_interval: u64,
    ) {
        info!("Adding device {} to service monitoring", address);

        let Some(ref client) = self.service_client else {
            self.send_event(SensorEvent::ServiceDeviceError {
                operation: "add".to_string(),
                error: "Service client not initialized".to_string(),
            })
            .await;
            return;
        };

        let device_config = aranet_core::service_client::DeviceConfig {
            address: address.to_string(),
            alias: alias.clone(),
            poll_interval,
        };

        match client.add_device(device_config).await {
            Ok(device) => {
                self.send_event(SensorEvent::ServiceDeviceAdded {
                    device: ServiceMonitoredDevice {
                        address: device.address,
                        alias: device.alias,
                        poll_interval: device.poll_interval,
                    },
                })
                .await;
                // Refresh status to show the new device
                self.handle_refresh_service_status().await;
            }
            Err(e) => {
                self.send_event(SensorEvent::ServiceDeviceError {
                    operation: "add".to_string(),
                    error: Self::format_service_error(&e),
                })
                .await;
            }
        }
    }

    /// Handle updating a device in service monitoring.
    async fn handle_update_service_device(
        &mut self,
        address: &str,
        alias: Option<String>,
        poll_interval: u64,
    ) {
        info!("Updating device {} in service monitoring", address);

        let Some(ref client) = self.service_client else {
            self.send_event(SensorEvent::ServiceDeviceError {
                operation: "update".to_string(),
                error: "Service client not initialized".to_string(),
            })
            .await;
            return;
        };

        match client
            .update_device(address, alias.clone(), Some(poll_interval))
            .await
        {
            Ok(device) => {
                self.send_event(SensorEvent::ServiceDeviceUpdated {
                    device: ServiceMonitoredDevice {
                        address: device.address,
                        alias: device.alias,
                        poll_interval: device.poll_interval,
                    },
                })
                .await;
                // Refresh status to show the updated device
                self.handle_refresh_service_status().await;
            }
            Err(e) => {
                self.send_event(SensorEvent::ServiceDeviceError {
                    operation: "update".to_string(),
                    error: Self::format_service_error(&e),
                })
                .await;
            }
        }
    }

    /// Handle removing a device from service monitoring.
    async fn handle_remove_service_device(&mut self, address: &str) {
        info!("Removing device {} from service monitoring", address);

        let Some(ref client) = self.service_client else {
            self.send_event(SensorEvent::ServiceDeviceError {
                operation: "remove".to_string(),
                error: "Service client not initialized".to_string(),
            })
            .await;
            return;
        };

        match client.remove_device(address).await {
            Ok(()) => {
                self.send_event(SensorEvent::ServiceDeviceRemoved {
                    address: address.to_string(),
                })
                .await;
                // Refresh status to reflect the removal
                self.handle_refresh_service_status().await;
            }
            Err(e) => {
                self.send_event(SensorEvent::ServiceDeviceError {
                    operation: "remove".to_string(),
                    error: Self::format_service_error(&e),
                })
                .await;
            }
        }
    }

    async fn handle_set_alias(&mut self, device_id: &str, alias: Option<String>) {
        info!("Setting alias for device {} to {:?}", device_id, alias);

        let Some(store) = self.get_store_mut() else {
            self.send_event(SensorEvent::AliasError {
                device_id: device_id.to_string(),
                error: "Could not open database".to_string(),
            })
            .await;
            return;
        };

        match store.update_device_metadata(device_id, alias.as_deref(), None) {
            Ok(()) => {
                info!("Alias updated successfully for {}", device_id);
                self.send_event(SensorEvent::AliasChanged {
                    device_id: device_id.to_string(),
                    alias,
                })
                .await;
            }
            Err(e) => {
                self.send_event(SensorEvent::AliasError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                })
                .await;
            }
        }
    }

    async fn handle_forget_device(&mut self, device_id: &str) {
        info!("Forgetting device {}", device_id);

        // Stop any background polling for this device
        if let Some(task) = self.polling_tasks.remove(device_id) {
            task.cancel_token.cancel();
        }

        let Some(store) = self.get_store_mut() else {
            self.send_event(SensorEvent::ForgetDeviceError {
                device_id: device_id.to_string(),
                error: "Could not open database".to_string(),
            })
            .await;
            return;
        };

        match store.delete_device(device_id) {
            Ok(deleted) => {
                if deleted {
                    info!("Device {} forgotten (deleted from store)", device_id);
                } else {
                    info!(
                        "Device {} not found in store (removing from UI only)",
                        device_id
                    );
                }
                self.send_event(SensorEvent::DeviceForgotten {
                    device_id: device_id.to_string(),
                })
                .await;
            }
            Err(e) => {
                self.send_event(SensorEvent::ForgetDeviceError {
                    device_id: device_id.to_string(),
                    error: e.to_string(),
                })
                .await;
            }
        }
    }
}

// -------------------------------------------------------------------------
// Standalone Helper Functions
// -------------------------------------------------------------------------

/// Refresh a single device (used for parallel refresh).
async fn refresh_single_device(
    device_id: &str,
    event_tx: &mpsc::Sender<SensorEvent>,
    cancel_token: &CancellationToken,
    store_path: &PathBuf,
) {
    // Check for cancellation
    if cancel_token.is_cancelled() {
        return;
    }

    let connect_result = tokio::select! {
        result = timeout(CONNECT_READ_TIMEOUT, connect_and_read_standalone(device_id)) => result,
        _ = cancel_token.cancelled() => {
            return;
        }
    };

    match connect_result {
        Ok(Ok((_, _, reading, settings, rssi))) => {
            // Send signal strength update if available
            if let Some(rssi) = rssi {
                let quality = SignalQuality::from_rssi(rssi);
                let _ = event_tx
                    .send(SensorEvent::SignalStrengthUpdate {
                        device_id: device_id.to_string(),
                        rssi,
                        quality,
                    })
                    .await;
            }

            // Send settings if we got them
            if let Some(settings) = settings {
                let _ = event_tx
                    .send(SensorEvent::SettingsLoaded {
                        device_id: device_id.to_string(),
                        settings,
                    })
                    .await;
            }

            if let Some(reading) = reading {
                // Save to store
                if let Ok(store) = Store::open(store_path) {
                    if let Err(e) = store.insert_reading(device_id, &reading) {
                        warn!(device_id, error = %e, "Failed to save reading to store");
                    }
                }

                let _ = event_tx
                    .send(SensorEvent::ReadingUpdated {
                        device_id: device_id.to_string(),
                        reading,
                    })
                    .await;
            } else {
                let context = ErrorContext::transient(
                    "Failed to read current values",
                    "The device responded but didn't send valid data. Try again.",
                );
                let _ = event_tx
                    .send(SensorEvent::ReadingError {
                        device_id: device_id.to_string(),
                        error: context.message.clone(),
                        context: Some(context),
                    })
                    .await;
            }
        }
        Ok(Err(e)) => {
            let context = ErrorContext::from_error(&e);
            let _ = event_tx
                .send(SensorEvent::ReadingError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
        }
        Err(_) => {
            let context = ErrorContext::transient(
                format!(
                    "Refresh timed out after {}s",
                    CONNECT_READ_TIMEOUT.as_secs()
                ),
                "The device may be out of range. Try moving closer.",
            );
            let _ = event_tx
                .send(SensorEvent::ReadingError {
                    device_id: device_id.to_string(),
                    error: context.message.clone(),
                    context: Some(context),
                })
                .await;
        }
    }
}

/// Standalone connect and read (for parallel operations).
async fn connect_and_read_standalone(
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
    with_retry(&default_retry_config(), "connect_and_read", || async {
        let device = Device::connect(device_id).await?;
        let name = device.name().map(String::from);
        let device_type = device.device_type();

        let reading = match device.read_current().await {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(device_id, error = %e, "Failed to read current values");
                None
            }
        };

        let settings = match device.get_settings().await {
            Ok(s) => Some(s),
            Err(e) => {
                debug!(device_id, error = %e, "Failed to read settings (non-critical)");
                None
            }
        };

        let rssi = match device.read_rssi().await {
            Ok(r) => Some(r),
            Err(e) => {
                debug!(device_id, error = %e, "Failed to read RSSI (non-critical)");
                None
            }
        };

        if let Err(e) = device.disconnect().await {
            warn!(device_id, error = %e, "Failed to disconnect");
        }

        Ok((name, device_type, reading, settings, rssi))
    })
    .await
}

/// Background polling task that periodically refreshes a device.
async fn background_polling_task(
    device_id: String,
    interval_secs: u64,
    event_tx: mpsc::Sender<SensorEvent>,
    store_path: PathBuf,
    cancel_token: CancellationToken,
) {
    info!(device_id, interval_secs, "Background polling task started");

    let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
    // Skip the first immediate tick
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                debug!(device_id, "Background poll tick");
                refresh_single_device(&device_id, &event_tx, &cancel_token, &store_path).await;
            }
            _ = cancel_token.cancelled() => {
                info!(device_id, "Background polling task cancelled");
                break;
            }
        }
    }
}
