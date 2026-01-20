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
