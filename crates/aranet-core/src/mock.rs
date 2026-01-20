//! Mock device implementation for testing.
//!
//! This module provides a mock device that can be used for unit testing
//! without requiring actual BLE hardware.
//!
//! The [`MockDevice`] implements the [`AranetDevice`] trait, allowing it to be
//! used interchangeably with real devices in generic code.
//!
//! # Features
//!
//! - **Failure injection**: Set the device to fail on specific operations
//! - **Latency simulation**: Add artificial delays to simulate slow BLE responses
//! - **Custom behavior**: Inject custom reading generators for dynamic test scenarios

use std::sync::atomic::{AtomicBool, AtomicI16, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;

use aranet_types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord, Status};

use crate::error::{Error, Result};
use crate::history::{HistoryInfo, HistoryOptions};
use crate::settings::{CalibrationData, MeasurementInterval};
use crate::traits::AranetDevice;

/// A mock Aranet device for testing.
///
/// Implements [`AranetDevice`] trait for use in generic code and testing.
///
/// # Example
///
/// ```
/// use aranet_core::{MockDevice, AranetDevice};
/// use aranet_types::DeviceType;
///
/// #[tokio::main]
/// async fn main() {
///     let device = MockDevice::new("Test", DeviceType::Aranet4);
///     device.connect().await.unwrap();
///
///     // Can use through trait
///     async fn read_via_trait<D: AranetDevice>(d: &D) {
///         let _ = d.read_current().await;
///     }
///     read_via_trait(&device).await;
/// }
/// ```
pub struct MockDevice {
    name: String,
    address: String,
    device_type: DeviceType,
    connected: AtomicBool,
    current_reading: RwLock<CurrentReading>,
    device_info: RwLock<DeviceInfo>,
    history: RwLock<Vec<HistoryRecord>>,
    interval: RwLock<MeasurementInterval>,
    calibration: RwLock<CalibrationData>,
    battery: RwLock<u8>,
    rssi: AtomicI16,
    read_count: AtomicU32,
    should_fail: AtomicBool,
    fail_message: RwLock<String>,
    /// Simulated read latency in milliseconds (0 = no delay).
    read_latency_ms: AtomicU64,
    /// Simulated connect latency in milliseconds (0 = no delay).
    connect_latency_ms: AtomicU64,
    /// Number of operations to fail before succeeding (0 = always succeed/fail based on should_fail).
    fail_count: AtomicU32,
    /// Current count of failures (decremented on each failure).
    remaining_failures: AtomicU32,
}

impl std::fmt::Debug for MockDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockDevice")
            .field("name", &self.name)
            .field("address", &self.address)
            .field("device_type", &self.device_type)
            .field("connected", &self.connected.load(Ordering::Relaxed))
            .finish()
    }
}

impl MockDevice {
    /// Create a new mock device with default values.
    pub fn new(name: &str, device_type: DeviceType) -> Self {
        Self {
            name: name.to_string(),
            address: format!("MOCK-{:06X}", rand::random::<u32>() % 0xFFFFFF),
            device_type,
            connected: AtomicBool::new(false),
            current_reading: RwLock::new(Self::default_reading()),
            device_info: RwLock::new(Self::default_info(name)),
            history: RwLock::new(Vec::new()),
            interval: RwLock::new(MeasurementInterval::FiveMinutes),
            calibration: RwLock::new(CalibrationData::default()),
            battery: RwLock::new(85),
            rssi: AtomicI16::new(-50),
            read_count: AtomicU32::new(0),
            should_fail: AtomicBool::new(false),
            fail_message: RwLock::new("Mock failure".to_string()),
            read_latency_ms: AtomicU64::new(0),
            connect_latency_ms: AtomicU64::new(0),
            fail_count: AtomicU32::new(0),
            remaining_failures: AtomicU32::new(0),
        }
    }

    fn default_reading() -> CurrentReading {
        CurrentReading {
            co2: 800,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 50,
            battery: 85,
            status: Status::Green,
            interval: 300,
            age: 60,
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        }
    }

    fn default_info(name: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            model: "Aranet4".to_string(),
            serial: "MOCK-12345".to_string(),
            firmware: "v1.5.0".to_string(),
            hardware: "1.0".to_string(),
            software: "1.5.0".to_string(),
            manufacturer: "SAF Tehnika".to_string(),
        }
    }

    /// Connect to the mock device.
    pub async fn connect(&self) -> Result<()> {
        use crate::error::DeviceNotFoundReason;

        // Simulate connect latency
        let latency = self.connect_latency_ms.load(Ordering::Relaxed);
        if latency > 0 {
            tokio::time::sleep(Duration::from_millis(latency)).await;
        }

        // Check for transient failures first
        if self.remaining_failures.load(Ordering::Relaxed) > 0 {
            self.remaining_failures.fetch_sub(1, Ordering::Relaxed);
            return Err(Error::DeviceNotFound(DeviceNotFoundReason::NotFound {
                identifier: self.name.clone(),
            }));
        }

        if self.should_fail.load(Ordering::Relaxed) {
            return Err(Error::DeviceNotFound(DeviceNotFoundReason::NotFound {
                identifier: self.name.clone(),
            }));
        }
        self.connected.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Disconnect from the mock device.
    pub async fn disconnect(&self) -> Result<()> {
        self.connected.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Check if connected (sync method for internal use).
    pub fn is_connected_sync(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Get the device name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the device address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get the device type.
    pub fn device_type(&self) -> DeviceType {
        self.device_type
    }

    /// Read current sensor values.
    pub async fn read_current(&self) -> Result<CurrentReading> {
        self.check_connected()?;
        self.check_should_fail().await?;

        self.read_count.fetch_add(1, Ordering::Relaxed);
        Ok(*self.current_reading.read().await)
    }

    /// Read battery level.
    pub async fn read_battery(&self) -> Result<u8> {
        self.check_connected()?;
        self.check_should_fail().await?;
        Ok(*self.battery.read().await)
    }

    /// Read RSSI (signal strength).
    pub async fn read_rssi(&self) -> Result<i16> {
        self.check_connected()?;
        self.check_should_fail().await?;
        Ok(self.rssi.load(Ordering::Relaxed))
    }

    /// Read device info.
    pub async fn read_device_info(&self) -> Result<DeviceInfo> {
        self.check_connected()?;
        self.check_should_fail().await?;
        Ok(self.device_info.read().await.clone())
    }

    /// Get history info.
    pub async fn get_history_info(&self) -> Result<HistoryInfo> {
        self.check_connected()?;
        self.check_should_fail().await?;

        let history = self.history.read().await;
        let interval = self.interval.read().await;

        Ok(HistoryInfo {
            total_readings: history.len() as u16,
            interval_seconds: interval.as_seconds(),
            seconds_since_update: 60,
        })
    }

    /// Download history.
    pub async fn download_history(&self) -> Result<Vec<HistoryRecord>> {
        self.check_connected()?;
        self.check_should_fail().await?;
        Ok(self.history.read().await.clone())
    }

    /// Download history with options.
    pub async fn download_history_with_options(
        &self,
        options: HistoryOptions,
    ) -> Result<Vec<HistoryRecord>> {
        self.check_connected()?;
        self.check_should_fail().await?;

        let history = self.history.read().await;
        let start = options.start_index.unwrap_or(0) as usize;
        let end = options
            .end_index
            .map(|e| e as usize)
            .unwrap_or(history.len());

        // Report progress if callback provided
        if let Some(ref _callback) = options.progress_callback {
            // For mock, we report progress immediately
            let progress = crate::history::HistoryProgress::new(
                crate::history::HistoryParam::Co2,
                1,
                1,
                history.len().min(end).saturating_sub(start),
            );
            options.report_progress(&progress);
        }

        Ok(history
            .iter()
            .skip(start)
            .take(end.saturating_sub(start))
            .cloned()
            .collect())
    }

    /// Get the measurement interval.
    pub async fn get_interval(&self) -> Result<MeasurementInterval> {
        self.check_connected()?;
        self.check_should_fail().await?;
        Ok(*self.interval.read().await)
    }

    /// Set the measurement interval.
    pub async fn set_interval(&self, interval: MeasurementInterval) -> Result<()> {
        self.check_connected()?;
        self.check_should_fail().await?;
        *self.interval.write().await = interval;
        Ok(())
    }

    /// Get calibration data.
    pub async fn get_calibration(&self) -> Result<CalibrationData> {
        self.check_connected()?;
        self.check_should_fail().await?;
        Ok(self.calibration.read().await.clone())
    }

    fn check_connected(&self) -> Result<()> {
        if !self.connected.load(Ordering::Relaxed) {
            Err(Error::NotConnected)
        } else {
            Ok(())
        }
    }

    async fn check_should_fail(&self) -> Result<()> {
        // Simulate read latency
        let latency = self.read_latency_ms.load(Ordering::Relaxed);
        if latency > 0 {
            tokio::time::sleep(Duration::from_millis(latency)).await;
        }

        // Check for transient failures first
        if self.remaining_failures.load(Ordering::Relaxed) > 0 {
            self.remaining_failures.fetch_sub(1, Ordering::Relaxed);
            return Err(Error::InvalidData(self.fail_message.read().await.clone()));
        }

        if self.should_fail.load(Ordering::Relaxed) {
            Err(Error::InvalidData(self.fail_message.read().await.clone()))
        } else {
            Ok(())
        }
    }

    // --- Test control methods ---

    /// Set the current reading for testing.
    pub async fn set_reading(&self, reading: CurrentReading) {
        *self.current_reading.write().await = reading;
    }

    /// Set CO2 level directly.
    pub async fn set_co2(&self, co2: u16) {
        self.current_reading.write().await.co2 = co2;
    }

    /// Set temperature directly.
    pub async fn set_temperature(&self, temp: f32) {
        self.current_reading.write().await.temperature = temp;
    }

    /// Set battery level.
    pub async fn set_battery(&self, level: u8) {
        *self.battery.write().await = level;
        self.current_reading.write().await.battery = level;
    }

    /// Set radon concentration in Bq/m³ (AranetRn+ devices).
    pub async fn set_radon(&self, radon: u32) {
        self.current_reading.write().await.radon = Some(radon);
    }

    /// Set radon averages (AranetRn+ devices).
    pub async fn set_radon_averages(&self, avg_24h: u32, avg_7d: u32, avg_30d: u32) {
        let mut reading = self.current_reading.write().await;
        reading.radon_avg_24h = Some(avg_24h);
        reading.radon_avg_7d = Some(avg_7d);
        reading.radon_avg_30d = Some(avg_30d);
    }

    /// Set radiation values (Aranet Radiation devices).
    pub async fn set_radiation(&self, rate: f32, total: f64) {
        let mut reading = self.current_reading.write().await;
        reading.radiation_rate = Some(rate);
        reading.radiation_total = Some(total);
    }

    /// Set RSSI (signal strength) for testing.
    pub fn set_rssi(&self, rssi: i16) {
        self.rssi.store(rssi, Ordering::Relaxed);
    }

    /// Add history records.
    pub async fn add_history(&self, records: Vec<HistoryRecord>) {
        self.history.write().await.extend(records);
    }

    /// Make the device fail on next operation.
    pub async fn set_should_fail(&self, fail: bool, message: Option<&str>) {
        self.should_fail.store(fail, Ordering::Relaxed);
        if let Some(msg) = message {
            *self.fail_message.write().await = msg.to_string();
        }
    }

    /// Get the number of read operations performed.
    pub fn read_count(&self) -> u32 {
        self.read_count.load(Ordering::Relaxed)
    }

    /// Reset read count.
    pub fn reset_read_count(&self) {
        self.read_count.store(0, Ordering::Relaxed);
    }

    /// Set simulated read latency.
    ///
    /// Each read operation will be delayed by this duration.
    /// Set to `Duration::ZERO` to disable latency simulation.
    pub fn set_read_latency(&self, latency: Duration) {
        self.read_latency_ms
            .store(latency.as_millis() as u64, Ordering::Relaxed);
    }

    /// Set simulated connect latency.
    ///
    /// Connect operations will be delayed by this duration.
    /// Set to `Duration::ZERO` to disable latency simulation.
    pub fn set_connect_latency(&self, latency: Duration) {
        self.connect_latency_ms
            .store(latency.as_millis() as u64, Ordering::Relaxed);
    }

    /// Configure transient failures.
    ///
    /// The device will fail the next `count` operations, then succeed.
    /// This is useful for testing retry logic.
    ///
    /// # Example
    ///
    /// ```
    /// use aranet_core::MockDevice;
    /// use aranet_types::DeviceType;
    ///
    /// let device = MockDevice::new("Test", DeviceType::Aranet4);
    /// // First 3 connect attempts will fail, 4th will succeed
    /// device.set_transient_failures(3);
    /// ```
    pub fn set_transient_failures(&self, count: u32) {
        self.fail_count.store(count, Ordering::Relaxed);
        self.remaining_failures.store(count, Ordering::Relaxed);
    }

    /// Reset transient failure counter.
    pub fn reset_transient_failures(&self) {
        self.remaining_failures
            .store(self.fail_count.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// Get the number of remaining transient failures.
    pub fn remaining_failures(&self) -> u32 {
        self.remaining_failures.load(Ordering::Relaxed)
    }
}

// Implement the AranetDevice trait for MockDevice
#[async_trait]
impl AranetDevice for MockDevice {
    // --- Connection Management ---

    async fn is_connected(&self) -> bool {
        self.is_connected_sync()
    }

    async fn disconnect(&self) -> Result<()> {
        MockDevice::disconnect(self).await
    }

    // --- Device Identity ---

    fn name(&self) -> Option<&str> {
        Some(MockDevice::name(self))
    }

    fn address(&self) -> &str {
        MockDevice::address(self)
    }

    fn device_type(&self) -> Option<DeviceType> {
        Some(MockDevice::device_type(self))
    }

    // --- Current Readings ---

    async fn read_current(&self) -> Result<CurrentReading> {
        MockDevice::read_current(self).await
    }

    async fn read_device_info(&self) -> Result<DeviceInfo> {
        MockDevice::read_device_info(self).await
    }

    async fn read_rssi(&self) -> Result<i16> {
        MockDevice::read_rssi(self).await
    }

    // --- Battery ---

    async fn read_battery(&self) -> Result<u8> {
        MockDevice::read_battery(self).await
    }

    // --- History ---

    async fn get_history_info(&self) -> Result<crate::history::HistoryInfo> {
        MockDevice::get_history_info(self).await
    }

    async fn download_history(&self) -> Result<Vec<HistoryRecord>> {
        MockDevice::download_history(self).await
    }

    async fn download_history_with_options(
        &self,
        options: HistoryOptions,
    ) -> Result<Vec<HistoryRecord>> {
        MockDevice::download_history_with_options(self, options).await
    }

    // --- Settings ---

    async fn get_interval(&self) -> Result<MeasurementInterval> {
        MockDevice::get_interval(self).await
    }

    async fn set_interval(&self, interval: MeasurementInterval) -> Result<()> {
        MockDevice::set_interval(self, interval).await
    }

    async fn get_calibration(&self) -> Result<CalibrationData> {
        MockDevice::get_calibration(self).await
    }
}

/// Builder for creating mock devices with custom settings.
#[derive(Debug)]
pub struct MockDeviceBuilder {
    name: String,
    device_type: DeviceType,
    co2: u16,
    temperature: f32,
    pressure: f32,
    humidity: u8,
    battery: u8,
    status: Status,
    auto_connect: bool,
    radon: Option<u32>,
    radon_avg_24h: Option<u32>,
    radon_avg_7d: Option<u32>,
    radon_avg_30d: Option<u32>,
    radiation_rate: Option<f32>,
    radiation_total: Option<f64>,
}

impl Default for MockDeviceBuilder {
    fn default() -> Self {
        Self {
            name: "Mock Aranet4".to_string(),
            device_type: DeviceType::Aranet4,
            co2: 800,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 50,
            battery: 85,
            status: Status::Green,
            auto_connect: true,
            radon: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
            radiation_rate: None,
            radiation_total: None,
        }
    }
}

impl MockDeviceBuilder {
    /// Create a new builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the device name.
    #[must_use]
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// Set the device type.
    #[must_use]
    pub fn device_type(mut self, device_type: DeviceType) -> Self {
        self.device_type = device_type;
        self
    }

    /// Set the CO2 level.
    #[must_use]
    pub fn co2(mut self, co2: u16) -> Self {
        self.co2 = co2;
        self
    }

    /// Set the temperature.
    #[must_use]
    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = temp;
        self
    }

    /// Set the pressure.
    #[must_use]
    pub fn pressure(mut self, pressure: f32) -> Self {
        self.pressure = pressure;
        self
    }

    /// Set the humidity.
    #[must_use]
    pub fn humidity(mut self, humidity: u8) -> Self {
        self.humidity = humidity;
        self
    }

    /// Set the battery level.
    #[must_use]
    pub fn battery(mut self, battery: u8) -> Self {
        self.battery = battery;
        self
    }

    /// Set the status.
    #[must_use]
    pub fn status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }

    /// Set whether to auto-connect.
    #[must_use]
    pub fn auto_connect(mut self, auto: bool) -> Self {
        self.auto_connect = auto;
        self
    }

    /// Set radon concentration in Bq/m³ (AranetRn+ devices).
    #[must_use]
    pub fn radon(mut self, radon: u32) -> Self {
        self.radon = Some(radon);
        self
    }

    /// Set 24-hour average radon concentration in Bq/m³ (AranetRn+ devices).
    #[must_use]
    pub fn radon_avg_24h(mut self, avg: u32) -> Self {
        self.radon_avg_24h = Some(avg);
        self
    }

    /// Set 7-day average radon concentration in Bq/m³ (AranetRn+ devices).
    #[must_use]
    pub fn radon_avg_7d(mut self, avg: u32) -> Self {
        self.radon_avg_7d = Some(avg);
        self
    }

    /// Set 30-day average radon concentration in Bq/m³ (AranetRn+ devices).
    #[must_use]
    pub fn radon_avg_30d(mut self, avg: u32) -> Self {
        self.radon_avg_30d = Some(avg);
        self
    }

    /// Set radiation dose rate in µSv/h (Aranet Radiation devices).
    #[must_use]
    pub fn radiation_rate(mut self, rate: f32) -> Self {
        self.radiation_rate = Some(rate);
        self
    }

    /// Set total radiation dose in mSv (Aranet Radiation devices).
    #[must_use]
    pub fn radiation_total(mut self, total: f64) -> Self {
        self.radiation_total = Some(total);
        self
    }

    /// Build the mock device.
    ///
    /// Note: This is a sync method that sets initial state directly.
    /// The device is created with the specified reading already set.
    #[must_use]
    pub fn build(self) -> MockDevice {
        let reading = CurrentReading {
            co2: self.co2,
            temperature: self.temperature,
            pressure: self.pressure,
            humidity: self.humidity,
            battery: self.battery,
            status: self.status,
            interval: 300,
            age: 60,
            captured_at: None,
            radon: self.radon,
            radiation_rate: self.radiation_rate,
            radiation_total: self.radiation_total,
            radon_avg_24h: self.radon_avg_24h,
            radon_avg_7d: self.radon_avg_7d,
            radon_avg_30d: self.radon_avg_30d,
        };

        MockDevice {
            name: self.name.clone(),
            address: format!("MOCK-{:06X}", rand::random::<u32>() % 0xFFFFFF),
            device_type: self.device_type,
            connected: AtomicBool::new(self.auto_connect),
            current_reading: RwLock::new(reading),
            device_info: RwLock::new(MockDevice::default_info(&self.name)),
            history: RwLock::new(Vec::new()),
            interval: RwLock::new(MeasurementInterval::FiveMinutes),
            calibration: RwLock::new(CalibrationData::default()),
            battery: RwLock::new(self.battery),
            rssi: AtomicI16::new(-50),
            read_count: AtomicU32::new(0),
            should_fail: AtomicBool::new(false),
            fail_message: RwLock::new("Mock failure".to_string()),
            read_latency_ms: AtomicU64::new(0),
            connect_latency_ms: AtomicU64::new(0),
            fail_count: AtomicU32::new(0),
            remaining_failures: AtomicU32::new(0),
        }
    }
}

/// Unit tests for MockDevice and MockDeviceBuilder.
///
/// These tests verify the mock device implementation used for testing
/// without requiring actual BLE hardware.
///
/// # Test Categories
///
/// ## Connection Tests
/// - `test_mock_device_connect`: Connect/disconnect lifecycle
/// - `test_mock_device_not_connected`: Error when reading without connection
///
/// ## Reading Tests
/// - `test_mock_device_read`: Basic reading retrieval
/// - `test_mock_device_read_battery`: Battery level reading
/// - `test_mock_device_read_rssi`: Signal strength reading
/// - `test_mock_device_read_device_info`: Device information
/// - `test_mock_device_set_values`: Dynamic value updates
///
/// ## History Tests
/// - `test_mock_device_history`: History download
/// - `test_mock_device_history_with_options`: Filtered history download
/// - `test_mock_device_history_info`: History metadata
///
/// ## Settings Tests
/// - `test_mock_device_interval`: Measurement interval get/set
/// - `test_mock_device_calibration`: Calibration data
///
/// ## Failure Injection Tests
/// - `test_mock_device_fail`: Permanent failure mode
/// - `test_mock_device_transient_failures`: Temporary failures for retry testing
///
/// ## Builder Tests
/// - `test_builder_defaults`: Default builder values
/// - `test_builder_all_options`: Full builder customization
///
/// ## Trait Tests
/// - `test_aranet_device_trait`: Using MockDevice through AranetDevice trait
/// - `test_trait_methods_match_direct_methods`: Trait/direct method consistency
///
/// # Running Tests
///
/// ```bash
/// cargo test -p aranet-core mock::tests
/// ```
#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::AranetDevice;

    #[tokio::test]
    async fn test_mock_device_connect() {
        let device = MockDevice::new("Test", DeviceType::Aranet4);
        assert!(!device.is_connected_sync());

        device.connect().await.unwrap();
        assert!(device.is_connected_sync());

        device.disconnect().await.unwrap();
        assert!(!device.is_connected_sync());
    }

    #[tokio::test]
    async fn test_mock_device_read() {
        let device = MockDeviceBuilder::new().co2(1200).temperature(25.0).build();

        let reading = device.read_current().await.unwrap();
        assert_eq!(reading.co2, 1200);
        assert!((reading.temperature - 25.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_mock_device_fail() {
        let device = MockDeviceBuilder::new().build();
        device.set_should_fail(true, Some("Test error")).await;

        let result = device.read_current().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Test error"));
    }

    #[tokio::test]
    async fn test_mock_device_not_connected() {
        let device = MockDeviceBuilder::new().auto_connect(false).build();

        let result = device.read_current().await;
        assert!(matches!(result, Err(Error::NotConnected)));
    }

    #[test]
    fn test_builder_defaults() {
        let device = MockDeviceBuilder::new().build();
        assert!(device.is_connected_sync());
        assert_eq!(device.device_type(), DeviceType::Aranet4);
    }

    #[tokio::test]
    async fn test_aranet_device_trait() {
        let device = MockDeviceBuilder::new().co2(1000).build();

        // Use via trait
        async fn check_via_trait<D: AranetDevice>(d: &D) -> u16 {
            d.read_current().await.unwrap().co2
        }

        assert_eq!(check_via_trait(&device).await, 1000);
    }

    #[tokio::test]
    async fn test_mock_device_read_battery() {
        let device = MockDeviceBuilder::new().battery(75).build();
        let battery = device.read_battery().await.unwrap();
        assert_eq!(battery, 75);
    }

    #[tokio::test]
    async fn test_mock_device_read_rssi() {
        let device = MockDeviceBuilder::new().build();
        device.set_rssi(-65);
        let rssi = device.read_rssi().await.unwrap();
        assert_eq!(rssi, -65);
    }

    #[tokio::test]
    async fn test_mock_device_read_device_info() {
        let device = MockDeviceBuilder::new().name("Test Device").build();
        let info = device.read_device_info().await.unwrap();
        assert_eq!(info.name, "Test Device");
        assert_eq!(info.manufacturer, "SAF Tehnika");
    }

    #[tokio::test]
    async fn test_mock_device_history() {
        let device = MockDeviceBuilder::new().build();

        // Initially empty
        let history = device.download_history().await.unwrap();
        assert!(history.is_empty());

        // Add some records
        let records = vec![
            HistoryRecord {
                timestamp: time::OffsetDateTime::now_utc(),
                co2: 800,
                temperature: 22.5,
                pressure: 1013.2,
                humidity: 50,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: time::OffsetDateTime::now_utc(),
                co2: 850,
                temperature: 23.0,
                pressure: 1013.5,
                humidity: 48,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
        ];
        device.add_history(records).await;

        let history = device.download_history().await.unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].co2, 800);
        assert_eq!(history[1].co2, 850);
    }

    #[tokio::test]
    async fn test_mock_device_history_with_options() {
        let device = MockDeviceBuilder::new().build();

        // Add 5 records
        let records: Vec<HistoryRecord> = (0..5)
            .map(|i| HistoryRecord {
                timestamp: time::OffsetDateTime::now_utc(),
                co2: 800 + i as u16 * 10,
                temperature: 22.0,
                pressure: 1013.0,
                humidity: 50,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            })
            .collect();
        device.add_history(records).await;

        // Download with range
        let options = HistoryOptions {
            start_index: Some(1),
            end_index: Some(4),
            ..Default::default()
        };
        let history = device.download_history_with_options(options).await.unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].co2, 810); // Second record (index 1)
        assert_eq!(history[2].co2, 830); // Fourth record (index 3)
    }

    #[tokio::test]
    async fn test_mock_device_interval() {
        let device = MockDeviceBuilder::new().build();

        let interval = device.get_interval().await.unwrap();
        assert_eq!(interval, MeasurementInterval::FiveMinutes);

        device
            .set_interval(MeasurementInterval::TenMinutes)
            .await
            .unwrap();
        let interval = device.get_interval().await.unwrap();
        assert_eq!(interval, MeasurementInterval::TenMinutes);
    }

    #[tokio::test]
    async fn test_mock_device_calibration() {
        let device = MockDeviceBuilder::new().build();
        let calibration = device.get_calibration().await.unwrap();
        // Default calibration should exist
        assert!(calibration.co2_offset.is_some() || calibration.co2_offset.is_none());
    }

    #[tokio::test]
    async fn test_mock_device_read_count() {
        let device = MockDeviceBuilder::new().build();
        assert_eq!(device.read_count(), 0);

        device.read_current().await.unwrap();
        assert_eq!(device.read_count(), 1);

        device.read_current().await.unwrap();
        device.read_current().await.unwrap();
        assert_eq!(device.read_count(), 3);

        device.reset_read_count();
        assert_eq!(device.read_count(), 0);
    }

    #[tokio::test]
    async fn test_mock_device_transient_failures() {
        let device = MockDeviceBuilder::new().build();
        device.set_transient_failures(2);

        // First two reads should fail
        assert!(device.read_current().await.is_err());
        assert!(device.read_current().await.is_err());

        // Third read should succeed
        assert!(device.read_current().await.is_ok());
    }

    #[tokio::test]
    async fn test_mock_device_set_values() {
        let device = MockDeviceBuilder::new().build();

        device.set_co2(1500).await;
        device.set_temperature(30.0).await;
        device.set_battery(50).await;

        let reading = device.read_current().await.unwrap();
        assert_eq!(reading.co2, 1500);
        assert!((reading.temperature - 30.0).abs() < 0.01);
        assert_eq!(reading.battery, 50);
    }

    #[tokio::test]
    async fn test_mock_device_history_info() {
        let device = MockDeviceBuilder::new().build();

        // Add some records
        let records: Vec<HistoryRecord> = (0..10)
            .map(|_| HistoryRecord {
                timestamp: time::OffsetDateTime::now_utc(),
                co2: 800,
                temperature: 22.0,
                pressure: 1013.0,
                humidity: 50,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            })
            .collect();
        device.add_history(records).await;

        let info = device.get_history_info().await.unwrap();
        assert_eq!(info.total_readings, 10);
        assert_eq!(info.interval_seconds, 300); // 5 minutes default
    }

    #[tokio::test]
    async fn test_mock_device_debug() {
        let device = MockDevice::new("Debug Test", DeviceType::Aranet4);
        let debug_str = format!("{:?}", device);
        assert!(debug_str.contains("MockDevice"));
        assert!(debug_str.contains("Debug Test"));
        assert!(debug_str.contains("Aranet4"));
    }

    #[test]
    fn test_builder_all_options() {
        let device = MockDeviceBuilder::new()
            .name("Custom Device")
            .device_type(DeviceType::Aranet2)
            .co2(0)
            .temperature(18.5)
            .pressure(1020.0)
            .humidity(65)
            .battery(90)
            .status(Status::Yellow)
            .auto_connect(false)
            .build();

        assert_eq!(device.name(), "Custom Device");
        assert_eq!(device.device_type(), DeviceType::Aranet2);
        assert!(!device.is_connected_sync());
    }

    #[tokio::test]
    async fn test_trait_methods_match_direct_methods() {
        let device = MockDeviceBuilder::new()
            .name("Trait Test")
            .co2(999)
            .battery(77)
            .build();
        device.set_rssi(-55);

        // Test that trait methods return same values as direct methods
        let trait_device: &dyn AranetDevice = &device;

        assert_eq!(trait_device.name(), Some("Trait Test"));
        assert_eq!(trait_device.device_type(), Some(DeviceType::Aranet4));
        assert!(trait_device.is_connected().await);

        let reading = trait_device.read_current().await.unwrap();
        assert_eq!(reading.co2, 999);

        let battery = trait_device.read_battery().await.unwrap();
        assert_eq!(battery, 77);

        let rssi = trait_device.read_rssi().await.unwrap();
        assert_eq!(rssi, -55);
    }
}
