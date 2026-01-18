//! BLE advertisement data parsing for passive monitoring.
//!
//! This module provides functionality to parse sensor data directly from
//! Bluetooth advertisements without requiring a connection. This enables
//! monitoring multiple devices simultaneously with lower power consumption.
//!
//! # Requirements
//!
//! For advertisement data to be available, Smart Home integration must be
//! enabled on the Aranet device (see [`Device::set_smart_home`](crate::device::Device::set_smart_home)).

use bytes::Buf;
use serde::{Deserialize, Serialize};

use aranet_types::{DeviceType, Status};

use crate::error::{Error, Result};

/// Parsed sensor data from a BLE advertisement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvertisementData {
    /// Device type detected from advertisement.
    pub device_type: DeviceType,
    /// CO2 concentration in ppm (Aranet4 only).
    pub co2: Option<u16>,
    /// Temperature in degrees Celsius.
    pub temperature: Option<f32>,
    /// Atmospheric pressure in hPa.
    pub pressure: Option<f32>,
    /// Relative humidity percentage (0-100).
    pub humidity: Option<u8>,
    /// Battery level percentage (0-100).
    pub battery: u8,
    /// CO2 status indicator.
    pub status: Status,
    /// Measurement interval in seconds.
    pub interval: u16,
    /// Age of reading in seconds since last measurement.
    pub age: u16,
    /// Radon concentration in Bq/m³ (Aranet Radon only).
    pub radon: Option<u32>,
    /// Radiation dose rate in µSv/h (Aranet Radiation only).
    pub radiation_dose_rate: Option<f32>,
    /// Advertisement counter (increments with each new reading).
    pub counter: Option<u8>,
    /// Raw manufacturer data flags.
    pub flags: u8,
}

/// Parse advertisement data from raw manufacturer data bytes.
///
/// The manufacturer data should be from manufacturer ID 0x0702 (SAF Tehnika).
///
/// # Arguments
///
/// * `data` - Raw manufacturer data bytes (excluding the manufacturer ID)
///
/// # Returns
///
/// Parsed advertisement data or an error if the data is invalid.
pub fn parse_advertisement(data: &[u8]) -> Result<AdvertisementData> {
    parse_advertisement_with_name(data, None)
}

/// Parse advertisement data with optional device name for better detection.
///
/// The device name helps distinguish Aranet4 from other device types since
/// Aranet4 advertisements don't include a device type prefix byte.
pub fn parse_advertisement_with_name(data: &[u8], name: Option<&str>) -> Result<AdvertisementData> {
    if data.is_empty() {
        return Err(Error::InvalidData(
            "Advertisement data is empty".to_string(),
        ));
    }

    // Aranet advertisement format detection:
    // - Aranet4: NO device type byte prefix, detect by name or length (7 or 22 bytes)
    // - Aranet2: First byte = 0x01
    // - Aranet Radiation: First byte = 0x02
    // - Aranet Radon: First byte = 0x03
    //
    // The data structure is:
    // - Bytes 0-3: Basic info (flags, version)
    // - Bit 5 of flags (byte 0): Smart Home integrations enabled
    // - Remaining bytes: Sensor measurements (if integrations enabled)

    let is_aranet4_by_name = name.map(|n| n.starts_with("Aranet4")).unwrap_or(false);
    let is_aranet4_by_len = data.len() == 7 || data.len() == 22;

    let (device_type, sensor_data) = if is_aranet4_by_name || is_aranet4_by_len {
        // Aranet4: prepend virtual 0x00 device type byte
        (DeviceType::Aranet4, data)
    } else {
        // Other devices have the device type as first byte
        let device_type = match data[0] {
            0x01 => DeviceType::Aranet2,
            0x02 => DeviceType::AranetRadiation,
            0x03 => DeviceType::AranetRadon,
            other => {
                return Err(Error::InvalidData(format!(
                    "Unknown device type byte: 0x{:02X}. Expected 0x01 (Aranet2), \
                     0x02 (Radiation), or 0x03 (Radon). Data length: {} bytes.",
                    other,
                    data.len()
                )));
            }
        };
        (device_type, &data[1..])
    };

    // Check if Smart Home integrations are enabled (bit 5 of flags byte)
    if sensor_data.is_empty() {
        return Err(Error::InvalidData(
            "Advertisement data too short for basic info".to_string(),
        ));
    }

    let flags = sensor_data[0];
    let integrations_enabled = (flags & (1 << 5)) != 0;

    if !integrations_enabled {
        return Err(Error::InvalidData(
            "Smart Home integration is not enabled on this device. \
             To enable: go to device Settings > Smart Home > Enable."
                .to_string(),
        ));
    }

    match device_type {
        DeviceType::Aranet4 => parse_aranet4_advertisement_v2(sensor_data),
        DeviceType::Aranet2 => parse_aranet2_advertisement_v2(sensor_data),
        DeviceType::AranetRadon => parse_aranet_radon_advertisement_v2(sensor_data),
        DeviceType::AranetRadiation => parse_aranet_radiation_advertisement_v2(sensor_data),
        _ => Err(Error::InvalidData(format!(
            "Unsupported device type for advertisement parsing: {:?}",
            device_type
        ))),
    }
}

/// Parse Aranet4 advertisement data (v2 format - actual device format).
///
/// Format (22 bytes, no device type prefix):
/// - bytes 0-7: Basic info (flags, version, etc.)
/// - bytes 8-9: CO2 (u16 LE)
/// - bytes 10-11: Temperature (u16 LE, *0.05 for °C)
/// - bytes 12-13: Pressure (u16 LE, *0.1 for hPa)
/// - byte 14: Humidity (u8)
/// - byte 15: Battery (u8)
/// - byte 16: Status (u8)
/// - bytes 17-18: Interval (u16 LE, seconds)
/// - bytes 19-20: Age (u16 LE, seconds)
/// - byte 21: Counter (u8)
fn parse_aranet4_advertisement_v2(data: &[u8]) -> Result<AdvertisementData> {
    // Minimum 22 bytes for full Aranet4 advertisement
    if data.len() < 22 {
        return Err(Error::InvalidData(format!(
            "Aranet4 advertisement requires 22 bytes, got {}",
            data.len()
        )));
    }

    let flags = data[0];
    // Skip to sensor data at offset 8
    let mut buf = &data[8..];
    let co2 = buf.get_u16_le();
    let temp_raw = buf.get_u16_le();
    let pressure_raw = buf.get_u16_le();
    let humidity = buf.get_u8();
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();
    let counter = if !buf.is_empty() {
        Some(buf.get_u8())
    } else {
        None
    };

    Ok(AdvertisementData {
        device_type: DeviceType::Aranet4,
        co2: Some(co2),
        temperature: Some(temp_raw as f32 * 0.05),
        pressure: Some(pressure_raw as f32 * 0.1),
        humidity: Some(humidity),
        battery,
        status,
        interval,
        age,
        radon: None,
        radiation_dose_rate: None,
        counter,
        flags,
    })
}

/// Parse Aranet4 advertisement data (legacy format for tests).
#[allow(dead_code)]
fn parse_aranet4_advertisement(data: &[u8]) -> Result<AdvertisementData> {
    if data.len() < 16 {
        return Err(Error::InvalidData(format!(
            "Aranet4 advertisement requires 16 bytes, got {}",
            data.len()
        )));
    }

    let mut buf = &data[1..]; // Skip device type byte
    let flags = buf.get_u8();
    let co2 = buf.get_u16_le();
    let temp_raw = buf.get_u16_le();
    let pressure_raw = buf.get_u16_le();
    let humidity = buf.get_u8();
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();
    let counter = buf.get_u8();

    Ok(AdvertisementData {
        device_type: DeviceType::Aranet4,
        co2: Some(co2),
        temperature: Some(temp_raw as f32 / 20.0),
        pressure: Some(pressure_raw as f32 / 10.0),
        humidity: Some(humidity),
        battery,
        status,
        interval,
        age,
        radon: None,
        radiation_dose_rate: None,
        counter: Some(counter),
        flags,
    })
}

/// Parse Aranet2 advertisement data (v2 format - actual device format).
///
/// Format (after device type byte removed, 19+ bytes):
/// - bytes 0-7: Basic info (flags, version, etc.)
/// - bytes 8-9: Temperature (u16 LE, *0.05 for °C)
/// - bytes 10-11: unused
/// - bytes 12-13: Humidity (u16 LE, *0.1 for %)
/// - byte 14: Battery (u8)
/// - byte 15: Status (u8)
/// - bytes 16-17: Interval (u16 LE, seconds)
/// - bytes 18-19: Age (u16 LE, seconds)
/// - byte 20: Counter (u8)
fn parse_aranet2_advertisement_v2(data: &[u8]) -> Result<AdvertisementData> {
    if data.len() < 19 {
        return Err(Error::InvalidData(format!(
            "Aranet2 advertisement requires at least 19 bytes, got {}",
            data.len()
        )));
    }

    let flags = data[0];
    // Skip to sensor data at offset 7
    let mut buf = &data[7..];
    let temp_raw = buf.get_u16_le();
    let _unused = buf.get_u16_le();
    let humidity_raw = buf.get_u16_le();
    let battery = buf.get_u8();
    let status_raw = buf.get_u8();
    // Status for Aranet2 encodes both temp and humidity status
    let status = Status::from(status_raw & 0x03);
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();
    let counter = if !buf.is_empty() {
        Some(buf.get_u8())
    } else {
        None
    };

    Ok(AdvertisementData {
        device_type: DeviceType::Aranet2,
        co2: None,
        temperature: Some(temp_raw as f32 * 0.05),
        pressure: None,
        humidity: Some((humidity_raw as f32 * 0.1).min(255.0) as u8),
        battery,
        status,
        interval,
        age,
        radon: None,
        radiation_dose_rate: None,
        counter,
        flags,
    })
}

/// Parse Aranet Radon advertisement data (v2 format - actual device format).
///
/// Format (after device type byte removed, 23 bytes):
/// Based on Python: `<xxxxxxxHHHHBBBHHB` (7 skip bytes, not 8)
/// - bytes 0-6: Basic info (flags, version, etc.) - 7 bytes
/// - bytes 7-8: Radon concentration (u16 LE, Bq/m³)
/// - bytes 9-10: Temperature (u16 LE, *0.05 for °C)
/// - bytes 11-12: Pressure (u16 LE, *0.1 for hPa)
/// - bytes 13-14: Humidity (u16 LE, *0.1 for %)
/// - byte 15: Unknown/reserved (u8) - skipped in Python decode
/// - byte 16: Battery (u8)
/// - byte 17: Status (u8)
/// - bytes 18-19: Interval (u16 LE, seconds)
/// - bytes 20-21: Age (u16 LE, seconds)
/// - byte 22: Counter (u8)
fn parse_aranet_radon_advertisement_v2(data: &[u8]) -> Result<AdvertisementData> {
    if data.len() < 22 {
        return Err(Error::InvalidData(format!(
            "Aranet Radon advertisement requires at least 22 bytes, got {}",
            data.len()
        )));
    }

    let flags = data[0];
    // Skip to sensor data at offset 7 (7 bytes of basic info)
    let mut buf = &data[7..];
    let radon = buf.get_u16_le() as u32;
    let temp_raw = buf.get_u16_le();
    let pressure_raw = buf.get_u16_le();
    let humidity_raw = buf.get_u16_le();
    let _reserved = buf.get_u8(); // Unknown/reserved byte (skipped in Python)
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();
    let counter = if !buf.is_empty() {
        Some(buf.get_u8())
    } else {
        None
    };

    Ok(AdvertisementData {
        device_type: DeviceType::AranetRadon,
        co2: None,
        temperature: Some(temp_raw as f32 * 0.05),
        pressure: Some(pressure_raw as f32 * 0.1),
        humidity: Some((humidity_raw as f32 * 0.1).min(255.0) as u8),
        battery,
        status,
        interval,
        age,
        radon: Some(radon),
        radiation_dose_rate: None,
        counter,
        flags,
    })
}

/// Parse Aranet Radiation advertisement data (v2 format - actual device format).
///
/// Format (after device type byte removed, 19+ bytes):
/// - bytes 0-5: Basic info (flags, version, etc.)
/// - bytes 6-9: Radiation total (u32 LE, nSv)
/// - bytes 10-13: Radiation duration (u32 LE, seconds)
/// - bytes 14-15: Radiation rate (u16 LE, *10 for nSv/h)
/// - byte 16: Battery (u8)
/// - byte 17: Status (u8)
/// - bytes 18-19: Interval (u16 LE, seconds)
/// - bytes 20-21: Age (u16 LE, seconds)
/// - byte 22: Counter (u8)
fn parse_aranet_radiation_advertisement_v2(data: &[u8]) -> Result<AdvertisementData> {
    // Need at least 21 bytes: 5 header + 4 total + 4 duration + 2 rate + 1 battery + 1 status + 2 interval + 2 age
    if data.len() < 21 {
        return Err(Error::InvalidData(format!(
            "Aranet Radiation advertisement requires at least 21 bytes, got {}",
            data.len()
        )));
    }

    let flags = data[0];
    // Skip to sensor data at offset 5
    let mut buf = &data[5..];
    let _radiation_total = buf.get_u32_le(); // nSv total dose
    let _radiation_duration = buf.get_u32_le(); // seconds
    let radiation_rate_raw = buf.get_u16_le(); // *10 for nSv/h
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();
    let counter = if !buf.is_empty() {
        Some(buf.get_u8())
    } else {
        None
    };

    // Convert from nSv/h * 10 to µSv/h
    let dose_rate_usv = (radiation_rate_raw as f32 * 10.0) / 1000.0;

    Ok(AdvertisementData {
        device_type: DeviceType::AranetRadiation,
        co2: None,
        temperature: None,
        pressure: None,
        humidity: None,
        battery,
        status,
        interval,
        age,
        radon: None,
        radiation_dose_rate: Some(dose_rate_usv),
        counter,
        flags,
    })
}

/// Parse Aranet2 advertisement data (legacy format for tests).
#[allow(dead_code)]
fn parse_aranet2_advertisement(data: &[u8]) -> Result<AdvertisementData> {
    if data.len() < 12 {
        return Err(Error::InvalidData(format!(
            "Aranet2 advertisement requires at least 12 bytes, got {}",
            data.len()
        )));
    }

    let mut buf = &data[1..];
    let flags = buf.get_u8();
    let temp_raw = buf.get_u16_le();
    let humidity_raw = buf.get_u16_le();
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();

    Ok(AdvertisementData {
        device_type: DeviceType::Aranet2,
        co2: None,
        temperature: Some(temp_raw as f32 / 20.0),
        pressure: None,
        humidity: Some((humidity_raw / 10).min(255) as u8),
        battery,
        status,
        interval,
        age,
        radon: None,
        radiation_dose_rate: None,
        counter: None,
        flags,
    })
}

/// Parse Aranet Radon advertisement data (legacy format for tests).
#[allow(dead_code)]
fn parse_aranet_radon_advertisement(data: &[u8]) -> Result<AdvertisementData> {
    if data.len() < 18 {
        return Err(Error::InvalidData(format!(
            "Aranet Radon advertisement requires at least 18 bytes, got {}",
            data.len()
        )));
    }

    let mut buf = &data[1..];
    let flags = buf.get_u8();
    let temp_raw = buf.get_u16_le();
    let pressure_raw = buf.get_u16_le();
    let humidity_raw = buf.get_u16_le();
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();
    let radon = buf.get_u32_le();

    Ok(AdvertisementData {
        device_type: DeviceType::AranetRadon,
        co2: None,
        temperature: Some(temp_raw as f32 / 20.0),
        pressure: Some(pressure_raw as f32 / 10.0),
        humidity: Some((humidity_raw / 10).min(255) as u8),
        battery,
        status,
        interval,
        age,
        radon: Some(radon),
        radiation_dose_rate: None,
        counter: None,
        flags,
    })
}

/// Parse Aranet Radiation advertisement data (legacy format for tests).
#[allow(dead_code)]
fn parse_aranet_radiation_advertisement(data: &[u8]) -> Result<AdvertisementData> {
    if data.len() < 16 {
        return Err(Error::InvalidData(format!(
            "Aranet Radiation advertisement requires at least 16 bytes, got {}",
            data.len()
        )));
    }

    let mut buf = &data[1..];
    let flags = buf.get_u8();
    let battery = buf.get_u8();
    let status = Status::from(buf.get_u8());
    let interval = buf.get_u16_le();
    let age = buf.get_u16_le();
    // Dose rate is in nSv/h, convert to µSv/h
    let dose_rate_nsv = buf.get_u32_le();
    let dose_rate_usv = dose_rate_nsv as f32 / 1000.0;

    Ok(AdvertisementData {
        device_type: DeviceType::AranetRadiation,
        co2: None,
        temperature: None,
        pressure: None,
        humidity: None,
        battery,
        status,
        interval,
        age,
        radon: None,
        radiation_dose_rate: Some(dose_rate_usv),
        counter: None,
        flags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_aranet4_advertisement() {
        // Aranet4 v2 format: 22 bytes, no device type prefix
        // Flags byte has bit 5 set (0x20) for Smart Home integration
        let data: [u8; 22] = [
            0x22, // flags (bit 5 = integrations enabled)
            0x13, 0x04, 0x01, 0x00, 0x0E, 0x0F, 0x01, // basic info (7 bytes)
            0x20, 0x03, // CO2 = 800
            0xC2, 0x01, // temp_raw = 450 (450 * 0.05 = 22.5°C)
            0x94, 0x27, // pressure_raw = 10132 (10132 * 0.1 = 1013.2 hPa)
            45,   // humidity
            85,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
            0x78, 0x00, // age = 120
            5,    // counter
        ];

        let result = parse_advertisement(&data).unwrap();
        assert_eq!(result.device_type, DeviceType::Aranet4);
        assert_eq!(result.co2, Some(800));
        assert!((result.temperature.unwrap() - 22.5).abs() < 0.01);
        assert!((result.pressure.unwrap() - 1013.2).abs() < 0.1);
        assert_eq!(result.humidity, Some(45));
        assert_eq!(result.battery, 85);
        assert_eq!(result.status, Status::Green);
        assert_eq!(result.interval, 300);
        assert_eq!(result.age, 120);
    }

    #[test]
    fn test_parse_aranet2_advertisement() {
        // Aranet2 v2 format: device type 0x01, then 19+ bytes
        // Flags byte has bit 5 set (0x20) for Smart Home integration
        let data: [u8; 20] = [
            0x01, // device type = Aranet2
            0x20, // flags (bit 5 = integrations enabled)
            0x13, 0x04, 0x01, 0x00, 0x0E, 0x0F, // basic info (6 bytes)
            0xC2, 0x01, // temp_raw = 450 (450 * 0.05 = 22.5°C)
            0x00, 0x00, // unused
            0xC2, 0x01, // humidity_raw = 450 (450 * 0.1 = 45%)
            85,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
            0x3C, 0x00, // age = 60
        ];

        let result = parse_advertisement(&data).unwrap();
        assert_eq!(result.device_type, DeviceType::Aranet2);
        assert!(result.co2.is_none());
        assert!((result.temperature.unwrap() - 22.5).abs() < 0.01);
        assert_eq!(result.humidity, Some(45));
        assert_eq!(result.battery, 85);
    }

    #[test]
    fn test_parse_aranet_radon_advertisement() {
        // Aranet Radon v2 format: device type 0x03, then 23 bytes
        // Format: <xxxxxxxHHHHBBBHHB (7 skip, 4xH, 3xB, 2xH, 1xB)
        // Flags byte has bit 5 set (0x20) for Smart Home integration
        let data: [u8; 24] = [
            0x03, // device type = Aranet Radon
            0x21, // flags (bit 5 = integrations enabled)
            0x00, 0x0C, 0x01, 0x00, 0x00, 0x00, // basic info (6 bytes, total 7 with flags)
            0x51, 0x00, // radon = 81 Bq/m³
            0xC2, 0x01, // temp_raw = 450 (450 * 0.05 = 22.5°C)
            0x94, 0x27, // pressure_raw = 10132 (10132 * 0.1 = 1013.2 hPa)
            0xC2, 0x01, // humidity_raw = 450 (450 * 0.1 = 45%)
            0x00, // reserved byte (skipped in Python decode)
            85,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
            0x3C, 0x00, // age = 60
            5,    // counter
        ];

        let result = parse_advertisement(&data).unwrap();
        assert_eq!(result.device_type, DeviceType::AranetRadon);
        assert!(result.co2.is_none());
        assert!((result.temperature.unwrap() - 22.5).abs() < 0.01);
        assert!((result.pressure.unwrap() - 1013.2).abs() < 0.1);
        assert_eq!(result.humidity, Some(45));
        assert_eq!(result.radon, Some(81));
        assert_eq!(result.battery, 85);
        assert_eq!(result.status, Status::Green);
    }

    #[test]
    fn test_parse_empty_data() {
        let result = parse_advertisement(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_parse_unknown_device_type() {
        // Unknown device type byte (not 0x01, 0x02, or 0x03)
        // and not Aranet4 length (7 or 22 bytes)
        let data: [u8; 16] = [0xFF; 16];
        let result = parse_advertisement(&data);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unknown device type byte"),
            "Expected unknown device type error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_parse_aranet4_insufficient_bytes() {
        // Aranet4 is detected by length (7 or 22 bytes)
        // 10 bytes is not a valid Aranet4 length, so it will try to parse as other device
        // But 0x22 is not a valid device type, so it will fail
        let data: [u8; 10] = [0x22; 10];
        let result = parse_advertisement(&data);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Unknown device type byte"),
            "Expected unknown device type error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_parse_aranet_radiation_advertisement() {
        // Aranet Radiation v2 format: device type 0x02, then 19+ bytes
        // Flags byte has bit 5 set (0x20) for Smart Home integration
        // Note: Using 23 bytes to avoid triggering Aranet4 detection (which uses 7 or 22 bytes)
        let data: [u8; 23] = [
            0x02, // device type = Radiation
            0x20, // flags (bit 5 = integrations enabled)
            0x13, 0x04, 0x01, 0x00, // basic info (4 bytes)
            0x00, 0x00, 0x00, 0x00, // radiation total (u32)
            0x00, 0x00, 0x00, 0x00, // radiation duration (u32)
            0x64, 0x00, // radiation rate = 100 (*10 = 1000 nSv/h = 1.0 µSv/h)
            85,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
            0x3C, 0x00, // age = 60
            5,    // counter
        ];

        let result = parse_advertisement(&data).unwrap();
        assert_eq!(result.device_type, DeviceType::AranetRadiation);
        assert!(result.co2.is_none());
        assert!(result.temperature.is_none());
        assert!(result.radon.is_none());
        assert!((result.radiation_dose_rate.unwrap() - 1.0).abs() < 0.001);
        assert_eq!(result.battery, 85);
        assert_eq!(result.status, Status::Green);
        assert_eq!(result.interval, 300);
        assert_eq!(result.age, 60);
    }

    #[test]
    fn test_parse_aranet_radiation_insufficient_bytes() {
        // Device type 0x02 but not enough bytes
        let data: [u8; 10] = [0x02, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        let result = parse_advertisement(&data);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("requires at least 21 bytes"),
            "Expected insufficient bytes error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_parse_smart_home_not_enabled() {
        // Aranet4 format (22 bytes) but bit 5 not set in flags
        let data: [u8; 22] = [
            0x00, // flags (bit 5 NOT set - integrations disabled)
            0x13, 0x04, 0x01, 0x00, 0x0E, 0x0F, 0x01, // basic info
            0x20, 0x03, // CO2
            0xC2, 0x01, // temp
            0x94, 0x27, // pressure
            45, 85, 1, // humidity, battery, status
            0x2C, 0x01, // interval
            0x78, 0x00, // age
            5,    // counter
        ];

        let result = parse_advertisement(&data);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Smart Home integration is not enabled"),
            "Expected Smart Home error, got: {}",
            err_msg
        );
    }
}

/// Property-based tests for BLE advertisement parsing.
///
/// These tests verify that advertisement parsing is safe with any input,
/// including malformed or random data that might be received from BLE scans.
///
/// # Test Categories
///
/// ## Panic Safety Tests
/// - `parse_advertisement_never_panics`: Any random bytes
/// - `parse_aranet4_advertisement_never_panics`: 22-byte sequences
/// - `parse_aranet2_advertisement_never_panics`: Aranet2 device type
/// - `parse_aranet_radon_advertisement_never_panics`: Radon device type
/// - `parse_aranet_radiation_advertisement_never_panics`: Radiation device type
///
/// # Running Tests
///
/// ```bash
/// cargo test -p aranet-core advertisement::proptests
/// ```
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Parsing random advertisement bytes should never panic.
        /// It may return an error, but should always be safe.
        #[test]
        fn parse_advertisement_never_panics(data: Vec<u8>) {
            let _ = parse_advertisement(&data);
        }

        /// Parsing with valid Aranet4 length (22 bytes) should not panic.
        #[test]
        fn parse_aranet4_advertisement_never_panics(data in proptest::collection::vec(any::<u8>(), 22)) {
            let _ = parse_advertisement(&data);
        }

        /// Parsing with Aranet2 format (device type 0x01) should not panic.
        #[test]
        fn parse_aranet2_advertisement_never_panics(data in proptest::collection::vec(any::<u8>(), 19..=30)) {
            let mut modified = data.clone();
            if !modified.is_empty() {
                modified[0] = 0x01; // Set device type to Aranet2
            }
            let _ = parse_advertisement(&modified);
        }

        /// Parsing with Aranet Radon format should not panic.
        #[test]
        fn parse_aranet_radon_advertisement_never_panics(data in proptest::collection::vec(any::<u8>(), 23..=30)) {
            let mut modified = data.clone();
            if !modified.is_empty() {
                modified[0] = 0x03; // Set device type to Radon
            }
            let _ = parse_advertisement(&modified);
        }

        /// Parsing with Aranet Radiation format should not panic.
        #[test]
        fn parse_aranet_radiation_advertisement_never_panics(data in proptest::collection::vec(any::<u8>(), 19..=30)) {
            let mut modified = data.clone();
            if !modified.is_empty() {
                modified[0] = 0x02; // Set device type to Radiation
            }
            let _ = parse_advertisement(&modified);
        }
    }
}
