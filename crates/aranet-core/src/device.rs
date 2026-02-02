//! Aranet device connection and communication.
//!
//! This module provides the main interface for connecting to and
//! communicating with Aranet sensors over Bluetooth Low Energy.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use btleplug::api::{Characteristic, Peripheral as _, WriteType};
use btleplug::platform::{Adapter, Peripheral};
use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::scan::{ScanOptions, find_device};
use crate::traits::AranetDevice;
use crate::util::{create_identifier, format_peripheral_id};
use crate::uuid::{
    BATTERY_LEVEL, BATTERY_SERVICE, CURRENT_READINGS_DETAIL, CURRENT_READINGS_DETAIL_ALT,
    DEVICE_INFO_SERVICE, DEVICE_NAME, FIRMWARE_REVISION, GAP_SERVICE, HARDWARE_REVISION,
    MANUFACTURER_NAME, MODEL_NUMBER, SAF_TEHNIKA_SERVICE_NEW, SAF_TEHNIKA_SERVICE_OLD,
    SERIAL_NUMBER, SOFTWARE_REVISION,
};
use aranet_types::{CurrentReading, DeviceInfo, DeviceType};

/// Represents a connected Aranet device.
///
/// # Note on Clone
///
/// This struct intentionally does not implement `Clone`. A `Device` represents
/// an active BLE connection with associated state (services discovered, notification
/// handlers, etc.). Cloning would create ambiguity about connection ownership and
/// could lead to resource conflicts. If you need to share a device across multiple
/// tasks, wrap it in `Arc<Device>`.
///
/// # Cleanup
///
/// You MUST call [`Device::disconnect`] before dropping the device to properly
/// release BLE resources. If a Device is dropped without calling disconnect,
/// a warning will be logged.
pub struct Device {
    /// The BLE adapter used for connection.
    ///
    /// This field is stored to keep the adapter alive for the lifetime of the
    /// peripheral connection. The peripheral may hold internal references to
    /// the adapter, and dropping the adapter could invalidate the connection.
    #[allow(dead_code)]
    adapter: Adapter,
    /// The underlying BLE peripheral.
    peripheral: Peripheral,
    /// Cached device name.
    name: Option<String>,
    /// Device address or identifier (MAC address on Linux/Windows, UUID on macOS).
    address: String,
    /// Detected device type.
    device_type: Option<DeviceType>,
    /// Whether services have been discovered.
    services_discovered: bool,
    /// Cache of discovered characteristics by UUID for O(1) lookup.
    /// Built after service discovery to avoid searching through services on each read.
    characteristics_cache: RwLock<HashMap<Uuid, Characteristic>>,
    /// Handles for spawned notification tasks (for cleanup).
    notification_handles: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Whether disconnect has been called (for Drop warning).
    disconnected: AtomicBool,
    /// Connection configuration (timeouts, etc.).
    config: ConnectionConfig,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Provide a clean debug output that excludes internal BLE details
        // (adapter, peripheral, notification_handles, characteristics_cache)
        // which are not useful for debugging application logic.
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("address", &self.address)
            .field("device_type", &self.device_type)
            .field("services_discovered", &self.services_discovered)
            .finish_non_exhaustive()
    }
}

/// Default timeout for BLE characteristic read operations.
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for BLE characteristic write operations.
const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for BLE connection operations.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Default timeout for service discovery.
const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for connection validation (keepalive check).
const DEFAULT_VALIDATION_TIMEOUT: Duration = Duration::from_secs(3);

/// Configuration for BLE connection timeouts and behavior.
///
/// Use this to customize timeout values for different environments.
/// For example, increase timeouts in challenging RF environments
/// (concrete walls, electromagnetic interference).
///
/// # Example
///
/// ```no_run
/// use std::time::Duration;
/// use aranet_core::device::ConnectionConfig;
///
/// // Create a config for challenging RF environments
/// let config = ConnectionConfig::default()
///     .connection_timeout(Duration::from_secs(20))
///     .read_timeout(Duration::from_secs(15));
/// ```
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Timeout for establishing a BLE connection.
    pub connection_timeout: Duration,
    /// Timeout for BLE read operations.
    pub read_timeout: Duration,
    /// Timeout for BLE write operations.
    pub write_timeout: Duration,
    /// Timeout for service discovery after connection.
    pub discovery_timeout: Duration,
    /// Timeout for connection validation (keepalive) checks.
    pub validation_timeout: Duration,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            connection_timeout: DEFAULT_CONNECT_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            write_timeout: DEFAULT_WRITE_TIMEOUT,
            discovery_timeout: DEFAULT_DISCOVERY_TIMEOUT,
            validation_timeout: DEFAULT_VALIDATION_TIMEOUT,
        }
    }
}

impl ConnectionConfig {
    /// Create a new connection config with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a config optimized for the current platform.
    pub fn for_current_platform() -> Self {
        let platform = crate::platform::PlatformConfig::for_current_platform();
        Self {
            connection_timeout: platform.recommended_connection_timeout,
            read_timeout: platform.recommended_operation_timeout,
            write_timeout: platform.recommended_operation_timeout,
            discovery_timeout: platform.recommended_operation_timeout,
            validation_timeout: DEFAULT_VALIDATION_TIMEOUT,
        }
    }

    /// Create a config for challenging RF environments.
    ///
    /// Uses longer timeouts to accommodate signal interference,
    /// thick walls, or long distances.
    pub fn challenging_environment() -> Self {
        Self {
            connection_timeout: Duration::from_secs(25),
            read_timeout: Duration::from_secs(15),
            write_timeout: Duration::from_secs(15),
            discovery_timeout: Duration::from_secs(15),
            validation_timeout: Duration::from_secs(5),
        }
    }

    /// Create a config for fast, reliable environments.
    ///
    /// Uses shorter timeouts for quicker failure detection
    /// when devices are nearby with strong signals.
    pub fn fast() -> Self {
        Self {
            connection_timeout: Duration::from_secs(8),
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            discovery_timeout: Duration::from_secs(5),
            validation_timeout: Duration::from_secs(2),
        }
    }

    /// Set the connection timeout.
    #[must_use]
    pub fn connection_timeout(mut self, timeout: Duration) -> Self {
        self.connection_timeout = timeout;
        self
    }

    /// Set the read timeout.
    #[must_use]
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Set the write timeout.
    #[must_use]
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.write_timeout = timeout;
        self
    }

    /// Set the service discovery timeout.
    #[must_use]
    pub fn discovery_timeout(mut self, timeout: Duration) -> Self {
        self.discovery_timeout = timeout;
        self
    }

    /// Set the validation timeout.
    #[must_use]
    pub fn validation_timeout(mut self, timeout: Duration) -> Self {
        self.validation_timeout = timeout;
        self
    }
}

/// Signal strength quality levels based on RSSI values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SignalQuality {
    /// Signal too weak for reliable operation (< -85 dBm).
    Poor,
    /// Usable but may have issues (-85 to -75 dBm).
    Fair,
    /// Good signal strength (-75 to -60 dBm).
    Good,
    /// Excellent signal strength (> -60 dBm).
    Excellent,
}

impl SignalQuality {
    /// Determine signal quality from RSSI value in dBm.
    ///
    /// # Arguments
    ///
    /// * `rssi` - Signal strength in dBm (typically -30 to -100)
    ///
    /// # Returns
    ///
    /// The signal quality category.
    pub fn from_rssi(rssi: i16) -> Self {
        match rssi {
            r if r > -60 => SignalQuality::Excellent,
            r if r > -75 => SignalQuality::Good,
            r if r > -85 => SignalQuality::Fair,
            _ => SignalQuality::Poor,
        }
    }

    /// Get a human-readable description of the signal quality.
    pub fn description(&self) -> &'static str {
        match self {
            SignalQuality::Excellent => "Excellent signal",
            SignalQuality::Good => "Good signal",
            SignalQuality::Fair => "Fair signal - connection may be unstable",
            SignalQuality::Poor => "Poor signal - consider moving closer",
        }
    }

    /// Get recommended read delay for history downloads based on signal quality.
    pub fn recommended_read_delay(&self) -> Duration {
        match self {
            SignalQuality::Excellent => Duration::from_millis(30),
            SignalQuality::Good => Duration::from_millis(50),
            SignalQuality::Fair => Duration::from_millis(100),
            SignalQuality::Poor => Duration::from_millis(200),
        }
    }

    /// Check if the signal is strong enough for reliable operations.
    pub fn is_usable(&self) -> bool {
        matches!(self, SignalQuality::Excellent | SignalQuality::Good | SignalQuality::Fair)
    }
}

impl Device {
    /// Connect to an Aranet device by name or MAC address.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use aranet_core::device::Device;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let device = Device::connect("Aranet4 12345").await?;
    ///     println!("Connected to {:?}", device);
    ///     Ok(())
    /// }
    /// ```
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier))]
    pub async fn connect(identifier: &str) -> Result<Self> {
        Self::connect_with_config(identifier, ConnectionConfig::default()).await
    }

    /// Connect to an Aranet device with a custom scan timeout.
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier, timeout_secs = scan_timeout.as_secs()))]
    pub async fn connect_with_timeout(identifier: &str, scan_timeout: Duration) -> Result<Self> {
        let config = ConnectionConfig::default().connection_timeout(scan_timeout);
        Self::connect_with_config(identifier, config).await
    }

    /// Connect to an Aranet device with full configuration.
    ///
    /// This is the most flexible connection method, allowing customization
    /// of all timeout values.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use aranet_core::device::{Device, ConnectionConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     // Use longer timeouts for challenging RF environment
    ///     let config = ConnectionConfig::challenging_environment();
    ///     let device = Device::connect_with_config("Aranet4 12345", config).await?;
    ///     Ok(())
    /// }
    /// ```
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier))]
    pub async fn connect_with_config(identifier: &str, config: ConnectionConfig) -> Result<Self> {
        let options = ScanOptions {
            duration: config.connection_timeout,
            filter_aranet_only: false, // We're looking for a specific device
            use_service_filter: false,
        };

        // Try find_device first (uses default 5s scan), then with custom options
        let (adapter, peripheral) = match find_device(identifier).await {
            Ok(result) => result,
            Err(_) => crate::scan::find_device_with_options(identifier, options).await?,
        };

        Self::from_peripheral_with_config(adapter, peripheral, config).await
    }

    /// Create a Device from an already-discovered peripheral.
    #[tracing::instrument(level = "info", skip_all)]
    pub async fn from_peripheral(adapter: Adapter, peripheral: Peripheral) -> Result<Self> {
        Self::from_peripheral_with_config(adapter, peripheral, ConnectionConfig::default()).await
    }

    /// Create a Device from an already-discovered peripheral with custom timeout.
    #[tracing::instrument(level = "info", skip_all, fields(timeout_secs = connect_timeout.as_secs()))]
    pub async fn from_peripheral_with_timeout(
        adapter: Adapter,
        peripheral: Peripheral,
        connect_timeout: Duration,
    ) -> Result<Self> {
        let config = ConnectionConfig::default().connection_timeout(connect_timeout);
        Self::from_peripheral_with_config(adapter, peripheral, config).await
    }

    /// Create a Device from an already-discovered peripheral with full configuration.
    #[tracing::instrument(level = "info", skip_all, fields(connect_timeout = ?config.connection_timeout))]
    pub async fn from_peripheral_with_config(
        adapter: Adapter,
        peripheral: Peripheral,
        config: ConnectionConfig,
    ) -> Result<Self> {
        // Connect to the device with timeout
        info!("Connecting to device...");
        timeout(config.connection_timeout, peripheral.connect())
            .await
            .map_err(|_| Error::Timeout {
                operation: "connect to device".to_string(),
                duration: config.connection_timeout,
            })??;
        info!("Connected!");

        // Discover services with timeout
        info!("Discovering services...");
        timeout(config.discovery_timeout, peripheral.discover_services())
            .await
            .map_err(|_| Error::Timeout {
                operation: "discover services".to_string(),
                duration: config.discovery_timeout,
            })??;

        let services = peripheral.services();
        debug!("Found {} services", services.len());

        // Build characteristics cache for O(1) lookups
        let mut characteristics_cache = HashMap::new();
        for service in &services {
            debug!("  Service: {}", service.uuid);
            for char in &service.characteristics {
                debug!("    Characteristic: {}", char.uuid);
                characteristics_cache.insert(char.uuid, char.clone());
            }
        }
        debug!(
            "Cached {} characteristics for fast lookup",
            characteristics_cache.len()
        );

        // Get device properties
        let properties = peripheral.properties().await?;
        let name = properties.as_ref().and_then(|p| p.local_name.clone());

        // Get address - on macOS this may be 00:00:00:00:00:00, so we use peripheral ID as fallback
        let address = properties
            .as_ref()
            .map(|p| create_identifier(&p.address.to_string(), &peripheral.id()))
            .unwrap_or_else(|| format_peripheral_id(&peripheral.id()));

        // Determine device type from name
        let device_type = name.as_ref().and_then(|n| DeviceType::from_name(n));

        Ok(Self {
            adapter,
            peripheral,
            name,
            address,
            device_type,
            services_discovered: true,
            characteristics_cache: RwLock::new(characteristics_cache),
            notification_handles: tokio::sync::Mutex::new(Vec::new()),
            disconnected: AtomicBool::new(false),
            config,
        })
    }

    /// Check if the device is connected (queries BLE stack state).
    ///
    /// Note: This only checks the BLE stack's connection state, which may be stale,
    /// especially on macOS. For a more reliable check, use [`validate_connection`].
    pub async fn is_connected(&self) -> bool {
        self.peripheral.is_connected().await.unwrap_or(false)
    }

    /// Validate the connection by performing a lightweight read operation.
    ///
    /// This is more reliable than `is_connected()` as it actively verifies
    /// the connection is working. Uses battery level read as it's fast and
    /// always available on Aranet devices.
    ///
    /// This method is useful for detecting "zombie connections" where the
    /// BLE stack thinks it's connected but the device is actually out of range.
    ///
    /// # Returns
    ///
    /// `true` if the connection is active and responsive, `false` otherwise.
    pub async fn validate_connection(&self) -> bool {
        timeout(self.config.validation_timeout, self.read_battery())
            .await
            .map(|r| r.is_ok())
            .unwrap_or(false)
    }

    /// Check if the connection is alive by performing a lightweight keepalive check.
    ///
    /// This is an alias for [`validate_connection`] that better describes
    /// the intent when used for connection health monitoring.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // In a health monitor loop
    /// if !device.is_connection_alive().await {
    ///     // Connection lost, need to reconnect
    /// }
    /// ```
    pub async fn is_connection_alive(&self) -> bool {
        self.validate_connection().await
    }

    /// Get the current connection configuration.
    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }

    /// Get the current signal quality based on RSSI.
    ///
    /// Returns `None` if RSSI cannot be read.
    pub async fn signal_quality(&self) -> Option<SignalQuality> {
        self.read_rssi().await.ok().map(SignalQuality::from_rssi)
    }

    /// Disconnect from the device.
    ///
    /// This will:
    /// 1. Abort all active notification handlers
    /// 2. Disconnect from the BLE peripheral
    ///
    /// **Important:** You MUST call this method before dropping the Device
    /// to ensure proper cleanup of BLE resources.
    #[tracing::instrument(level = "info", skip(self), fields(device_name = ?self.name))]
    pub async fn disconnect(&self) -> Result<()> {
        info!("Disconnecting from device...");
        self.disconnected.store(true, Ordering::SeqCst);

        // Abort all notification handlers
        {
            let mut handles = self.notification_handles.lock().await;
            for handle in handles.drain(..) {
                handle.abort();
            }
        }

        self.peripheral.disconnect().await?;
        Ok(())
    }

    /// Get the device name.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Get the device address or identifier.
    ///
    /// On Linux and Windows, this returns the Bluetooth MAC address (e.g., "AA:BB:CC:DD:EE:FF").
    /// On macOS, this returns a UUID identifier since MAC addresses are not exposed.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get the detected device type.
    pub fn device_type(&self) -> Option<DeviceType> {
        self.device_type
    }

    /// Read the current RSSI (signal strength) of the connection.
    ///
    /// Returns the RSSI in dBm. More negative values indicate weaker signals.
    /// Typical values range from -30 (strong) to -90 (weak).
    pub async fn read_rssi(&self) -> Result<i16> {
        let properties = self.peripheral.properties().await?;
        properties
            .and_then(|p| p.rssi)
            .ok_or_else(|| Error::InvalidData("RSSI not available".to_string()))
    }

    /// Find a characteristic by UUID using the cached lookup table.
    ///
    /// Uses O(1) lookup from the characteristics cache built during service discovery.
    /// Falls back to searching through services if the cache is empty (shouldn't happen
    /// normally, but provides robustness).
    async fn find_characteristic(&self, uuid: Uuid) -> Result<Characteristic> {
        // Try cache first (O(1) lookup)
        {
            let cache = self.characteristics_cache.read().await;
            if let Some(char) = cache.get(&uuid) {
                return Ok(char.clone());
            }

            // If cache is populated but characteristic not found, it doesn't exist
            if !cache.is_empty() {
                return Err(Error::characteristic_not_found(
                    uuid.to_string(),
                    self.peripheral.services().len(),
                ));
            }
        }

        // Fallback: search services directly (shouldn't happen in normal operation)
        warn!(
            "Characteristics cache empty, falling back to service search for {}",
            uuid
        );
        let services = self.peripheral.services();
        let service_count = services.len();

        // First try Aranet-specific services
        for service in &services {
            if service.uuid == SAF_TEHNIKA_SERVICE_NEW || service.uuid == SAF_TEHNIKA_SERVICE_OLD {
                for char in &service.characteristics {
                    if char.uuid == uuid {
                        return Ok(char.clone());
                    }
                }
            }
        }

        // Then try standard services (GAP, Device Info, Battery)
        for service in &services {
            if service.uuid == GAP_SERVICE
                || service.uuid == DEVICE_INFO_SERVICE
                || service.uuid == BATTERY_SERVICE
            {
                for char in &service.characteristics {
                    if char.uuid == uuid {
                        return Ok(char.clone());
                    }
                }
            }
        }

        // Finally search all services
        for service in &services {
            for char in &service.characteristics {
                if char.uuid == uuid {
                    return Ok(char.clone());
                }
            }
        }

        Err(Error::characteristic_not_found(
            uuid.to_string(),
            service_count,
        ))
    }

    /// Read a characteristic value by UUID.
    ///
    /// This method includes a timeout to prevent indefinite hangs on BLE operations.
    /// The timeout is controlled by [`ConnectionConfig::read_timeout`].
    pub async fn read_characteristic(&self, uuid: Uuid) -> Result<Vec<u8>> {
        let characteristic = self.find_characteristic(uuid).await?;
        let data = timeout(self.config.read_timeout, self.peripheral.read(&characteristic))
            .await
            .map_err(|_| Error::Timeout {
                operation: format!("read characteristic {}", uuid),
                duration: self.config.read_timeout,
            })??;
        Ok(data)
    }

    /// Read a characteristic value with a custom timeout.
    ///
    /// Use this when you need a different timeout than the default,
    /// for example when reading large data.
    pub async fn read_characteristic_with_timeout(
        &self,
        uuid: Uuid,
        read_timeout: Duration,
    ) -> Result<Vec<u8>> {
        let characteristic = self.find_characteristic(uuid).await?;
        let data = timeout(read_timeout, self.peripheral.read(&characteristic))
            .await
            .map_err(|_| Error::Timeout {
                operation: format!("read characteristic {}", uuid),
                duration: read_timeout,
            })??;
        Ok(data)
    }

    /// Write a value to a characteristic.
    ///
    /// This method includes a timeout to prevent indefinite hangs on BLE operations.
    /// The timeout is controlled by [`ConnectionConfig::write_timeout`].
    pub async fn write_characteristic(&self, uuid: Uuid, data: &[u8]) -> Result<()> {
        let characteristic = self.find_characteristic(uuid).await?;
        timeout(
            self.config.write_timeout,
            self.peripheral
                .write(&characteristic, data, WriteType::WithResponse),
        )
        .await
        .map_err(|_| Error::Timeout {
            operation: format!("write characteristic {}", uuid),
            duration: self.config.write_timeout,
        })??;
        Ok(())
    }

    /// Write a value to a characteristic with a custom timeout.
    pub async fn write_characteristic_with_timeout(
        &self,
        uuid: Uuid,
        data: &[u8],
        write_timeout: Duration,
    ) -> Result<()> {
        let characteristic = self.find_characteristic(uuid).await?;
        timeout(
            write_timeout,
            self.peripheral
                .write(&characteristic, data, WriteType::WithResponse),
        )
        .await
        .map_err(|_| Error::Timeout {
            operation: format!("write characteristic {}", uuid),
            duration: write_timeout,
        })??;
        Ok(())
    }

    /// Read current sensor measurements.
    ///
    /// Automatically selects the correct characteristic UUID based on device type:
    /// - Aranet4 uses `f0cd3001`
    /// - Aranet2, Radon, Radiation use `f0cd3003`
    #[tracing::instrument(level = "debug", skip(self), fields(device_name = ?self.name, device_type = ?self.device_type))]
    pub async fn read_current(&self) -> Result<CurrentReading> {
        // Try primary characteristic first (Aranet4)
        let data = match self.read_characteristic(CURRENT_READINGS_DETAIL).await {
            Ok(data) => data,
            Err(Error::CharacteristicNotFound { .. }) => {
                // Try alternative characteristic (Aranet2/Radon/Radiation)
                debug!("Primary reading characteristic not found, trying alternative");
                self.read_characteristic(CURRENT_READINGS_DETAIL_ALT)
                    .await?
            }
            Err(e) => return Err(e),
        };

        // Parse based on device type
        match self.device_type {
            Some(DeviceType::Aranet4) | None => {
                // Default to Aranet4 parsing
                Ok(CurrentReading::from_bytes(&data)?)
            }
            Some(DeviceType::Aranet2) => crate::readings::parse_aranet2_reading(&data),
            Some(DeviceType::AranetRadon) => crate::readings::parse_aranet_radon_gatt(&data),
            Some(DeviceType::AranetRadiation) => {
                // Use dedicated radiation parser that extracts dose rate, total dose, and duration
                crate::readings::parse_aranet_radiation_gatt(&data).map(|ext| ext.reading)
            }
            // Handle future device types - default to Aranet4 parsing
            Some(_) => Ok(CurrentReading::from_bytes(&data)?),
        }
    }

    /// Read the battery level (0-100).
    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn read_battery(&self) -> Result<u8> {
        let data = self.read_characteristic(BATTERY_LEVEL).await?;
        if data.is_empty() {
            return Err(Error::InvalidData("Empty battery data".to_string()));
        }
        Ok(data[0])
    }

    /// Read device information.
    ///
    /// This method reads all device info characteristics in parallel for better performance.
    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn read_device_info(&self) -> Result<DeviceInfo> {
        fn read_string(data: Vec<u8>) -> String {
            String::from_utf8(data)
                .unwrap_or_default()
                .trim_end_matches('\0')
                .to_string()
        }

        // Read all characteristics in parallel for better performance
        let (
            name_result,
            model_result,
            serial_result,
            firmware_result,
            hardware_result,
            software_result,
            manufacturer_result,
        ) = tokio::join!(
            self.read_characteristic(DEVICE_NAME),
            self.read_characteristic(MODEL_NUMBER),
            self.read_characteristic(SERIAL_NUMBER),
            self.read_characteristic(FIRMWARE_REVISION),
            self.read_characteristic(HARDWARE_REVISION),
            self.read_characteristic(SOFTWARE_REVISION),
            self.read_characteristic(MANUFACTURER_NAME),
        );

        let name = name_result
            .map(read_string)
            .unwrap_or_else(|_| self.name.clone().unwrap_or_default());

        let model = model_result.map(read_string).unwrap_or_default();
        let serial = serial_result.map(read_string).unwrap_or_default();
        let firmware = firmware_result.map(read_string).unwrap_or_default();
        let hardware = hardware_result.map(read_string).unwrap_or_default();
        let software = software_result.map(read_string).unwrap_or_default();
        let manufacturer = manufacturer_result.map(read_string).unwrap_or_default();

        Ok(DeviceInfo {
            name,
            model,
            serial,
            firmware,
            hardware,
            software,
            manufacturer,
        })
    }

    /// Read essential device information only.
    ///
    /// This is a faster alternative to [`read_device_info`] that only reads
    /// the most critical characteristics: name, serial number, and firmware version.
    /// Use this for faster startup when full device info isn't needed immediately.
    #[tracing::instrument(level = "debug", skip(self))]
    pub async fn read_device_info_essential(&self) -> Result<DeviceInfo> {
        fn read_string(data: Vec<u8>) -> String {
            String::from_utf8(data)
                .unwrap_or_default()
                .trim_end_matches('\0')
                .to_string()
        }

        // Only read the essential characteristics in parallel
        let (name_result, serial_result, firmware_result) = tokio::join!(
            self.read_characteristic(DEVICE_NAME),
            self.read_characteristic(SERIAL_NUMBER),
            self.read_characteristic(FIRMWARE_REVISION),
        );

        let name = name_result
            .map(read_string)
            .unwrap_or_else(|_| self.name.clone().unwrap_or_default());
        let serial = serial_result.map(read_string).unwrap_or_default();
        let firmware = firmware_result.map(read_string).unwrap_or_default();

        Ok(DeviceInfo {
            name,
            model: String::new(),
            serial,
            firmware,
            hardware: String::new(),
            software: String::new(),
            manufacturer: String::new(),
        })
    }

    /// Subscribe to notifications on a characteristic.
    ///
    /// The callback will be invoked for each notification received.
    /// The notification handler task is tracked and will be aborted when
    /// `disconnect()` is called.
    pub async fn subscribe_to_notifications<F>(&self, uuid: Uuid, callback: F) -> Result<()>
    where
        F: Fn(&[u8]) + Send + Sync + 'static,
    {
        let characteristic = self.find_characteristic(uuid).await?;

        self.peripheral.subscribe(&characteristic).await?;

        // Set up notification handler
        let mut stream = self.peripheral.notifications().await?;
        let char_uuid = characteristic.uuid;

        let handle = tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(notification) = stream.next().await {
                if notification.uuid == char_uuid {
                    callback(&notification.value);
                }
            }
        });

        // Store the handle for cleanup on disconnect
        self.notification_handles.lock().await.push(handle);

        Ok(())
    }

    /// Unsubscribe from notifications on a characteristic.
    pub async fn unsubscribe_from_notifications(&self, uuid: Uuid) -> Result<()> {
        let characteristic = self.find_characteristic(uuid).await?;
        self.peripheral.unsubscribe(&characteristic).await?;
        Ok(())
    }

    /// Get the number of cached characteristics.
    ///
    /// This is useful for debugging and testing to verify service discovery worked.
    pub async fn cached_characteristic_count(&self) -> usize {
        self.characteristics_cache.read().await.len()
    }
}

// NOTE: Drop performs best-effort cleanup if disconnect() was not called.
// The cleanup is spawned as a background task and may not complete during shutdown.
// For reliable cleanup, callers SHOULD explicitly call `device.disconnect().await`
// before dropping the Device.
//
// The cleanup behavior:
// 1. Aborts all notification handlers (sync operation)
// 2. Spawns an async task to disconnect the peripheral (best-effort)
// 3. Logs a warning about the implicit cleanup
//
// For automatic cleanup, consider using `ReconnectingDevice` which manages the lifecycle.

impl Drop for Device {
    fn drop(&mut self) {
        if !self.disconnected.load(Ordering::SeqCst) {
            // Mark as disconnected to prevent double-cleanup
            self.disconnected.store(true, Ordering::SeqCst);

            // Log warning about implicit cleanup
            warn!(
                device_name = ?self.name,
                device_address = %self.address,
                "Device dropped without calling disconnect() - performing best-effort cleanup. \
                 For reliable cleanup, call device.disconnect().await before dropping."
            );

            // Best-effort cleanup: abort notification handlers
            // We can't use .await here, so we try_lock and abort synchronously
            if let Ok(mut handles) = self.notification_handles.try_lock() {
                for handle in handles.drain(..) {
                    handle.abort();
                }
            }

            // Spawn a best-effort cleanup task for the BLE disconnect
            // This uses try_runtime to handle the case where the runtime is shutting down
            let peripheral = self.peripheral.clone();
            let address = self.address.clone();

            // Try to spawn cleanup task - this may fail if runtime is shutting down
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    if let Err(e) = peripheral.disconnect().await {
                        debug!(
                            device_address = %address,
                            error = %e,
                            "Best-effort disconnect failed (device may already be disconnected)"
                        );
                    } else {
                        debug!(
                            device_address = %address,
                            "Best-effort disconnect completed"
                        );
                    }
                });
            }
        }
    }
}

#[async_trait]
impl AranetDevice for Device {
    // --- Connection Management ---

    async fn is_connected(&self) -> bool {
        Device::is_connected(self).await
    }

    async fn disconnect(&self) -> Result<()> {
        Device::disconnect(self).await
    }

    // --- Device Identity ---

    fn name(&self) -> Option<&str> {
        Device::name(self)
    }

    fn address(&self) -> &str {
        Device::address(self)
    }

    fn device_type(&self) -> Option<DeviceType> {
        Device::device_type(self)
    }

    // --- Current Readings ---

    async fn read_current(&self) -> Result<CurrentReading> {
        Device::read_current(self).await
    }

    async fn read_device_info(&self) -> Result<DeviceInfo> {
        Device::read_device_info(self).await
    }

    async fn read_rssi(&self) -> Result<i16> {
        Device::read_rssi(self).await
    }

    // --- Battery ---

    async fn read_battery(&self) -> Result<u8> {
        Device::read_battery(self).await
    }

    // --- History ---

    async fn get_history_info(&self) -> Result<crate::history::HistoryInfo> {
        Device::get_history_info(self).await
    }

    async fn download_history(&self) -> Result<Vec<aranet_types::HistoryRecord>> {
        Device::download_history(self).await
    }

    async fn download_history_with_options(
        &self,
        options: crate::history::HistoryOptions,
    ) -> Result<Vec<aranet_types::HistoryRecord>> {
        Device::download_history_with_options(self, options).await
    }

    // --- Settings ---

    async fn get_interval(&self) -> Result<crate::settings::MeasurementInterval> {
        Device::get_interval(self).await
    }

    async fn set_interval(&self, interval: crate::settings::MeasurementInterval) -> Result<()> {
        Device::set_interval(self, interval).await
    }

    async fn get_calibration(&self) -> Result<crate::settings::CalibrationData> {
        Device::get_calibration(self).await
    }
}
