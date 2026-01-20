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
pub fn co2_color(ppm: u16) -> Color {
    match ppm {
        0..=800 => Color::Green,
        801..=1000 => Color::Yellow,
        1001..=1500 => Color::Rgb(255, 165, 0),
        _ => Color::Red,
    }
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
pub fn radon_color(bq_m3: u32) -> Color {
    match bq_m3 {
        0..=100 => Color::Green,
        101..=150 => Color::Yellow,
        151..=300 => Color::Rgb(255, 165, 0),
        _ => Color::Red,
    }
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
pub fn status_color(status: &Status) -> Color {
    match status {
        Status::Green => Color::Green,
        Status::Yellow => Color::Yellow,
        Status::Red => Color::Red,
        Status::Error => Color::DarkGray,
        // Handle future non_exhaustive variants
        _ => Color::DarkGray,
    }
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
pub fn battery_color(percent: u8) -> Color {
    match percent {
        0..=20 => Color::Red,
        21..=50 => Color::Yellow,
        _ => Color::Green,
    }
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
pub fn connection_status_color(status: &ConnectionStatus) -> Color {
    match status {
        ConnectionStatus::Disconnected => Color::DarkGray,
        ConnectionStatus::Connecting => Color::Yellow,
        ConnectionStatus::Connected => Color::Green,
        ConnectionStatus::Error(_) => Color::Red,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_co2_color_good() {
        assert_eq!(co2_color(0), Color::Green);
        assert_eq!(co2_color(400), Color::Green);
        assert_eq!(co2_color(800), Color::Green);
    }

    #[test]
    fn test_co2_color_moderate() {
        assert_eq!(co2_color(801), Color::Yellow);
        assert_eq!(co2_color(900), Color::Yellow);
        assert_eq!(co2_color(1000), Color::Yellow);
    }

    #[test]
    fn test_co2_color_elevated() {
        assert_eq!(co2_color(1001), Color::Rgb(255, 165, 0));
        assert_eq!(co2_color(1250), Color::Rgb(255, 165, 0));
        assert_eq!(co2_color(1500), Color::Rgb(255, 165, 0));
    }

    #[test]
    fn test_co2_color_high() {
        assert_eq!(co2_color(1501), Color::Red);
        assert_eq!(co2_color(2000), Color::Red);
        assert_eq!(co2_color(5000), Color::Red);
    }

    #[test]
    fn test_battery_color() {
        assert_eq!(battery_color(0), Color::Red);
        assert_eq!(battery_color(20), Color::Red);
        assert_eq!(battery_color(21), Color::Yellow);
        assert_eq!(battery_color(50), Color::Yellow);
        assert_eq!(battery_color(51), Color::Green);
        assert_eq!(battery_color(100), Color::Green);
    }

    #[test]
    fn test_status_color() {
        assert_eq!(status_color(&Status::Green), Color::Green);
        assert_eq!(status_color(&Status::Yellow), Color::Yellow);
        assert_eq!(status_color(&Status::Red), Color::Red);
        assert_eq!(status_color(&Status::Error), Color::DarkGray);
    }

    #[test]
    fn test_connection_status_color() {
        assert_eq!(
            connection_status_color(&ConnectionStatus::Disconnected),
            Color::DarkGray
        );
        assert_eq!(
            connection_status_color(&ConnectionStatus::Connecting),
            Color::Yellow
        );
        assert_eq!(
            connection_status_color(&ConnectionStatus::Connected),
            Color::Green
        );
        assert_eq!(
            connection_status_color(&ConnectionStatus::Error("test".to_string())),
            Color::Red
        );
    }

    #[test]
    fn test_radon_color_good() {
        assert_eq!(radon_color(0), Color::Green);
        assert_eq!(radon_color(50), Color::Green);
        assert_eq!(radon_color(100), Color::Green);
    }

    #[test]
    fn test_radon_color_moderate() {
        assert_eq!(radon_color(101), Color::Yellow);
        assert_eq!(radon_color(125), Color::Yellow);
        assert_eq!(radon_color(150), Color::Yellow);
    }

    #[test]
    fn test_radon_color_elevated() {
        assert_eq!(radon_color(151), Color::Rgb(255, 165, 0));
        assert_eq!(radon_color(200), Color::Rgb(255, 165, 0));
        assert_eq!(radon_color(300), Color::Rgb(255, 165, 0));
    }

    #[test]
    fn test_radon_color_high() {
        assert_eq!(radon_color(301), Color::Red);
        assert_eq!(radon_color(500), Color::Red);
        assert_eq!(radon_color(1000), Color::Red);
    }
}
