//! Color helper functions for the TUI.
//!
//! This module provides color coding for various sensor readings and status indicators.
//!
//! # CO2 Level Color Coding
//!
//! CO2 levels are color-coded based on air quality guidelines:
//!
//! | Range (ppm)  | Color  | Meaning                                    |
//! |--------------|--------|--------------------------------------------|
//! | 0-800        | Green  | Good - Normal outdoor/indoor air quality   |
//! | 801-1000     | Yellow | Moderate - Acceptable, some sensitivity    |
//! | 1001-1500    | Orange | Elevated - Consider improving ventilation  |
//! | 1501+        | Red    | High - Poor ventilation, take action       |
//!
//! These thresholds are based on indoor air quality standards and the Aranet4
//! device's built-in status indicators.

use aranet_types::Status;
use ratatui::style::Color;

use super::super::app::ConnectionStatus;
use super::theme::AppTheme;

/// Returns a color based on CO2 concentration level.
///
/// # Arguments
///
/// * `ppm` - CO2 concentration in parts per million
///
/// # Returns
///
/// A [`Color`] representing the air quality:
/// - Green: 0-800 ppm (good)
/// - Yellow: 801-1000 ppm (moderate)
/// - Orange (RGB 255,165,0): 1001-1500 ppm (elevated)
/// - Red: 1501+ ppm (high)
#[must_use]
pub fn co2_color(theme: &AppTheme, ppm: u16) -> Color {
    theme.co2_level_color(ppm)
}

/// Returns a color based on radon concentration level.
///
/// Radon levels are color-coded based on EPA guidelines:
///
/// | Range (Bq/m³) | Color  | Meaning                                    |
/// |---------------|--------|--------------------------------------------|
/// | 0-100         | Green  | Good - Low risk                            |
/// | 101-150       | Yellow | Moderate - Consider mitigation             |
/// | 151-300       | Orange | Elevated - Mitigation recommended          |
/// | 301+          | Red    | High - Take action                         |
///
/// Note: EPA action level is 4 pCi/L ≈ 148 Bq/m³
#[must_use]
pub fn radon_color(theme: &AppTheme, bq_m3: u32) -> Color {
    theme.radon_level_color(bq_m3)
}

/// Returns a color based on the sensor status indicator.
///
/// # Arguments
///
/// * `status` - The status from the sensor reading
///
/// # Returns
///
/// A [`Color`] matching the status:
/// - Green status → Green color
/// - Yellow status → Yellow color
/// - Red status → Red color
/// - Error status → DarkGray color
// Kept for future use: displaying sensor status indicators in the TUI
#[allow(dead_code)]
#[must_use]
pub fn status_color(theme: &AppTheme, status: &Status) -> Color {
    theme.sensor_status_color(status)
}

/// Returns a color based on battery percentage.
///
/// # Arguments
///
/// * `percent` - Battery level as a percentage (0-100)
///
/// # Returns
///
/// A [`Color`] representing the battery level:
/// - Red: 0-20% (low, needs charging)
/// - Yellow: 21-50% (moderate)
/// - Green: 51-100% (good)
#[must_use]
pub fn battery_color(theme: &AppTheme, percent: u8) -> Color {
    theme.battery_level_color(percent)
}

/// Returns a color based on the connection status.
///
/// # Arguments
///
/// * `status` - The current connection status
///
/// # Returns
///
/// A [`Color`] representing the connection state:
/// - DarkGray: Disconnected
/// - Yellow: Connecting (in progress)
/// - Green: Connected
/// - Red: Error
// Kept for future use: displaying connection status in the TUI header/footer
#[allow(dead_code)]
#[must_use]
pub fn connection_status_color(theme: &AppTheme, status: &ConnectionStatus) -> Color {
    theme.connection_color(status)
}

/// Returns signal bars and color based on RSSI value.
#[must_use]
pub fn signal_strength_display(theme: &AppTheme, rssi: i16) -> (&'static str, Color) {
    theme.signal_strength_display(rssi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_co2_color_good() {
        let theme = AppTheme::dark();
        assert_eq!(co2_color(&theme, 0), theme.success);
        assert_eq!(co2_color(&theme, 400), theme.success);
        assert_eq!(co2_color(&theme, 800), theme.success);
    }

    #[test]
    fn test_co2_color_moderate() {
        let theme = AppTheme::dark();
        assert_eq!(co2_color(&theme, 801), theme.warning);
        assert_eq!(co2_color(&theme, 900), theme.warning);
        assert_eq!(co2_color(&theme, 1000), theme.warning);
    }

    #[test]
    fn test_co2_color_elevated() {
        let theme = AppTheme::dark();
        assert_eq!(co2_color(&theme, 1001), theme.caution);
        assert_eq!(co2_color(&theme, 1250), theme.caution);
        assert_eq!(co2_color(&theme, 1500), theme.caution);
    }

    #[test]
    fn test_co2_color_high() {
        let theme = AppTheme::dark();
        assert_eq!(co2_color(&theme, 1501), theme.danger);
        assert_eq!(co2_color(&theme, 2000), theme.danger);
        assert_eq!(co2_color(&theme, 5000), theme.danger);
    }

    #[test]
    fn test_battery_color() {
        let theme = AppTheme::dark();
        assert_eq!(battery_color(&theme, 0), theme.danger);
        assert_eq!(battery_color(&theme, 20), theme.danger);
        assert_eq!(battery_color(&theme, 21), theme.warning);
        assert_eq!(battery_color(&theme, 50), theme.warning);
        assert_eq!(battery_color(&theme, 51), theme.success);
        assert_eq!(battery_color(&theme, 100), theme.success);
    }

    #[test]
    fn test_status_color() {
        let theme = AppTheme::dark();
        assert_eq!(status_color(&theme, &Status::Green), theme.success);
        assert_eq!(status_color(&theme, &Status::Yellow), theme.warning);
        assert_eq!(status_color(&theme, &Status::Red), theme.danger);
        assert_eq!(status_color(&theme, &Status::Error), theme.signal_offline);
    }

    #[test]
    fn test_connection_status_color() {
        let theme = AppTheme::dark();
        assert_eq!(
            connection_status_color(&theme, &ConnectionStatus::Disconnected),
            theme.signal_offline
        );
        assert_eq!(
            connection_status_color(&theme, &ConnectionStatus::Connecting),
            theme.warning
        );
        assert_eq!(
            connection_status_color(&theme, &ConnectionStatus::Connected),
            theme.success
        );
        assert_eq!(
            connection_status_color(&theme, &ConnectionStatus::Error("test".to_string())),
            theme.danger
        );
    }

    #[test]
    fn test_radon_color_good() {
        let theme = AppTheme::dark();
        assert_eq!(radon_color(&theme, 0), theme.success);
        assert_eq!(radon_color(&theme, 50), theme.success);
        assert_eq!(radon_color(&theme, 100), theme.success);
    }

    #[test]
    fn test_radon_color_moderate() {
        let theme = AppTheme::dark();
        assert_eq!(radon_color(&theme, 101), theme.warning);
        assert_eq!(radon_color(&theme, 125), theme.warning);
        assert_eq!(radon_color(&theme, 150), theme.warning);
    }

    #[test]
    fn test_radon_color_elevated() {
        let theme = AppTheme::dark();
        assert_eq!(radon_color(&theme, 151), theme.caution);
        assert_eq!(radon_color(&theme, 200), theme.caution);
        assert_eq!(radon_color(&theme, 300), theme.caution);
    }

    #[test]
    fn test_radon_color_high() {
        let theme = AppTheme::dark();
        assert_eq!(radon_color(&theme, 301), theme.danger);
        assert_eq!(radon_color(&theme, 500), theme.danger);
        assert_eq!(radon_color(&theme, 1000), theme.danger);
    }
}
