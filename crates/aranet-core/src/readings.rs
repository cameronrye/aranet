//! Reading current sensor values.
//!
//! This module provides functionality to read the current sensor
//! values from a connected Aranet device.
//!
//! The primary methods for reading are on the [`Device`](crate::device::Device) struct,
//! but this module provides parsing utilities for different device types.

use bytes::Buf;

use crate::error::{Error, Result};
use aranet_types::{CurrentReading, DeviceType, Status};

/// Convert an `aranet_types::ParseError` into our crate's `Error`.
fn from_parse_error(e: aranet_types::ParseError) -> Error {
    Error::InvalidData(e.to_string())
}

/// Extended reading that includes all available sensor data.
///
/// This struct wraps `CurrentReading` and adds fields that don't fit
/// in the base reading structure (like measurement duration).
///
/// Note: Radon, radiation rate, and radiation total are now part of
/// `CurrentReading` directly.
#[derive(Debug, Clone)]
pub struct ExtendedReading {
    /// The current reading with all sensor values.
    pub reading: CurrentReading,
    /// Measurement duration in seconds (Aranet Radiation only).
    pub radiation_duration: Option<u64>,
}

/// Parse Aranet4 current readings from the detailed characteristic.
///
/// Format (13 bytes):
/// - bytes 0-1: CO2 (u16 LE)
/// - bytes 2-3: Temperature (u16 LE, /20 for °C)
/// - bytes 4-5: Pressure (u16 LE, /10 for hPa)
/// - byte 6: Humidity (u8)
/// - byte 7: Battery (u8)
/// - byte 8: Status (u8)
/// - bytes 9-10: Interval (u16 LE, seconds)
/// - bytes 11-12: Age (u16 LE, seconds since last reading)
pub fn parse_aranet4_reading(data: &[u8]) -> Result<CurrentReading> {
    CurrentReading::from_bytes(data).map_err(|e| Error::InvalidData(e.to_string()))
}

/// Parse Aranet2 current readings from GATT characteristic (f0cd3003).
///
/// Delegates to [`CurrentReading::from_bytes_aranet2`].
pub fn parse_aranet2_reading(data: &[u8]) -> Result<CurrentReading> {
    CurrentReading::from_bytes_aranet2(data).map_err(from_parse_error)
}

/// Parse Aranet Radon readings from advertisement data.
///
/// Format includes radon concentration in Bq/m³.
pub fn parse_aranet_radon_reading(data: &[u8]) -> Result<ExtendedReading> {
    if data.len() < 15 {
        return Err(Error::InvalidData(format!(
            "Aranet Radon reading requires 15 bytes, got {}",
            data.len()
        )));
    }

    let mut buf = data;

    // Standard fields
    let co2 = buf.get_u16_le();
    let temp_raw = buf.get_i16_le();
    let pressure_raw = buf.get_u16_le();
    let humidity = buf.get_u8();
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();

    // Radon-specific field (store as u32 for consistency)
    let radon = buf.get_u16_le() as u32;

    let reading = CurrentReading {
        co2,
        temperature: temp_raw as f32 / 20.0,
        pressure: pressure_raw as f32 / 10.0,
        humidity,
        battery,
        status,
        interval,
        age,
        captured_at: None,
        radon: Some(radon),
        radiation_rate: None,
        radiation_total: None,
        radon_avg_24h: None,
        radon_avg_7d: None,
        radon_avg_30d: None,
    };

    Ok(ExtendedReading {
        reading,
        radiation_duration: None,
    })
}

/// Parse Aranet Radon readings from GATT characteristic (f0cd3003 or f0cd1504).
///
/// Delegates to [`CurrentReading::from_bytes_radon`].
pub fn parse_aranet_radon_gatt(data: &[u8]) -> Result<CurrentReading> {
    CurrentReading::from_bytes_radon(data).map_err(from_parse_error)
}

/// Parse Aranet Radiation readings from GATT characteristic.
///
/// Delegates to [`CurrentReading::from_bytes_radiation`] for the core reading,
/// then extracts the measurement duration from bytes 19-26 (which `CurrentReading`
/// does not store).
pub fn parse_aranet_radiation_gatt(data: &[u8]) -> Result<ExtendedReading> {
    let reading = CurrentReading::from_bytes_radiation(data).map_err(from_parse_error)?;

    // Extract radiation duration from bytes 19-26 (u64 LE, seconds).
    // from_bytes_radiation already validated length >= 28.
    let duration = (&data[19..27]).get_u64_le();

    Ok(ExtendedReading {
        reading,
        radiation_duration: Some(duration),
    })
}

/// Parse a reading based on device type (GATT format).
///
/// Delegates to [`CurrentReading::from_bytes_for_device`].
pub fn parse_reading_for_device(data: &[u8], device_type: DeviceType) -> Result<CurrentReading> {
    CurrentReading::from_bytes_for_device(data, device_type).map_err(from_parse_error)
}

/// Parse an extended reading based on device type (GATT format).
pub fn parse_extended_reading(data: &[u8], device_type: DeviceType) -> Result<ExtendedReading> {
    match device_type {
        DeviceType::AranetRadiation => parse_aranet_radiation_gatt(data),
        _ => {
            let reading = parse_reading_for_device(data, device_type)?;
            Ok(ExtendedReading {
                reading,
                radiation_duration: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Aranet2 GATT parsing tests ---

    #[test]
    fn test_parse_aranet2_reading() {
        // GATT format: header, interval, age, battery, temp, humidity, status_flags
        // Temperature: 450 raw (22.5°C)
        // Humidity: 550 raw (55.0%)
        // Battery: 90
        // Status flags: 0x04 = bits[2:3]=01 = Green (temperature status)
        // Interval: 300 (5 min)
        // Age: 120 (2 min)
        let data: [u8; 12] = [
            0x02, 0x00, // header (device type marker)
            0x2C, 0x01, // interval = 300
            0x78, 0x00, // age = 120
            90,   // battery
            0xC2, 0x01, // temp = 450 (22.5°C)
            0x26, 0x02, // humidity = 550 (55.0%)
            0x04, // status flags: bits[2:3] = 01 = Green
        ];

        let reading = parse_aranet2_reading(&data).unwrap();
        assert_eq!(reading.co2, 0);
        assert!((reading.temperature - 22.5).abs() < 0.01);
        assert_eq!(reading.humidity, 55);
        assert_eq!(reading.battery, 90);
        assert_eq!(reading.status, Status::Green);
        assert_eq!(reading.interval, 300);
        assert_eq!(reading.age, 120);
    }

    #[test]
    fn test_parse_aranet2_reading_all_status_values() {
        // Status flags: bits[2:3] = temperature status
        // 0b0000_00XX where XX is in bits[2:3]
        for (status_flags, expected_status) in [
            (0x00, Status::Error),  // bits[2:3] = 00
            (0x04, Status::Green),  // bits[2:3] = 01
            (0x08, Status::Yellow), // bits[2:3] = 10
            (0x0C, Status::Red),    // bits[2:3] = 11
        ] {
            let data: [u8; 12] = [
                0x02,
                0x00, // header
                0x2C,
                0x01, // interval = 300
                0x78,
                0x00, // age = 120
                90,   // battery
                0xC2,
                0x01, // temp = 450
                0x26,
                0x02, // humidity = 550
                status_flags,
            ];

            let reading = parse_aranet2_reading(&data).unwrap();
            assert_eq!(reading.status, expected_status);
        }
    }

    #[test]
    fn test_parse_aranet2_reading_insufficient_bytes() {
        let data: [u8; 8] = [0x02, 0x00, 0x2C, 0x01, 0x78, 0x00, 90, 0xC2];

        let result = parse_aranet2_reading(&data);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(err.to_string().contains("expected 12"));
        assert!(err.to_string().contains("got 8"));
    }

    #[test]
    fn test_parse_aranet2_reading_edge_values() {
        // Test with all-zero values
        let data: [u8; 12] = [0; 12];

        let reading = parse_aranet2_reading(&data).unwrap();
        assert_eq!(reading.co2, 0);
        assert!((reading.temperature - 0.0).abs() < 0.01);
        assert_eq!(reading.humidity, 0);
        assert_eq!(reading.battery, 0);
        assert_eq!(reading.status, Status::Error);
        assert_eq!(reading.interval, 0);
        assert_eq!(reading.age, 0);
    }

    #[test]
    fn test_parse_aranet2_reading_max_values() {
        let data: [u8; 12] = [
            0xFF, 0xFF, // header
            0xFF, 0xFF, // interval = 65535
            0xFF, 0xFF, // age = 65535
            100,  // battery = 100
            0xFF, 0xFF, // temp = -1 as i16 (-0.05°C with signed parsing)
            0xFF, 0xFF, // humidity = 65535 (6553 / 10 = 6553 → 6553 as u8 wraps)
            0x0C, // status flags: bits[2:3] = 11 = Red
        ];

        let reading = parse_aranet2_reading(&data).unwrap();
        assert!((reading.temperature - (-0.05)).abs() < 0.01); // -1 as i16 / 20
        assert_eq!(reading.battery, 100);
        assert_eq!(reading.status, Status::Red);
        assert_eq!(reading.interval, 65535);
        assert_eq!(reading.age, 65535);
    }

    // --- Aranet4 parsing tests ---

    #[test]
    fn test_parse_aranet4_reading() {
        // Full 13-byte Aranet4 reading
        let data: [u8; 13] = [
            0x20, 0x03, // CO2 = 800
            0xC2, 0x01, // temp_raw = 450 (22.5°C)
            0x94, 0x27, // pressure_raw = 10132 (1013.2 hPa)
            45,   // humidity
            85,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
            0x78, 0x00, // age = 120
        ];

        let reading = parse_aranet4_reading(&data).unwrap();
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
    fn test_parse_aranet4_reading_high_co2() {
        // High CO2 reading - red status
        let data: [u8; 13] = [
            0xD0, 0x07, // CO2 = 2000 ppm
            0x90, 0x01, // temp_raw = 400 (20.0°C)
            0x88, 0x27, // pressure_raw = 10120 (1012.0 hPa)
            60,   // humidity
            75,   // battery
            3,    // status = Red
            0x3C, 0x00, // interval = 60
            0x1E, 0x00, // age = 30
        ];

        let reading = parse_aranet4_reading(&data).unwrap();
        assert_eq!(reading.co2, 2000);
        assert_eq!(reading.status, Status::Red);
    }

    #[test]
    fn test_parse_aranet4_reading_insufficient_bytes() {
        let data: [u8; 10] = [0; 10];

        let result = parse_aranet4_reading(&data);
        assert!(result.is_err());

        let err = result.unwrap_err();
        // The error message format changed: "Insufficient bytes: expected 13, got 10"
        assert!(err.to_string().contains("expected 13"));
        assert!(err.to_string().contains("got 10"));
    }

    // --- Aranet Radon parsing tests ---

    #[test]
    fn test_parse_aranet_radon_reading() {
        // 15-byte extended reading format
        let data: [u8; 15] = [
            0x00, 0x00, // CO2 = 0 (not applicable for radon)
            0xC2, 0x01, // temp_raw = 450 (22.5°C)
            0x94, 0x27, // pressure_raw = 10132 (1013.2 hPa)
            50,   // humidity
            80,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
            0x3C, 0x00, // age = 60
            0x64, 0x00, // radon = 100 Bq/m³
        ];

        let result = parse_aranet_radon_reading(&data).unwrap();
        assert_eq!(result.reading.radon, Some(100));
        assert!(result.reading.radiation_rate.is_none());
        assert!((result.reading.temperature - 22.5).abs() < 0.01);
        assert_eq!(result.reading.humidity, 50);
    }

    #[test]
    fn test_parse_aranet_radon_reading_high_radon() {
        let mut data: [u8; 15] = [0; 15];
        // Set radon to high value: 500 Bq/m³
        data[13] = 0xF4;
        data[14] = 0x01; // 500 in LE

        let result = parse_aranet_radon_reading(&data).unwrap();
        assert_eq!(result.reading.radon, Some(500));
    }

    #[test]
    fn test_parse_aranet_radon_reading_insufficient_bytes() {
        let data: [u8; 12] = [0; 12];

        let result = parse_aranet_radon_reading(&data);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("requires 15 bytes")
        );
    }

    // --- Aranet Radon GATT parsing tests ---

    #[test]
    fn test_parse_aranet_radon_gatt() {
        // GATT format: device_type(2) + interval(2) + age(2) + battery(1) + temp(2) + pressure(2) + humidity(2) + radon(4) + status(1)
        let mut data: [u8; 18] = [0; 18];
        // Bytes 0-1: device type (0x0003 for radon)
        data[0] = 0x03;
        data[1] = 0x00;
        // Bytes 2-3: interval = 600 seconds
        data[2] = 0x58;
        data[3] = 0x02;
        // Bytes 4-5: age = 120 seconds
        data[4] = 0x78;
        data[5] = 0x00;
        // Byte 6: battery = 85%
        data[6] = 85;
        // Bytes 7-8: temp = 450 (22.5°C)
        data[7] = 0xC2;
        data[8] = 0x01;
        // Bytes 9-10: pressure = 10132 (1013.2 hPa)
        data[9] = 0x94;
        data[10] = 0x27;
        // Bytes 11-12: humidity_raw = 450 (45.0%)
        data[11] = 0xC2;
        data[12] = 0x01;
        // Bytes 13-16: radon = 100 Bq/m³
        data[13] = 0x64;
        data[14] = 0x00;
        data[15] = 0x00;
        data[16] = 0x00;
        // Byte 17: status = Green
        data[17] = 1;

        let reading = parse_aranet_radon_gatt(&data).unwrap();
        assert_eq!(reading.battery, 85);
        assert!((reading.temperature - 22.5).abs() < 0.01);
        assert_eq!(reading.radon, Some(100)); // Radon stored in dedicated field
        assert_eq!(reading.co2, 0); // CO2 is 0 for radon devices
        assert_eq!(reading.status, Status::Green);
        assert_eq!(reading.interval, 600);
        assert_eq!(reading.age, 120);
    }

    #[test]
    fn test_parse_aranet_radon_gatt_insufficient_bytes() {
        let data: [u8; 15] = [0; 15];

        let result = parse_aranet_radon_gatt(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("expected 18"));
    }

    #[test]
    fn test_parse_aranet_radon_gatt_high_radon() {
        // Test that high radon values are stored correctly in the u32 field
        let mut data: [u8; 18] = [0; 18];
        // Bytes 0-5: header (device type, interval, age)
        data[0] = 0x03; // device type = radon
        // Bytes 13-16: Radon = 100000
        data[13] = 0xA0;
        data[14] = 0x86;
        data[15] = 0x01;
        data[16] = 0x00; // 100000 in LE u32

        let reading = parse_aranet_radon_gatt(&data).unwrap();
        assert_eq!(reading.radon, Some(100000)); // Full u32 value preserved
    }

    // --- parse_reading_for_device tests ---

    #[test]
    fn test_parse_reading_for_device_aranet4() {
        let data: [u8; 13] = [
            0x20, 0x03, // CO2 = 800
            0xC2, 0x01, // temp
            0x94, 0x27, // pressure
            45, 85, 1, // humidity, battery, status
            0x2C, 0x01, // interval
            0x78, 0x00, // age
        ];

        let reading = parse_reading_for_device(&data, DeviceType::Aranet4).unwrap();
        assert_eq!(reading.co2, 800);
    }

    #[test]
    fn test_parse_reading_for_device_aranet2() {
        let data: [u8; 12] = [
            0x02, 0x00, // header
            0x2C, 0x01, // interval = 300
            0x78, 0x00, // age = 120
            90,   // battery
            0xC2, 0x01, // temp = 450 (22.5°C)
            0x26, 0x02, // humidity = 550 (55.0%)
            0x04, // status flags
        ];

        let reading = parse_reading_for_device(&data, DeviceType::Aranet2).unwrap();
        assert_eq!(reading.co2, 0); // Aranet2 doesn't have CO2
        assert!((reading.temperature - 22.5).abs() < 0.01);
    }

    // --- ExtendedReading tests ---

    #[test]
    fn test_extended_reading_with_radon() {
        let reading = CurrentReading {
            co2: 0,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 50,
            battery: 80,
            status: Status::Green,
            interval: 300,
            age: 60,
            captured_at: None,
            radon: Some(150),
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        };

        let extended = ExtendedReading {
            reading,
            radiation_duration: None,
        };

        assert_eq!(extended.reading.radon, Some(150));
        assert!(extended.reading.radiation_rate.is_none());
        assert!((extended.reading.temperature - 22.5).abs() < 0.01);
    }

    #[test]
    fn test_extended_reading_with_radiation() {
        let reading = CurrentReading {
            co2: 0,
            temperature: 20.0,
            pressure: 1000.0,
            humidity: 45,
            battery: 90,
            status: Status::Green,
            interval: 60,
            age: 30,
            captured_at: None,
            radon: None,
            radiation_rate: Some(0.15),
            radiation_total: Some(0.001),
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        };

        let extended = ExtendedReading {
            reading,
            radiation_duration: Some(3600),
        };

        assert!(extended.reading.radon.is_none());
        assert!((extended.reading.radiation_rate.unwrap() - 0.15).abs() < 0.001);
        assert_eq!(extended.radiation_duration, Some(3600));
    }

    #[test]
    fn test_extended_reading_debug() {
        let reading = CurrentReading {
            co2: 800,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 50,
            battery: 80,
            status: Status::Green,
            interval: 300,
            age: 60,
            captured_at: None,
            radon: Some(100),
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        };

        let extended = ExtendedReading {
            reading,
            radiation_duration: None,
        };

        let debug_str = format!("{:?}", extended);
        assert!(debug_str.contains("radon"));
        assert!(debug_str.contains("100"));
    }

    #[test]
    fn test_extended_reading_clone() {
        let reading = CurrentReading {
            co2: 800,
            temperature: 22.5,
            pressure: 1013.2,
            humidity: 50,
            battery: 80,
            status: Status::Green,
            interval: 300,
            age: 60,
            captured_at: None,
            radon: Some(100),
            radiation_rate: Some(0.1),
            radiation_total: Some(0.001),
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        };

        let extended = ExtendedReading {
            reading,
            radiation_duration: Some(3600),
        };

        let cloned = extended.clone();
        assert_eq!(cloned.reading.radon, extended.reading.radon);
        assert_eq!(
            cloned.reading.radiation_rate,
            extended.reading.radiation_rate
        );
        assert_eq!(cloned.reading.co2, extended.reading.co2);
        assert_eq!(cloned.radiation_duration, extended.radiation_duration);
    }

    #[test]
    fn test_parse_aranet_radiation_gatt() {
        // 28 bytes: 2 unknown + 2 interval + 2 age + 1 battery + 4 dose_rate + 8 total_dose + 8 duration + 1 status
        let data = [
            0x00, 0x00, // Unknown bytes
            0x3C, 0x00, // Interval = 60 seconds
            0x1E, 0x00, // Age = 30 seconds
            0x5A, // Battery = 90%
            0xE8, 0x03, 0x00, 0x00, // Dose rate = 1000 nSv/h = 1.0 µSv/h
            0x40, 0x42, 0x0F, 0x00, 0x00, 0x00, 0x00,
            0x00, // Total dose = 1,000,000 nSv = 1.0 mSv
            0x10, 0x0E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Duration = 3600 seconds
            0x01, // Status = Green
        ];

        let result = parse_aranet_radiation_gatt(&data).unwrap();
        assert_eq!(result.reading.interval, 60);
        assert_eq!(result.reading.age, 30);
        assert_eq!(result.reading.battery, 90);
        assert!((result.reading.radiation_rate.unwrap() - 1.0).abs() < 0.001);
        assert!((result.reading.radiation_total.unwrap() - 1.0).abs() < 0.001);
        assert_eq!(result.radiation_duration, Some(3600));
        assert_eq!(result.reading.status, Status::Green);
        assert!(result.reading.radon.is_none());
    }

    #[test]
    fn test_parse_aranet_radiation_gatt_insufficient_bytes() {
        let data = [0x00; 20]; // Only 20 bytes, need 28
        let result = parse_aranet_radiation_gatt(&data);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("expected 28"));
    }

    #[test]
    fn test_parse_aranet_radiation_gatt_high_values() {
        // Test with high radiation values
        let data = [
            0x00, 0x00, // Unknown bytes
            0x2C, 0x01, // Interval = 300 seconds
            0x0A, 0x00, // Age = 10 seconds
            0x64, // Battery = 100%
            0x10, 0x27, 0x00, 0x00, // Dose rate = 10,000 nSv/h = 10.0 µSv/h
            0x00, 0xE1, 0xF5, 0x05, 0x00, 0x00, 0x00,
            0x00, // Total dose = 100,000,000 nSv = 100.0 mSv
            0x80, 0x51, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, // Duration = 86400 seconds (1 day)
            0x02, // Status = Yellow
        ];

        let result = parse_aranet_radiation_gatt(&data).unwrap();
        assert_eq!(result.reading.interval, 300);
        assert!((result.reading.radiation_rate.unwrap() - 10.0).abs() < 0.001);
        assert!((result.reading.radiation_total.unwrap() - 100.0).abs() < 0.001);
        assert_eq!(result.radiation_duration, Some(86400));
        assert_eq!(result.reading.status, Status::Yellow);
    }
}

/// Property-based tests for BLE reading parsers.
///
/// These tests verify that all parsing functions are safe to call with any input,
/// ensuring they never panic regardless of the byte sequence provided.
///
/// # Test Categories
///
/// ## Panic Safety Tests
/// Each device type parser is tested with random byte sequences:
/// - `parse_aranet4_never_panics`: Aranet4 CO2 sensor format
/// - `parse_aranet2_never_panics`: Aranet2 temperature/humidity format
/// - `parse_aranet_radon_never_panics`: Aranet Radon sensor format
/// - `parse_aranet_radon_gatt_never_panics`: Aranet Radon GATT format
/// - `parse_aranet_radiation_gatt_never_panics`: Aranet Radiation format
/// - `parse_reading_for_device_never_panics`: Generic dispatcher
///
/// ## Valid Input Tests
/// - `aranet4_valid_bytes_parse_correctly`: Structured Aranet4 data
/// - `aranet2_valid_bytes_parse_correctly`: Structured Aranet2 data
///
/// # Running Tests
///
/// ```bash
/// cargo test -p aranet-core proptests
/// ```
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Parsing random bytes should never panic for any device type.
        #[test]
        fn parse_aranet4_never_panics(data: Vec<u8>) {
            let _ = parse_aranet4_reading(&data);
        }

        #[test]
        fn parse_aranet2_never_panics(data: Vec<u8>) {
            let _ = parse_aranet2_reading(&data);
        }

        #[test]
        fn parse_aranet_radon_never_panics(data: Vec<u8>) {
            let _ = parse_aranet_radon_reading(&data);
        }

        #[test]
        fn parse_aranet_radon_gatt_never_panics(data: Vec<u8>) {
            let _ = parse_aranet_radon_gatt(&data);
        }

        #[test]
        fn parse_aranet_radiation_gatt_never_panics(data: Vec<u8>) {
            let _ = parse_aranet_radiation_gatt(&data);
        }

        /// parse_reading_for_device should never panic regardless of input.
        #[test]
        fn parse_reading_for_device_never_panics(
            data: Vec<u8>,
            device_type_byte in 0xF1u8..=0xF4u8,
        ) {
            if let Ok(device_type) = DeviceType::try_from(device_type_byte) {
                let _ = parse_reading_for_device(&data, device_type);
            }
        }

        /// Valid Aranet4 readings should round-trip correctly.
        #[test]
        fn aranet4_valid_bytes_parse_correctly(
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

            let result = parse_aranet4_reading(&data);
            prop_assert!(result.is_ok());

            let reading = result.unwrap();
            prop_assert_eq!(reading.co2, co2);
            prop_assert_eq!(reading.humidity, humidity);
            prop_assert_eq!(reading.battery, battery);
            prop_assert_eq!(reading.interval, interval);
            prop_assert_eq!(reading.age, age);
        }

        /// Valid Aranet2 GATT readings should parse correctly.
        #[test]
        fn aranet2_valid_bytes_parse_correctly(
            temp_raw in 0u16..2000u16,
            humidity_raw in 0u16..1000u16,
            battery in 0u8..100u8,
            status_flags in 0u8..16u8,
            interval in 60u16..3600u16,
            age in 0u16..3600u16,
        ) {
            let mut data = [0u8; 12];
            data[0..2].copy_from_slice(&0x0002u16.to_le_bytes()); // header
            data[2..4].copy_from_slice(&interval.to_le_bytes());
            data[4..6].copy_from_slice(&age.to_le_bytes());
            data[6] = battery;
            data[7..9].copy_from_slice(&temp_raw.to_le_bytes());
            data[9..11].copy_from_slice(&humidity_raw.to_le_bytes());
            data[11] = status_flags;

            let result = parse_aranet2_reading(&data);
            prop_assert!(result.is_ok());

            let reading = result.unwrap();
            prop_assert_eq!(reading.co2, 0); // Aranet2 has no CO2
            prop_assert_eq!(reading.humidity, (humidity_raw / 10) as u8);
            prop_assert_eq!(reading.battery, battery);
            prop_assert_eq!(reading.interval, interval);
            prop_assert_eq!(reading.age, age);
        }
    }
}
