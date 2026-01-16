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

/// Parse Aranet2 current readings (temperature and humidity only).
///
/// Format (7 bytes):
/// - bytes 0-1: Temperature (u16 LE, /20 for °C)
/// - byte 2: Humidity (u8)
/// - byte 3: Battery (u8)
/// - byte 4: Status (u8)
/// - bytes 5-6: Interval (u16 LE, seconds)
pub fn parse_aranet2_reading(data: &[u8]) -> Result<CurrentReading> {
    if data.len() < 7 {
        return Err(Error::InvalidData(format!(
            "Aranet2 reading requires 7 bytes, got {}",
            data.len()
        )));
    }

    let mut buf = data;
    let temp_raw = buf.get_u16_le();
    let humidity = buf.get_u8();
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();

    Ok(CurrentReading {
        co2: 0, // Aranet2 doesn't have CO2
        temperature: temp_raw as f32 / 20.0,
        pressure: 0.0, // Aranet2 doesn't have pressure
        humidity,
        battery,
        status,
        interval,
        age: 0,
        radon: None,
        radiation_rate: None,
        radiation_total: None,
    })
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
    let temp_raw = buf.get_u16_le();
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
        radon: Some(radon),
        radiation_rate: None,
        radiation_total: None,
    };

    Ok(ExtendedReading {
        reading,
        radiation_duration: None,
    })
}

/// Parse Aranet Radon readings from GATT characteristic (f0cd3003 or f0cd1504).
///
/// Format (47 bytes):
/// - Bytes 0-1: Device type marker (0x0003 for radon)
/// - Bytes 2-3: Interval (LE16, seconds)
/// - Bytes 4-5: Seconds since update (LE16)
/// - Byte 6: Battery (0-100%)
/// - Bytes 7-8: Temperature (LE16, raw / 20 = °C)
/// - Bytes 9-10: Pressure (LE16, raw / 10 = hPa)
/// - Bytes 11-12: Humidity (LE16, raw / 10 = %)
/// - Bytes 13-16: Radon concentration (LE32, Bq/m³)
/// - Byte 17: Status/color
/// - Remaining: Averages (24h, 7d, 30d)
pub fn parse_aranet_radon_gatt(data: &[u8]) -> Result<CurrentReading> {
    if data.len() < 18 {
        return Err(Error::InvalidData(format!(
            "Aranet Radon GATT reading requires at least 18 bytes, got {}",
            data.len()
        )));
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
        temperature: temp_raw as f32 / 20.0,
        pressure: pressure_raw as f32 / 10.0,
        humidity: (humidity_raw / 10).min(255) as u8, // Convert from 10ths to percent
        battery,
        status,
        interval,
        age,
        radon: Some(radon),
        radiation_rate: None,
        radiation_total: None,
    })
}

/// Parse Aranet Radiation readings from GATT characteristic.
///
/// Format (28 bytes):
/// - bytes 0-1: Unknown
/// - bytes 2-3: Interval (LE16, seconds)
/// - bytes 4-5: Age (LE16, seconds)
/// - byte 6: Battery
/// - bytes 7-10: Dose rate (LE32, nSv/h)
/// - bytes 11-18: Total dose (LE64, nSv)
/// - bytes 19-26: Duration (LE64, seconds)
/// - byte 27: Status
pub fn parse_aranet_radiation_gatt(data: &[u8]) -> Result<ExtendedReading> {
    if data.len() < 28 {
        return Err(Error::InvalidData(format!(
            "Aranet Radiation GATT reading requires at least 28 bytes, got {}",
            data.len()
        )));
    }

    let mut buf = data;

    // Skip 2 unknown bytes
    buf.advance(2);

    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();
    let battery = buf.get_u8();

    // Dose rate in nSv/h, convert to µSv/h
    let dose_rate_nsv = buf.get_u32_le();
    let dose_rate_usv = dose_rate_nsv as f32 / 1000.0;

    // Total dose in nSv, convert to mSv
    let total_dose_nsv = buf.get_u64_le();
    let total_dose_msv = total_dose_nsv as f64 / 1_000_000.0;

    // Duration in seconds
    let duration = buf.get_u64_le();

    let status = if buf.has_remaining() {
        Status::from(buf.get_u8())
    } else {
        Status::Green
    };

    let reading = CurrentReading {
        co2: 0,
        temperature: 0.0,
        pressure: 0.0,
        humidity: 0,
        battery,
        status,
        interval,
        age,
        radon: None,
        radiation_rate: Some(dose_rate_usv),
        radiation_total: Some(total_dose_msv),
    };

    Ok(ExtendedReading {
        reading,
        radiation_duration: Some(duration),
    })
}

/// Parse a reading based on device type.
pub fn parse_reading_for_device(data: &[u8], device_type: DeviceType) -> Result<CurrentReading> {
    match device_type {
        DeviceType::Aranet4 => parse_aranet4_reading(data),
        DeviceType::Aranet2 => parse_aranet2_reading(data),
        DeviceType::AranetRadon => parse_aranet_radon_reading(data).map(|ext| ext.reading),
        DeviceType::AranetRadiation => parse_aranet_radiation_gatt(data).map(|ext| ext.reading),
        // Handle future device types - default to Aranet4 parsing
        _ => parse_aranet4_reading(data),
    }
}

/// Parse an extended reading based on device type.
pub fn parse_extended_reading(data: &[u8], device_type: DeviceType) -> Result<ExtendedReading> {
    match device_type {
        DeviceType::Aranet4 => {
            let reading = parse_aranet4_reading(data)?;
            Ok(ExtendedReading {
                reading,
                radiation_duration: None,
            })
        }
        DeviceType::Aranet2 => {
            let reading = parse_aranet2_reading(data)?;
            Ok(ExtendedReading {
                reading,
                radiation_duration: None,
            })
        }
        DeviceType::AranetRadon => parse_aranet_radon_reading(data),
        DeviceType::AranetRadiation => parse_aranet_radiation_gatt(data),
        // Handle future device types - default to Aranet4 parsing
        _ => {
            let reading = parse_aranet4_reading(data)?;
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

    // --- Aranet2 parsing tests ---

    #[test]
    fn test_parse_aranet2_reading() {
        // Temperature: 450 raw (22.5°C)
        // Humidity: 55
        // Battery: 90
        // Status: Green (1)
        // Interval: 300 (5 min)
        let data: [u8; 7] = [
            0xC2, 0x01, // temp = 450
            55,   // humidity
            90,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
        ];

        let reading = parse_aranet2_reading(&data).unwrap();
        assert_eq!(reading.co2, 0);
        assert!((reading.temperature - 22.5).abs() < 0.01);
        assert_eq!(reading.humidity, 55);
        assert_eq!(reading.battery, 90);
        assert_eq!(reading.status, Status::Green);
        assert_eq!(reading.interval, 300);
    }

    #[test]
    fn test_parse_aranet2_reading_all_status_values() {
        // Test different status values
        for (status_byte, expected_status) in [
            (0, Status::Error),
            (1, Status::Green),
            (2, Status::Yellow),
            (3, Status::Red),
            (4, Status::Error), // Unknown maps to Error
        ] {
            let data: [u8; 7] = [
                0xC2,
                0x01, // temp = 450
                55,
                90,
                status_byte,
                0x2C,
                0x01,
            ];

            let reading = parse_aranet2_reading(&data).unwrap();
            assert_eq!(reading.status, expected_status);
        }
    }

    #[test]
    fn test_parse_aranet2_reading_insufficient_bytes() {
        let data: [u8; 5] = [0xC2, 0x01, 55, 90, 1]; // Only 5 bytes, need 7

        let result = parse_aranet2_reading(&data);
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(err.to_string().contains("requires 7 bytes"));
        assert!(err.to_string().contains("got 5"));
    }

    #[test]
    fn test_parse_aranet2_reading_edge_values() {
        // Test with edge case values
        let data: [u8; 7] = [
            0x00, 0x00, // temp = 0 (0°C)
            0,    // humidity = 0
            0,    // battery = 0
            0,    // status = Error
            0x00, 0x00, // interval = 0
        ];

        let reading = parse_aranet2_reading(&data).unwrap();
        assert_eq!(reading.co2, 0);
        assert!((reading.temperature - 0.0).abs() < 0.01);
        assert_eq!(reading.humidity, 0);
        assert_eq!(reading.battery, 0);
        assert_eq!(reading.status, Status::Error);
        assert_eq!(reading.interval, 0);
    }

    #[test]
    fn test_parse_aranet2_reading_max_values() {
        let data: [u8; 7] = [
            0xFF, 0xFF, // temp = 65535
            255,  // humidity = 255 (invalid but possible)
            100,  // battery = 100
            3,    // status = Red
            0xFF, 0xFF, // interval = 65535
        ];

        let reading = parse_aranet2_reading(&data).unwrap();
        assert!((reading.temperature - 3276.75).abs() < 0.01); // 65535/20
        assert_eq!(reading.humidity, 255);
        assert_eq!(reading.battery, 100);
        assert_eq!(reading.status, Status::Red);
        assert_eq!(reading.interval, 65535);
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
        assert!(err.to_string().contains("requires 13 bytes"));
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
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("at least 18 bytes")
        );
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
        let data: [u8; 7] = [0xC2, 0x01, 55, 90, 1, 0x2C, 0x01];

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
            radon: Some(150),
            radiation_rate: None,
            radiation_total: None,
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
            radon: None,
            radiation_rate: Some(0.15),
            radiation_total: Some(0.001),
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
            radon: Some(100),
            radiation_rate: None,
            radiation_total: None,
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
            radon: Some(100),
            radiation_rate: Some(0.1),
            radiation_total: Some(0.001),
        };

        let extended = ExtendedReading {
            reading,
            radiation_duration: Some(3600),
        };

        let cloned = extended.clone();
        assert_eq!(cloned.reading.radon, extended.reading.radon);
        assert_eq!(cloned.reading.radiation_rate, extended.reading.radiation_rate);
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
            0x40, 0x42, 0x0F, 0x00, 0x00, 0x00, 0x00, 0x00, // Total dose = 1,000,000 nSv = 1.0 mSv
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
        assert!(err.to_string().contains("28 bytes"));
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
            0x00, 0xE1, 0xF5, 0x05, 0x00, 0x00, 0x00, 0x00, // Total dose = 100,000,000 nSv = 100.0 mSv
            0x80, 0x51, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, // Duration = 86400 seconds (1 day)
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
