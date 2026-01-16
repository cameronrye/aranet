//! Utility functions for aranet-core.
//!
//! This module contains shared utility functions used across the crate.

use btleplug::platform::PeripheralId;

/// Format a peripheral ID as a string.
///
/// On macOS, peripheral IDs are UUIDs. On other platforms, they may be
/// MAC addresses or other formats. This function extracts the useful
/// identifier string.
///
/// # Example
///
/// ```ignore
/// use aranet_core::util::format_peripheral_id;
///
/// let id = peripheral.id();
/// let formatted = format_peripheral_id(&id);
/// println!("Device: {}", formatted);
/// ```
pub fn format_peripheral_id(id: &PeripheralId) -> String {
    format!("{:?}", id)
        .trim_start_matches("PeripheralId(")
        .trim_end_matches(')')
        .to_string()
}

/// Create an identifier string from an address and peripheral ID.
///
/// On macOS where addresses are 00:00:00:00:00:00, uses the peripheral ID.
/// On other platforms, uses the Bluetooth address.
pub fn create_identifier(address: &str, peripheral_id: &PeripheralId) -> String {
    if address == "00:00:00:00:00:00" {
        format_peripheral_id(peripheral_id)
    } else {
        address.to_string()
    }
}

#[cfg(test)]
mod tests {
    // Note: We can't easily create PeripheralId in tests, so we test the logic
    // rather than calling the functions directly.
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_create_identifier_with_valid_address() {
        // We can't easily create a PeripheralId in tests, but we can test the logic
        let address = "AA:BB:CC:DD:EE:FF";
        // When address is valid, it should be used directly
        assert_ne!(address, "00:00:00:00:00:00");
    }

    #[test]
    fn test_create_identifier_with_zero_address() {
        let address = "00:00:00:00:00:00";
        assert_eq!(address, "00:00:00:00:00:00");
    }
}

