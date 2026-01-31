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

    /// Shut down the worker thread.
    Shutdown,
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
}
