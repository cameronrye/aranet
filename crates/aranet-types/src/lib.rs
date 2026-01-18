//! Platform-agnostic types for Aranet environmental sensors.
//!
//! This crate provides shared types that can be used by both native
//! (aranet-core) and WebAssembly (aranet-wasm) implementations.
//!
//! # Features
//!
//! - Core data types for sensor readings
//! - Device information structures
//! - UUID constants for BLE characteristics
//! - Error types for data parsing
//!
//! # Example
//!
//! ```
//! use aranet_types::{CurrentReading, Status, DeviceType};
//!
//! // Types can be used for parsing and serialization
//! ```

pub mod error;
pub mod types;
pub mod uuid;

pub use error::{ParseError, ParseResult};
pub use types::{
    CurrentReading, CurrentReadingBuilder, DeviceInfo, DeviceInfoBuilder, DeviceType,
    HistoryRecord, HistoryRecordBuilder, MIN_CURRENT_READING_BYTES, Status,
};

// Re-export uuid module with a clearer name to avoid confusion with the `uuid` crate.
// The `uuids` alias is kept for backwards compatibility.
pub use uuid as ble;
#[doc(hidden)]
pub use uuid as uuids;

/// Unit tests for aranet-types.
///
/// # Test Coverage
///
/// This module provides comprehensive tests for all public types and parsing functions:
///
/// ## CurrentReading Tests
/// - Parsing from valid 13-byte Aranet4 format
/// - Parsing from valid 7-byte Aranet2 format
/// - Error handling for insufficient bytes
/// - Edge cases (all zeros, max values)
/// - Builder pattern validation
/// - Serialization/deserialization roundtrips
///
/// ## Status Enum Tests
/// - Conversion from u8 values (0-3 and unknown)
/// - Display and Debug formatting
/// - Equality and ordering
///
/// ## DeviceType Tests
/// - Conversion from u8 device codes (0xF1-0xF4)
/// - Name-based detection from device names
/// - Display formatting
/// - Hash implementation for use in collections
///
/// ## DeviceInfo Tests
/// - Clone and Debug implementations
/// - Default values
/// - Equality comparisons
///
/// ## HistoryRecord Tests
/// - Clone and equality
/// - Timestamp handling
///
/// ## ParseError Tests
/// - Error message formatting
/// - Equality comparisons
/// - Helper constructors
///
/// ## BLE UUID Tests
/// - Service UUID constants
/// - Characteristic UUID constants
///
/// # Running Tests
///
/// ```bash
/// cargo test -p aranet-types
/// ```
#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // CurrentReading parsing tests
    // ========================================================================

    #[test]
    fn test_parse_current_reading_from_valid_bytes() {
        // Construct test bytes:
        // CO2: 800 (0x0320 LE -> [0x20, 0x03])
        // Temperature: 450 raw (22.5°C = 450/20) -> [0xC2, 0x01]
        // Pressure: 10132 raw (1013.2 hPa = 10132/10) -> [0x94, 0x27]
        // Humidity: 45
        // Battery: 85
        // Status: 1 (Green)
        // Interval: 300 -> [0x2C, 0x01]
        // Age: 120 -> [0x78, 0x00]
        let bytes: [u8; 13] = [
            0x20, 0x03, // CO2 = 800
            0xC2, 0x01, // temp_raw = 450
            0x94, 0x27, // pressure_raw = 10132
            45,   // humidity
            85,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
            0x78, 0x00, // age = 120
        ];

        let reading = CurrentReading::from_bytes(&bytes).unwrap();

        assert_eq!(reading.co2, 800);
        assert!((reading.temperature - 22.5).abs() < 0.01);
        assert!((reading.pressure - 1013.2).abs() < 0.1);
        assert_eq!(reading.humidity, 45);
        assert_eq!(reading.battery, 85);
        assert_eq!(reading.status, Status::Green);
        assert_eq!(reading.interval, 300);
        assert_eq!(reading.age, 120);
    }

    #[test]
    fn test_parse_current_reading_from_insufficient_bytes() {
        let bytes: [u8; 10] = [0; 10]; // Only 10 bytes, need 13

        let result = CurrentReading::from_bytes(&bytes);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err,
            ParseError::InsufficientBytes {
                expected: 13,
                actual: 10
            }
        );
        assert!(err.to_string().contains("expected 13"));
        assert!(err.to_string().contains("got 10"));
    }

    #[test]
    fn test_parse_current_reading_zero_bytes() {
        let bytes: [u8; 0] = [];

        let result = CurrentReading::from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_current_reading_all_zeros() {
        let bytes: [u8; 13] = [0; 13];

        let reading = CurrentReading::from_bytes(&bytes).unwrap();
        assert_eq!(reading.co2, 0);
        assert!((reading.temperature - 0.0).abs() < 0.01);
        assert!((reading.pressure - 0.0).abs() < 0.1);
        assert_eq!(reading.humidity, 0);
        assert_eq!(reading.battery, 0);
        assert_eq!(reading.status, Status::Error);
        assert_eq!(reading.interval, 0);
        assert_eq!(reading.age, 0);
    }

    #[test]
    fn test_parse_current_reading_max_values() {
        let bytes: [u8; 13] = [
            0xFF, 0xFF, // CO2 = 65535
            0xFF, 0xFF, // temp_raw = 65535
            0xFF, 0xFF, // pressure_raw = 65535
            0xFF, // humidity = 255
            0xFF, // battery = 255
            3,    // status = Red
            0xFF, 0xFF, // interval = 65535
            0xFF, 0xFF, // age = 65535
        ];

        let reading = CurrentReading::from_bytes(&bytes).unwrap();
        assert_eq!(reading.co2, 65535);
        assert!((reading.temperature - 3276.75).abs() < 0.01); // 65535/20
        assert!((reading.pressure - 6553.5).abs() < 0.1); // 65535/10
        assert_eq!(reading.humidity, 255);
        assert_eq!(reading.battery, 255);
        assert_eq!(reading.interval, 65535);
        assert_eq!(reading.age, 65535);
    }

    #[test]
    fn test_parse_current_reading_high_co2_red_status() {
        // 2000 ppm CO2 = Red status
        let bytes: [u8; 13] = [
            0xD0, 0x07, // CO2 = 2000
            0xC2, 0x01, // temp
            0x94, 0x27, // pressure
            50, 80, 3, // Red status
            0x2C, 0x01, 0x78, 0x00,
        ];

        let reading = CurrentReading::from_bytes(&bytes).unwrap();
        assert_eq!(reading.co2, 2000);
        assert_eq!(reading.status, Status::Red);
    }

    #[test]
    fn test_parse_current_reading_moderate_co2_yellow_status() {
        // 1200 ppm CO2 = Yellow status
        let bytes: [u8; 13] = [
            0xB0, 0x04, // CO2 = 1200
            0xC2, 0x01, 0x94, 0x27, 50, 80, 2, // Yellow status
            0x2C, 0x01, 0x78, 0x00,
        ];

        let reading = CurrentReading::from_bytes(&bytes).unwrap();
        assert_eq!(reading.co2, 1200);
        assert_eq!(reading.status, Status::Yellow);
    }

    #[test]
    fn test_parse_current_reading_extra_bytes_ignored() {
        // More than 13 bytes should work (extra bytes ignored)
        let bytes: [u8; 16] = [
            0x20, 0x03, 0xC2, 0x01, 0x94, 0x27, 45, 85, 1, 0x2C, 0x01, 0x78, 0x00, 0xAA, 0xBB, 0xCC,
        ];

        let reading = CurrentReading::from_bytes(&bytes).unwrap();
        assert_eq!(reading.co2, 800);
    }

    // --- Status enum tests ---

    #[test]
    fn test_status_from_u8() {
        assert_eq!(Status::from(0), Status::Error);
        assert_eq!(Status::from(1), Status::Green);
        assert_eq!(Status::from(2), Status::Yellow);
        assert_eq!(Status::from(3), Status::Red);
        // Unknown values should map to Error
        assert_eq!(Status::from(4), Status::Error);
        assert_eq!(Status::from(255), Status::Error);
    }

    #[test]
    fn test_status_repr_values() {
        assert_eq!(Status::Error as u8, 0);
        assert_eq!(Status::Green as u8, 1);
        assert_eq!(Status::Yellow as u8, 2);
        assert_eq!(Status::Red as u8, 3);
    }

    #[test]
    fn test_status_debug() {
        assert_eq!(format!("{:?}", Status::Green), "Green");
        assert_eq!(format!("{:?}", Status::Yellow), "Yellow");
        assert_eq!(format!("{:?}", Status::Red), "Red");
        assert_eq!(format!("{:?}", Status::Error), "Error");
    }

    #[test]
    fn test_status_clone() {
        let status = Status::Green;
        // Status implements Copy, so we can just copy it
        let cloned = status;
        assert_eq!(status, cloned);
    }

    #[test]
    fn test_status_copy() {
        let status = Status::Red;
        let copied = status; // Copy
        assert_eq!(status, copied); // Original still valid
    }

    // --- DeviceType enum tests ---

    #[test]
    fn test_device_type_values() {
        assert_eq!(DeviceType::Aranet4 as u8, 0xF1);
        assert_eq!(DeviceType::Aranet2 as u8, 0xF2);
        assert_eq!(DeviceType::AranetRadon as u8, 0xF3);
        assert_eq!(DeviceType::AranetRadiation as u8, 0xF4);
    }

    #[test]
    fn test_device_type_debug() {
        assert_eq!(format!("{:?}", DeviceType::Aranet4), "Aranet4");
        assert_eq!(format!("{:?}", DeviceType::Aranet2), "Aranet2");
        assert_eq!(format!("{:?}", DeviceType::AranetRadon), "AranetRadon");
        assert_eq!(
            format!("{:?}", DeviceType::AranetRadiation),
            "AranetRadiation"
        );
    }

    #[test]
    fn test_device_type_clone() {
        let device_type = DeviceType::Aranet4;
        // DeviceType implements Copy, so we can just copy it
        let cloned = device_type;
        assert_eq!(device_type, cloned);
    }

    #[test]
    fn test_device_type_try_from_u8() {
        assert_eq!(DeviceType::try_from(0xF1), Ok(DeviceType::Aranet4));
        assert_eq!(DeviceType::try_from(0xF2), Ok(DeviceType::Aranet2));
        assert_eq!(DeviceType::try_from(0xF3), Ok(DeviceType::AranetRadon));
        assert_eq!(DeviceType::try_from(0xF4), Ok(DeviceType::AranetRadiation));
    }

    #[test]
    fn test_device_type_try_from_u8_invalid() {
        let result = DeviceType::try_from(0x00);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ParseError::UnknownDeviceType(0x00));

        let result = DeviceType::try_from(0xFF);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), ParseError::UnknownDeviceType(0xFF));
    }

    #[test]
    fn test_device_type_display() {
        assert_eq!(format!("{}", DeviceType::Aranet4), "Aranet4");
        assert_eq!(format!("{}", DeviceType::Aranet2), "Aranet2");
        assert_eq!(format!("{}", DeviceType::AranetRadon), "Aranet Radon");
        assert_eq!(
            format!("{}", DeviceType::AranetRadiation),
            "Aranet Radiation"
        );
    }

    #[test]
    fn test_device_type_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(DeviceType::Aranet4);
        set.insert(DeviceType::Aranet2);
        set.insert(DeviceType::Aranet4); // duplicate
        assert_eq!(set.len(), 2);
        assert!(set.contains(&DeviceType::Aranet4));
        assert!(set.contains(&DeviceType::Aranet2));
    }

    #[test]
    fn test_status_display() {
        assert_eq!(format!("{}", Status::Error), "Error");
        assert_eq!(format!("{}", Status::Green), "Good");
        assert_eq!(format!("{}", Status::Yellow), "Moderate");
        assert_eq!(format!("{}", Status::Red), "High");
    }

    #[test]
    fn test_status_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Status::Green);
        set.insert(Status::Yellow);
        set.insert(Status::Green); // duplicate
        assert_eq!(set.len(), 2);
        assert!(set.contains(&Status::Green));
        assert!(set.contains(&Status::Yellow));
    }

    // --- DeviceInfo tests ---

    #[test]
    fn test_device_info_creation() {
        let info = types::DeviceInfo {
            name: "Aranet4 12345".to_string(),
            model: "Aranet4".to_string(),
            serial: "12345".to_string(),
            firmware: "v1.2.0".to_string(),
            hardware: "1.0".to_string(),
            software: "1.2.0".to_string(),
            manufacturer: "SAF Tehnika".to_string(),
        };

        assert_eq!(info.name, "Aranet4 12345");
        assert_eq!(info.serial, "12345");
        assert_eq!(info.manufacturer, "SAF Tehnika");
    }

    #[test]
    fn test_device_info_clone() {
        let info = types::DeviceInfo {
            name: "Test".to_string(),
            model: "Model".to_string(),
            serial: "123".to_string(),
            firmware: "1.0".to_string(),
            hardware: "1.0".to_string(),
            software: "1.0".to_string(),
            manufacturer: "Mfg".to_string(),
        };

        let cloned = info.clone();
        assert_eq!(cloned.name, info.name);
        assert_eq!(cloned.serial, info.serial);
    }

    #[test]
    fn test_device_info_debug() {
        let info = types::DeviceInfo {
            name: "Aranet4".to_string(),
            model: "".to_string(),
            serial: "".to_string(),
            firmware: "".to_string(),
            hardware: "".to_string(),
            software: "".to_string(),
            manufacturer: "".to_string(),
        };

        let debug_str = format!("{:?}", info);
        assert!(debug_str.contains("Aranet4"));
    }

    #[test]
    fn test_device_info_default() {
        let info = types::DeviceInfo::default();
        assert_eq!(info.name, "");
        assert_eq!(info.model, "");
        assert_eq!(info.serial, "");
        assert_eq!(info.firmware, "");
        assert_eq!(info.hardware, "");
        assert_eq!(info.software, "");
        assert_eq!(info.manufacturer, "");
    }

    #[test]
    fn test_device_info_equality() {
        let info1 = types::DeviceInfo {
            name: "Test".to_string(),
            model: "Model".to_string(),
            serial: "123".to_string(),
            firmware: "1.0".to_string(),
            hardware: "1.0".to_string(),
            software: "1.0".to_string(),
            manufacturer: "Mfg".to_string(),
        };
        let info2 = info1.clone();
        let info3 = types::DeviceInfo {
            name: "Different".to_string(),
            ..info1.clone()
        };
        assert_eq!(info1, info2);
        assert_ne!(info1, info3);
    }

    // --- HistoryRecord tests ---

    #[test]
    fn test_history_record_creation() {
        use time::OffsetDateTime;

        let record = types::HistoryRecord {
            timestamp: OffsetDateTime::UNIX_EPOCH,
            co2: 800,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 45,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        };

        assert_eq!(record.co2, 800);
        assert!((record.temperature - 22.5).abs() < 0.01);
        assert!((record.pressure - 1013.2).abs() < 0.1);
        assert_eq!(record.humidity, 45);
        assert!(record.radon.is_none());
        assert!(record.radiation_rate.is_none());
        assert!(record.radiation_total.is_none());
    }

    #[test]
    fn test_history_record_clone() {
        use time::OffsetDateTime;

        let record = types::HistoryRecord {
            timestamp: OffsetDateTime::UNIX_EPOCH,
            co2: 500,
            temperature: 20.0,
            pressure: 1000.0,
            humidity: 50,
            radon: Some(100),
            radiation_rate: Some(0.15),
            radiation_total: Some(1.5),
        };

        let cloned = record.clone();
        assert_eq!(cloned.co2, record.co2);
        assert_eq!(cloned.humidity, record.humidity);
        assert_eq!(cloned.radon, Some(100));
        assert_eq!(cloned.radiation_rate, Some(0.15));
        assert_eq!(cloned.radiation_total, Some(1.5));
    }

    #[test]
    fn test_history_record_equality() {
        use time::OffsetDateTime;

        let record1 = types::HistoryRecord {
            timestamp: OffsetDateTime::UNIX_EPOCH,
            co2: 800,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 45,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
        };
        let record2 = record1.clone();
        assert_eq!(record1, record2);
    }

    #[test]
    fn test_current_reading_equality() {
        let reading1 = CurrentReading {
            co2: 800,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 45,
            battery: 85,
            status: Status::Green,
            interval: 300,
            age: 120,
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        };
        // CurrentReading implements Copy, so we can just copy it
        let reading2 = reading1;
        assert_eq!(reading1, reading2);
    }

    #[test]
    fn test_min_current_reading_bytes_const() {
        assert_eq!(MIN_CURRENT_READING_BYTES, 13);
        // Ensure buffer of exact size works
        let bytes = [0u8; MIN_CURRENT_READING_BYTES];
        assert!(CurrentReading::from_bytes(&bytes).is_ok());
        // Ensure buffer one byte short fails
        let short_bytes = [0u8; MIN_CURRENT_READING_BYTES - 1];
        assert!(CurrentReading::from_bytes(&short_bytes).is_err());
    }

    // --- ParseError tests ---

    #[test]
    fn test_parse_error_display() {
        let err = ParseError::invalid_value("test message");
        assert_eq!(err.to_string(), "Invalid value: test message");
    }

    #[test]
    fn test_parse_error_insufficient_bytes() {
        let err = ParseError::InsufficientBytes {
            expected: 13,
            actual: 5,
        };
        assert_eq!(err.to_string(), "Insufficient bytes: expected 13, got 5");
    }

    #[test]
    fn test_parse_error_unknown_device_type() {
        let err = ParseError::UnknownDeviceType(0xAB);
        assert_eq!(err.to_string(), "Unknown device type: 0xAB");
    }

    #[test]
    fn test_parse_error_invalid_value() {
        let err = ParseError::InvalidValue("bad value".to_string());
        assert_eq!(err.to_string(), "Invalid value: bad value");
    }

    #[test]
    fn test_parse_error_debug() {
        let err = ParseError::invalid_value("debug test");
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("InvalidValue"));
        assert!(debug_str.contains("debug test"));
    }

    #[test]
    fn test_parse_error_equality() {
        let err1 = ParseError::InsufficientBytes {
            expected: 10,
            actual: 5,
        };
        let err2 = ParseError::InsufficientBytes {
            expected: 10,
            actual: 5,
        };
        let err3 = ParseError::InsufficientBytes {
            expected: 10,
            actual: 6,
        };
        assert_eq!(err1, err2);
        assert_ne!(err1, err3);
    }

    // --- Serialization tests ---

    #[test]
    fn test_current_reading_serialization() {
        let reading = CurrentReading {
            co2: 800,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 45,
            battery: 85,
            status: Status::Green,
            interval: 300,
            age: 120,
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        };

        let json = serde_json::to_string(&reading).unwrap();
        assert!(json.contains("\"co2\":800"));
        assert!(json.contains("\"humidity\":45"));
    }

    #[test]
    fn test_current_reading_deserialization() {
        let json = r#"{"co2":800,"temperature":22.5,"pressure":1013.2,"humidity":45,"battery":85,"status":"Green","interval":300,"age":120,"radon":null,"radiation_rate":null,"radiation_total":null}"#;

        let reading: CurrentReading = serde_json::from_str(json).unwrap();
        assert_eq!(reading.co2, 800);
        assert_eq!(reading.status, Status::Green);
    }

    #[test]
    fn test_status_serialization() {
        assert_eq!(serde_json::to_string(&Status::Green).unwrap(), "\"Green\"");
        assert_eq!(
            serde_json::to_string(&Status::Yellow).unwrap(),
            "\"Yellow\""
        );
        assert_eq!(serde_json::to_string(&Status::Red).unwrap(), "\"Red\"");
        assert_eq!(serde_json::to_string(&Status::Error).unwrap(), "\"Error\"");
    }

    #[test]
    fn test_device_type_serialization() {
        assert_eq!(
            serde_json::to_string(&DeviceType::Aranet4).unwrap(),
            "\"Aranet4\""
        );
        assert_eq!(
            serde_json::to_string(&DeviceType::AranetRadon).unwrap(),
            "\"AranetRadon\""
        );
    }

    #[test]
    fn test_device_info_serialization_roundtrip() {
        let info = types::DeviceInfo {
            name: "Test Device".to_string(),
            model: "Model X".to_string(),
            serial: "SN12345".to_string(),
            firmware: "1.2.3".to_string(),
            hardware: "2.0".to_string(),
            software: "3.0".to_string(),
            manufacturer: "Acme Corp".to_string(),
        };

        let json = serde_json::to_string(&info).unwrap();
        let deserialized: types::DeviceInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, info.name);
        assert_eq!(deserialized.serial, info.serial);
        assert_eq!(deserialized.manufacturer, info.manufacturer);
    }

    // --- New feature tests ---

    #[test]
    fn test_status_ordering() {
        // Status should be ordered by severity
        assert!(Status::Error < Status::Green);
        assert!(Status::Green < Status::Yellow);
        assert!(Status::Yellow < Status::Red);

        // Test comparison operators
        assert!(Status::Red > Status::Yellow);
        assert!(Status::Yellow >= Status::Yellow);
        assert!(Status::Green <= Status::Yellow);
    }

    #[test]
    fn test_device_type_readings_characteristic() {
        use crate::ble;

        // Aranet4 uses the original characteristic
        assert_eq!(
            DeviceType::Aranet4.readings_characteristic(),
            ble::CURRENT_READINGS_DETAIL
        );

        // Other devices use the alternate characteristic
        assert_eq!(
            DeviceType::Aranet2.readings_characteristic(),
            ble::CURRENT_READINGS_DETAIL_ALT
        );
        assert_eq!(
            DeviceType::AranetRadon.readings_characteristic(),
            ble::CURRENT_READINGS_DETAIL_ALT
        );
        assert_eq!(
            DeviceType::AranetRadiation.readings_characteristic(),
            ble::CURRENT_READINGS_DETAIL_ALT
        );
    }

    #[test]
    fn test_device_type_from_name_word_boundary() {
        // Should match at word boundaries
        assert_eq!(
            DeviceType::from_name("Aranet4 12345"),
            Some(DeviceType::Aranet4)
        );
        assert_eq!(
            DeviceType::from_name("My Aranet4"),
            Some(DeviceType::Aranet4)
        );

        // Should match case-insensitively
        assert_eq!(DeviceType::from_name("ARANET4"), Some(DeviceType::Aranet4));
        assert_eq!(DeviceType::from_name("aranet2"), Some(DeviceType::Aranet2));

        // Should match AranetRn+ naming convention (real device name format)
        assert_eq!(
            DeviceType::from_name("AranetRn+ 306B8"),
            Some(DeviceType::AranetRadon)
        );
        assert_eq!(
            DeviceType::from_name("aranetrn+ 12345"),
            Some(DeviceType::AranetRadon)
        );
    }

    #[test]
    fn test_byte_size_constants() {
        assert_eq!(MIN_CURRENT_READING_BYTES, 13);
        assert_eq!(types::MIN_ARANET2_READING_BYTES, 7);
        assert_eq!(types::MIN_RADON_READING_BYTES, 15);
        assert_eq!(types::MIN_RADON_GATT_READING_BYTES, 18);
        assert_eq!(types::MIN_RADIATION_READING_BYTES, 28);
    }

    #[test]
    fn test_from_bytes_aranet2() {
        // 7 bytes: temp(2), humidity(1), battery(1), status(1), interval(2)
        let data = [
            0x90, 0x01, // temp = 400 -> 20.0°C
            0x32, // humidity = 50
            0x55, // battery = 85
            0x01, // status = Green
            0x2C, 0x01, // interval = 300
        ];

        let reading = CurrentReading::from_bytes_aranet2(&data).unwrap();
        assert_eq!(reading.co2, 0); // Aranet2 has no CO2
        assert!((reading.temperature - 20.0).abs() < 0.1);
        assert_eq!(reading.humidity, 50);
        assert_eq!(reading.battery, 85);
        assert_eq!(reading.status, Status::Green);
        assert_eq!(reading.interval, 300);
        assert_eq!(reading.pressure, 0.0); // Aranet2 has no pressure
    }

    #[test]
    fn test_from_bytes_aranet2_insufficient() {
        let data = [0u8; 6]; // Too short
        let result = CurrentReading::from_bytes_aranet2(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes_for_device() {
        // Test dispatch to correct parser
        let aranet4_data = [0u8; 13];
        let result = CurrentReading::from_bytes_for_device(&aranet4_data, DeviceType::Aranet4);
        assert!(result.is_ok());

        let aranet2_data = [0u8; 7];
        let result = CurrentReading::from_bytes_for_device(&aranet2_data, DeviceType::Aranet2);
        assert!(result.is_ok());
    }

    #[test]
    fn test_builder_with_captured_at() {
        use time::OffsetDateTime;

        let now = OffsetDateTime::now_utc();
        let reading = CurrentReading::builder()
            .co2(800)
            .temperature(22.5)
            .captured_at(now)
            .build();

        assert_eq!(reading.co2, 800);
        assert_eq!(reading.captured_at, Some(now));
    }

    #[test]
    fn test_builder_try_build_valid() {
        let result = CurrentReading::builder()
            .co2(800)
            .temperature(22.5)
            .pressure(1013.0)
            .humidity(50)
            .battery(85)
            .try_build();

        assert!(result.is_ok());
    }

    #[test]
    fn test_builder_try_build_invalid_humidity() {
        let result = CurrentReading::builder()
            .humidity(150) // Invalid: > 100
            .try_build();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("humidity"));
    }

    #[test]
    fn test_builder_try_build_invalid_battery() {
        let result = CurrentReading::builder()
            .battery(120) // Invalid: > 100
            .try_build();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("battery"));
    }

    #[test]
    fn test_builder_try_build_invalid_temperature() {
        let result = CurrentReading::builder()
            .temperature(-50.0) // Invalid: < -40
            .try_build();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("temperature"));
    }

    #[test]
    fn test_builder_try_build_invalid_pressure() {
        let result = CurrentReading::builder()
            .temperature(22.0) // Valid temperature
            .pressure(500.0) // Invalid: < 800
            .try_build();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("pressure"));
    }

    #[test]
    fn test_with_captured_at() {
        use time::OffsetDateTime;

        let reading = CurrentReading::builder().age(60).build();

        let now = OffsetDateTime::now_utc();
        let reading_with_time = reading.with_captured_at(now);

        assert!(reading_with_time.captured_at.is_some());
        // The captured_at should be approximately now - 60 seconds
        let captured = reading_with_time.captured_at.unwrap();
        let expected = now - time::Duration::seconds(60);
        assert!((captured - expected).whole_seconds().abs() < 2);
    }

    #[test]
    fn test_parse_error_invalid_value_helper() {
        let err = ParseError::invalid_value("test error");
        assert_eq!(err.to_string(), "Invalid value: test error");
    }
}

/// Property-based tests using proptest.
///
/// These tests use randomized inputs to verify that parsing functions:
/// 1. Never panic on any input (safety guarantee)
/// 2. Correctly parse valid inputs (correctness guarantee)
/// 3. Properly roundtrip through serialization (consistency guarantee)
///
/// # Test Categories
///
/// ## Panic Safety Tests
/// - `parse_current_reading_never_panics`: Random bytes to `from_bytes`
/// - `parse_aranet2_never_panics`: Random bytes to `from_bytes_aranet2`
/// - `status_from_u8_never_panics`: Any u8 to Status
/// - `device_type_try_from_never_panics`: Any u8 to DeviceType
///
/// ## Valid Input Tests
/// - `parse_valid_aranet4_bytes`: Structured valid Aranet4 data
/// - `parse_valid_aranet2_bytes`: Structured valid Aranet2 data
///
/// ## Roundtrip Tests
/// - `current_reading_json_roundtrip`: JSON serialization consistency
///
/// # Running Property Tests
///
/// ```bash
/// cargo test -p aranet-types proptests
/// ```
///
/// To run with more test cases:
/// ```bash
/// PROPTEST_CASES=10000 cargo test -p aranet-types proptests
/// ```
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Parsing random bytes should never panic - it may return Ok or Err,
        /// but should always be safe to call.
        #[test]
        fn parse_current_reading_never_panics(data: Vec<u8>) {
            let _ = CurrentReading::from_bytes(&data);
        }

        /// Parsing random bytes for Aranet2 should never panic.
        #[test]
        fn parse_aranet2_never_panics(data: Vec<u8>) {
            let _ = CurrentReading::from_bytes_aranet2(&data);
        }

        /// Status conversion from any u8 should never panic.
        #[test]
        fn status_from_u8_never_panics(value: u8) {
            let status = Status::from(value);
            // Should always produce a valid Status variant
            let _ = format!("{:?}", status);
        }

        /// DeviceType conversion should return Ok or Err, never panic.
        #[test]
        fn device_type_try_from_never_panics(value: u8) {
            let _ = DeviceType::try_from(value);
        }

        /// Valid 13-byte input should always parse successfully for Aranet4.
        #[test]
        fn parse_valid_aranet4_bytes(
            co2 in 0u16..10000u16,
            temp_raw in 0u16..2000u16,
            pressure_raw in 8000u16..12000u16,
            humidity in 0u8..100u8,
            battery in 0u8..100u8,
            status_byte in 0u8..4u8,
            interval in 60u16..3600u16,
            age in 0u16..3600u16,
        ) {
            let mut data = [0u8; 13];
            data[0..2].copy_from_slice(&co2.to_le_bytes());
            data[2..4].copy_from_slice(&temp_raw.to_le_bytes());
            data[4..6].copy_from_slice(&pressure_raw.to_le_bytes());
            data[6] = humidity;
            data[7] = battery;
            data[8] = status_byte;
            data[9..11].copy_from_slice(&interval.to_le_bytes());
            data[11..13].copy_from_slice(&age.to_le_bytes());

            let result = CurrentReading::from_bytes(&data);
            prop_assert!(result.is_ok());

            let reading = result.unwrap();
            prop_assert_eq!(reading.co2, co2);
            prop_assert_eq!(reading.humidity, humidity);
            prop_assert_eq!(reading.battery, battery);
            prop_assert_eq!(reading.interval, interval);
            prop_assert_eq!(reading.age, age);
        }

        /// Valid 7-byte input should always parse successfully for Aranet2.
        #[test]
        fn parse_valid_aranet2_bytes(
            temp_raw in 0u16..2000u16,
            humidity in 0u8..100u8,
            battery in 0u8..100u8,
            status_byte in 0u8..4u8,
            interval in 60u16..3600u16,
        ) {
            let mut data = [0u8; 7];
            data[0..2].copy_from_slice(&temp_raw.to_le_bytes());
            data[2] = humidity;
            data[3] = battery;
            data[4] = status_byte;
            data[5..7].copy_from_slice(&interval.to_le_bytes());

            let result = CurrentReading::from_bytes_aranet2(&data);
            prop_assert!(result.is_ok());

            let reading = result.unwrap();
            prop_assert_eq!(reading.humidity, humidity);
            prop_assert_eq!(reading.battery, battery);
            prop_assert_eq!(reading.interval, interval);
        }

        /// JSON serialization roundtrip should preserve all values.
        #[test]
        fn current_reading_json_roundtrip(
            co2 in 0u16..10000u16,
            temperature in -20.0f32..60.0f32,
            pressure in 800.0f32..1200.0f32,
            humidity in 0u8..100u8,
            battery in 0u8..100u8,
            interval in 60u16..3600u16,
            age in 0u16..3600u16,
        ) {
            let reading = CurrentReading {
                co2,
                temperature,
                pressure,
                humidity,
                battery,
                status: Status::Green,
                interval,
                age,
                captured_at: None,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
                radon_avg_24h: None,
                radon_avg_7d: None,
                radon_avg_30d: None,
            };

            let json = serde_json::to_string(&reading).unwrap();
            let parsed: CurrentReading = serde_json::from_str(&json).unwrap();

            prop_assert_eq!(parsed.co2, reading.co2);
            prop_assert_eq!(parsed.humidity, reading.humidity);
            prop_assert_eq!(parsed.battery, reading.battery);
            prop_assert_eq!(parsed.interval, reading.interval);
            prop_assert_eq!(parsed.age, reading.age);
        }
    }
}
