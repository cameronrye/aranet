//! Core types for Aranet sensor data.

use core::fmt;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::error::ParseError;

/// Type of Aranet device.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new device types
/// in future versions without breaking downstream code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
    /// based on common naming patterns. Uses word-boundary-aware matching to avoid
    /// false positives (e.g., `"Aranet4"` won't match `"NotAranet4Device"`).
    ///
    /// # Examples
    ///
    /// ```
    /// use aranet_types::DeviceType;
    ///
    /// assert_eq!(DeviceType::from_name("Aranet4 12345"), Some(DeviceType::Aranet4));
    /// assert_eq!(DeviceType::from_name("Aranet2 Home"), Some(DeviceType::Aranet2));
    /// assert_eq!(DeviceType::from_name("Aranet4"), Some(DeviceType::Aranet4));
    /// assert_eq!(DeviceType::from_name("RN+ Radon"), Some(DeviceType::AranetRadon));
    /// assert_eq!(DeviceType::from_name("Aranet Radiation"), Some(DeviceType::AranetRadiation));
    /// assert_eq!(DeviceType::from_name("Unknown Device"), None);
    /// ```
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let name_lower = name.to_lowercase();

        // Check for Aranet4 - must be at word boundary (start or after non-alphanumeric)
        if Self::contains_word(&name_lower, "aranet4") {
            return Some(DeviceType::Aranet4);
        }

        // Check for Aranet2
        if Self::contains_word(&name_lower, "aranet2") {
            return Some(DeviceType::Aranet2);
        }

        // Check for Radon devices (RN+ or Radon keyword)
        if Self::contains_word(&name_lower, "rn+")
            || Self::contains_word(&name_lower, "aranet radon")
            || (name_lower.starts_with("radon") || name_lower.contains(" radon"))
        {
            return Some(DeviceType::AranetRadon);
        }

        // Check for Radiation devices
        if Self::contains_word(&name_lower, "radiation")
            || Self::contains_word(&name_lower, "aranet radiation")
        {
            return Some(DeviceType::AranetRadiation);
        }

        None
    }

    /// Check if a string contains a word at a word boundary.
    ///
    /// A word boundary is defined as the start/end of the string or a non-alphanumeric character.
    fn contains_word(haystack: &str, needle: &str) -> bool {
        if let Some(pos) = haystack.find(needle) {
            // Check character before the match (if any)
            let before_ok = pos == 0
                || haystack[..pos]
                    .chars()
                    .last()
                    .is_none_or(|c| !c.is_alphanumeric());

            // Check character after the match (if any)
            let end_pos = pos + needle.len();
            let after_ok = end_pos >= haystack.len()
                || haystack[end_pos..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric());

            before_ok && after_ok
        } else {
            false
        }
    }

    /// Returns the BLE characteristic UUID for reading current sensor values.
    ///
    /// - **Aranet4**: Uses `CURRENT_READINGS_DETAIL` (f0cd3001)
    /// - **Other devices**: Use `CURRENT_READINGS_DETAIL_ALT` (f0cd3003)
    ///
    /// # Examples
    ///
    /// ```
    /// use aranet_types::DeviceType;
    /// use aranet_types::ble;
    ///
    /// assert_eq!(DeviceType::Aranet4.readings_characteristic(), ble::CURRENT_READINGS_DETAIL);
    /// assert_eq!(DeviceType::Aranet2.readings_characteristic(), ble::CURRENT_READINGS_DETAIL_ALT);
    /// ```
    #[must_use]
    pub fn readings_characteristic(&self) -> uuid::Uuid {
        match self {
            DeviceType::Aranet4 => crate::uuid::CURRENT_READINGS_DETAIL,
            _ => crate::uuid::CURRENT_READINGS_DETAIL_ALT,
        }
    }
}

impl TryFrom<u8> for DeviceType {
    type Error = ParseError;

    /// Convert a byte value to a `DeviceType`.
    ///
    /// # Examples
    ///
    /// ```
    /// use aranet_types::DeviceType;
    ///
    /// assert_eq!(DeviceType::try_from(0xF1), Ok(DeviceType::Aranet4));
    /// assert_eq!(DeviceType::try_from(0xF2), Ok(DeviceType::Aranet2));
    /// assert!(DeviceType::try_from(0x00).is_err());
    /// ```
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0xF1 => Ok(DeviceType::Aranet4),
            0xF2 => Ok(DeviceType::Aranet2),
            0xF3 => Ok(DeviceType::AranetRadon),
            0xF4 => Ok(DeviceType::AranetRadiation),
            _ => Err(ParseError::UnknownDeviceType(value)),
        }
    }
}

impl fmt::Display for DeviceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceType::Aranet4 => write!(f, "Aranet4"),
            DeviceType::Aranet2 => write!(f, "Aranet2"),
            DeviceType::AranetRadon => write!(f, "Aranet Radon"),
            DeviceType::AranetRadiation => write!(f, "Aranet Radiation"),
        }
    }
}

/// CO2 level status indicator.
///
/// This enum is marked `#[non_exhaustive]` to allow adding new status levels
/// in future versions without breaking downstream code.
///
/// # Ordering
///
/// Status values are ordered by severity: `Error < Green < Yellow < Red`.
/// This allows threshold comparisons like `if status >= Status::Yellow { warn!(...) }`.
///
/// # Display vs Serialization
///
/// **Note:** The `Display` trait returns human-readable labels ("Good", "Moderate", "High"),
/// while serde serialization uses the variant names ("Green", "Yellow", "Red").
///
/// ```
/// use aranet_types::Status;
///
/// // Display is human-readable
/// assert_eq!(format!("{}", Status::Green), "Good");
///
/// // Ordering works for threshold comparisons
/// assert!(Status::Red > Status::Yellow);
/// assert!(Status::Yellow > Status::Green);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Error => write!(f, "Error"),
            Status::Green => write!(f, "Good"),
            Status::Yellow => write!(f, "Moderate"),
            Status::Red => write!(f, "High"),
        }
    }
}

/// Minimum number of bytes required to parse an Aranet4 [`CurrentReading`].
pub const MIN_CURRENT_READING_BYTES: usize = 13;

/// Minimum number of bytes required to parse an Aranet2 [`CurrentReading`].
pub const MIN_ARANET2_READING_BYTES: usize = 7;

/// Minimum number of bytes required to parse an Aranet Radon [`CurrentReading`] (advertisement format).
pub const MIN_RADON_READING_BYTES: usize = 15;

/// Minimum number of bytes required to parse an Aranet Radon GATT [`CurrentReading`].
pub const MIN_RADON_GATT_READING_BYTES: usize = 18;

/// Minimum number of bytes required to parse an Aranet Radiation [`CurrentReading`].
pub const MIN_RADIATION_READING_BYTES: usize = 28;

/// Current reading from an Aranet sensor.
///
/// This struct supports all Aranet device types:
/// - **Aranet4**: CO2, temperature, pressure, humidity
/// - **Aranet2**: Temperature, humidity (co2 and pressure will be 0)
/// - **`AranetRn+` (Radon)**: Radon, temperature, pressure, humidity (co2 will be 0)
/// - **Aranet Radiation**: Radiation dose, temperature (uses `radiation_*` fields)
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
    /// Timestamp when the reading was captured (if known).
    ///
    /// This is typically set by the library when reading from a device,
    /// calculated as `now - age`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub captured_at: Option<time::OffsetDateTime>,
    /// Radon concentration in Bq/m³ (`AranetRn+` only).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub radon: Option<u32>,
    /// Radiation dose rate in µSv/h (Aranet Radiation only).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub radiation_rate: Option<f32>,
    /// Total radiation dose in mSv (Aranet Radiation only).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
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
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }
    }
}

impl CurrentReading {
    /// Parse a `CurrentReading` from raw bytes (Aranet4 format).
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
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::InsufficientBytes`] if `data` contains fewer than
    /// [`MIN_CURRENT_READING_BYTES`] (13) bytes.
    #[must_use = "parsing returns a Result that should be handled"]
    pub fn from_bytes(data: &[u8]) -> Result<Self, ParseError> {
        Self::from_bytes_aranet4(data)
    }

    /// Parse a `CurrentReading` from raw bytes (Aranet4 format).
    ///
    /// This is an alias for [`from_bytes`](Self::from_bytes) for explicit device type parsing.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::InsufficientBytes`] if `data` contains fewer than
    /// [`MIN_CURRENT_READING_BYTES`] (13) bytes.
    #[must_use = "parsing returns a Result that should be handled"]
    pub fn from_bytes_aranet4(data: &[u8]) -> Result<Self, ParseError> {
        use bytes::Buf;

        if data.len() < MIN_CURRENT_READING_BYTES {
            return Err(ParseError::InsufficientBytes {
                expected: MIN_CURRENT_READING_BYTES,
                actual: data.len(),
            });
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
            temperature: f32::from(temp_raw) / 20.0,
            pressure: f32::from(pressure_raw) / 10.0,
            humidity,
            battery,
            status,
            interval,
            age,
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        })
    }

    /// Parse a `CurrentReading` from raw bytes (Aranet2 format).
    ///
    /// The byte format is:
    /// - bytes 0-1: Temperature (u16 LE, divide by 20 for Celsius)
    /// - byte 2: Humidity (u8)
    /// - byte 3: Battery (u8)
    /// - byte 4: Status (u8)
    /// - bytes 5-6: Interval (u16 LE)
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::InsufficientBytes`] if `data` contains fewer than
    /// [`MIN_ARANET2_READING_BYTES`] (7) bytes.
    #[must_use = "parsing returns a Result that should be handled"]
    pub fn from_bytes_aranet2(data: &[u8]) -> Result<Self, ParseError> {
        use bytes::Buf;

        if data.len() < MIN_ARANET2_READING_BYTES {
            return Err(ParseError::InsufficientBytes {
                expected: MIN_ARANET2_READING_BYTES,
                actual: data.len(),
            });
        }

        let mut buf = data;
        let temp_raw = buf.get_u16_le();
        let humidity = buf.get_u8();
        let battery = buf.get_u8();
        let status = Status::from(buf.get_u8());
        let interval = buf.get_u16_le();

        Ok(CurrentReading {
            co2: 0, // Aranet2 doesn't have CO2
            temperature: f32::from(temp_raw) / 20.0,
            pressure: 0.0, // Aranet2 doesn't have pressure
            humidity,
            battery,
            status,
            interval,
            age: 0,
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        })
    }

    /// Parse a `CurrentReading` from raw bytes (Aranet Radon GATT format).
    ///
    /// The byte format is:
    /// - bytes 0-1: Device type marker (u16 LE, 0x0003 for radon)
    /// - bytes 2-3: Interval (u16 LE, seconds)
    /// - bytes 4-5: Age (u16 LE, seconds since update)
    /// - byte 6: Battery (u8)
    /// - bytes 7-8: Temperature (u16 LE, divide by 20 for Celsius)
    /// - bytes 9-10: Pressure (u16 LE, divide by 10 for hPa)
    /// - bytes 11-12: Humidity (u16 LE, divide by 10 for percent)
    /// - bytes 13-16: Radon (u32 LE, Bq/m³)
    /// - byte 17: Status (u8)
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::InsufficientBytes`] if `data` contains fewer than
    /// [`MIN_RADON_GATT_READING_BYTES`] (18) bytes.
    #[must_use = "parsing returns a Result that should be handled"]
    pub fn from_bytes_radon(data: &[u8]) -> Result<Self, ParseError> {
        use bytes::Buf;

        if data.len() < MIN_RADON_GATT_READING_BYTES {
            return Err(ParseError::InsufficientBytes {
                expected: MIN_RADON_GATT_READING_BYTES,
                actual: data.len(),
            });
        }

        let mut buf = data;

        // Parse header
        let _device_type = buf.get_u16_le(); // 0x0003 for radon
        let interval = buf.get_u16_le();
        let age = buf.get_u16_le();
        let battery = buf.get_u8();

        // Parse sensor values
        let temp_raw = buf.get_u16_le();
        let pressure_raw = buf.get_u16_le();
        let humidity_raw = buf.get_u16_le();
        let radon = buf.get_u32_le();
        let status = if buf.has_remaining() {
            Status::from(buf.get_u8())
        } else {
            Status::Green
        };

        Ok(CurrentReading {
            co2: 0,
            temperature: f32::from(temp_raw) / 20.0,
            pressure: f32::from(pressure_raw) / 10.0,
            humidity: (humidity_raw / 10).min(255) as u8, // Convert from 10ths to percent
            battery,
            status,
            interval,
            age,
            captured_at: None,
            radon: Some(radon),
            radiation_rate: None,
            radiation_total: None,
        })
    }

    /// Parse a `CurrentReading` from raw bytes (Aranet Radiation GATT format).
    ///
    /// The byte format is:
    /// - bytes 0-1: Unknown/header (u16 LE)
    /// - bytes 2-3: Interval (u16 LE, seconds)
    /// - bytes 4-5: Age (u16 LE, seconds)
    /// - byte 6: Battery (u8)
    /// - bytes 7-10: Dose rate (u32 LE, nSv/h, divide by 1000 for µSv/h)
    /// - bytes 11-18: Total dose (u64 LE, nSv, divide by `1_000_000` for mSv)
    /// - bytes 19-26: Duration (u64 LE, seconds) - not stored in `CurrentReading`
    /// - byte 27: Status (u8)
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::InsufficientBytes`] if `data` contains fewer than
    /// [`MIN_RADIATION_READING_BYTES`] (28) bytes.
    #[must_use = "parsing returns a Result that should be handled"]
    #[allow(clippy::similar_names, clippy::cast_precision_loss)]
    pub fn from_bytes_radiation(data: &[u8]) -> Result<Self, ParseError> {
        use bytes::Buf;

        if data.len() < MIN_RADIATION_READING_BYTES {
            return Err(ParseError::InsufficientBytes {
                expected: MIN_RADIATION_READING_BYTES,
                actual: data.len(),
            });
        }

        let mut buf = data;

        // Parse header
        let _unknown = buf.get_u16_le();
        let interval = buf.get_u16_le();
        let age = buf.get_u16_le();
        let battery = buf.get_u8();

        // Parse radiation values
        let dose_rate_nsv = buf.get_u32_le();
        let total_dose_nsv = buf.get_u64_le();
        let _duration = buf.get_u64_le(); // Duration in seconds (not stored)
        let status = if buf.has_remaining() {
            Status::from(buf.get_u8())
        } else {
            Status::Green
        };

        // Convert units: nSv/h -> µSv/h, nSv -> mSv
        let dose_rate_usv = dose_rate_nsv as f32 / 1000.0;
        let total_dose_msv = total_dose_nsv as f64 / 1_000_000.0;

        Ok(CurrentReading {
            co2: 0,
            temperature: 0.0, // Radiation devices don't report temperature
            pressure: 0.0,
            humidity: 0,
            battery,
            status,
            interval,
            age,
            captured_at: None,
            radon: None,
            radiation_rate: Some(dose_rate_usv),
            radiation_total: Some(total_dose_msv),
        })
    }

    /// Parse a `CurrentReading` from raw bytes based on device type.
    ///
    /// This dispatches to the appropriate parsing method based on the device type.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::InsufficientBytes`] if `data` doesn't contain enough bytes
    /// for the specified device type.
    #[must_use = "parsing returns a Result that should be handled"]
    pub fn from_bytes_for_device(data: &[u8], device_type: DeviceType) -> Result<Self, ParseError> {
        match device_type {
            DeviceType::Aranet4 => Self::from_bytes_aranet4(data),
            DeviceType::Aranet2 => Self::from_bytes_aranet2(data),
            DeviceType::AranetRadon => Self::from_bytes_radon(data),
            DeviceType::AranetRadiation => Self::from_bytes_radiation(data),
        }
    }

    /// Set the captured timestamp to the current time minus the age.
    ///
    /// This is useful for setting the timestamp when reading from a device.
    #[must_use]
    pub fn with_captured_at(mut self, now: time::OffsetDateTime) -> Self {
        self.captured_at =
            Some(now - time::Duration::seconds(i64::from(self.age)));
        self
    }

    /// Create a builder for constructing `CurrentReading` with optional fields.
    pub fn builder() -> CurrentReadingBuilder {
        CurrentReadingBuilder::default()
    }
}

/// Builder for constructing `CurrentReading` with device-specific fields.
///
/// Use [`build`](Self::build) for unchecked construction, or [`try_build`](Self::try_build)
/// for validation of field values.
#[derive(Debug, Default)]
#[must_use]
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

    /// Set humidity (0-100).
    pub fn humidity(mut self, humidity: u8) -> Self {
        self.reading.humidity = humidity;
        self
    }

    /// Set battery level (0-100).
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

    /// Set the captured timestamp.
    pub fn captured_at(mut self, timestamp: time::OffsetDateTime) -> Self {
        self.reading.captured_at = Some(timestamp);
        self
    }

    /// Set radon concentration (`AranetRn+`).
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

    /// Build the `CurrentReading` without validation.
    #[must_use]
    pub fn build(self) -> CurrentReading {
        self.reading
    }

    /// Build the `CurrentReading` with validation.
    ///
    /// Validates:
    /// - `humidity` is 0-100
    /// - `battery` is 0-100
    /// - `temperature` is within reasonable range (-40 to 100°C)
    /// - `pressure` is within reasonable range (800-1200 hPa) or 0
    ///
    /// # Errors
    ///
    /// Returns [`ParseError::InvalidValue`] if any field has an invalid value.
    pub fn try_build(self) -> Result<CurrentReading, ParseError> {
        if self.reading.humidity > 100 {
            return Err(ParseError::InvalidValue(format!(
                "humidity {} exceeds maximum of 100",
                self.reading.humidity
            )));
        }

        if self.reading.battery > 100 {
            return Err(ParseError::InvalidValue(format!(
                "battery {} exceeds maximum of 100",
                self.reading.battery
            )));
        }

        // Temperature range check (typical sensor range)
        if self.reading.temperature < -40.0 || self.reading.temperature > 100.0 {
            return Err(ParseError::InvalidValue(format!(
                "temperature {} is outside valid range (-40 to 100°C)",
                self.reading.temperature
            )));
        }

        // Pressure range check (0 is valid for devices without pressure sensor)
        if self.reading.pressure != 0.0
            && (self.reading.pressure < 800.0 || self.reading.pressure > 1200.0)
        {
            return Err(ParseError::InvalidValue(format!(
                "pressure {} is outside valid range (800-1200 hPa)",
                self.reading.pressure
            )));
        }

        Ok(self.reading)
    }
}

/// Device information from an Aranet sensor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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

impl DeviceInfo {
    /// Create a builder for constructing `DeviceInfo`.
    pub fn builder() -> DeviceInfoBuilder {
        DeviceInfoBuilder::default()
    }
}

/// Builder for constructing `DeviceInfo`.
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct DeviceInfoBuilder {
    info: DeviceInfo,
}

impl DeviceInfoBuilder {
    /// Set the device name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.info.name = name.into();
        self
    }

    /// Set the model number.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.info.model = model.into();
        self
    }

    /// Set the serial number.
    pub fn serial(mut self, serial: impl Into<String>) -> Self {
        self.info.serial = serial.into();
        self
    }

    /// Set the firmware version.
    pub fn firmware(mut self, firmware: impl Into<String>) -> Self {
        self.info.firmware = firmware.into();
        self
    }

    /// Set the hardware revision.
    pub fn hardware(mut self, hardware: impl Into<String>) -> Self {
        self.info.hardware = hardware.into();
        self
    }

    /// Set the software revision.
    pub fn software(mut self, software: impl Into<String>) -> Self {
        self.info.software = software.into();
        self
    }

    /// Set the manufacturer name.
    pub fn manufacturer(mut self, manufacturer: impl Into<String>) -> Self {
        self.info.manufacturer = manufacturer.into();
        self
    }

    /// Build the `DeviceInfo`.
    #[must_use]
    pub fn build(self) -> DeviceInfo {
        self.info
    }
}

/// A historical reading record from an Aranet sensor.
///
/// This struct supports all Aranet device types:
/// - **Aranet4**: CO2, temperature, pressure, humidity
/// - **Aranet2**: Temperature, humidity (co2 and pressure will be 0)
/// - **`AranetRn+`**: Radon, temperature, pressure, humidity (co2 will be 0)
/// - **Aranet Radiation**: Radiation rate/total, temperature (uses `radiation_*` fields)
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
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
    /// Radon concentration in Bq/m³ (`AranetRn+` only).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub radon: Option<u32>,
    /// Radiation dose rate in µSv/h (Aranet Radiation only).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub radiation_rate: Option<f32>,
    /// Total radiation dose in mSv (Aranet Radiation only).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub radiation_total: Option<f64>,
}

impl Default for HistoryRecord {
    fn default() -> Self {
        Self {
            timestamp: time::OffsetDateTime::UNIX_EPOCH,
            co2: 0,
            temperature: 0.0,
            pressure: 0.0,
            humidity: 0,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }
    }
}

impl HistoryRecord {
    /// Create a builder for constructing `HistoryRecord` with optional fields.
    pub fn builder() -> HistoryRecordBuilder {
        HistoryRecordBuilder::default()
    }
}

/// Builder for constructing `HistoryRecord` with device-specific fields.
#[derive(Debug, Default)]
#[must_use]
pub struct HistoryRecordBuilder {
    record: HistoryRecord,
}

impl HistoryRecordBuilder {
    /// Set the timestamp.
    pub fn timestamp(mut self, timestamp: time::OffsetDateTime) -> Self {
        self.record.timestamp = timestamp;
        self
    }

    /// Set CO2 concentration (Aranet4).
    pub fn co2(mut self, co2: u16) -> Self {
        self.record.co2 = co2;
        self
    }

    /// Set temperature.
    pub fn temperature(mut self, temp: f32) -> Self {
        self.record.temperature = temp;
        self
    }

    /// Set pressure.
    pub fn pressure(mut self, pressure: f32) -> Self {
        self.record.pressure = pressure;
        self
    }

    /// Set humidity.
    pub fn humidity(mut self, humidity: u8) -> Self {
        self.record.humidity = humidity;
        self
    }

    /// Set radon concentration (`AranetRn+`).
    pub fn radon(mut self, radon: u32) -> Self {
        self.record.radon = Some(radon);
        self
    }

    /// Set radiation dose rate (Aranet Radiation).
    pub fn radiation_rate(mut self, rate: f32) -> Self {
        self.record.radiation_rate = Some(rate);
        self
    }

    /// Set total radiation dose (Aranet Radiation).
    pub fn radiation_total(mut self, total: f64) -> Self {
        self.record.radiation_total = Some(total);
        self
    }

    /// Build the `HistoryRecord`.
    #[must_use]
    pub fn build(self) -> HistoryRecord {
        self.record
    }
}
