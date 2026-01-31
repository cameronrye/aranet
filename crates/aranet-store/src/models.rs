//! Data models for stored sensor data.
//!
//! This module defines the types returned by [`Store`](crate::Store) queries:
//!
//! - [`StoredDevice`] - Device metadata and tracking information
//! - [`StoredReading`] - Current/real-time sensor readings with database IDs
//! - [`StoredHistoryRecord`] - Historical readings downloaded from device memory
//! - [`SyncState`] - Tracks incremental history sync progress
//!
//! All types implement `Serialize` and `Deserialize` for easy JSON export/import.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use aranet_types::{CurrentReading, DeviceType, HistoryRecord, Status};

/// A device stored in the database with metadata and tracking information.
///
/// Devices are automatically created when readings are inserted. Additional
/// metadata (name, type, firmware version) can be updated separately.
///
/// # Example
///
/// ```
/// use aranet_store::Store;
///
/// let store = Store::open_in_memory()?;
/// let device = store.upsert_device("Aranet4 17C3C", Some("Kitchen"))?;
///
/// println!("Device: {}", device.id);
/// println!("Name: {:?}", device.name);
/// println!("First seen: {}", device.first_seen);
/// println!("Last seen: {}", device.last_seen);
/// # Ok::<(), aranet_store::Error>(())
/// ```
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

/// A current sensor reading stored in the database.
///
/// This represents a point-in-time reading captured from a device, typically
/// via BLE connection. Unlike [`StoredHistoryRecord`], these are readings
/// captured by your application, not downloaded from the device's internal memory.
///
/// # Supported Sensor Types
///
/// - **Aranet4**: CO2, temperature, pressure, humidity
/// - **Aranet2**: Temperature, humidity
/// - **AranetRn+ (Radon)**: Radon level, temperature, humidity, pressure
/// - **AranetRad (Radiation)**: Radiation rate/total, temperature, humidity
///
/// Fields for unsupported sensors (e.g., `radon` for Aranet4) will be `None`.
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
    /// Create a `StoredReading` from an `aranet_types::CurrentReading`.
    ///
    /// The database `id` is set to 0 and will be assigned by SQLite on insert.
    /// If `captured_at` is `None` in the source reading, the current time is used.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device identifier this reading came from
    /// * `reading` - The source reading from `aranet-types`
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

    /// Convert back to an `aranet_types::CurrentReading`.
    ///
    /// Note: Some fields are not preserved in storage:
    /// - `interval` and `age` are set to 0
    /// - `radon_avg_*` fields are set to `None`
    ///
    /// Use this when you need to pass stored data to functions expecting `CurrentReading`.
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

/// A historical sensor reading downloaded from device memory.
///
/// Aranet devices store readings in internal memory at their configured interval.
/// These records are downloaded via BLE and cached locally to avoid repeated
/// downloads. The `timestamp` is the original measurement time from the device,
/// while `synced_at` tracks when it was downloaded to this database.
///
/// Records are deduplicated by `(device_id, timestamp)` - downloading the same
/// record twice will not create duplicates.
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
    /// Create a `StoredHistoryRecord` from an `aranet_types::HistoryRecord`.
    ///
    /// The database `id` is set to 0 and will be assigned by SQLite on insert.
    /// The `synced_at` timestamp is set to the current time.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The device identifier this record came from
    /// * `record` - The source history record from `aranet-types`
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

    /// Convert back to an `aranet_types::HistoryRecord`.
    ///
    /// Use this when you need to pass stored data to functions expecting `HistoryRecord`.
    /// The `synced_at` metadata is not included in the result.
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

/// Tracks incremental sync progress for a device's history.
///
/// Aranet devices use a ring buffer for history storage, with a 1-based index.
/// `SyncState` tracks the last downloaded index so subsequent syncs can
/// download only new records instead of re-downloading everything.
///
/// # Incremental Sync Algorithm
///
/// 1. Read device's current `total_readings` count
/// 2. Call [`Store::calculate_sync_start`](crate::Store::calculate_sync_start) to get start index
/// 3. Download records from `start_index` to `total_readings`
/// 4. Call [`Store::update_sync_state`](crate::Store::update_sync_state) to save progress
///
/// # Example
///
/// ```
/// use aranet_store::Store;
///
/// let store = Store::open_in_memory()?;
/// store.upsert_device("Aranet4 17C3C", None)?;
///
/// // First sync downloads all 500 records
/// let start = store.calculate_sync_start("Aranet4 17C3C", 500)?;
/// assert_eq!(start, 1); // Start from beginning
///
/// // After syncing, save state
/// store.update_sync_state("Aranet4 17C3C", 500, 500)?;
///
/// // Next sync: device now has 510 records
/// let start = store.calculate_sync_start("Aranet4 17C3C", 510)?;
/// assert_eq!(start, 501); // Only download new records
/// # Ok::<(), aranet_store::Error>(())
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// Device identifier.
    pub device_id: String,
    /// Last downloaded history index (1-based).
    pub last_history_index: Option<u16>,
    /// Total readings on device at last sync.
    pub total_readings: Option<u16>,
    /// When the last sync completed.
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_sync_at: Option<OffsetDateTime>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    // ==================== StoredReading Tests ====================

    fn create_current_reading() -> CurrentReading {
        CurrentReading {
            co2: 850,
            temperature: 23.5,
            pressure: 1015.25,
            humidity: 48,
            battery: 75,
            status: Status::Green,
            interval: 60,
            age: 45,
            captured_at: Some(datetime!(2024-06-15 14:30:00 UTC)),
            radon: None,
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        }
    }

    fn create_current_reading_radon() -> CurrentReading {
        CurrentReading {
            co2: 0,
            temperature: 21.0,
            pressure: 1013.0,
            humidity: 55,
            battery: 90,
            status: Status::Yellow,
            interval: 3600,
            age: 1800,
            captured_at: Some(datetime!(2024-06-15 12:00:00 UTC)),
            radon: Some(150),
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: Some(145),
            radon_avg_7d: Some(140),
            radon_avg_30d: Some(138),
        }
    }

    fn create_current_reading_radiation() -> CurrentReading {
        CurrentReading {
            co2: 0,
            temperature: 20.0,
            pressure: 1010.0,
            humidity: 50,
            battery: 80,
            status: Status::Green,
            interval: 60,
            age: 30,
            captured_at: Some(datetime!(2024-06-15 16:00:00 UTC)),
            radon: None,
            radiation_rate: Some(0.12),
            radiation_total: Some(0.0025),
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        }
    }

    #[test]
    fn test_stored_reading_from_reading_basic() {
        let reading = create_current_reading();
        let stored = StoredReading::from_reading("aranet4-abc", &reading);

        assert_eq!(stored.id, 0); // ID is set by database
        assert_eq!(stored.device_id, "aranet4-abc");
        assert_eq!(stored.co2, 850);
        assert_eq!(stored.temperature, 23.5);
        assert_eq!(stored.pressure, 1015.25);
        assert_eq!(stored.humidity, 48);
        assert_eq!(stored.battery, 75);
        assert_eq!(stored.status, Status::Green);
        assert_eq!(stored.captured_at, datetime!(2024-06-15 14:30:00 UTC));
        assert!(stored.radon.is_none());
        assert!(stored.radiation_rate.is_none());
        assert!(stored.radiation_total.is_none());
    }

    #[test]
    fn test_stored_reading_from_reading_with_radon() {
        let reading = create_current_reading_radon();
        let stored = StoredReading::from_reading("aranet-rn", &reading);

        assert_eq!(stored.radon, Some(150));
        assert!(stored.radiation_rate.is_none());
        assert!(stored.radiation_total.is_none());
    }

    #[test]
    fn test_stored_reading_from_reading_with_radiation() {
        let reading = create_current_reading_radiation();
        let stored = StoredReading::from_reading("aranet-rad", &reading);

        assert!(stored.radon.is_none());
        assert_eq!(stored.radiation_rate, Some(0.12));
        assert_eq!(stored.radiation_total, Some(0.0025));
    }

    #[test]
    fn test_stored_reading_from_reading_without_captured_at() {
        let mut reading = create_current_reading();
        reading.captured_at = None;

        let before = OffsetDateTime::now_utc();
        let stored = StoredReading::from_reading("device", &reading);
        let after = OffsetDateTime::now_utc();

        // Should use current time if captured_at is None
        assert!(stored.captured_at >= before);
        assert!(stored.captured_at <= after);
    }

    #[test]
    fn test_stored_reading_to_reading_roundtrip() {
        let original = create_current_reading();
        let stored = StoredReading::from_reading("test-device", &original);
        let converted = stored.to_reading();

        assert_eq!(converted.co2, original.co2);
        assert_eq!(converted.temperature, original.temperature);
        assert_eq!(converted.pressure, original.pressure);
        assert_eq!(converted.humidity, original.humidity);
        assert_eq!(converted.battery, original.battery);
        assert_eq!(converted.status, original.status);
        assert_eq!(converted.captured_at, original.captured_at);
        assert_eq!(converted.radon, original.radon);
        assert_eq!(converted.radiation_rate, original.radiation_rate);
        assert_eq!(converted.radiation_total, original.radiation_total);
    }

    #[test]
    fn test_stored_reading_to_reading_sets_defaults() {
        let reading = create_current_reading();
        let stored = StoredReading::from_reading("test", &reading);
        let converted = stored.to_reading();

        // These fields are lost in storage but should have defaults
        assert_eq!(converted.interval, 0);
        assert_eq!(converted.age, 0);
        assert!(converted.radon_avg_24h.is_none());
        assert!(converted.radon_avg_7d.is_none());
        assert!(converted.radon_avg_30d.is_none());
    }

    #[test]
    fn test_stored_reading_to_reading_with_radon() {
        let original = create_current_reading_radon();
        let stored = StoredReading::from_reading("radon-device", &original);
        let converted = stored.to_reading();

        assert_eq!(converted.radon, Some(150));
        // Note: radon averages are NOT preserved in storage
        assert!(converted.radon_avg_24h.is_none());
    }

    #[test]
    fn test_stored_reading_all_status_values() {
        for status in [Status::Green, Status::Yellow, Status::Red, Status::Error] {
            let mut reading = create_current_reading();
            reading.status = status;
            let stored = StoredReading::from_reading("dev", &reading);
            assert_eq!(stored.status, status);
        }
    }

    #[test]
    fn test_stored_reading_serialization() {
        let reading = create_current_reading();
        let stored = StoredReading::from_reading("test", &reading);

        let json = serde_json::to_string(&stored).unwrap();
        let deserialized: StoredReading = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.device_id, stored.device_id);
        assert_eq!(deserialized.co2, stored.co2);
        assert_eq!(deserialized.temperature, stored.temperature);
    }

    #[test]
    fn test_stored_reading_clone() {
        let reading = create_current_reading();
        let stored = StoredReading::from_reading("test", &reading);
        let cloned = stored.clone();

        assert_eq!(cloned.device_id, stored.device_id);
        assert_eq!(cloned.co2, stored.co2);
    }

    // ==================== StoredHistoryRecord Tests ====================

    fn create_history_record() -> HistoryRecord {
        HistoryRecord {
            timestamp: datetime!(2024-05-20 10:00:00 UTC),
            co2: 720,
            temperature: 21.5,
            pressure: 1018.5,
            humidity: 52,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        }
    }

    fn create_history_record_radon() -> HistoryRecord {
        HistoryRecord {
            timestamp: datetime!(2024-05-20 11:00:00 UTC),
            co2: 0,
            temperature: 20.0,
            pressure: 1012.0,
            humidity: 60,
            radon: Some(180),
            radiation_rate: None,
            radiation_total: None,
        }
    }

    fn create_history_record_radiation() -> HistoryRecord {
        HistoryRecord {
            timestamp: datetime!(2024-05-20 12:00:00 UTC),
            co2: 0,
            temperature: 19.5,
            pressure: 1011.0,
            humidity: 58,
            radon: None,
            radiation_rate: Some(0.15),
            radiation_total: Some(0.003),
        }
    }

    #[test]
    fn test_stored_history_record_from_history_basic() {
        let record = create_history_record();
        let stored = StoredHistoryRecord::from_history("device-123", &record);

        assert_eq!(stored.id, 0);
        assert_eq!(stored.device_id, "device-123");
        assert_eq!(stored.timestamp, datetime!(2024-05-20 10:00:00 UTC));
        assert_eq!(stored.co2, 720);
        assert_eq!(stored.temperature, 21.5);
        assert_eq!(stored.pressure, 1018.5);
        assert_eq!(stored.humidity, 52);
        assert!(stored.radon.is_none());
        assert!(stored.radiation_rate.is_none());
        assert!(stored.radiation_total.is_none());
    }

    #[test]
    fn test_stored_history_record_from_history_sets_synced_at() {
        let record = create_history_record();

        let before = OffsetDateTime::now_utc();
        let stored = StoredHistoryRecord::from_history("device", &record);
        let after = OffsetDateTime::now_utc();

        assert!(stored.synced_at >= before);
        assert!(stored.synced_at <= after);
    }

    #[test]
    fn test_stored_history_record_from_history_with_radon() {
        let record = create_history_record_radon();
        let stored = StoredHistoryRecord::from_history("radon-dev", &record);

        assert_eq!(stored.radon, Some(180));
        assert!(stored.radiation_rate.is_none());
    }

    #[test]
    fn test_stored_history_record_from_history_with_radiation() {
        let record = create_history_record_radiation();
        let stored = StoredHistoryRecord::from_history("rad-dev", &record);

        assert!(stored.radon.is_none());
        assert_eq!(stored.radiation_rate, Some(0.15));
        assert_eq!(stored.radiation_total, Some(0.003));
    }

    #[test]
    fn test_stored_history_record_to_history_roundtrip() {
        let original = create_history_record();
        let stored = StoredHistoryRecord::from_history("test", &original);
        let converted = stored.to_history();

        assert_eq!(converted.timestamp, original.timestamp);
        assert_eq!(converted.co2, original.co2);
        assert_eq!(converted.temperature, original.temperature);
        assert_eq!(converted.pressure, original.pressure);
        assert_eq!(converted.humidity, original.humidity);
        assert_eq!(converted.radon, original.radon);
        assert_eq!(converted.radiation_rate, original.radiation_rate);
        assert_eq!(converted.radiation_total, original.radiation_total);
    }

    #[test]
    fn test_stored_history_record_to_history_radon_roundtrip() {
        let original = create_history_record_radon();
        let stored = StoredHistoryRecord::from_history("test", &original);
        let converted = stored.to_history();

        assert_eq!(converted.radon, Some(180));
    }

    #[test]
    fn test_stored_history_record_to_history_radiation_roundtrip() {
        let original = create_history_record_radiation();
        let stored = StoredHistoryRecord::from_history("test", &original);
        let converted = stored.to_history();

        assert_eq!(converted.radiation_rate, Some(0.15));
        assert_eq!(converted.radiation_total, Some(0.003));
    }

    #[test]
    fn test_stored_history_record_serialization() {
        let record = create_history_record();
        let stored = StoredHistoryRecord::from_history("test", &record);

        let json = serde_json::to_string(&stored).unwrap();
        let deserialized: StoredHistoryRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.device_id, stored.device_id);
        assert_eq!(deserialized.timestamp, stored.timestamp);
        assert_eq!(deserialized.co2, stored.co2);
    }

    #[test]
    fn test_stored_history_record_clone() {
        let record = create_history_record();
        let stored = StoredHistoryRecord::from_history("test", &record);
        let cloned = stored.clone();

        assert_eq!(cloned.device_id, stored.device_id);
        assert_eq!(cloned.timestamp, stored.timestamp);
    }

    // ==================== StoredDevice Tests ====================

    #[test]
    fn test_stored_device_serialization() {
        let device = StoredDevice {
            id: "aranet4-xyz".to_string(),
            name: Some("Living Room".to_string()),
            device_type: Some(DeviceType::Aranet4),
            serial: Some("1234567".to_string()),
            firmware: Some("v1.2.0".to_string()),
            hardware: Some("1.0".to_string()),
            first_seen: datetime!(2024-01-01 00:00:00 UTC),
            last_seen: datetime!(2024-06-15 12:00:00 UTC),
        };

        let json = serde_json::to_string(&device).unwrap();
        let deserialized: StoredDevice = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, device.id);
        assert_eq!(deserialized.name, device.name);
        assert_eq!(deserialized.device_type, device.device_type);
        assert_eq!(deserialized.serial, device.serial);
        assert_eq!(deserialized.firmware, device.firmware);
        assert_eq!(deserialized.first_seen, device.first_seen);
        assert_eq!(deserialized.last_seen, device.last_seen);
    }

    #[test]
    fn test_stored_device_all_device_types() {
        for device_type in [
            DeviceType::Aranet4,
            DeviceType::Aranet2,
            DeviceType::AranetRadon,
            DeviceType::AranetRadiation,
        ] {
            let device = StoredDevice {
                id: "test".to_string(),
                name: None,
                device_type: Some(device_type),
                serial: None,
                firmware: None,
                hardware: None,
                first_seen: OffsetDateTime::now_utc(),
                last_seen: OffsetDateTime::now_utc(),
            };

            let json = serde_json::to_string(&device).unwrap();
            let deserialized: StoredDevice = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized.device_type, Some(device_type));
        }
    }

    #[test]
    fn test_stored_device_optional_fields() {
        let device = StoredDevice {
            id: "minimal-device".to_string(),
            name: None,
            device_type: None,
            serial: None,
            firmware: None,
            hardware: None,
            first_seen: datetime!(2024-06-01 00:00:00 UTC),
            last_seen: datetime!(2024-06-01 00:00:00 UTC),
        };

        assert!(device.name.is_none());
        assert!(device.device_type.is_none());
        assert!(device.serial.is_none());
        assert!(device.firmware.is_none());
        assert!(device.hardware.is_none());
    }

    #[test]
    fn test_stored_device_clone() {
        let device = StoredDevice {
            id: "clone-test".to_string(),
            name: Some("Test".to_string()),
            device_type: Some(DeviceType::Aranet4),
            serial: Some("123".to_string()),
            firmware: Some("v1.0".to_string()),
            hardware: Some("1.0".to_string()),
            first_seen: OffsetDateTime::now_utc(),
            last_seen: OffsetDateTime::now_utc(),
        };

        let cloned = device.clone();
        assert_eq!(cloned.id, device.id);
        assert_eq!(cloned.name, device.name);
    }

    // ==================== SyncState Tests ====================

    #[test]
    fn test_sync_state_serialization() {
        let state = SyncState {
            device_id: "sync-device".to_string(),
            last_history_index: Some(500),
            total_readings: Some(500),
            last_sync_at: Some(datetime!(2024-06-15 18:00:00 UTC)),
        };

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: SyncState = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.device_id, state.device_id);
        assert_eq!(deserialized.last_history_index, state.last_history_index);
        assert_eq!(deserialized.total_readings, state.total_readings);
        assert_eq!(deserialized.last_sync_at, state.last_sync_at);
    }

    #[test]
    fn test_sync_state_with_none_values() {
        let state = SyncState {
            device_id: "new-device".to_string(),
            last_history_index: None,
            total_readings: None,
            last_sync_at: None,
        };

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: SyncState = serde_json::from_str(&json).unwrap();

        assert!(deserialized.last_history_index.is_none());
        assert!(deserialized.total_readings.is_none());
        assert!(deserialized.last_sync_at.is_none());
    }

    #[test]
    fn test_sync_state_clone() {
        let state = SyncState {
            device_id: "clone-test".to_string(),
            last_history_index: Some(100),
            total_readings: Some(100),
            last_sync_at: Some(OffsetDateTime::now_utc()),
        };

        let cloned = state.clone();
        assert_eq!(cloned.device_id, state.device_id);
        assert_eq!(cloned.last_history_index, state.last_history_index);
    }

    #[test]
    fn test_sync_state_debug() {
        let state = SyncState {
            device_id: "debug-test".to_string(),
            last_history_index: Some(42),
            total_readings: Some(42),
            last_sync_at: None,
        };

        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("SyncState"));
        assert!(debug_str.contains("debug-test"));
        assert!(debug_str.contains("42"));
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_stored_reading_extreme_values() {
        let reading = CurrentReading {
            co2: u16::MAX,
            temperature: f32::MAX,
            pressure: f32::MAX,
            humidity: u8::MAX,
            battery: u8::MAX,
            status: Status::Error,
            interval: u16::MAX,
            age: u16::MAX,
            captured_at: Some(OffsetDateTime::UNIX_EPOCH),
            radon: Some(u32::MAX),
            radiation_rate: Some(f32::MAX),
            radiation_total: Some(f64::MAX),
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        };

        let stored = StoredReading::from_reading("extreme", &reading);
        let converted = stored.to_reading();

        assert_eq!(converted.co2, u16::MAX);
        assert_eq!(converted.humidity, u8::MAX);
        assert_eq!(converted.battery, u8::MAX);
        assert_eq!(converted.radon, Some(u32::MAX));
    }

    #[test]
    fn test_stored_reading_zero_values() {
        let reading = CurrentReading {
            co2: 0,
            temperature: 0.0,
            pressure: 0.0,
            humidity: 0,
            battery: 0,
            status: Status::Green,
            interval: 0,
            age: 0,
            captured_at: Some(OffsetDateTime::UNIX_EPOCH),
            radon: Some(0),
            radiation_rate: Some(0.0),
            radiation_total: Some(0.0),
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        };

        let stored = StoredReading::from_reading("zero", &reading);
        let converted = stored.to_reading();

        assert_eq!(converted.co2, 0);
        assert_eq!(converted.temperature, 0.0);
        assert_eq!(converted.radon, Some(0));
    }

    #[test]
    fn test_stored_history_record_zero_values() {
        let record = HistoryRecord {
            timestamp: OffsetDateTime::UNIX_EPOCH,
            co2: 0,
            temperature: 0.0,
            pressure: 0.0,
            humidity: 0,
            radon: Some(0),
            radiation_rate: Some(0.0),
            radiation_total: Some(0.0),
        };

        let stored = StoredHistoryRecord::from_history("zero", &record);
        let converted = stored.to_history();

        assert_eq!(converted.co2, 0);
        assert_eq!(converted.radon, Some(0));
    }
}
