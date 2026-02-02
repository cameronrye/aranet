//! Message types for UI/worker communication.
//!
//! This module defines the command and event enums used for bidirectional
//! communication between UI threads and background BLE workers. These types
//! are shared between TUI and GUI applications.
//!
//! # Architecture
//!
//! ```text
//! +------------------+     Command      +------------------+
//! |    UI Thread     | --------------> |  SensorWorker    |
//! |  (egui/ratatui)  |                 |  (tokio runtime) |
//! |                  | <-------------- |                  |
//! +------------------+   SensorEvent   +------------------+
//! ```
//!
//! - [`Command`]: Messages sent from the UI thread to the background worker
//! - [`SensorEvent`]: Events sent from the worker back to the UI thread

use std::time::Duration;

use crate::DiscoveredDevice;
use crate::settings::DeviceSettings;
use aranet_types::{CurrentReading, DeviceType, HistoryRecord};

/// Describes why an error occurred and whether it can be retried.
#[derive(Debug, Clone)]
pub struct ErrorContext {
    /// The error message.
    pub message: String,
    /// Whether this error is likely transient and worth retrying.
    pub retryable: bool,
    /// A user-friendly suggestion for resolving the error.
    pub suggestion: Option<String>,
}

impl ErrorContext {
    /// Create a new non-retryable error.
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            suggestion: None,
        }
    }

    /// Create a new retryable error with a suggestion.
    pub fn transient(message: impl Into<String>, suggestion: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            suggestion: Some(suggestion.into()),
        }
    }

    /// Create from an aranet_core::Error with automatic classification.
    pub fn from_error(error: &crate::Error) -> Self {
        use crate::error::ConnectionFailureReason;

        match error {
            crate::Error::Timeout { operation, .. } => Self::transient(
                error.to_string(),
                format!(
                    "The {} operation timed out. The device may be out of range or busy. Try moving closer.",
                    operation
                ),
            ),
            crate::Error::ConnectionFailed { reason, .. } => match reason {
                ConnectionFailureReason::OutOfRange => Self::transient(
                    error.to_string(),
                    "Device is out of Bluetooth range. Move closer and try again.",
                ),
                ConnectionFailureReason::Timeout => Self::transient(
                    error.to_string(),
                    "Connection timed out. The device may be busy or out of range.",
                ),
                ConnectionFailureReason::BleError(_) => Self::transient(
                    error.to_string(),
                    "Bluetooth error occurred. Try toggling Bluetooth off and on.",
                ),
                ConnectionFailureReason::AdapterUnavailable => Self {
                    message: error.to_string(),
                    retryable: false,
                    suggestion: Some(
                        "Bluetooth adapter is unavailable. Enable Bluetooth and try again."
                            .to_string(),
                    ),
                },
                ConnectionFailureReason::Rejected => Self {
                    message: error.to_string(),
                    retryable: false,
                    suggestion: Some(
                        "Connection was rejected by the device. Try re-pairing.".to_string(),
                    ),
                },
                ConnectionFailureReason::AlreadyConnected => Self {
                    message: error.to_string(),
                    retryable: false,
                    suggestion: Some("Device is already connected.".to_string()),
                },
                ConnectionFailureReason::PairingFailed => Self {
                    message: error.to_string(),
                    retryable: false,
                    suggestion: Some(
                        "Pairing failed. Try removing the device and re-pairing.".to_string(),
                    ),
                },
                ConnectionFailureReason::Other(_) => Self::transient(
                    error.to_string(),
                    "Connection failed. Try again or restart the device.",
                ),
            },
            crate::Error::NotConnected => Self::transient(
                error.to_string(),
                "Device disconnected unexpectedly. Reconnecting...",
            ),
            crate::Error::Bluetooth(_) => Self::transient(
                error.to_string(),
                "Bluetooth error. Try moving closer to the device or restarting Bluetooth.",
            ),
            crate::Error::DeviceNotFound(_) => Self::permanent(error.to_string()),
            crate::Error::CharacteristicNotFound { .. } => Self {
                message: error.to_string(),
                retryable: false,
                suggestion: Some(
                    "This device may have incompatible firmware. Check for updates.".to_string(),
                ),
            },
            crate::Error::InvalidData(_)
            | crate::Error::InvalidHistoryData { .. }
            | crate::Error::InvalidReadingFormat { .. } => Self::permanent(error.to_string()),
            crate::Error::Cancelled => Self::permanent("Operation was cancelled.".to_string()),
            crate::Error::WriteFailed { .. } => {
                Self::transient(error.to_string(), "Failed to write to device. Try again.")
            }
            crate::Error::Io(_) => {
                Self::transient(error.to_string(), "I/O error occurred. Try again.")
            }
            crate::Error::InvalidConfig(_) => Self::permanent(error.to_string()),
        }
    }
}

/// Commands sent from the UI thread to the background worker.
///
/// These commands represent user-initiated actions that require
/// Bluetooth operations or other background processing.
#[derive(Debug, Clone)]
pub enum Command {
    /// Load cached devices and readings from the store on startup.
    LoadCachedData,

    /// Scan for nearby Aranet devices.
    Scan {
        /// How long to scan for devices.
        duration: Duration,
    },

    /// Connect to a specific device.
    Connect {
        /// The device identifier to connect to.
        device_id: String,
    },

    /// Disconnect from a specific device.
    Disconnect {
        /// The device identifier to disconnect from.
        device_id: String,
    },

    /// Refresh the current reading for a single device.
    RefreshReading {
        /// The device identifier to refresh.
        device_id: String,
    },

    /// Refresh readings for all connected devices.
    RefreshAll,

    /// Sync history from device (download from BLE and save to store).
    SyncHistory {
        /// The device identifier to sync history for.
        device_id: String,
    },

    /// Set the measurement interval for a device.
    SetInterval {
        /// The device identifier.
        device_id: String,
        /// The new interval in seconds.
        interval_secs: u16,
    },

    /// Set the Bluetooth range for a device.
    SetBluetoothRange {
        /// The device identifier.
        device_id: String,
        /// Whether to use extended range (true) or standard (false).
        extended: bool,
    },

    /// Set Smart Home integration mode for a device.
    SetSmartHome {
        /// The device identifier.
        device_id: String,
        /// Whether to enable Smart Home mode.
        enabled: bool,
    },

    /// Refresh the aranet-service status.
    RefreshServiceStatus,

    /// Start the aranet-service collector.
    StartServiceCollector,

    /// Stop the aranet-service collector.
    StopServiceCollector,

    /// Set a friendly alias/name for a device.
    SetAlias {
        /// The device identifier.
        device_id: String,
        /// The new alias (or None to clear).
        alias: Option<String>,
    },

    /// Forget (remove) a device from the known devices list and store.
    ForgetDevice {
        /// The device identifier.
        device_id: String,
    },

    /// Cancel the current long-running operation (scan, history sync, etc.).
    CancelOperation,

    /// Start automatic background polling for a device.
    StartBackgroundPolling {
        /// The device identifier.
        device_id: String,
        /// Polling interval in seconds.
        interval_secs: u64,
    },

    /// Stop automatic background polling for a device.
    StopBackgroundPolling {
        /// The device identifier.
        device_id: String,
    },

    /// Shut down the worker thread.
    Shutdown,

    /// Install aranet-service as a system service.
    InstallSystemService {
        /// Install as user-level service (no root/admin required).
        user_level: bool,
    },

    /// Uninstall aranet-service system service.
    UninstallSystemService {
        /// Uninstall user-level service.
        user_level: bool,
    },

    /// Start the aranet-service system service.
    StartSystemService {
        /// Start user-level service.
        user_level: bool,
    },

    /// Stop the aranet-service system service.
    StopSystemService {
        /// Stop user-level service.
        user_level: bool,
    },

    /// Check the status of the aranet-service system service.
    CheckSystemServiceStatus {
        /// Check user-level service status.
        user_level: bool,
    },

    /// Fetch the service configuration.
    FetchServiceConfig,

    /// Add a device to the service's monitored device list.
    AddServiceDevice {
        /// Device address/ID.
        address: String,
        /// Optional alias.
        alias: Option<String>,
        /// Poll interval in seconds.
        poll_interval: u64,
    },

    /// Update a device in the service's monitored device list.
    UpdateServiceDevice {
        /// Device address/ID.
        address: String,
        /// Optional new alias.
        alias: Option<String>,
        /// New poll interval in seconds.
        poll_interval: u64,
    },

    /// Remove a device from the service's monitored device list.
    RemoveServiceDevice {
        /// Device address/ID to remove.
        address: String,
    },
}

/// Cached device data loaded from the store.
#[derive(Debug, Clone)]
pub struct CachedDevice {
    /// Device identifier.
    pub id: String,
    /// Device name.
    pub name: Option<String>,
    /// Device type.
    pub device_type: Option<DeviceType>,
    /// Latest reading, if available.
    pub reading: Option<CurrentReading>,
    /// When history was last synced.
    pub last_sync: Option<time::OffsetDateTime>,
}

/// Events sent from the background worker to the UI thread.
///
/// These events represent the results of background operations
/// and are used to update the UI state.
#[derive(Debug, Clone)]
pub enum SensorEvent {
    /// Cached data loaded from the store on startup.
    CachedDataLoaded {
        /// Cached devices with their latest readings.
        devices: Vec<CachedDevice>,
    },

    /// A device scan has started.
    ScanStarted,

    /// A device scan has completed successfully.
    ScanComplete {
        /// The list of discovered devices.
        devices: Vec<DiscoveredDevice>,
    },

    /// A device scan failed.
    ScanError {
        /// Description of the error.
        error: String,
    },

    /// Attempting to connect to a device.
    DeviceConnecting {
        /// The device identifier.
        device_id: String,
    },

    /// Successfully connected to a device.
    DeviceConnected {
        /// The device identifier.
        device_id: String,
        /// The device name, if available.
        name: Option<String>,
        /// The device type, if detected.
        device_type: Option<DeviceType>,
        /// RSSI signal strength in dBm.
        rssi: Option<i16>,
    },

    /// Disconnected from a device.
    DeviceDisconnected {
        /// The device identifier.
        device_id: String,
    },

    /// Failed to connect to a device.
    ConnectionError {
        /// The device identifier.
        device_id: String,
        /// Description of the error.
        error: String,
        /// Additional error context with retry info.
        context: Option<ErrorContext>,
    },

    /// Received an updated reading from a device.
    ReadingUpdated {
        /// The device identifier.
        device_id: String,
        /// The current sensor reading.
        reading: CurrentReading,
    },

    /// Failed to read from a device.
    ReadingError {
        /// The device identifier.
        device_id: String,
        /// Description of the error.
        error: String,
        /// Additional error context with retry info.
        context: Option<ErrorContext>,
    },

    /// Historical data loaded for a device.
    HistoryLoaded {
        /// The device identifier.
        device_id: String,
        /// The historical records.
        records: Vec<HistoryRecord>,
    },

    /// History sync started for a device.
    HistorySyncStarted {
        /// The device identifier.
        device_id: String,
        /// Total number of records to download (if known).
        total_records: Option<u16>,
    },

    /// History sync progress update.
    HistorySyncProgress {
        /// The device identifier.
        device_id: String,
        /// Number of records downloaded so far.
        downloaded: usize,
        /// Total number of records to download.
        total: usize,
    },

    /// History sync completed for a device.
    HistorySynced {
        /// The device identifier.
        device_id: String,
        /// Number of records synced.
        count: usize,
    },

    /// History sync failed for a device.
    HistorySyncError {
        /// The device identifier.
        device_id: String,
        /// Description of the error.
        error: String,
        /// Additional error context with retry info.
        context: Option<ErrorContext>,
    },

    /// Measurement interval changed for a device.
    IntervalChanged {
        /// The device identifier.
        device_id: String,
        /// The new interval in seconds.
        interval_secs: u16,
    },

    /// Failed to set measurement interval.
    IntervalError {
        /// The device identifier.
        device_id: String,
        /// Description of the error.
        error: String,
        /// Additional error context with retry info.
        context: Option<ErrorContext>,
    },

    /// Device settings loaded from the device.
    SettingsLoaded {
        /// The device identifier.
        device_id: String,
        /// The device settings.
        settings: DeviceSettings,
    },

    /// Bluetooth range changed for a device.
    BluetoothRangeChanged {
        /// The device identifier.
        device_id: String,
        /// Whether extended range is now enabled.
        extended: bool,
    },

    /// Failed to set Bluetooth range.
    BluetoothRangeError {
        /// The device identifier.
        device_id: String,
        /// Description of the error.
        error: String,
        /// Additional error context with retry info.
        context: Option<ErrorContext>,
    },

    /// Smart Home setting changed for a device.
    SmartHomeChanged {
        /// The device identifier.
        device_id: String,
        /// Whether Smart Home mode is now enabled.
        enabled: bool,
    },

    /// Failed to set Smart Home mode.
    SmartHomeError {
        /// The device identifier.
        device_id: String,
        /// Description of the error.
        error: String,
        /// Additional error context with retry info.
        context: Option<ErrorContext>,
    },

    /// Service status refreshed successfully.
    ServiceStatusRefreshed {
        /// Whether the service is reachable.
        reachable: bool,
        /// Whether the collector is running.
        collector_running: bool,
        /// Service uptime in seconds.
        uptime_seconds: Option<u64>,
        /// Monitored devices with their collection stats.
        devices: Vec<ServiceDeviceStats>,
    },

    /// Service status refresh failed.
    ServiceStatusError {
        /// Description of the error.
        error: String,
    },

    /// Service collector started successfully.
    ServiceCollectorStarted,

    /// Service collector stopped successfully.
    ServiceCollectorStopped,

    /// Service collector action failed.
    ServiceCollectorError {
        /// Description of the error.
        error: String,
    },

    /// Device alias changed successfully.
    AliasChanged {
        /// The device identifier.
        device_id: String,
        /// The new alias (or None if cleared).
        alias: Option<String>,
    },

    /// Failed to set device alias.
    AliasError {
        /// The device identifier.
        device_id: String,
        /// Description of the error.
        error: String,
    },

    /// Device was forgotten (removed from known devices).
    DeviceForgotten {
        /// The device identifier.
        device_id: String,
    },

    /// Failed to forget device.
    ForgetDeviceError {
        /// The device identifier.
        device_id: String,
        /// Description of the error.
        error: String,
    },

    /// An operation was cancelled by user request.
    OperationCancelled {
        /// Description of what was cancelled.
        operation: String,
    },

    /// Background polling started for a device.
    BackgroundPollingStarted {
        /// The device identifier.
        device_id: String,
        /// Polling interval in seconds.
        interval_secs: u64,
    },

    /// Background polling stopped for a device.
    BackgroundPollingStopped {
        /// The device identifier.
        device_id: String,
    },

    /// Signal strength update (can be sent periodically or on connect).
    SignalStrengthUpdate {
        /// The device identifier.
        device_id: String,
        /// RSSI in dBm.
        rssi: i16,
        /// Quality assessment.
        quality: SignalQuality,
    },

    /// System service status retrieved.
    SystemServiceStatus {
        /// Whether the service is installed.
        installed: bool,
        /// Whether the service is running.
        running: bool,
    },

    /// System service was installed successfully.
    SystemServiceInstalled,

    /// System service was uninstalled successfully.
    SystemServiceUninstalled,

    /// System service was started successfully.
    SystemServiceStarted,

    /// System service was stopped successfully.
    SystemServiceStopped,

    /// System service operation failed.
    SystemServiceError {
        /// The operation that failed.
        operation: String,
        /// Description of the error.
        error: String,
    },

    /// Service configuration fetched.
    ServiceConfigFetched {
        /// List of monitored devices in service config.
        devices: Vec<ServiceMonitoredDevice>,
    },

    /// Failed to fetch service configuration.
    ServiceConfigError {
        /// Error message.
        error: String,
    },

    /// Device added to service monitoring.
    ServiceDeviceAdded {
        /// The device that was added.
        device: ServiceMonitoredDevice,
    },

    /// Device updated in service monitoring.
    ServiceDeviceUpdated {
        /// The device that was updated.
        device: ServiceMonitoredDevice,
    },

    /// Device removed from service monitoring.
    ServiceDeviceRemoved {
        /// The device address that was removed.
        address: String,
    },

    /// Failed to modify service device.
    ServiceDeviceError {
        /// The operation that failed.
        operation: String,
        /// Error message.
        error: String,
    },
}

/// A device being monitored by the service.
#[derive(Debug, Clone)]
pub struct ServiceMonitoredDevice {
    /// Device address/ID.
    pub address: String,
    /// Device alias.
    pub alias: Option<String>,
    /// Poll interval in seconds.
    pub poll_interval: u64,
}

/// Signal quality assessment based on RSSI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalQuality {
    /// Excellent signal (> -50 dBm).
    Excellent,
    /// Good signal (-50 to -70 dBm).
    Good,
    /// Fair signal (-70 to -80 dBm).
    Fair,
    /// Weak signal (< -80 dBm).
    Weak,
}

impl SignalQuality {
    /// Determine signal quality from RSSI value.
    pub fn from_rssi(rssi: i16) -> Self {
        match rssi {
            r if r > -50 => SignalQuality::Excellent,
            r if r > -70 => SignalQuality::Good,
            r if r > -80 => SignalQuality::Fair,
            _ => SignalQuality::Weak,
        }
    }

    /// Get a user-friendly description of the signal quality.
    pub fn description(&self) -> &'static str {
        match self {
            SignalQuality::Excellent => "Excellent",
            SignalQuality::Good => "Good",
            SignalQuality::Fair => "Fair",
            SignalQuality::Weak => "Weak - move closer",
        }
    }
}

/// Statistics for a device being monitored by the service.
#[derive(Debug, Clone)]
pub struct ServiceDeviceStats {
    /// Device identifier.
    pub device_id: String,
    /// Device alias/name.
    pub alias: Option<String>,
    /// Poll interval in seconds.
    pub poll_interval: u64,
    /// Whether the device is currently being polled.
    pub polling: bool,
    /// Number of successful polls.
    pub success_count: u64,
    /// Number of failed polls.
    pub failure_count: u64,
    /// Last poll time.
    pub last_poll_at: Option<time::OffsetDateTime>,
    /// Last error message.
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_debug() {
        let cmd = Command::Scan {
            duration: Duration::from_secs(5),
        };
        let debug = format!("{:?}", cmd);
        assert!(debug.contains("Scan"));
        assert!(debug.contains("5"));
    }

    #[test]
    fn test_command_clone() {
        let cmd = Command::Connect {
            device_id: "test-device".to_string(),
        };
        let cloned = cmd.clone();
        match cloned {
            Command::Connect { device_id } => assert_eq!(device_id, "test-device"),
            _ => panic!("Expected Connect variant"),
        }
    }

    #[test]
    fn test_sensor_event_debug() {
        let event = SensorEvent::ScanStarted;
        let debug = format!("{:?}", event);
        assert!(debug.contains("ScanStarted"));
    }

    #[test]
    fn test_cached_device_default_values() {
        let device = CachedDevice {
            id: "test".to_string(),
            name: None,
            device_type: None,
            reading: None,
            last_sync: None,
        };
        assert_eq!(device.id, "test");
        assert!(device.name.is_none());
    }

    #[test]
    fn test_signal_quality_from_rssi() {
        assert_eq!(SignalQuality::from_rssi(-40), SignalQuality::Excellent);
        assert_eq!(SignalQuality::from_rssi(-50), SignalQuality::Good);
        assert_eq!(SignalQuality::from_rssi(-60), SignalQuality::Good);
        assert_eq!(SignalQuality::from_rssi(-70), SignalQuality::Fair);
        assert_eq!(SignalQuality::from_rssi(-75), SignalQuality::Fair);
        assert_eq!(SignalQuality::from_rssi(-80), SignalQuality::Weak);
        assert_eq!(SignalQuality::from_rssi(-90), SignalQuality::Weak);
    }

    #[test]
    fn test_signal_quality_description() {
        assert_eq!(SignalQuality::Excellent.description(), "Excellent");
        assert_eq!(SignalQuality::Good.description(), "Good");
        assert_eq!(SignalQuality::Fair.description(), "Fair");
        assert_eq!(SignalQuality::Weak.description(), "Weak - move closer");
    }

    #[test]
    fn test_error_context_permanent() {
        let ctx = ErrorContext::permanent("Device not found");
        assert!(!ctx.retryable);
        assert!(ctx.suggestion.is_none());
        assert_eq!(ctx.message, "Device not found");
    }

    #[test]
    fn test_error_context_transient() {
        let ctx = ErrorContext::transient("Connection timeout", "Move closer and retry");
        assert!(ctx.retryable);
        assert_eq!(ctx.suggestion, Some("Move closer and retry".to_string()));
    }
}
