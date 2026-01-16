//! Trait abstractions for Aranet device operations.
//!
//! This module provides the [`AranetDevice`] trait that abstracts over
//! real Bluetooth devices and mock devices for testing.

use async_trait::async_trait;

use aranet_types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord};

use crate::error::Result;
use crate::history::{HistoryInfo, HistoryOptions};
use crate::settings::{CalibrationData, MeasurementInterval};

/// Trait abstracting Aranet device operations.
///
/// This trait enables writing code that works with both real Bluetooth devices
/// and mock devices for testing. Implement this trait for any type that can
/// provide Aranet sensor data.
///
/// # Example
///
/// ```ignore
/// use aranet_core::{AranetDevice, Result};
///
/// async fn print_reading<D: AranetDevice>(device: &D) -> Result<()> {
///     let reading = device.read_current().await?;
///     println!("CO2: {} ppm", reading.co2);
///     Ok(())
/// }
/// ```
#[async_trait]
pub trait AranetDevice: Send + Sync {
    // --- Connection Management ---

    /// Check if the device is connected.
    async fn is_connected(&self) -> bool;

    /// Connect to the device.
    ///
    /// For devices that are already connected, this should be a no-op.
    /// For devices that support reconnection, this should attempt to reconnect.
    ///
    /// The default implementation returns `Ok(())` for backwards compatibility.
    async fn connect(&self) -> Result<()> {
        Ok(())
    }

    /// Disconnect from the device.
    async fn disconnect(&self) -> Result<()>;

    // --- Device Identity ---

    /// Get the device name, if available.
    fn name(&self) -> Option<&str>;

    /// Get the device address or identifier.
    ///
    /// On Linux/Windows this is typically the MAC address.
    /// On macOS this is a UUID since MAC addresses are not exposed.
    fn address(&self) -> &str;

    /// Get the detected device type, if available.
    fn device_type(&self) -> Option<DeviceType>;

    // --- Current Readings ---

    /// Read the current sensor values.
    async fn read_current(&self) -> Result<CurrentReading>;

    /// Read device information (model, serial, firmware version, etc.).
    async fn read_device_info(&self) -> Result<DeviceInfo>;

    /// Read the current RSSI (signal strength) in dBm.
    ///
    /// More negative values indicate weaker signals.
    /// Typical values range from -30 (strong) to -90 (weak).
    async fn read_rssi(&self) -> Result<i16>;

    // --- Battery ---

    /// Read the battery level (0-100).
    async fn read_battery(&self) -> Result<u8>;

    // --- History ---

    /// Get information about stored history.
    async fn get_history_info(&self) -> Result<HistoryInfo>;

    /// Download all historical readings.
    async fn download_history(&self) -> Result<Vec<HistoryRecord>>;

    /// Download historical readings with custom options.
    async fn download_history_with_options(
        &self,
        options: HistoryOptions,
    ) -> Result<Vec<HistoryRecord>>;

    // --- Settings ---

    /// Get the current measurement interval.
    async fn get_interval(&self) -> Result<MeasurementInterval>;

    /// Set the measurement interval.
    async fn set_interval(&self, interval: MeasurementInterval) -> Result<()>;

    /// Read calibration data from the device.
    async fn get_calibration(&self) -> Result<CalibrationData>;
}

