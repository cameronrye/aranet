//! BLE command constants for Aranet devices.
//!
//! This module contains the command bytes used in the Aranet BLE protocol.

/// History V2 request command (read-based protocol).
/// Format: `[HISTORY_V2_REQUEST, param, start_lo, start_hi]`
pub const HISTORY_V2_REQUEST: u8 = 0x61;

/// History V1 request command (notification-based protocol).
/// Format: `[HISTORY_V1_REQUEST, param, start_lo, start_hi, count_lo, count_hi]`
pub const HISTORY_V1_REQUEST: u8 = 0x82;

/// Set measurement interval command.
/// Format: `[SET_INTERVAL, minutes]`
/// Valid minutes: 1, 2, 5, 10
pub const SET_INTERVAL: u8 = 0x90;

/// Enable/disable Smart Home integration command.
/// Format: `[SET_SMART_HOME, enabled]`
/// enabled: 0x00 = disabled, 0x01 = enabled
pub const SET_SMART_HOME: u8 = 0x91;

/// Set Bluetooth range command.
/// Format: `[SET_BLUETOOTH_RANGE, range]`
/// range: 0x00 = standard, 0x01 = extended
pub const SET_BLUETOOTH_RANGE: u8 = 0x92;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_values() {
        assert_eq!(HISTORY_V2_REQUEST, 0x61);
        assert_eq!(HISTORY_V1_REQUEST, 0x82);
        assert_eq!(SET_INTERVAL, 0x90);
        assert_eq!(SET_SMART_HOME, 0x91);
        assert_eq!(SET_BLUETOOTH_RANGE, 0x92);
    }
}

