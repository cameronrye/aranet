//! Core BLE library for Aranet environmental sensors.
//!
//! This crate provides low-level Bluetooth Low Energy (BLE) communication
//! with Aranet sensors including the Aranet4, Aranet2, AranetRn+ (Radon), and
//! Aranet Radiation devices.
//!
//! # Features
//!
//! - **Device discovery**: Scan for nearby Aranet devices via BLE
//! - **Current readings**: CO₂, temperature, pressure, humidity, radon, radiation
//! - **Historical data**: Download measurement history with timestamps
//! - **Device settings**: Read/write measurement interval, Bluetooth range
//! - **Auto-reconnection**: Configurable backoff and retry logic
//! - **Real-time streaming**: Subscribe to sensor value changes
//! - **Multi-device support**: Manage multiple sensors simultaneously
//!
//! # Supported Devices
//!
//! | Device | Sensors |
//! |--------|---------|
//! | Aranet4 | CO₂, Temperature, Pressure, Humidity |
//! | Aranet2 | Temperature, Humidity |
//! | AranetRn+ | Radon (Bq/m³), Temperature, Pressure, Humidity |
//! | Aranet Radiation | Dose Rate (µSv/h), Total Dose (mSv) |
//!
//! # Platform Differences
//!
//! Device identification varies by platform due to differences in BLE implementations:
//!
//! - **macOS**: Devices are identified by a UUID assigned by CoreBluetooth. This UUID
//!   is stable for a given device on a given Mac, but differs between Macs. The UUID
//!   is not the same as the device's MAC address.
//!
//! - **Linux/Windows**: Devices are identified by their Bluetooth MAC address
//!   (e.g., `AA:BB:CC:DD:EE:FF`). This is consistent across machines.
//!
//! When storing device identifiers for reconnection, be aware that:
//! - On macOS, the UUID may change if Bluetooth is reset or the device is unpaired
//! - Cross-platform applications should store both the device name and identifier
//! - The [`Device::address()`] method returns the appropriate identifier for the platform
//!
//! # Quick Start
//!
//! ```no_run
//! use aranet_core::{Device, scan};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Scan for devices
//!     let devices = scan::scan_for_devices().await?;
//!     println!("Found {} devices", devices.len());
//!
//!     // Connect to a device
//!     let device = Device::connect("Aranet4 12345").await?;
//!
//!     // Read current values
//!     let reading = device.read_current().await?;
//!     println!("CO2: {} ppm", reading.co2);
//!
//!     // Read device info
//!     let info = device.read_device_info().await?;
//!     println!("Serial: {}", info.serial);
//!
//!     Ok(())
//! }
//! ```

pub mod advertisement;
pub mod commands;
pub mod device;
pub mod diagnostics;
pub mod error;
pub mod events;
pub mod guard;
pub mod history;
pub mod manager;
pub mod messages;
pub mod metrics;
pub mod mock;
pub mod passive;
pub mod platform;
pub mod readings;
pub mod reconnect;
pub mod retry;
pub mod scan;
pub mod settings;
pub mod streaming;
pub mod thresholds;
pub mod traits;
pub mod util;
pub mod validation;

#[cfg(feature = "service-client")]
pub mod service_client;

// Re-export types and uuid modules from aranet-types for backwards compatibility
pub use aranet_types::types;
pub use aranet_types::uuid;

// Core exports
pub use device::{ConnectionConfig, Device, SignalQuality};
pub use error::{ConnectionFailureReason, DeviceNotFoundReason, Error, Result};
pub use history::{HistoryCheckpoint, HistoryInfo, HistoryOptions, HistoryParam, PartialHistoryData};
pub use readings::ExtendedReading;
pub use scan::{
    DiscoveredDevice, FindProgress, ProgressCallback, ScanOptions, find_device_with_progress,
    scan_with_retry,
};
pub use settings::{BluetoothRange, CalibrationData, DeviceSettings, MeasurementInterval};
pub use traits::AranetDevice;

/// Type alias for a shared device reference.
///
/// This is the recommended way to share a `Device` across multiple tasks.
/// Since `Device` intentionally does not implement `Clone` (to prevent
/// connection ownership ambiguity), wrapping it in `Arc` is the standard
/// pattern for concurrent access.
///
/// # Choosing the Right Device Type
///
/// This crate provides several device types for different use cases:
///
/// | Type | Use Case | Auto-Reconnect | Thread-Safe |
/// |------|----------|----------------|-------------|
/// | [`Device`] | Single command, short-lived | No | Yes (via Arc) |
/// | [`ReconnectingDevice`] | Long-running apps | Yes | Yes |
/// | [`SharedDevice`] | Sharing Device across tasks | No | Yes |
/// | [`DeviceManager`] | Managing multiple devices | Yes | Yes |
///
/// ## Decision Guide
///
/// ### Use [`Device`] when:
/// - Running a single command (read, history download)
/// - Connection lifetime is short and well-defined
/// - You'll handle reconnection yourself
///
/// ```no_run
/// # async fn example() -> aranet_core::Result<()> {
/// use aranet_core::Device;
/// let device = Device::connect("Aranet4 12345").await?;
/// let reading = device.read_current().await?;
/// device.disconnect().await?;
/// # Ok(())
/// # }
/// ```
///
/// ### Use [`ReconnectingDevice`] when:
/// - Building a long-running application (daemon, service)
/// - You want automatic reconnection on connection loss
/// - Continuous monitoring over extended periods
///
/// ```no_run
/// # async fn example() -> aranet_core::Result<()> {
/// use aranet_core::{AranetDevice, ReconnectingDevice, ReconnectOptions};
/// let options = ReconnectOptions::default();
/// let device = ReconnectingDevice::connect("Aranet4 12345", options).await?;
/// // Will auto-reconnect on connection loss
/// let reading = device.read_current().await?;
/// # Ok(())
/// # }
/// ```
///
/// ### Use [`SharedDevice`] when:
/// - Sharing a single [`Device`] across multiple async tasks
/// - You need concurrent reads but want one connection
///
/// ```no_run
/// # async fn example() -> aranet_core::Result<()> {
/// use aranet_core::{Device, SharedDevice};
/// use std::sync::Arc;
///
/// let device = Device::connect("Aranet4 12345").await?;
/// let shared: SharedDevice = Arc::new(device);
///
/// let shared_clone = Arc::clone(&shared);
/// tokio::spawn(async move {
///     let reading = shared_clone.read_current().await;
/// });
/// # Ok(())
/// # }
/// ```
///
/// ### Use [`DeviceManager`] when:
/// - Managing multiple devices simultaneously
/// - Need centralized connection/disconnection handling
/// - Building a multi-device monitoring application
///
/// ```no_run
/// # async fn example() -> aranet_core::Result<()> {
/// use aranet_core::DeviceManager;
/// let manager = DeviceManager::new();
/// manager.add_device("AA:BB:CC:DD:EE:FF").await?;
/// manager.add_device("11:22:33:44:55:66").await?;
/// // Manager handles connections for all devices
/// # Ok(())
/// # }
/// ```
pub type SharedDevice = std::sync::Arc<Device>;

// New module exports
pub use advertisement::{AdvertisementData, parse_advertisement, parse_advertisement_with_name};
pub use commands::{
    HISTORY_V1_REQUEST, HISTORY_V2_REQUEST, SET_BLUETOOTH_RANGE, SET_INTERVAL, SET_SMART_HOME,
};
pub use events::{DeviceEvent, EventReceiver, EventSender};
pub use guard::{DeviceGuard, SharedDeviceGuard};
pub use manager::{AdaptiveInterval, DeviceManager, DevicePriority, ManagedDevice, ManagerConfig};
pub use messages::{CachedDevice, Command, SensorEvent};
pub use metrics::{ConnectionMetrics, OperationMetrics};
pub use mock::{MockDevice, MockDeviceBuilder};
pub use reconnect::{ReconnectOptions, ReconnectingDevice};
pub use retry::{RetryConfig, with_retry};
pub use streaming::{ReadingStream, StreamOptions, StreamOptionsBuilder};
pub use thresholds::{Co2Level, ThresholdConfig, Thresholds};
pub use util::{create_identifier, format_peripheral_id};
pub use validation::{ReadingValidator, ValidationResult, ValidationWarning};
pub use platform::{
    AliasStore, DeviceAlias, Platform, PlatformConfig, current_platform, platform_config,
};
pub use passive::{PassiveMonitor, PassiveMonitorOptions, PassiveReading};
pub use diagnostics::{
    AdapterInfo, AdapterState, BluetoothDiagnostics, ConnectionStats, DiagnosticsCollector,
    ErrorCategory, OperationStats, RecordedError, global_diagnostics,
};

// Re-export from aranet-types
pub use aranet_types::uuid as uuids;
pub use aranet_types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord, Status};
