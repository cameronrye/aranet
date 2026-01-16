//! Device settings read/write.
//!
//! This module provides functionality to read and modify device
//! settings on Aranet sensors.

use tracing::info;

use crate::device::Device;
use crate::error::{Error, Result};
use crate::uuid::{CALIBRATION, COMMAND, READ_INTERVAL};

/// Measurement interval options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MeasurementInterval {
    /// 1 minute interval.
    OneMinute = 0x01,
    /// 2 minute interval.
    TwoMinutes = 0x02,
    /// 5 minute interval.
    FiveMinutes = 0x05,
    /// 10 minute interval.
    TenMinutes = 0x0A,
}

impl MeasurementInterval {
    /// Get the interval in seconds.
    pub fn as_seconds(&self) -> u16 {
        match self {
            MeasurementInterval::OneMinute => 60,
            MeasurementInterval::TwoMinutes => 120,
            MeasurementInterval::FiveMinutes => 300,
            MeasurementInterval::TenMinutes => 600,
        }
    }

    /// Try to create from seconds value.
    pub fn from_seconds(seconds: u16) -> Option<Self> {
        match seconds {
            60 => Some(MeasurementInterval::OneMinute),
            120 => Some(MeasurementInterval::TwoMinutes),
            300 => Some(MeasurementInterval::FiveMinutes),
            600 => Some(MeasurementInterval::TenMinutes),
            _ => None,
        }
    }

    /// Try to create from minutes value.
    pub fn from_minutes(minutes: u8) -> Option<Self> {
        match minutes {
            1 => Some(MeasurementInterval::OneMinute),
            2 => Some(MeasurementInterval::TwoMinutes),
            5 => Some(MeasurementInterval::FiveMinutes),
            10 => Some(MeasurementInterval::TenMinutes),
            _ => None,
        }
    }
}

/// Bluetooth range options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BluetoothRange {
    /// Standard range.
    Standard = 0x00,
    /// Extended range.
    Extended = 0x01,
}

/// Device settings.
#[derive(Debug, Clone)]
pub struct DeviceSettings {
    /// Current measurement interval.
    pub interval: MeasurementInterval,
    /// Smart Home integration enabled.
    pub smart_home_enabled: bool,
    /// Bluetooth range setting.
    pub bluetooth_range: BluetoothRange,
}

/// Calibration data from the device.
#[derive(Debug, Clone, Default)]
pub struct CalibrationData {
    /// Raw calibration bytes.
    pub raw: Vec<u8>,
    /// CO2 calibration offset (if available).
    pub co2_offset: Option<i16>,
}

impl Device {
    /// Get the current measurement interval.
    pub async fn get_interval(&self) -> Result<MeasurementInterval> {
        let data = self.read_characteristic(READ_INTERVAL).await?;

        if data.len() < 2 {
            return Err(Error::InvalidData("Invalid interval data".to_string()));
        }

        let seconds = u16::from_le_bytes([data[0], data[1]]);

        MeasurementInterval::from_seconds(seconds)
            .ok_or_else(|| Error::InvalidData(format!("Unknown interval: {} seconds", seconds)))
    }

    /// Set the measurement interval.
    ///
    /// The device will start using the new interval after the current
    /// measurement cycle completes.
    pub async fn set_interval(&self, interval: MeasurementInterval) -> Result<()> {
        info!("Setting measurement interval to {:?}", interval);

        // Command format: 0x90 XX (XX = interval in minutes)
        let minutes = match interval {
            MeasurementInterval::OneMinute => 0x01,
            MeasurementInterval::TwoMinutes => 0x02,
            MeasurementInterval::FiveMinutes => 0x05,
            MeasurementInterval::TenMinutes => 0x0A,
        };

        let cmd = [0x90, minutes];
        self.write_characteristic(COMMAND, &cmd).await?;

        Ok(())
    }

    /// Enable or disable Smart Home integration.
    ///
    /// When enabled, the device advertises sensor data that can be read
    /// without connecting (passive scanning).
    pub async fn set_smart_home(&self, enabled: bool) -> Result<()> {
        info!("Setting Smart Home integration to {}", enabled);

        // Command format: 0x91 XX (XX = 00 disabled, 01 enabled)
        let cmd = [0x91, if enabled { 0x01 } else { 0x00 }];
        self.write_characteristic(COMMAND, &cmd).await?;

        Ok(())
    }

    /// Set the Bluetooth range.
    pub async fn set_bluetooth_range(&self, range: BluetoothRange) -> Result<()> {
        info!("Setting Bluetooth range to {:?}", range);

        // Command format: 0x92 XX (XX = 00 standard, 01 extended)
        let cmd = [0x92, range as u8];
        self.write_characteristic(COMMAND, &cmd).await?;

        Ok(())
    }

    /// Read calibration data from the device.
    pub async fn get_calibration(&self) -> Result<CalibrationData> {
        let raw = self.read_characteristic(CALIBRATION).await?;

        // Parse CO2 offset if available (typically at offset 2-3)
        let co2_offset = if raw.len() >= 4 {
            Some(i16::from_le_bytes([raw[2], raw[3]]))
        } else {
            None
        };

        Ok(CalibrationData { raw, co2_offset })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interval_from_seconds() {
        assert_eq!(
            MeasurementInterval::from_seconds(60),
            Some(MeasurementInterval::OneMinute)
        );
        assert_eq!(
            MeasurementInterval::from_seconds(120),
            Some(MeasurementInterval::TwoMinutes)
        );
        assert_eq!(
            MeasurementInterval::from_seconds(300),
            Some(MeasurementInterval::FiveMinutes)
        );
        assert_eq!(
            MeasurementInterval::from_seconds(600),
            Some(MeasurementInterval::TenMinutes)
        );
        assert_eq!(MeasurementInterval::from_seconds(100), None);
    }

    #[test]
    fn test_interval_from_minutes() {
        assert_eq!(
            MeasurementInterval::from_minutes(1),
            Some(MeasurementInterval::OneMinute)
        );
        assert_eq!(
            MeasurementInterval::from_minutes(2),
            Some(MeasurementInterval::TwoMinutes)
        );
        assert_eq!(
            MeasurementInterval::from_minutes(5),
            Some(MeasurementInterval::FiveMinutes)
        );
        assert_eq!(
            MeasurementInterval::from_minutes(10),
            Some(MeasurementInterval::TenMinutes)
        );
        assert_eq!(MeasurementInterval::from_minutes(3), None);
    }

    #[test]
    fn test_interval_as_seconds() {
        assert_eq!(MeasurementInterval::OneMinute.as_seconds(), 60);
        assert_eq!(MeasurementInterval::TwoMinutes.as_seconds(), 120);
        assert_eq!(MeasurementInterval::FiveMinutes.as_seconds(), 300);
        assert_eq!(MeasurementInterval::TenMinutes.as_seconds(), 600);
    }
}
