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
pub use types::{CurrentReading, DeviceInfo, DeviceType, HistoryRecord, Status};
pub use uuid as uuids;

#[cfg(test)]
mod tests {
    use super::*;

    // --- CurrentReading parsing tests ---

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
        assert!(err.to_string().contains("requires 13 bytes"));
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
        let cloned = status.clone();
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
        let cloned = device_type.clone();
        assert_eq!(device_type, cloned);
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

    // --- ParseError tests ---

    #[test]
    fn test_parse_error_display() {
        let err = ParseError::InvalidData("test message".to_string());
        assert_eq!(err.to_string(), "Invalid data: test message");
    }

    #[test]
    fn test_parse_error_debug() {
        let err = ParseError::InvalidData("debug test".to_string());
        let debug_str = format!("{:?}", err);
        assert!(debug_str.contains("InvalidData"));
        assert!(debug_str.contains("debug test"));
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
            radon: None,
            radiation_rate: None,
            radiation_total: None,
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
}
