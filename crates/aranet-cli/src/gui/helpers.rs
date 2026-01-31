//! Helper types and conversion functions for the Aranet GUI.
//!
//! This module contains utility types like [`Toast`] notifications and
//! unit conversion functions for temperature, pressure, and radon measurements.

use std::time::{Duration, Instant};

use aranet_core::settings::{DeviceSettings, RadonUnit, TemperatureUnit};

/// How long toast notifications are displayed.
pub const TOAST_DURATION: Duration = Duration::from_secs(4);

/// Default scan duration.
pub const SCAN_DURATION: Duration = Duration::from_secs(5);

/// Available measurement intervals in seconds.
pub const INTERVAL_OPTIONS: &[(u16, &str)] = &[
    (60, "1 min"),
    (120, "2 min"),
    (300, "5 min"),
    (600, "10 min"),
];

/// Toast notification type.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Info may be used later
pub enum ToastType {
    Success,
    Error,
    Info,
}

/// A toast notification.
#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub toast_type: ToastType,
    pub created_at: Instant,
}

impl Toast {
    /// Create a new toast notification.
    pub fn new(message: impl Into<String>, toast_type: ToastType) -> Self {
        Self {
            message: message.into(),
            toast_type,
            created_at: Instant::now(),
        }
    }

    /// Check if this toast has expired.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > TOAST_DURATION
    }
}

/// Convert Celsius to Fahrenheit.
#[inline]
pub fn celsius_to_fahrenheit(celsius: f32) -> f32 {
    celsius * 9.0 / 5.0 + 32.0
}

/// Convert Bq/m³ to pCi/L (1 Bq/m³ = 0.027 pCi/L).
#[inline]
pub fn bq_to_pci(bq: u32) -> f32 {
    bq as f32 * 0.027
}

/// Convert hPa to inches of mercury (1 hPa = 0.02953 inHg).
#[inline]
pub fn hpa_to_inhg(hpa: f32) -> f32 {
    hpa * 0.02953
}

/// Format temperature value and unit based on device settings or app preference.
///
/// Priority: device settings > app_preference > Celsius
/// Returns (value_string, unit_string) tuple.
pub fn format_temperature(
    celsius: f32,
    settings: Option<&DeviceSettings>,
    app_preference: Option<&str>,
) -> (String, &'static str) {
    let use_fahrenheit = settings
        .map(|s| s.temperature_unit == TemperatureUnit::Fahrenheit)
        .unwrap_or_else(|| app_preference == Some("fahrenheit"));

    if use_fahrenheit {
        (format!("{:.1}", celsius_to_fahrenheit(celsius)), "°F")
    } else {
        (format!("{:.1}", celsius), "°C")
    }
}

/// Format pressure value and unit based on app preference.
///
/// Returns (value_string, unit_string) tuple.
pub fn format_pressure(hpa: f32, app_preference: &str) -> (String, &'static str) {
    if app_preference == "inhg" {
        (format!("{:.2}", hpa_to_inhg(hpa)), "inHg")
    } else {
        (format!("{:.1}", hpa), "hPa")
    }
}

/// Format radon value and unit based on device settings.
///
/// Returns (value_string, unit_string) tuple.
pub fn format_radon(bq: u32, settings: Option<&DeviceSettings>) -> (String, &'static str) {
    let use_pci = settings
        .map(|s| s.radon_unit == RadonUnit::PciL)
        .unwrap_or(false);

    if use_pci {
        (format!("{:.2}", bq_to_pci(bq)), "pCi/L")
    } else {
        (format!("{}", bq), "Bq/m3")
    }
}

/// Format uptime duration in human-readable form.
pub fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else if seconds < 86400 {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        format!("{}h {}m", hours, mins)
    } else {
        let days = seconds / 86400;
        let hours = (seconds % 86400) / 3600;
        format!("{}d {}h", days, hours)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // celsius_to_fahrenheit tests
    // ========================================================================

    #[test]
    fn test_celsius_to_fahrenheit_freezing() {
        let result = celsius_to_fahrenheit(0.0);
        assert!((result - 32.0).abs() < 0.01);
    }

    #[test]
    fn test_celsius_to_fahrenheit_boiling() {
        let result = celsius_to_fahrenheit(100.0);
        assert!((result - 212.0).abs() < 0.01);
    }

    #[test]
    fn test_celsius_to_fahrenheit_room_temp() {
        // 20°C should be 68°F
        let result = celsius_to_fahrenheit(20.0);
        assert!((result - 68.0).abs() < 0.01);
    }

    #[test]
    fn test_celsius_to_fahrenheit_negative() {
        // -40°C = -40°F (where both scales meet)
        let result = celsius_to_fahrenheit(-40.0);
        assert!((result - (-40.0)).abs() < 0.01);
    }

    // ========================================================================
    // bq_to_pci tests
    // ========================================================================

    #[test]
    fn test_bq_to_pci_zero() {
        let result = bq_to_pci(0);
        assert!((result - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_bq_to_pci_100() {
        // 100 Bq/m³ = 2.7 pCi/L
        let result = bq_to_pci(100);
        assert!((result - 2.7).abs() < 0.01);
    }

    #[test]
    fn test_bq_to_pci_who_action_level() {
        // WHO recommends action at 100 Bq/m³
        // In pCi/L, that's about 2.7
        let result = bq_to_pci(100);
        assert!((result - 2.7).abs() < 0.1);
    }

    // ========================================================================
    // format_temperature tests
    // ========================================================================

    #[test]
    fn test_format_temperature_no_settings_defaults_celsius() {
        let (value, unit) = format_temperature(20.5, None, None);
        assert_eq!(value, "20.5");
        assert_eq!(unit, "°C");
    }

    #[test]
    fn test_format_temperature_celsius_setting() {
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Celsius,
            ..Default::default()
        };
        let (value, unit) = format_temperature(20.5, Some(&settings), None);
        assert_eq!(value, "20.5");
        assert_eq!(unit, "°C");
    }

    #[test]
    fn test_format_temperature_fahrenheit_setting() {
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Fahrenheit,
            ..Default::default()
        };
        let (value, unit) = format_temperature(20.0, Some(&settings), None);
        assert_eq!(value, "68.0");
        assert_eq!(unit, "°F");
    }

    #[test]
    fn test_format_temperature_fahrenheit_decimal() {
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Fahrenheit,
            ..Default::default()
        };
        let (value, unit) = format_temperature(21.5, Some(&settings), None);
        // 21.5°C = 70.7°F
        assert_eq!(value, "70.7");
        assert_eq!(unit, "°F");
    }

    #[test]
    fn test_format_temperature_app_preference_fahrenheit() {
        let (value, unit) = format_temperature(20.0, None, Some("fahrenheit"));
        assert_eq!(value, "68.0");
        assert_eq!(unit, "°F");
    }

    #[test]
    fn test_format_temperature_device_overrides_app_preference() {
        // Device is set to Celsius, but app preference is Fahrenheit
        // Device should win
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Celsius,
            ..Default::default()
        };
        let (value, unit) = format_temperature(20.0, Some(&settings), Some("fahrenheit"));
        assert_eq!(value, "20.0");
        assert_eq!(unit, "°C");
    }

    // ========================================================================
    // format_pressure tests
    // ========================================================================

    #[test]
    fn test_format_pressure_hpa() {
        let (value, unit) = format_pressure(1013.25, "hpa");
        // 1013.25 rounded to 1 decimal place (floating point representation)
        assert_eq!(value, "1013.2");
        assert_eq!(unit, "hPa");
    }

    #[test]
    fn test_format_pressure_inhg() {
        let (value, unit) = format_pressure(1013.25, "inhg");
        // 1013.25 hPa ≈ 29.92 inHg
        assert_eq!(unit, "inHg");
        let val: f32 = value.parse().unwrap();
        assert!((val - 29.92).abs() < 0.1);
    }

    #[test]
    fn test_hpa_to_inhg_standard_atm() {
        // Standard atmospheric pressure: 1013.25 hPa = 29.92 inHg
        let result = hpa_to_inhg(1013.25);
        assert!((result - 29.92).abs() < 0.1);
    }

    // ========================================================================
    // format_radon tests
    // ========================================================================

    #[test]
    fn test_format_radon_no_settings_defaults_bq() {
        let (value, unit) = format_radon(100, None);
        assert_eq!(value, "100");
        assert_eq!(unit, "Bq/m3");
    }

    #[test]
    fn test_format_radon_bq_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::BqM3,
            ..Default::default()
        };
        let (value, unit) = format_radon(100, Some(&settings));
        assert_eq!(value, "100");
        assert_eq!(unit, "Bq/m3");
    }

    #[test]
    fn test_format_radon_pci_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let (value, unit) = format_radon(100, Some(&settings));
        assert_eq!(value, "2.70");
        assert_eq!(unit, "pCi/L");
    }

    #[test]
    fn test_format_radon_pci_zero() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let (value, unit) = format_radon(0, Some(&settings));
        assert_eq!(value, "0.00");
        assert_eq!(unit, "pCi/L");
    }

    #[test]
    fn test_format_radon_pci_high_value() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let (value, unit) = format_radon(300, Some(&settings));
        // 300 Bq/m³ = 8.1 pCi/L
        assert_eq!(value, "8.10");
        assert_eq!(unit, "pCi/L");
    }
}
