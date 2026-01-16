//! Core types for Aranet sensor data.

use bytes::Buf;
use serde::{Deserialize, Serialize};

use crate::error::ParseError;

/// Type of Aranet device.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new device types
/// in future versions without breaking downstream code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[repr(u8)]
pub enum DeviceType {
    /// Aranet4 CO2, temperature, humidity, and pressure sensor.
    Aranet4 = 0xF1,
    /// Aranet2 temperature and humidity sensor.
    Aranet2 = 0xF2,
    /// Aranet Radon sensor.
    AranetRadon = 0xF3,
    /// Aranet Radiation sensor.
    AranetRadiation = 0xF4,
}

impl DeviceType {
    /// Detect device type from a device name.
    ///
    /// Analyzes the device name (case-insensitive) to determine the device type
    /// based on common naming patterns.
    ///
    /// # Examples
    ///
    /// ```
    /// use aranet_types::DeviceType;
    ///
    /// assert_eq!(DeviceType::from_name("Aranet4 12345"), Some(DeviceType::Aranet4));
    /// assert_eq!(DeviceType::from_name("Aranet2 Home"), Some(DeviceType::Aranet2));
    /// assert_eq!(DeviceType::from_name("RN+ Radon"), Some(DeviceType::AranetRadon));
    /// assert_eq!(DeviceType::from_name("Unknown Device"), None);
    /// ```
    pub fn from_name(name: &str) -> Option<Self> {
        let name_lower = name.to_lowercase();
        if name_lower.contains("aranet4") {
            Some(DeviceType::Aranet4)
        } else if name_lower.contains("aranet2") {
            Some(DeviceType::Aranet2)
        } else if name_lower.contains("rn+") || name_lower.contains("radon") {
            Some(DeviceType::AranetRadon)
        } else if name_lower.contains("radiation") {
            Some(DeviceType::AranetRadiation)
        } else {
            None
        }
    }
}

/// CO2 level status indicator.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new status levels
/// in future versions without breaking downstream code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[repr(u8)]
pub enum Status {
    /// Error or invalid reading.
    Error = 0,
    /// CO2 level is good (green).
    Green = 1,
    /// CO2 level is moderate (yellow).
    Yellow = 2,
    /// CO2 level is high (red).
    Red = 3,
}

impl From<u8> for Status {
    fn from(value: u8) -> Self {
        match value {
            1 => Status::Green,
            2 => Status::Yellow,
            3 => Status::Red,
            _ => Status::Error,
        }
    }
}

/// Current reading from an Aranet sensor.
///
/// This struct supports all Aranet device types:
/// - **Aranet4**: CO2, temperature, pressure, humidity
/// - **Aranet2**: Temperature, humidity (co2 and pressure will be 0)
/// - **AranetRn+ (Radon)**: Radon, temperature, pressure, humidity (co2 will be 0)
/// - **Aranet Radiation**: Radiation dose, temperature (uses radiation_* fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentReading {
    /// CO2 concentration in ppm (Aranet4 only, 0 for other devices).
    pub co2: u16,
    /// Temperature in degrees Celsius.
    pub temperature: f32,
    /// Atmospheric pressure in hPa (0 for Aranet2).
    pub pressure: f32,
    /// Relative humidity percentage (0-100).
    pub humidity: u8,
    /// Battery level percentage (0-100).
    pub battery: u8,
    /// CO2 status indicator.
    pub status: Status,
    /// Measurement interval in seconds.
    pub interval: u16,
    /// Age of reading in seconds since last measurement.
    pub age: u16,
    /// Radon concentration in Bq/m³ (AranetRn+ only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radon: Option<u32>,
    /// Radiation dose rate in µSv/h (Aranet Radiation only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radiation_rate: Option<f32>,
    /// Total radiation dose in mSv (Aranet Radiation only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radiation_total: Option<f64>,
}

impl Default for CurrentReading {
    fn default() -> Self {
        Self {
            co2: 0,
            temperature: 0.0,
            pressure: 0.0,
            humidity: 0,
            battery: 0,
            status: Status::Error,
            interval: 0,
            age: 0,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }
    }
}

impl CurrentReading {
    /// Parse a CurrentReading from raw bytes (Aranet4 format).
    ///
    /// The byte format is:
    /// - bytes 0-1: CO2 (u16 LE)
    /// - bytes 2-3: Temperature (u16 LE, divide by 20 for Celsius)
    /// - bytes 4-5: Pressure (u16 LE, divide by 10 for hPa)
    /// - byte 6: Humidity (u8)
    /// - byte 7: Battery (u8)
    /// - byte 8: Status (u8)
    /// - bytes 9-10: Interval (u16 LE)
    /// - bytes 11-12: Age (u16 LE)
    pub fn from_bytes(data: &[u8]) -> Result<Self, ParseError> {
        if data.len() < 13 {
            return Err(ParseError::InvalidData(format!(
                "CurrentReading requires 13 bytes, got {}",
                data.len()
            )));
        }

        let mut buf = data;
        let co2 = buf.get_u16_le();
        let temp_raw = buf.get_u16_le();
        let pressure_raw = buf.get_u16_le();
        let humidity = buf.get_u8();
        let battery = buf.get_u8();
        let status = Status::from(buf.get_u8());
        let interval = buf.get_u16_le();
        let age = buf.get_u16_le();

        Ok(CurrentReading {
            co2,
            temperature: temp_raw as f32 / 20.0,
            pressure: pressure_raw as f32 / 10.0,
            humidity,
            battery,
            status,
            interval,
            age,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        })
    }

    /// Create a builder for constructing CurrentReading with optional fields.
    pub fn builder() -> CurrentReadingBuilder {
        CurrentReadingBuilder::default()
    }
}

/// Builder for constructing CurrentReading with device-specific fields.
#[derive(Debug, Default)]
pub struct CurrentReadingBuilder {
    reading: CurrentReading,
}

impl CurrentReadingBuilder {
    /// Set CO2 concentration (Aranet4).
    pub fn co2(mut self, co2: u16) -> Self {
        self.reading.co2 = co2;
        self
    }

    /// Set temperature.
    pub fn temperature(mut self, temp: f32) -> Self {
        self.reading.temperature = temp;
        self
    }

    /// Set pressure.
    pub fn pressure(mut self, pressure: f32) -> Self {
        self.reading.pressure = pressure;
        self
    }

    /// Set humidity.
    pub fn humidity(mut self, humidity: u8) -> Self {
        self.reading.humidity = humidity;
        self
    }

    /// Set battery level.
    pub fn battery(mut self, battery: u8) -> Self {
        self.reading.battery = battery;
        self
    }

    /// Set status.
    pub fn status(mut self, status: Status) -> Self {
        self.reading.status = status;
        self
    }

    /// Set measurement interval.
    pub fn interval(mut self, interval: u16) -> Self {
        self.reading.interval = interval;
        self
    }

    /// Set reading age.
    pub fn age(mut self, age: u16) -> Self {
        self.reading.age = age;
        self
    }

    /// Set radon concentration (AranetRn+).
    pub fn radon(mut self, radon: u32) -> Self {
        self.reading.radon = Some(radon);
        self
    }

    /// Set radiation dose rate (Aranet Radiation).
    pub fn radiation_rate(mut self, rate: f32) -> Self {
        self.reading.radiation_rate = Some(rate);
        self
    }

    /// Set total radiation dose (Aranet Radiation).
    pub fn radiation_total(mut self, total: f64) -> Self {
        self.reading.radiation_total = Some(total);
        self
    }

    /// Build the CurrentReading.
    pub fn build(self) -> CurrentReading {
        self.reading
    }
}

/// Device information from an Aranet sensor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Device name.
    pub name: String,
    /// Model number.
    pub model: String,
    /// Serial number.
    pub serial: String,
    /// Firmware version.
    pub firmware: String,
    /// Hardware revision.
    pub hardware: String,
    /// Software revision.
    pub software: String,
    /// Manufacturer name.
    pub manufacturer: String,
}

/// A historical reading record from an Aranet sensor.
///
/// This struct supports all Aranet device types:
/// - **Aranet4**: CO2, temperature, pressure, humidity
/// - **Aranet2**: Temperature, humidity (co2 and pressure will be 0)
/// - **AranetRn+**: Radon, temperature, pressure, humidity (co2 will be 0)
/// - **Aranet Radiation**: Radiation rate/total, temperature (uses radiation_* fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRecord {
    /// Timestamp of the reading.
    pub timestamp: time::OffsetDateTime,
    /// CO2 concentration in ppm (Aranet4) or 0 for other devices.
    pub co2: u16,
    /// Temperature in degrees Celsius.
    pub temperature: f32,
    /// Atmospheric pressure in hPa (0 for Aranet2).
    pub pressure: f32,
    /// Relative humidity percentage (0-100).
    pub humidity: u8,
    /// Radon concentration in Bq/m³ (AranetRn+ only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radon: Option<u32>,
    /// Radiation dose rate in µSv/h (Aranet Radiation only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radiation_rate: Option<f32>,
    /// Total radiation dose in mSv (Aranet Radiation only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub radiation_total: Option<f64>,
}
