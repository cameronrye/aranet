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
    if data.is_empty() {
        return Err(Error::InvalidData(
            "Advertisement data is empty".to_string(),
        ));
    }

    // First byte is device type
    let device_type = match data[0] {
        0xF1 => DeviceType::Aranet4,
        0xF2 => DeviceType::Aranet2,
        0xF3 => DeviceType::AranetRadon,
        0xF4 => DeviceType::AranetRadiation,
        other => {
            return Err(Error::InvalidData(format!(
                "Unknown device type: 0x{:02X}",
                other
            )));
        }
    };

    match device_type {
        DeviceType::Aranet4 => parse_aranet4_advertisement(data),
        DeviceType::Aranet2 => parse_aranet2_advertisement(data),
        DeviceType::AranetRadon => parse_aranet_radon_advertisement(data),
        DeviceType::AranetRadiation => parse_aranet_radiation_advertisement(data),
        // Handle future device types - return error for unknown types
        _ => Err(Error::InvalidData(format!(
            "Unsupported device type for advertisement parsing: {:?}",
            device_type
        ))),
    }
}

/// Parse Aranet4 advertisement data.
///
/// Format (16 bytes):
/// - byte 0: Type (0xF1)
/// - byte 1: Flags
/// - bytes 2-3: CO2 (u16 LE)
/// - bytes 4-5: Temperature (u16 LE, /20 for °C)
/// - bytes 6-7: Pressure (u16 LE, /10 for hPa)
/// - byte 8: Humidity (u8)
/// - byte 9: Battery (u8)
/// - byte 10: Status (u8)
/// - bytes 11-12: Interval (u16 LE, seconds)
/// - bytes 13-14: Age (u16 LE, seconds)
/// - byte 15: Counter (u8)
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

/// Parse Aranet2 advertisement data.
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

/// Parse Aranet Radon advertisement data.
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

/// Parse Aranet Radiation advertisement data.
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
        let data: [u8; 16] = [
            0xF1, // device type
            0x00, // flags
            0x20, 0x03, // CO2 = 800
            0xC2, 0x01, // temp_raw = 450 (22.5°C)
            0x94, 0x27, // pressure_raw = 10132 (1013.2 hPa)
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
        assert_eq!(result.counter, Some(5));
    }

    #[test]
    fn test_parse_aranet2_advertisement() {
        let data: [u8; 12] = [
            0xF2, // device type
            0x00, // flags
            0xC2, 0x01, // temp_raw = 450 (22.5°C)
            0xC2, 0x01, // humidity_raw = 450 (45%)
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
    fn test_parse_empty_data() {
        let result = parse_advertisement(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_parse_unknown_device_type() {
        let data: [u8; 16] = [0xFF; 16];
        let result = parse_advertisement(&data);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unknown device type")
        );
    }

    #[test]
    fn test_parse_aranet4_insufficient_bytes() {
        let data: [u8; 10] = [0xF1; 10];
        let result = parse_advertisement(&data);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("requires 16 bytes")
        );
    }

    #[test]
    fn test_parse_aranet_radiation_advertisement() {
        let data: [u8; 16] = [
            0xF4, // device type = Radiation
            0x00, // flags
            85,   // battery
            1,    // status = Green
            0x2C, 0x01, // interval = 300
            0x3C, 0x00, // age = 60
            0xE8, 0x03, 0x00, 0x00, // dose rate = 1000 nSv/h = 1.0 µSv/h
            0x00, 0x00, 0x00, 0x00, // padding
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
        let data: [u8; 10] = [0xF4; 10];
        let result = parse_advertisement(&data);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("16 bytes")
        );
    }
}
