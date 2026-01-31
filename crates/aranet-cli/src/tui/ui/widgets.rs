//! Reusable widget components for the TUI.
//!
//! This module provides helper functions for creating styled widget components
//! used throughout the terminal user interface.

use ratatui::prelude::*;

use aranet_core::settings::{DeviceSettings, RadonUnit, TemperatureUnit};
use aranet_types::HistoryRecord;

/// Convert Celsius to Fahrenheit.
#[inline]
fn celsius_to_fahrenheit(celsius: f32) -> f32 {
    celsius * 9.0 / 5.0 + 32.0
}

/// Convert Bq/m³ to pCi/L (1 Bq/m³ = 0.027 pCi/L).
#[inline]
fn bq_to_pci(bq: u32) -> f32 {
    bq as f32 * 0.027
}

/// Format temperature value based on device settings.
///
/// Uses the device's temperature unit setting if available, otherwise defaults to Celsius.
#[must_use]
pub fn format_temp_for_device(celsius: f32, settings: Option<&DeviceSettings>) -> String {
    let use_fahrenheit = settings
        .map(|s| s.temperature_unit == TemperatureUnit::Fahrenheit)
        .unwrap_or(false);

    if use_fahrenheit {
        format!("{:.1}°F", celsius_to_fahrenheit(celsius))
    } else {
        format!("{:.1}°C", celsius)
    }
}

/// Format radon value based on device settings.
///
/// Uses the device's radon unit setting if available, otherwise defaults to Bq/m³.
#[must_use]
pub fn format_radon_for_device(bq: u32, settings: Option<&DeviceSettings>) -> String {
    let use_pci = settings
        .map(|s| s.radon_unit == RadonUnit::PciL)
        .unwrap_or(false);

    if use_pci {
        format!("{:.2} pCi/L", bq_to_pci(bq))
    } else {
        format!("{} Bq/m³", bq)
    }
}

/// Get the radon unit string based on device settings.
#[must_use]
pub fn radon_unit_for_device(settings: Option<&DeviceSettings>) -> &'static str {
    let use_pci = settings
        .map(|s| s.radon_unit == RadonUnit::PciL)
        .unwrap_or(false);

    if use_pci { "pCi/L" } else { "Bq/m³" }
}

/// Convert radon value for display based on device settings.
///
/// Returns the value converted to the appropriate unit.
#[must_use]
pub fn convert_radon_for_device(bq: u32, settings: Option<&DeviceSettings>) -> f32 {
    let use_pci = settings
        .map(|s| s.radon_unit == RadonUnit::PciL)
        .unwrap_or(false);

    if use_pci { bq_to_pci(bq) } else { bq as f32 }
}

/// Extracts primary sensor values from history records for use in a sparkline widget.
///
/// Returns CO2 for Aranet4, radon for AranetRadon, or attempts both for unknown types.
///
/// # Arguments
///
/// * `history` - Slice of history records
/// * `device_type` - Optional device type to determine which values to extract
///
/// # Returns
///
/// A [`Vec<u64>`] containing the primary sensor values.
/// Returns an empty vector if no valid data is found.
#[must_use]
pub fn sparkline_data(
    history: &[HistoryRecord],
    device_type: Option<aranet_types::DeviceType>,
) -> Vec<u64> {
    use aranet_types::DeviceType;

    match device_type {
        Some(DeviceType::AranetRadon) => {
            // For radon devices, extract radon values
            history
                .iter()
                .filter_map(|record| record.radon)
                .map(u64::from)
                .collect()
        }
        Some(DeviceType::AranetRadiation) => {
            // For radiation devices, extract radiation rate if available
            history
                .iter()
                .filter_map(|record| record.radiation_rate)
                .map(|r| r as u64)
                .collect()
        }
        _ => {
            // For Aranet4 and others, use CO2
            history
                .iter()
                .filter(|record| record.co2 > 0)
                .map(|record| u64::from(record.co2))
                .collect()
        }
    }
}

/// Resample sparkline data to fit a target width.
///
/// If the data has fewer points than the target width, it will be upsampled
/// by repeating values to fill the space. If it has more points, it will
/// be downsampled by averaging values into buckets.
///
/// # Arguments
///
/// * `data` - The original sparkline data
/// * `target_width` - The desired number of data points (typically the screen width)
///
/// # Returns
///
/// A [`Vec<u64>`] with exactly `target_width` data points.
#[must_use]
pub fn resample_sparkline_data(data: &[u64], target_width: usize) -> Vec<u64> {
    if data.is_empty() || target_width == 0 {
        return Vec::new();
    }

    if data.len() == target_width {
        return data.to_vec();
    }

    let mut result = Vec::with_capacity(target_width);

    if data.len() < target_width {
        // Upsample: repeat values to fill the space
        // Use linear interpolation-like approach for smoother visualization
        for i in 0..target_width {
            let src_idx = i * (data.len() - 1) / (target_width - 1).max(1);
            result.push(data[src_idx.min(data.len() - 1)]);
        }
    } else {
        // Downsample: average values into buckets
        let bucket_size = data.len() as f64 / target_width as f64;
        for i in 0..target_width {
            let start = (i as f64 * bucket_size) as usize;
            let end = ((i + 1) as f64 * bucket_size) as usize;
            let end = end.min(data.len());

            if start < end {
                let sum: u64 = data[start..end].iter().sum();
                let avg = sum / (end - start) as u64;
                result.push(avg);
            } else if start < data.len() {
                result.push(data[start]);
            }
        }
    }

    result
}

/// Calculate trend indicator based on current and previous values.
/// Returns (arrow character, color) tuple.
pub fn trend_indicator(current: i32, previous: i32, threshold: i32) -> (&'static str, Color) {
    let diff = current - previous;
    if diff > threshold {
        ("↑", Color::Red) // Rising (bad for CO2)
    } else if diff < -threshold {
        ("↓", Color::Green) // Falling (good for CO2)
    } else {
        ("→", Color::DarkGray) // Stable
    }
}

/// Calculate trend for CO2 readings.
pub fn co2_trend(current: u16, previous: Option<u16>) -> Option<(&'static str, Color)> {
    previous.map(|prev| trend_indicator(current as i32, prev as i32, 20))
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
    fn test_celsius_to_fahrenheit_negative() {
        // -40 is where C and F are equal
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
        // 100 Bq/m3 = 2.7 pCi/L
        let result = bq_to_pci(100);
        assert!((result - 2.7).abs() < 0.01);
    }

    // ========================================================================
    // format_temp_for_device tests
    // ========================================================================

    #[test]
    fn test_format_temp_no_settings_defaults_celsius() {
        let result = format_temp_for_device(20.5, None);
        assert_eq!(result, "20.5°C");
    }

    #[test]
    fn test_format_temp_celsius_setting() {
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Celsius,
            ..Default::default()
        };
        let result = format_temp_for_device(20.5, Some(&settings));
        assert_eq!(result, "20.5°C");
    }

    #[test]
    fn test_format_temp_fahrenheit_setting() {
        let settings = DeviceSettings {
            temperature_unit: TemperatureUnit::Fahrenheit,
            ..Default::default()
        };
        let result = format_temp_for_device(20.0, Some(&settings));
        // 20C = 68F
        assert_eq!(result, "68.0°F");
    }

    // ========================================================================
    // format_radon_for_device tests
    // ========================================================================

    #[test]
    fn test_format_radon_no_settings_defaults_bq() {
        let result = format_radon_for_device(150, None);
        assert_eq!(result, "150 Bq/m³");
    }

    #[test]
    fn test_format_radon_bq_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::BqM3,
            ..Default::default()
        };
        let result = format_radon_for_device(150, Some(&settings));
        assert_eq!(result, "150 Bq/m³");
    }

    #[test]
    fn test_format_radon_pci_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let result = format_radon_for_device(100, Some(&settings));
        // 100 Bq/m3 = 2.70 pCi/L
        assert_eq!(result, "2.70 pCi/L");
    }

    // ========================================================================
    // resample_sparkline_data tests
    // ========================================================================

    #[test]
    fn test_resample_empty_data() {
        let result = resample_sparkline_data(&[], 10);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resample_zero_width() {
        let result = resample_sparkline_data(&[1, 2, 3], 0);
        assert!(result.is_empty());
    }

    #[test]
    fn test_resample_same_size() {
        let data = vec![1, 2, 3, 4, 5];
        let result = resample_sparkline_data(&data, 5);
        assert_eq!(result, data);
    }

    #[test]
    fn test_resample_upsample() {
        let data = vec![100, 200];
        let result = resample_sparkline_data(&data, 4);
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_resample_downsample() {
        let data = vec![100, 100, 200, 200];
        let result = resample_sparkline_data(&data, 2);
        assert_eq!(result.len(), 2);
        // Should average buckets
        assert_eq!(result[0], 100);
        assert_eq!(result[1], 200);
    }

    // ========================================================================
    // trend_indicator tests
    // ========================================================================

    #[test]
    fn test_trend_indicator_rising() {
        let (arrow, color) = trend_indicator(500, 400, 20);
        assert_eq!(arrow, "↑");
        assert_eq!(color, Color::Red);
    }

    #[test]
    fn test_trend_indicator_falling() {
        let (arrow, color) = trend_indicator(400, 500, 20);
        assert_eq!(arrow, "↓");
        assert_eq!(color, Color::Green);
    }

    #[test]
    fn test_trend_indicator_stable() {
        let (arrow, color) = trend_indicator(500, 505, 20);
        assert_eq!(arrow, "→");
        assert_eq!(color, Color::DarkGray);
    }

    // ========================================================================
    // co2_trend tests
    // ========================================================================

    #[test]
    fn test_co2_trend_no_previous() {
        let result = co2_trend(800, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_co2_trend_rising() {
        let result = co2_trend(850, Some(800));
        assert!(result.is_some());
        let (arrow, color) = result.unwrap();
        assert_eq!(arrow, "↑");
        assert_eq!(color, Color::Red);
    }

    #[test]
    fn test_co2_trend_falling() {
        let result = co2_trend(750, Some(800));
        assert!(result.is_some());
        let (arrow, color) = result.unwrap();
        assert_eq!(arrow, "↓");
        assert_eq!(color, Color::Green);
    }

    #[test]
    fn test_co2_trend_stable() {
        let result = co2_trend(805, Some(800));
        assert!(result.is_some());
        let (arrow, _) = result.unwrap();
        assert_eq!(arrow, "→");
    }

    // ========================================================================
    // radon_unit_for_device tests
    // ========================================================================

    #[test]
    fn test_radon_unit_no_settings() {
        let result = radon_unit_for_device(None);
        assert_eq!(result, "Bq/m³");
    }

    #[test]
    fn test_radon_unit_bq_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::BqM3,
            ..Default::default()
        };
        let result = radon_unit_for_device(Some(&settings));
        assert_eq!(result, "Bq/m³");
    }

    #[test]
    fn test_radon_unit_pci_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let result = radon_unit_for_device(Some(&settings));
        assert_eq!(result, "pCi/L");
    }

    // ========================================================================
    // convert_radon_for_device tests
    // ========================================================================

    #[test]
    fn test_convert_radon_no_settings() {
        let result = convert_radon_for_device(100, None);
        assert_eq!(result, 100.0);
    }

    #[test]
    fn test_convert_radon_bq_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::BqM3,
            ..Default::default()
        };
        let result = convert_radon_for_device(100, Some(&settings));
        assert_eq!(result, 100.0);
    }

    #[test]
    fn test_convert_radon_pci_setting() {
        let settings = DeviceSettings {
            radon_unit: RadonUnit::PciL,
            ..Default::default()
        };
        let result = convert_radon_for_device(100, Some(&settings));
        // 100 Bq/m3 = 2.7 pCi/L
        assert!((result - 2.7).abs() < 0.01);
    }

    // ========================================================================
    // sparkline_data tests
    // ========================================================================

    #[test]
    fn test_sparkline_data_empty() {
        let result = sparkline_data(&[], None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_sparkline_data_aranet4() {
        use aranet_types::DeviceType;
        use time::OffsetDateTime;

        let history = vec![
            HistoryRecord {
                timestamp: OffsetDateTime::now_utc(),
                co2: 800,
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.0,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: OffsetDateTime::now_utc(),
                co2: 850,
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.0,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        let result = sparkline_data(&history, Some(DeviceType::Aranet4));
        assert_eq!(result, vec![800, 850]);
    }

    #[test]
    fn test_sparkline_data_radon() {
        use aranet_types::DeviceType;
        use time::OffsetDateTime;

        let history = vec![
            HistoryRecord {
                timestamp: OffsetDateTime::now_utc(),
                co2: 0,
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.0,
                radon: Some(100),
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: OffsetDateTime::now_utc(),
                co2: 0,
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.0,
                radon: Some(150),
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        let result = sparkline_data(&history, Some(DeviceType::AranetRadon));
        assert_eq!(result, vec![100, 150]);
    }

    #[test]
    fn test_sparkline_data_filters_zero_co2() {
        use time::OffsetDateTime;

        let history = vec![
            HistoryRecord {
                timestamp: OffsetDateTime::now_utc(),
                co2: 0, // Should be filtered out
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.0,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
            HistoryRecord {
                timestamp: OffsetDateTime::now_utc(),
                co2: 800,
                temperature: 22.5,
                humidity: 45,
                pressure: 1013.0,
                radon: None,
                radiation_rate: None,
                radiation_total: None,
            },
        ];

        let result = sparkline_data(&history, None);
        assert_eq!(result, vec![800]); // Zero CO2 filtered out
    }
}
