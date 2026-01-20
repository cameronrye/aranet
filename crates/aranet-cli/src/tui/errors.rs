//! User-friendly error message formatting.
//!
//! This module provides functions to convert technical BLE error messages
//! into user-friendly text with actionable suggestions.

/// Convert a technical error message to a user-friendly message with guidance.
///
/// Takes the raw error string and returns a tuple of (short_message, suggestion).
pub fn format_error_with_guidance(error: &str) -> (String, Option<String>) {
    let error_lower = error.to_lowercase();

    // Characteristic not found (device issue) - check before generic "not found"
    if error_lower.contains("characteristic not found") {
        return (
            "Device communication error".to_string(),
            Some("The device may need a firmware update or may be incompatible.".to_string()),
        );
    }

    // Device not found / no devices in range
    if error_lower.contains("not found") || error_lower.contains("no devices") {
        return (
            "Device not found".to_string(),
            Some(
                "Make sure the device is powered on, in range, and not connected to another app."
                    .to_string(),
            ),
        );
    }

    // No Bluetooth adapter
    if error_lower.contains("no bluetooth adapter") || error_lower.contains("adapter unavailable") {
        return (
            "Bluetooth unavailable".to_string(),
            Some("Check that Bluetooth is enabled in System Settings.".to_string()),
        );
    }

    // Connection timeout
    if error_lower.contains("timed out") || error_lower.contains("timeout") {
        return (
            "Connection timed out".to_string(),
            Some("Move closer to the device or try again. The device may be busy.".to_string()),
        );
    }

    // Device already connected
    if error_lower.contains("already connected") {
        return (
            "Device busy".to_string(),
            Some("The device may be connected to another app. Close other Bluetooth apps and try again.".to_string()),
        );
    }

    // Permission denied
    if error_lower.contains("permission") || error_lower.contains("access denied") {
        return (
            "Bluetooth permission denied".to_string(),
            Some(
                "Grant Bluetooth permissions in System Settings > Privacy & Security.".to_string(),
            ),
        );
    }

    // Connection rejected / pairing failed
    if error_lower.contains("rejected") || error_lower.contains("pairing") {
        return (
            "Connection rejected".to_string(),
            Some("Try removing the device from Bluetooth settings and reconnecting.".to_string()),
        );
    }

    // Out of range
    if error_lower.contains("out of range") {
        return (
            "Device out of range".to_string(),
            Some(
                "Move closer to the device (within 10 meters with clear line of sight)."
                    .to_string(),
            ),
        );
    }

    // Invalid reading format
    if error_lower.contains("invalid reading") || error_lower.contains("invalid data") {
        return (
            "Invalid sensor data".to_string(),
            Some(
                "The device returned unexpected data. Try refreshing or reconnecting.".to_string(),
            ),
        );
    }

    // Generic BLE error
    if error_lower.contains("bluetooth error") || error_lower.contains("ble error") {
        return (
            "Bluetooth error".to_string(),
            Some(
                "Try disabling and re-enabling Bluetooth, or restart the application.".to_string(),
            ),
        );
    }

    // Default - show original error, no suggestion
    (error.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_not_found() {
        let (msg, suggestion) = format_error_with_guidance("Device 'Aranet4 12345' not found");
        assert_eq!(msg, "Device not found");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("powered on"));
    }

    #[test]
    fn test_no_devices() {
        let (msg, suggestion) = format_error_with_guidance("No devices available in range");
        assert_eq!(msg, "Device not found");
        assert!(suggestion.is_some());
    }

    #[test]
    fn test_timeout() {
        let (msg, suggestion) = format_error_with_guidance("Connection timed out after 30s");
        assert_eq!(msg, "Connection timed out");
        assert!(suggestion.is_some());
    }

    #[test]
    fn test_timeout_variant() {
        let (msg, suggestion) = format_error_with_guidance("Operation timeout");
        assert_eq!(msg, "Connection timed out");
        assert!(suggestion.unwrap().contains("Move closer"));
    }

    #[test]
    fn test_no_bluetooth_adapter() {
        let (msg, suggestion) = format_error_with_guidance("No Bluetooth adapter available");
        assert_eq!(msg, "Bluetooth unavailable");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("System Settings"));
    }

    #[test]
    fn test_adapter_unavailable() {
        let (msg, suggestion) = format_error_with_guidance("Bluetooth adapter unavailable");
        assert_eq!(msg, "Bluetooth unavailable");
        assert!(suggestion.is_some());
    }

    #[test]
    fn test_device_already_connected() {
        let (msg, suggestion) = format_error_with_guidance("Device is already connected");
        assert_eq!(msg, "Device busy");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("other Bluetooth apps"));
    }

    #[test]
    fn test_permission_denied() {
        let (msg, suggestion) = format_error_with_guidance("Bluetooth permission denied");
        assert_eq!(msg, "Bluetooth permission denied");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("Privacy & Security"));
    }

    #[test]
    fn test_access_denied() {
        let (msg, suggestion) = format_error_with_guidance("Access denied to Bluetooth");
        assert_eq!(msg, "Bluetooth permission denied");
        assert!(suggestion.is_some());
    }

    #[test]
    fn test_connection_rejected() {
        let (msg, suggestion) = format_error_with_guidance("Connection rejected by device");
        assert_eq!(msg, "Connection rejected");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("removing the device"));
    }

    #[test]
    fn test_pairing_failed() {
        let (msg, suggestion) = format_error_with_guidance("Pairing failed with device");
        assert_eq!(msg, "Connection rejected");
        assert!(suggestion.is_some());
    }

    #[test]
    fn test_out_of_range() {
        let (msg, suggestion) = format_error_with_guidance("Device is out of range");
        assert_eq!(msg, "Device out of range");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("10 meters"));
    }

    #[test]
    fn test_characteristic_not_found() {
        let (msg, suggestion) = format_error_with_guidance("Characteristic not found on device");
        assert_eq!(msg, "Device communication error");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("firmware update"));
    }

    #[test]
    fn test_invalid_reading() {
        let (msg, suggestion) = format_error_with_guidance("Invalid reading received from sensor");
        assert_eq!(msg, "Invalid sensor data");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("reconnecting"));
    }

    #[test]
    fn test_invalid_data() {
        let (msg, suggestion) = format_error_with_guidance("Invalid data format");
        assert_eq!(msg, "Invalid sensor data");
        assert!(suggestion.is_some());
    }

    #[test]
    fn test_generic_bluetooth_error() {
        let (msg, suggestion) = format_error_with_guidance("Bluetooth error occurred");
        assert_eq!(msg, "Bluetooth error");
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("re-enabling Bluetooth"));
    }

    #[test]
    fn test_generic_ble_error() {
        let (msg, suggestion) = format_error_with_guidance("BLE error: connection failed");
        assert_eq!(msg, "Bluetooth error");
        assert!(suggestion.is_some());
    }

    #[test]
    fn test_unknown_error() {
        let (msg, suggestion) = format_error_with_guidance("Some random error xyz");
        assert_eq!(msg, "Some random error xyz");
        assert!(suggestion.is_none());
    }

    #[test]
    fn test_case_insensitivity() {
        // Ensure matching works regardless of case
        let (msg, _) = format_error_with_guidance("DEVICE NOT FOUND");
        assert_eq!(msg, "Device not found");

        let (msg, _) = format_error_with_guidance("TIMED OUT");
        assert_eq!(msg, "Connection timed out");

        let (msg, _) = format_error_with_guidance("OUT OF RANGE");
        assert_eq!(msg, "Device out of range");
    }
}
