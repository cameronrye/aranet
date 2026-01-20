//! Bluetooth UUIDs for Aranet devices.
//!
//! This module contains all the UUIDs needed to communicate with Aranet
//! sensors over Bluetooth Low Energy.

use uuid::{Uuid, uuid};

// --- Saf Tehnika (Aranet) Service UUIDs ---

/// Saf Tehnika custom service UUID for firmware v1.2.0 and newer.
pub const SAF_TEHNIKA_SERVICE_NEW: Uuid = uuid!("0000fce0-0000-1000-8000-00805f9b34fb");

/// Saf Tehnika custom service UUID for firmware versions before v1.2.0.
pub const SAF_TEHNIKA_SERVICE_OLD: Uuid = uuid!("f0cd1400-95da-4f4b-9ac8-aa55d312af0c");

/// Saf Tehnika manufacturer ID for BLE advertisements.
pub const MANUFACTURER_ID: u16 = 0x0702;

// --- Aranet Characteristic UUIDs ---

/// Current readings characteristic (basic).
pub const CURRENT_READINGS: Uuid = uuid!("f0cd1503-95da-4f4b-9ac8-aa55d312af0c");

/// Current readings characteristic (detailed) - Aranet4.
pub const CURRENT_READINGS_DETAIL: Uuid = uuid!("f0cd3001-95da-4f4b-9ac8-aa55d312af0c");

/// Current readings characteristic (detailed) - Aranet2/Radon/Radiation.
pub const CURRENT_READINGS_DETAIL_ALT: Uuid = uuid!("f0cd3003-95da-4f4b-9ac8-aa55d312af0c");

/// Total number of readings stored in device memory.
pub const TOTAL_READINGS: Uuid = uuid!("f0cd2001-95da-4f4b-9ac8-aa55d312af0c");

/// Measurement interval in seconds.
pub const READ_INTERVAL: Uuid = uuid!("f0cd2002-95da-4f4b-9ac8-aa55d312af0c");

/// History data characteristic (version 1) - notification-based.
pub const HISTORY_V1: Uuid = uuid!("f0cd2003-95da-4f4b-9ac8-aa55d312af0c");

/// History data characteristic (version 2) - read-based.
pub const HISTORY_V2: Uuid = uuid!("f0cd2005-95da-4f4b-9ac8-aa55d312af0c");

/// Sensor state characteristic for reading device settings.
pub const SENSOR_STATE: Uuid = uuid!("f0cd1401-95da-4f4b-9ac8-aa55d312af0c");

/// Command characteristic for device control.
pub const COMMAND: Uuid = uuid!("f0cd1402-95da-4f4b-9ac8-aa55d312af0c");

/// Seconds since last measurement.
pub const SECONDS_SINCE_UPDATE: Uuid = uuid!("f0cd2004-95da-4f4b-9ac8-aa55d312af0c");

/// Calibration data characteristic.
pub const CALIBRATION: Uuid = uuid!("f0cd1502-95da-4f4b-9ac8-aa55d312af0c");

// --- Standard BLE Service UUIDs ---

/// Generic Access Profile (GAP) service.
pub const GAP_SERVICE: Uuid = uuid!("00001800-0000-1000-8000-00805f9b34fb");

/// Device Information service.
pub const DEVICE_INFO_SERVICE: Uuid = uuid!("0000180a-0000-1000-8000-00805f9b34fb");

/// Battery service.
pub const BATTERY_SERVICE: Uuid = uuid!("0000180f-0000-1000-8000-00805f9b34fb");

// --- Device Information Characteristic UUIDs ---

/// Device name characteristic.
pub const DEVICE_NAME: Uuid = uuid!("00002a00-0000-1000-8000-00805f9b34fb");

/// Model number string characteristic.
pub const MODEL_NUMBER: Uuid = uuid!("00002a24-0000-1000-8000-00805f9b34fb");

/// Serial number string characteristic.
pub const SERIAL_NUMBER: Uuid = uuid!("00002a25-0000-1000-8000-00805f9b34fb");

/// Firmware revision string characteristic.
pub const FIRMWARE_REVISION: Uuid = uuid!("00002a26-0000-1000-8000-00805f9b34fb");

/// Hardware revision string characteristic.
pub const HARDWARE_REVISION: Uuid = uuid!("00002a27-0000-1000-8000-00805f9b34fb");

/// Software revision string characteristic.
pub const SOFTWARE_REVISION: Uuid = uuid!("00002a28-0000-1000-8000-00805f9b34fb");

/// Manufacturer name string characteristic.
pub const MANUFACTURER_NAME: Uuid = uuid!("00002a29-0000-1000-8000-00805f9b34fb");

// --- Battery Characteristic UUIDs ---

/// Battery level characteristic.
pub const BATTERY_LEVEL: Uuid = uuid!("00002a19-0000-1000-8000-00805f9b34fb");

#[cfg(test)]
mod tests {
    use super::*;

    // --- Service UUID tests ---

    #[test]
    fn test_saf_tehnika_service_new_uuid() {
        // New firmware (v1.2.0+) service UUID
        let expected = "0000fce0-0000-1000-8000-00805f9b34fb";
        assert_eq!(SAF_TEHNIKA_SERVICE_NEW.to_string(), expected);
    }

    #[test]
    fn test_saf_tehnika_service_old_uuid() {
        // Old firmware (pre-1.2.0) service UUID
        let expected = "f0cd1400-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(SAF_TEHNIKA_SERVICE_OLD.to_string(), expected);
    }

    #[test]
    fn test_manufacturer_id() {
        // SAF Tehnika manufacturer ID
        assert_eq!(MANUFACTURER_ID, 0x0702);
        assert_eq!(MANUFACTURER_ID, 1794);
    }

    // --- Aranet Characteristic UUID tests ---

    #[test]
    fn test_current_readings_uuid() {
        let expected = "f0cd1503-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(CURRENT_READINGS.to_string(), expected);
    }

    #[test]
    fn test_current_readings_detail_uuid() {
        // Aranet4 detailed readings
        let expected = "f0cd3001-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(CURRENT_READINGS_DETAIL.to_string(), expected);
    }

    #[test]
    fn test_current_readings_detail_alt_uuid() {
        // Aranet2/Radon/Radiation detailed readings
        let expected = "f0cd3003-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(CURRENT_READINGS_DETAIL_ALT.to_string(), expected);
    }

    #[test]
    fn test_total_readings_uuid() {
        let expected = "f0cd2001-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(TOTAL_READINGS.to_string(), expected);
    }

    #[test]
    fn test_read_interval_uuid() {
        let expected = "f0cd2002-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(READ_INTERVAL.to_string(), expected);
    }

    #[test]
    fn test_history_v1_uuid() {
        let expected = "f0cd2003-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(HISTORY_V1.to_string(), expected);
    }

    #[test]
    fn test_history_v2_uuid() {
        let expected = "f0cd2005-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(HISTORY_V2.to_string(), expected);
    }

    #[test]
    fn test_sensor_state_uuid() {
        let expected = "f0cd1401-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(SENSOR_STATE.to_string(), expected);
    }

    #[test]
    fn test_command_uuid() {
        let expected = "f0cd1402-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(COMMAND.to_string(), expected);
    }

    #[test]
    fn test_seconds_since_update_uuid() {
        let expected = "f0cd2004-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(SECONDS_SINCE_UPDATE.to_string(), expected);
    }

    #[test]
    fn test_calibration_uuid() {
        let expected = "f0cd1502-95da-4f4b-9ac8-aa55d312af0c";
        assert_eq!(CALIBRATION.to_string(), expected);
    }

    // --- Standard BLE Service UUID tests ---

    #[test]
    fn test_gap_service_uuid() {
        let expected = "00001800-0000-1000-8000-00805f9b34fb";
        assert_eq!(GAP_SERVICE.to_string(), expected);
    }

    #[test]
    fn test_device_info_service_uuid() {
        let expected = "0000180a-0000-1000-8000-00805f9b34fb";
        assert_eq!(DEVICE_INFO_SERVICE.to_string(), expected);
    }

    #[test]
    fn test_battery_service_uuid() {
        let expected = "0000180f-0000-1000-8000-00805f9b34fb";
        assert_eq!(BATTERY_SERVICE.to_string(), expected);
    }

    // --- Device Information Characteristic UUID tests ---

    #[test]
    fn test_device_name_uuid() {
        let expected = "00002a00-0000-1000-8000-00805f9b34fb";
        assert_eq!(DEVICE_NAME.to_string(), expected);
    }

    #[test]
    fn test_model_number_uuid() {
        let expected = "00002a24-0000-1000-8000-00805f9b34fb";
        assert_eq!(MODEL_NUMBER.to_string(), expected);
    }

    #[test]
    fn test_serial_number_uuid() {
        let expected = "00002a25-0000-1000-8000-00805f9b34fb";
        assert_eq!(SERIAL_NUMBER.to_string(), expected);
    }

    #[test]
    fn test_firmware_revision_uuid() {
        let expected = "00002a26-0000-1000-8000-00805f9b34fb";
        assert_eq!(FIRMWARE_REVISION.to_string(), expected);
    }

    #[test]
    fn test_hardware_revision_uuid() {
        let expected = "00002a27-0000-1000-8000-00805f9b34fb";
        assert_eq!(HARDWARE_REVISION.to_string(), expected);
    }

    #[test]
    fn test_software_revision_uuid() {
        let expected = "00002a28-0000-1000-8000-00805f9b34fb";
        assert_eq!(SOFTWARE_REVISION.to_string(), expected);
    }

    #[test]
    fn test_manufacturer_name_uuid() {
        let expected = "00002a29-0000-1000-8000-00805f9b34fb";
        assert_eq!(MANUFACTURER_NAME.to_string(), expected);
    }

    #[test]
    fn test_battery_level_uuid() {
        let expected = "00002a19-0000-1000-8000-00805f9b34fb";
        assert_eq!(BATTERY_LEVEL.to_string(), expected);
    }

    // --- UUID distinctness tests ---

    #[test]
    fn test_aranet_service_uuids_are_distinct() {
        assert_ne!(SAF_TEHNIKA_SERVICE_NEW, SAF_TEHNIKA_SERVICE_OLD);
    }

    #[test]
    fn test_current_readings_uuids_are_distinct() {
        assert_ne!(CURRENT_READINGS, CURRENT_READINGS_DETAIL);
        assert_ne!(CURRENT_READINGS_DETAIL, CURRENT_READINGS_DETAIL_ALT);
        assert_ne!(CURRENT_READINGS, CURRENT_READINGS_DETAIL_ALT);
    }

    #[test]
    fn test_history_uuids_are_distinct() {
        assert_ne!(HISTORY_V1, HISTORY_V2);
    }

    #[test]
    fn test_standard_service_uuids_are_distinct() {
        assert_ne!(GAP_SERVICE, DEVICE_INFO_SERVICE);
        assert_ne!(DEVICE_INFO_SERVICE, BATTERY_SERVICE);
        assert_ne!(GAP_SERVICE, BATTERY_SERVICE);
    }

    // --- UUID format validation tests ---

    #[test]
    fn test_aranet_characteristic_prefix() {
        // All Aranet-specific characteristics start with f0cd
        let aranet_uuids = [
            CURRENT_READINGS,
            CURRENT_READINGS_DETAIL,
            CURRENT_READINGS_DETAIL_ALT,
            TOTAL_READINGS,
            READ_INTERVAL,
            HISTORY_V1,
            HISTORY_V2,
            SENSOR_STATE,
            COMMAND,
            SECONDS_SINCE_UPDATE,
            CALIBRATION,
        ];

        for uuid in aranet_uuids {
            assert!(
                uuid.to_string().starts_with("f0cd"),
                "UUID {} should start with f0cd",
                uuid
            );
        }
    }

    #[test]
    fn test_standard_ble_characteristic_prefix() {
        // Standard BLE characteristics use 16-bit UUIDs (start with 00002aXX)
        let standard_uuids = [
            DEVICE_NAME,
            MODEL_NUMBER,
            SERIAL_NUMBER,
            FIRMWARE_REVISION,
            HARDWARE_REVISION,
            SOFTWARE_REVISION,
            MANUFACTURER_NAME,
            BATTERY_LEVEL,
        ];

        for uuid in standard_uuids {
            assert!(
                uuid.to_string().starts_with("00002a"),
                "UUID {} should start with 00002a",
                uuid
            );
        }
    }
}
