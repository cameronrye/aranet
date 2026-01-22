//! Data models for stored data.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aranet_types::{CurrentReading, DeviceType, HistoryRecord, Status};

/// A device stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredDevice {
    /// Device identifier (address or UUID).
    pub id: String,
    /// Device name.
    pub name: Option<String>,
    /// Device type.
    pub device_type: Option<DeviceType>,
    /// Serial number.
    pub serial: Option<String>,
    /// Firmware version.
    pub firmware: Option<String>,
    /// Hardware version.
    pub hardware: Option<String>,
    /// First time this device was seen.
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen: OffsetDateTime,
    /// Last time this device was seen.
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen: OffsetDateTime,
}

/// A reading stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredReading {
    /// Database row ID.
    pub id: i64,
    /// Device identifier.
    pub device_id: String,
    /// When this reading was captured.
    #[serde(with = "time::serde::rfc3339")]
    pub captured_at: OffsetDateTime,
    /// CO2 concentration in ppm.
    pub co2: u16,
    /// Temperature in Celsius.
    pub temperature: f32,
    /// Pressure in hPa.
    pub pressure: f32,
    /// Humidity percentage.
    pub humidity: u8,
    /// Battery percentage.
    pub battery: u8,
    /// Status indicator.
    pub status: Status,
    /// Radon level (Bq/m3) for radon devices.
    pub radon: Option<u32>,
    /// Radiation rate in uSv/h for radiation devices.
    pub radiation_rate: Option<f32>,
    /// Total radiation dose in mSv for radiation devices.
    pub radiation_total: Option<f64>,
}

impl StoredReading {
    /// Create a StoredReading from a CurrentReading.
    pub fn from_reading(device_id: &str, reading: &CurrentReading) -> Self {
        Self {
            id: 0, // Will be set by database
            device_id: device_id.to_string(),
            captured_at: reading.captured_at.unwrap_or_else(OffsetDateTime::now_utc),
            co2: reading.co2,
            temperature: reading.temperature,
            pressure: reading.pressure,
            humidity: reading.humidity,
            battery: reading.battery,
            status: reading.status,
            radon: reading.radon,
            radiation_rate: reading.radiation_rate,
            radiation_total: reading.radiation_total,
        }
    }

    /// Convert to a CurrentReading.
    pub fn to_reading(&self) -> CurrentReading {
        CurrentReading {
            co2: self.co2,
            temperature: self.temperature,
            pressure: self.pressure,
            humidity: self.humidity,
            battery: self.battery,
            status: self.status,
            interval: 0,
            age: 0,
            captured_at: Some(self.captured_at),
            radon: self.radon,
            radiation_rate: self.radiation_rate,
            radiation_total: self.radiation_total,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        }
    }
}

/// A history record stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredHistoryRecord {
    /// Database row ID.
    pub id: i64,
    /// Device identifier.
    pub device_id: String,
    /// Timestamp of the reading from the device.
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// When this record was synced to the database.
    #[serde(with = "time::serde::rfc3339")]
    pub synced_at: OffsetDateTime,
    /// CO2 concentration in ppm.
    pub co2: u16,
    /// Temperature in Celsius.
    pub temperature: f32,
    /// Pressure in hPa.
    pub pressure: f32,
    /// Humidity percentage.
    pub humidity: u8,
    /// Radon level (Bq/m3) for radon devices.
    pub radon: Option<u32>,
    /// Radiation rate in uSv/h for radiation devices.
    pub radiation_rate: Option<f32>,
    /// Total radiation dose in mSv for radiation devices.
    pub radiation_total: Option<f64>,
}

impl StoredHistoryRecord {
    /// Create a StoredHistoryRecord from a HistoryRecord.
    pub fn from_history(device_id: &str, record: &HistoryRecord) -> Self {
        Self {
            id: 0,
            device_id: device_id.to_string(),
            timestamp: record.timestamp,
            synced_at: OffsetDateTime::now_utc(),
            co2: record.co2,
            temperature: record.temperature,
            pressure: record.pressure,
            humidity: record.humidity,
            radon: record.radon,
            radiation_rate: record.radiation_rate,
            radiation_total: record.radiation_total,
        }
    }

    /// Convert to a HistoryRecord.
    pub fn to_history(&self) -> HistoryRecord {
        HistoryRecord {
            timestamp: self.timestamp,
            co2: self.co2,
            temperature: self.temperature,
            pressure: self.pressure,
            humidity: self.humidity,
            radon: self.radon,
            radiation_rate: self.radiation_rate,
            radiation_total: self.radiation_total,
        }
    }
}

/// Sync state for a device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// Device identifier.
    pub device_id: String,
    /// Last downloaded history index.
    pub last_history_index: Option<u16>,
    /// Total readings on device at last sync.
    pub total_readings: Option<u16>,
    /// When last synced.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_sync_at: Option<OffsetDateTime>,
}
