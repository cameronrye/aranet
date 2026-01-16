//! Aranet device connection and communication.
//!
//! This module provides the main interface for connecting to and
//! communicating with Aranet sensors over Bluetooth Low Energy.

use std::time::Duration;

use async_trait::async_trait;
use btleplug::api::{Characteristic, Peripheral as _, WriteType};
use btleplug::platform::{Adapter, Peripheral};
use tokio::time::timeout;
use tracing::{debug, info};
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::scan::{find_device, ScanOptions};
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
    /// Handles for spawned notification tasks (for cleanup).
    notification_handles: tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Provide a clean debug output that excludes internal BLE details
        // (adapter, peripheral, notification_handles) which are not useful
        // for debugging application logic and may expose implementation details.
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("address", &self.address)
            .field("device_type", &self.device_type)
            .field("services_discovered", &self.services_discovered)
            .finish_non_exhaustive()
    }
}

/// Default timeout for BLE characteristic read operations.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for BLE characteristic write operations.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

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
        Self::connect_with_timeout(identifier, Duration::from_secs(15)).await
    }

    /// Connect to an Aranet device with a custom scan timeout.
    #[tracing::instrument(level = "info", skip_all, fields(identifier = %identifier, timeout_secs = timeout.as_secs()))]
    pub async fn connect_with_timeout(identifier: &str, timeout: Duration) -> Result<Self> {
        let options = ScanOptions {
            duration: timeout,
            filter_aranet_only: false, // We're looking for a specific device
        };

        // Try find_device first (uses default 5s scan), then with custom options
        let (adapter, peripheral) = match find_device(identifier).await {
            Ok(result) => result,
            Err(_) => crate::scan::find_device_with_options(identifier, options).await?,
        };

        Self::from_peripheral(adapter, peripheral).await
    }

    /// Create a Device from an already-discovered peripheral.
    #[tracing::instrument(level = "info", skip_all)]
    pub async fn from_peripheral(adapter: Adapter, peripheral: Peripheral) -> Result<Self> {
        // Connect to the device
        info!("Connecting to device...");
        peripheral.connect().await?;
        info!("Connected!");

        // Discover services
        info!("Discovering services...");
        peripheral.discover_services().await?;

        let services = peripheral.services();
        debug!("Found {} services", services.len());
        for service in &services {
            debug!("  Service: {}", service.uuid);
            for char in &service.characteristics {
                debug!("    Characteristic: {}", char.uuid);
            }
        }

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
            notification_handles: tokio::sync::Mutex::new(Vec::new()),
        })
    }

    /// Check if the device is connected.
    pub async fn is_connected(&self) -> bool {
        self.peripheral.is_connected().await.unwrap_or(false)
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

    /// Find a characteristic by UUID, searching through known Aranet services.
    fn find_characteristic(&self, uuid: Uuid) -> Result<Characteristic> {
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

        Err(Error::characteristic_not_found(uuid.to_string(), service_count))
    }

    /// Read a characteristic value by UUID.
    ///
    /// This method includes a timeout to prevent indefinite hangs on BLE operations.
    /// The default timeout is 10 seconds.
    pub async fn read_characteristic(&self, uuid: Uuid) -> Result<Vec<u8>> {
        let characteristic = self.find_characteristic(uuid)?;
        let data = timeout(READ_TIMEOUT, self.peripheral.read(&characteristic))
            .await
            .map_err(|_| Error::Timeout {
                operation: format!("read characteristic {}", uuid),
                duration: READ_TIMEOUT,
            })??;
        Ok(data)
    }

    /// Write a value to a characteristic.
    ///
    /// This method includes a timeout to prevent indefinite hangs on BLE operations.
    /// The default timeout is 10 seconds.
    pub async fn write_characteristic(&self, uuid: Uuid, data: &[u8]) -> Result<()> {
        let characteristic = self.find_characteristic(uuid)?;
        timeout(
            WRITE_TIMEOUT,
            self.peripheral
                .write(&characteristic, data, WriteType::WithResponse),
        )
        .await
        .map_err(|_| Error::Timeout {
            operation: format!("write characteristic {}", uuid),
            duration: WRITE_TIMEOUT,
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
        let (name_result, model_result, serial_result, firmware_result, hardware_result, software_result, manufacturer_result) = tokio::join!(
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

    /// Subscribe to notifications on a characteristic.
    ///
    /// The callback will be invoked for each notification received.
    /// The notification handler task is tracked and will be aborted when
    /// `disconnect()` is called.
    pub async fn subscribe_to_notifications<F>(&self, uuid: Uuid, callback: F) -> Result<()>
    where
        F: Fn(&[u8]) + Send + Sync + 'static,
    {
        let characteristic = self.find_characteristic(uuid)?;

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
        let characteristic = self.find_characteristic(uuid)?;
        self.peripheral.unsubscribe(&characteristic).await?;
        Ok(())
    }
}

// NOTE: We intentionally do NOT implement Drop for Device.
//
// The previous implementation spawned a thread and used `futures::executor::block_on`
// which can panic if called from within an async runtime. This is problematic because:
// 1. Device is typically used in async contexts
// 2. Spawning threads in Drop is unpredictable and can cause issues during shutdown
// 3. Cleanup should be explicit, not implicit
//
// Callers MUST explicitly call `device.disconnect().await` before dropping the Device.
// For automatic cleanup, consider using `ReconnectingDevice` which manages the lifecycle.

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
