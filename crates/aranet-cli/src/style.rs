//! Visual styling utilities for the CLI.
//!
//! This module provides consistent styling across all CLI output including:
//! - Spinners for long-running operations
//! - Color themes and thresholds
//! - Table formatting
//! - Box drawing for panels (Rich mode)
//! - Error message boxes

use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;

use crate::cli::StyleMode;

// ============================================================================
// Progress Indicators (Spinners and Progress Bars)
// ============================================================================

/// Standard spinner tick characters (Braille dots animation)
const SPINNER_TICK_CHARS: &str = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

/// Standard spinner tick interval
const SPINNER_TICK_MS: u64 = 80;

/// Standard progress bar characters
const PROGRESS_CHARS: &str = "###";

/// Get the standard spinner style.
fn spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .template("{spinner:.cyan} {msg}")
        .expect("valid template")
        .tick_chars(SPINNER_TICK_CHARS)
}

/// Get the standard progress bar style.
pub fn progress_bar_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}% {msg}")
        .expect("valid template")
        .progress_chars(PROGRESS_CHARS)
}

/// Create a spinner for scanning operations.
pub fn scanning_spinner(timeout_secs: u64) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style());
    pb.set_message(format!(
        "Scanning for Aranet devices... ({}s)",
        timeout_secs
    ));
    pb.enable_steady_tick(Duration::from_millis(SPINNER_TICK_MS));
    pb
}

/// Create a spinner for connecting to a device.
pub fn connecting_spinner(device: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style());
    pb.set_message(format!("Connecting to {}...", device));
    pb.enable_steady_tick(Duration::from_millis(SPINNER_TICK_MS));
    pb
}

/// Create a spinner for generic operations.
#[allow(dead_code)]
pub fn operation_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style());
    pb.set_message(message.to_string());
    pb.enable_steady_tick(Duration::from_millis(SPINNER_TICK_MS));
    pb
}

/// Create a progress bar for download operations.
pub fn download_progress_bar() -> ProgressBar {
    let pb = ProgressBar::new(100);
    pb.set_style(progress_bar_style());
    pb.enable_steady_tick(Duration::from_millis(SPINNER_TICK_MS));
    pb
}

/// Print a message while suspending a spinner to prevent visual glitches.
/// If no spinner is provided, just prints normally.
#[allow(dead_code)]
pub fn print_suspended(spinner: Option<&ProgressBar>, message: &str) {
    if let Some(pb) = spinner {
        pb.suspend(|| {
            eprintln!("{}", message);
        });
    } else {
        eprintln!("{}", message);
    }
}

/// Print a message to stdout while suspending a spinner.
#[allow(dead_code)]
pub fn print_suspended_stdout(spinner: Option<&ProgressBar>, message: &str) {
    if let Some(pb) = spinner {
        pb.suspend(|| {
            println!("{}", message);
        });
    } else {
        println!("{}", message);
    }
}

// ============================================================================
// Color Thresholds
// ============================================================================

/// CO2 thresholds (ppm) based on indoor air quality guidelines.
pub mod co2 {
    pub const GOOD: u16 = 800; // Green: < 800 ppm
    pub const MODERATE: u16 = 1000; // Yellow: 800-1000 ppm
    pub const POOR: u16 = 1500; // Orange: 1000-1500 ppm
    // Red: > 1500 ppm
}

/// Radon thresholds (Bq/m³) based on EPA guidelines.
/// EPA action level is 4 pCi/L = ~148 Bq/m³
pub mod radon {
    pub const GOOD: u32 = 74; // Green: < 2 pCi/L (74 Bq/m³)
    pub const MODERATE: u32 = 148; // Yellow: 2-4 pCi/L (74-148 Bq/m³)
    // Red: > 4 pCi/L (148 Bq/m³)
}

/// Battery thresholds (percentage).
pub mod battery {
    pub const LOW: u8 = 20; // Red: < 20%
    pub const MEDIUM: u8 = 40; // Yellow: 20-40%
    // Green: > 40%
}

/// Humidity thresholds (percentage) for comfort.
pub mod humidity {
    pub const LOW: u8 = 30; // Yellow: < 30% (too dry)
    pub const HIGH: u8 = 70; // Yellow: > 70% (too humid)
    // Green: 30-70%
}

/// Temperature thresholds (Celsius) for comfort.
pub mod temperature {
    pub const COLD: f32 = 18.0; // Blue: < 18°C
    pub const WARM: f32 = 26.0; // Orange: > 26°C
    // Green: 18-26°C
}

// ============================================================================
// Colored Value Formatting
// ============================================================================

/// Format CO2 value with appropriate color based on thresholds.
pub fn format_co2_colored(ppm: u16, no_color: bool) -> String {
    if no_color {
        return format!("{}", ppm);
    }

    if ppm < co2::GOOD {
        format!("{}", ppm.green())
    } else if ppm < co2::MODERATE {
        format!("{}", ppm.yellow())
    } else if ppm < co2::POOR {
        // Orange color (RGB: 255, 165, 0)
        format!("{}", ppm.truecolor(255, 165, 0))
    } else {
        format!("{}", ppm.red())
    }
}

/// Format radon value with appropriate color based on EPA thresholds.
/// This formats the raw Bq/m³ value with color coding.
pub fn format_radon_colored(bq: u32, no_color: bool) -> String {
    if no_color {
        return format!("{}", bq);
    }

    if bq < radon::GOOD {
        format!("{}", bq.green())
    } else if bq < radon::MODERATE {
        format!("{}", bq.yellow())
    } else {
        format!("{}", bq.red())
    }
}

/// Format radon value in pCi/L with appropriate color based on EPA thresholds.
/// Takes raw Bq/m³ value, converts to pCi/L, and applies color coding.
/// The color thresholds are based on the original Bq/m³ values.
pub fn format_radon_pci_colored(bq: u32, pci: f32, no_color: bool) -> String {
    if no_color {
        return format!("{:.2}", pci);
    }

    let formatted = format!("{:.2}", pci);
    if bq < radon::GOOD {
        format!("{}", formatted.green())
    } else if bq < radon::MODERATE {
        format!("{}", formatted.yellow())
    } else {
        format!("{}", formatted.red())
    }
}

/// Format battery percentage with appropriate color.
pub fn format_battery_colored(percent: u8, no_color: bool) -> String {
    if no_color {
        return format!("{}%", percent);
    }

    if percent < battery::LOW {
        format!("{}%", percent.red())
    } else if percent < battery::MEDIUM {
        format!("{}%", percent.yellow())
    } else {
        format!("{}%", percent.green())
    }
}

/// Format humidity percentage with appropriate color.
pub fn format_humidity_colored(percent: u8, no_color: bool) -> String {
    if no_color {
        return format!("{}%", percent);
    }

    if !(humidity::LOW..=humidity::HIGH).contains(&percent) {
        format!("{}%", percent.yellow())
    } else {
        format!("{}%", percent.green())
    }
}

/// Format temperature with appropriate color.
pub fn format_temp_colored(celsius: f32, no_color: bool) -> String {
    if no_color {
        return format!("{:.1}", celsius);
    }

    let formatted = format!("{:.1}", celsius);
    if celsius < temperature::COLD {
        format!("{}", formatted.cyan())
    } else if celsius > temperature::WARM {
        // Orange color (RGB: 255, 165, 0)
        format!("{}", formatted.truecolor(255, 165, 0))
    } else {
        format!("{}", formatted.green())
    }
}

// ============================================================================
// Signal Strength Bar
// ============================================================================

/// Format RSSI as a visual signal bar.
/// RSSI typically ranges from -100 dBm (weak) to -30 dBm (strong).
pub fn format_signal_bar(rssi: Option<i16>, no_color: bool) -> String {
    let rssi = match rssi {
        Some(r) => r,
        None => return "N/A".to_string(),
    };

    // Normalize RSSI to 0-10 scale
    // -30 dBm = excellent (10), -100 dBm = very weak (0)
    let strength = ((rssi + 100).clamp(0, 70) as f32 / 7.0).round() as usize;
    let filled = strength.min(10);
    let empty = 10 - filled;

    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    if no_color {
        format!("{} {:>3}", bar, rssi)
    } else if filled >= 7 {
        format!("{} {:>3}", bar.green(), rssi)
    } else if filled >= 4 {
        format!("{} {:>3}", bar.yellow(), rssi)
    } else {
        format!("{} {:>3}", bar.red(), rssi)
    }
}

// ============================================================================
// Air Quality Summary
// ============================================================================

/// Get air quality summary text based on CO2 level.
pub fn air_quality_summary(co2: u16) -> &'static str {
    if co2 < co2::GOOD {
        "Excellent"
    } else if co2 < co2::MODERATE {
        "Good"
    } else if co2 < co2::POOR {
        "Moderate"
    } else {
        "Poor"
    }
}

/// Get colored air quality summary.
pub fn air_quality_summary_colored(co2: u16, no_color: bool) -> String {
    let summary = air_quality_summary(co2);
    if no_color {
        return summary.to_string();
    }

    if co2 < co2::MODERATE {
        format!("{}", summary.green())
    } else if co2 < co2::POOR {
        format!("{}", summary.yellow())
    } else {
        format!("{}", summary.red())
    }
}

/// Get radon risk summary based on EPA guidelines.
#[allow(dead_code)]
pub fn radon_risk_summary(bq: u32) -> &'static str {
    if bq < radon::GOOD {
        "Low Risk"
    } else if bq < radon::MODERATE {
        "Moderate Risk"
    } else {
        "High Risk - Action Recommended"
    }
}

// ============================================================================
// Box Drawing / Error Formatting
// ============================================================================

/// Format an error message in a styled box.
#[allow(dead_code)]
pub fn format_error_box(title: &str, message: &str, suggestions: &[&str]) -> String {
    let width = 60;
    let border_top = format!("┌─ {} {}", title, "─".repeat(width - title.len() - 4));
    let border_bottom = format!("└{}┘", "─".repeat(width - 2));

    let mut lines = vec![border_top, "│".to_string()];

    // Wrap message
    for line in message.lines() {
        lines.push(format!("│  {}", line));
    }

    if !suggestions.is_empty() {
        lines.push("│".to_string());
        lines.push("│  Troubleshooting:".to_string());
        for (i, suggestion) in suggestions.iter().enumerate() {
            lines.push(format!("│    {}. {}", i + 1, suggestion));
        }
    }

    lines.push("│".to_string());
    lines.push(border_bottom);

    lines.join("\n")
}

/// Format a success message.
pub fn format_success(message: &str, no_color: bool) -> String {
    if no_color {
        format!("[OK] {}", message)
    } else {
        format!("{} {}", "[OK]".green(), message)
    }
}

/// Format an info message.
#[allow(dead_code)]
pub fn format_info(message: &str, no_color: bool) -> String {
    if no_color {
        format!("[--] {}", message)
    } else {
        format!("{} {}", "[--]".cyan(), message)
    }
}

/// Format a warning message.
#[allow(dead_code)]
pub fn format_warning(message: &str, no_color: bool) -> String {
    if no_color {
        format!("[!!] {}", message)
    } else {
        format!("{} {}", "[!!]".yellow(), message)
    }
}

// ============================================================================
// Trend Indicators
// ============================================================================

/// Get trend indicator comparing current and previous values.
pub fn trend_indicator(current: f32, previous: f32, no_color: bool) -> &'static str {
    let diff = current - previous;
    if diff.abs() < 0.5 {
        "-"
    } else if diff > 0.0 {
        if no_color { "^" } else { "↑" }
    } else if no_color {
        "v"
    } else {
        "↓"
    }
}

/// Get trend indicator for integer values.
pub fn trend_indicator_int(current: i32, previous: i32, no_color: bool) -> &'static str {
    let diff = current - previous;
    if diff.abs() < 5 {
        "-"
    } else if diff > 0 {
        if no_color { "^" } else { "↑" }
    } else if no_color {
        "v"
    } else {
        "↓"
    }
}

// ============================================================================
// Section Headers
// ============================================================================

/// Format a section header with device name.
#[allow(dead_code)]
pub fn format_device_header(name: &str, no_color: bool) -> String {
    let line = "─".repeat(40);
    if no_color {
        format!("── {} {}", name, line)
    } else {
        format!("── {} {}", name.cyan(), line.dimmed())
    }
}

/// Format a title header.
pub fn format_title(title: &str, no_color: bool) -> String {
    if no_color {
        format!("{}\n{}", title, "━".repeat(title.len()))
    } else {
        format!("{}\n{}", title.bold(), "━".repeat(title.len()).dimmed())
    }
}

// ============================================================================
// Rich Mode Panel Formatting
// ============================================================================

/// Get terminal width, defaulting to 80 if detection fails.
pub fn terminal_width() -> usize {
    terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80)
}

/// Format a status badge (Rich mode)
#[allow(dead_code)]
pub fn format_status_badge(
    label: &str,
    status: aranet_types::Status,
    style: StyleMode,
    no_color: bool,
) -> String {
    if style == StyleMode::Plain || no_color {
        return format!("[{}]", label);
    }

    // Rich mode: use colored background for emphasis
    match status {
        aranet_types::Status::Green => format!("[{}]", label.green().bold()),
        aranet_types::Status::Yellow => format!("[{}]", label.yellow().bold()),
        aranet_types::Status::Red => format!("[{}]", label.red().bold()),
        _ => format!("[{}]", label.dimmed()),
    }
}

/// Format a large value display (Rich mode) - for key metrics
#[allow(dead_code)]
pub fn format_large_value(value: &str, unit: &str, no_color: bool) -> String {
    if no_color {
        format!("{} {}", value, unit)
    } else {
        format!("{} {}", value.bold(), unit.dimmed())
    }
}

/// Format air quality indicator (Rich mode) - visual bar
pub fn format_air_quality_bar(co2: u16, no_color: bool) -> String {
    // Create a visual indicator based on CO2 level
    // Scale: 0-400 excellent, 400-800 good, 800-1000 moderate, 1000-1500 poor, >1500 bad
    let level = if co2 < 400 {
        5
    } else if co2 < 800 {
        4
    } else if co2 < 1000 {
        3
    } else if co2 < 1500 {
        2
    } else {
        1
    };

    let filled = "█".repeat(level);
    let empty = "░".repeat(5 - level);
    let bar = format!("{}{}", filled, empty);

    if no_color {
        bar
    } else if level >= 4 {
        format!("{}", bar.green())
    } else if level >= 3 {
        format!("{}", bar.yellow())
    } else {
        format!("{}", bar.red())
    }
}

/// Apply table style based on StyleMode.
pub fn apply_table_style(table: &mut tabled::Table, style: StyleMode) {
    use tabled::settings::Style;
    match style {
        StyleMode::Rich | StyleMode::Minimal => {
            table.with(Style::rounded());
        }
        StyleMode::Plain => {
            table.with(Style::blank());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== Threshold Constants Tests ====================
    // These are compile-time assertions to ensure threshold ordering invariants

    const _: () = {
        assert!(co2::GOOD < co2::MODERATE);
        assert!(co2::MODERATE < co2::POOR);
        assert!(radon::GOOD < radon::MODERATE);
        assert!(battery::LOW < battery::MEDIUM);
        assert!(humidity::LOW < humidity::HIGH);
        assert!(humidity::LOW > 0);
        assert!(humidity::HIGH <= 100);
        assert!(temperature::COLD < temperature::WARM);
    };

    // ==================== CO2 Formatting Tests ====================

    #[test]
    fn test_format_co2_colored_no_color() {
        assert_eq!(format_co2_colored(500, true), "500");
        assert_eq!(format_co2_colored(800, true), "800");
        assert_eq!(format_co2_colored(1500, true), "1500");
    }

    #[test]
    fn test_format_co2_colored_excellent() {
        let result = format_co2_colored(400, false);
        // Should contain ANSI escape codes for green
        assert!(!result.is_empty());
        assert!(result.contains("400"));
    }

    #[test]
    fn test_format_co2_colored_good() {
        let result = format_co2_colored(850, false);
        assert!(result.contains("850"));
    }

    #[test]
    fn test_format_co2_colored_moderate() {
        let result = format_co2_colored(1200, false);
        assert!(result.contains("1200"));
    }

    #[test]
    fn test_format_co2_colored_poor() {
        let result = format_co2_colored(2000, false);
        assert!(result.contains("2000"));
    }

    #[test]
    fn test_format_co2_colored_boundary_values() {
        // Test exact boundary values
        let _ = format_co2_colored(co2::GOOD, false);
        let _ = format_co2_colored(co2::GOOD - 1, false);
        let _ = format_co2_colored(co2::MODERATE, false);
        let _ = format_co2_colored(co2::POOR, false);
    }

    // ==================== Radon Formatting Tests ====================

    #[test]
    fn test_format_radon_colored_no_color() {
        assert_eq!(format_radon_colored(50, true), "50");
        assert_eq!(format_radon_colored(100, true), "100");
        assert_eq!(format_radon_colored(200, true), "200");
    }

    #[test]
    fn test_format_radon_colored_levels() {
        let good = format_radon_colored(50, false);
        assert!(good.contains("50"));

        let moderate = format_radon_colored(100, false);
        assert!(moderate.contains("100"));

        let high = format_radon_colored(200, false);
        assert!(high.contains("200"));
    }

    #[test]
    fn test_format_radon_pci_colored_no_color() {
        let result = format_radon_pci_colored(50, 1.35, true);
        assert_eq!(result, "1.35");
    }

    #[test]
    fn test_format_radon_pci_colored_with_color() {
        let good = format_radon_pci_colored(50, 1.35, false);
        assert!(good.contains("1.35"));

        let moderate = format_radon_pci_colored(100, 2.70, false);
        assert!(moderate.contains("2.70"));

        let high = format_radon_pci_colored(200, 5.40, false);
        assert!(high.contains("5.40"));
    }

    // ==================== Battery Formatting Tests ====================

    #[test]
    fn test_format_battery_colored_no_color() {
        assert_eq!(format_battery_colored(10, true), "10%");
        assert_eq!(format_battery_colored(50, true), "50%");
        assert_eq!(format_battery_colored(100, true), "100%");
    }

    #[test]
    fn test_format_battery_colored_levels() {
        let low = format_battery_colored(10, false);
        // Contains the number and % (possibly with ANSI codes between)
        assert!(low.contains("10"));
        assert!(low.contains('%'));

        let medium = format_battery_colored(30, false);
        assert!(medium.contains("30"));
        assert!(medium.contains('%'));

        let high = format_battery_colored(80, false);
        assert!(high.contains("80"));
        assert!(high.contains('%'));
    }

    #[test]
    fn test_format_battery_colored_boundaries() {
        let _ = format_battery_colored(battery::LOW, false);
        let _ = format_battery_colored(battery::LOW - 1, false);
        let _ = format_battery_colored(battery::MEDIUM, false);
        let _ = format_battery_colored(battery::MEDIUM + 1, false);
    }

    // ==================== Humidity Formatting Tests ====================

    #[test]
    fn test_format_humidity_colored_no_color() {
        assert_eq!(format_humidity_colored(45, true), "45%");
        assert_eq!(format_humidity_colored(20, true), "20%");
        assert_eq!(format_humidity_colored(80, true), "80%");
    }

    #[test]
    fn test_format_humidity_colored_comfort_range() {
        // In comfort range (30-70%)
        let comfortable = format_humidity_colored(50, false);
        // Contains the number and % (possibly with ANSI codes between)
        assert!(comfortable.contains("50"));
        assert!(comfortable.contains('%'));
    }

    #[test]
    fn test_format_humidity_colored_too_dry() {
        let dry = format_humidity_colored(20, false);
        assert!(dry.contains("20"));
        assert!(dry.contains('%'));
    }

    #[test]
    fn test_format_humidity_colored_too_humid() {
        let humid = format_humidity_colored(80, false);
        assert!(humid.contains("80"));
        assert!(humid.contains('%'));
    }

    #[test]
    fn test_format_humidity_colored_boundaries() {
        let _ = format_humidity_colored(humidity::LOW, false);
        let _ = format_humidity_colored(humidity::LOW - 1, false);
        let _ = format_humidity_colored(humidity::HIGH, false);
        let _ = format_humidity_colored(humidity::HIGH + 1, false);
    }

    // ==================== Temperature Formatting Tests ====================

    #[test]
    fn test_format_temp_colored_no_color() {
        assert_eq!(format_temp_colored(22.5, true), "22.5");
        assert_eq!(format_temp_colored(15.0, true), "15.0");
        assert_eq!(format_temp_colored(30.0, true), "30.0");
    }

    #[test]
    fn test_format_temp_colored_comfortable() {
        let comfortable = format_temp_colored(22.0, false);
        assert!(comfortable.contains("22.0"));
    }

    #[test]
    fn test_format_temp_colored_cold() {
        let cold = format_temp_colored(15.0, false);
        assert!(cold.contains("15.0"));
    }

    #[test]
    fn test_format_temp_colored_warm() {
        let warm = format_temp_colored(30.0, false);
        assert!(warm.contains("30.0"));
    }

    #[test]
    fn test_format_temp_colored_precision() {
        // Test decimal precision
        let result = format_temp_colored(22.567, true);
        assert_eq!(result, "22.6"); // Should round to 1 decimal
    }

    // ==================== Signal Bar Tests ====================

    #[test]
    fn test_format_signal_bar_none() {
        assert_eq!(format_signal_bar(None, true), "N/A");
        assert_eq!(format_signal_bar(None, false), "N/A");
    }

    #[test]
    fn test_format_signal_bar_strong() {
        let bar = format_signal_bar(Some(-30), true);
        assert!(bar.contains("█")); // Should have filled bars
        assert!(bar.contains("-30"));
    }

    #[test]
    fn test_format_signal_bar_weak() {
        let bar = format_signal_bar(Some(-100), true);
        assert!(bar.contains("░")); // Should have empty bars
        assert!(bar.contains("-100"));
    }

    #[test]
    fn test_format_signal_bar_medium() {
        let bar = format_signal_bar(Some(-65), false);
        assert!(bar.contains("█"));
        assert!(bar.contains("-65"));
    }

    #[test]
    fn test_format_signal_bar_extreme_values() {
        // Very strong signal
        let strong = format_signal_bar(Some(-20), true);
        assert!(strong.contains("-20"));

        // Very weak signal
        let weak = format_signal_bar(Some(-120), true);
        assert!(weak.contains("-120"));
    }

    // ==================== Air Quality Summary Tests ====================

    #[test]
    fn test_air_quality_summary_excellent() {
        assert_eq!(air_quality_summary(400), "Excellent");
        assert_eq!(air_quality_summary(0), "Excellent");
    }

    #[test]
    fn test_air_quality_summary_good() {
        assert_eq!(air_quality_summary(850), "Good");
    }

    #[test]
    fn test_air_quality_summary_moderate() {
        assert_eq!(air_quality_summary(1200), "Moderate");
    }

    #[test]
    fn test_air_quality_summary_poor() {
        assert_eq!(air_quality_summary(2000), "Poor");
        assert_eq!(air_quality_summary(10000), "Poor");
    }

    #[test]
    fn test_air_quality_summary_boundaries() {
        // Just below good
        assert_eq!(air_quality_summary(co2::GOOD - 1), "Excellent");
        // At good threshold
        assert_eq!(air_quality_summary(co2::GOOD), "Good");
        // Just below moderate
        assert_eq!(air_quality_summary(co2::MODERATE - 1), "Good");
        // At moderate threshold
        assert_eq!(air_quality_summary(co2::MODERATE), "Moderate");
    }

    #[test]
    fn test_air_quality_summary_colored_no_color() {
        assert_eq!(air_quality_summary_colored(400, true), "Excellent");
        assert_eq!(air_quality_summary_colored(850, true), "Good");
        assert_eq!(air_quality_summary_colored(1200, true), "Moderate");
        assert_eq!(air_quality_summary_colored(2000, true), "Poor");
    }

    #[test]
    fn test_air_quality_summary_colored_with_color() {
        let excellent = air_quality_summary_colored(400, false);
        assert!(excellent.contains("Excellent"));

        let poor = air_quality_summary_colored(2000, false);
        assert!(poor.contains("Poor"));
    }

    // ==================== Radon Risk Summary Tests ====================

    #[test]
    fn test_radon_risk_summary() {
        assert_eq!(radon_risk_summary(50), "Low Risk");
        assert_eq!(radon_risk_summary(100), "Moderate Risk");
        assert_eq!(radon_risk_summary(200), "High Risk - Action Recommended");
    }

    #[test]
    fn test_radon_risk_summary_boundaries() {
        assert_eq!(radon_risk_summary(radon::GOOD - 1), "Low Risk");
        assert_eq!(radon_risk_summary(radon::GOOD), "Moderate Risk");
        assert_eq!(radon_risk_summary(radon::MODERATE - 1), "Moderate Risk");
        assert_eq!(
            radon_risk_summary(radon::MODERATE),
            "High Risk - Action Recommended"
        );
    }

    // ==================== Message Formatting Tests ====================

    #[test]
    fn test_format_success_no_color() {
        let result = format_success("Test message", true);
        assert_eq!(result, "[OK] Test message");
    }

    #[test]
    fn test_format_success_with_color() {
        let result = format_success("Test message", false);
        assert!(result.contains("[OK]"));
        assert!(result.contains("Test message"));
    }

    #[test]
    fn test_format_info_no_color() {
        let result = format_info("Info message", true);
        assert_eq!(result, "[--] Info message");
    }

    #[test]
    fn test_format_info_with_color() {
        let result = format_info("Info message", false);
        assert!(result.contains("[--]"));
        assert!(result.contains("Info message"));
    }

    #[test]
    fn test_format_warning_no_color() {
        let result = format_warning("Warning message", true);
        assert_eq!(result, "[!!] Warning message");
    }

    #[test]
    fn test_format_warning_with_color() {
        let result = format_warning("Warning message", false);
        assert!(result.contains("[!!]"));
        assert!(result.contains("Warning message"));
    }

    // ==================== Trend Indicator Tests ====================

    #[test]
    fn test_trend_indicator_stable() {
        assert_eq!(trend_indicator(22.0, 22.0, true), "-");
        assert_eq!(trend_indicator(22.0, 22.0, false), "-");
        assert_eq!(trend_indicator(22.3, 22.0, true), "-"); // Diff < 0.5
    }

    #[test]
    fn test_trend_indicator_increasing() {
        assert_eq!(trend_indicator(25.0, 22.0, true), "^");
        assert_eq!(trend_indicator(25.0, 22.0, false), "↑");
    }

    #[test]
    fn test_trend_indicator_decreasing() {
        assert_eq!(trend_indicator(20.0, 25.0, true), "v");
        assert_eq!(trend_indicator(20.0, 25.0, false), "↓");
    }

    #[test]
    fn test_trend_indicator_int_stable() {
        assert_eq!(trend_indicator_int(800, 800, true), "-");
        assert_eq!(trend_indicator_int(802, 800, true), "-"); // Diff < 5
    }

    #[test]
    fn test_trend_indicator_int_increasing() {
        assert_eq!(trend_indicator_int(850, 800, true), "^");
        assert_eq!(trend_indicator_int(850, 800, false), "↑");
    }

    #[test]
    fn test_trend_indicator_int_decreasing() {
        assert_eq!(trend_indicator_int(750, 800, true), "v");
        assert_eq!(trend_indicator_int(750, 800, false), "↓");
    }

    // ==================== Error Box Tests ====================

    #[test]
    fn test_format_error_box_basic() {
        let box_str = format_error_box("Error", "Something went wrong", &[]);
        assert!(box_str.contains("Error"));
        assert!(box_str.contains("Something went wrong"));
        assert!(box_str.contains("┌"));
        assert!(box_str.contains("└"));
    }

    #[test]
    fn test_format_error_box_with_suggestions() {
        let suggestions = ["Check connection", "Try again"];
        let box_str = format_error_box("Error", "Connection failed", &suggestions);
        assert!(box_str.contains("Troubleshooting"));
        assert!(box_str.contains("1. Check connection"));
        assert!(box_str.contains("2. Try again"));
    }

    #[test]
    fn test_format_error_box_multiline_message() {
        let box_str = format_error_box("Error", "Line 1\nLine 2\nLine 3", &[]);
        assert!(box_str.contains("Line 1"));
        assert!(box_str.contains("Line 2"));
        assert!(box_str.contains("Line 3"));
    }

    // ==================== Title Formatting Tests ====================

    #[test]
    fn test_format_title_no_color() {
        let result = format_title("Test Title", true);
        assert!(result.contains("Test Title"));
        assert!(result.contains("━")); // Underline
    }

    #[test]
    fn test_format_title_with_color() {
        let result = format_title("Test Title", false);
        assert!(result.contains("Test Title"));
        assert!(result.contains("━"));
    }

    // ==================== Air Quality Bar Tests ====================

    #[test]
    fn test_format_air_quality_bar_excellent() {
        let bar = format_air_quality_bar(300, true);
        assert_eq!(bar.matches('█').count(), 5); // All filled
    }

    #[test]
    fn test_format_air_quality_bar_good() {
        let bar = format_air_quality_bar(600, true);
        assert_eq!(bar.matches('█').count(), 4);
        assert_eq!(bar.matches('░').count(), 1);
    }

    #[test]
    fn test_format_air_quality_bar_moderate() {
        let bar = format_air_quality_bar(900, true);
        assert_eq!(bar.matches('█').count(), 3);
        assert_eq!(bar.matches('░').count(), 2);
    }

    #[test]
    fn test_format_air_quality_bar_poor() {
        let bar = format_air_quality_bar(1200, true);
        assert_eq!(bar.matches('█').count(), 2);
        assert_eq!(bar.matches('░').count(), 3);
    }

    #[test]
    fn test_format_air_quality_bar_very_poor() {
        let bar = format_air_quality_bar(2000, true);
        assert_eq!(bar.matches('█').count(), 1);
        assert_eq!(bar.matches('░').count(), 4);
    }

    #[test]
    fn test_format_air_quality_bar_with_color() {
        let _ = format_air_quality_bar(300, false);
        let _ = format_air_quality_bar(900, false);
        let _ = format_air_quality_bar(2000, false);
    }

    // ==================== Terminal Width Tests ====================

    #[test]
    fn test_terminal_width() {
        let width = terminal_width();
        // Should return a reasonable value
        assert!(width >= 20);
        assert!(width <= 1000);
    }

    // ==================== Device Header Tests ====================

    #[test]
    fn test_format_device_header_no_color() {
        let result = format_device_header("Aranet4 12345", true);
        assert!(result.contains("Aranet4 12345"));
        assert!(result.contains("─"));
    }

    #[test]
    fn test_format_device_header_with_color() {
        let result = format_device_header("Aranet4 12345", false);
        assert!(result.contains("Aranet4 12345"));
    }

    // ==================== Table Style Tests ====================

    #[test]
    fn test_apply_table_style_rich() {
        use tabled::builder::Builder;

        let mut builder = Builder::default();
        builder.push_record(["Header1", "Header2"]);
        builder.push_record(["Value1", "Value2"]);
        let mut table = builder.build();

        apply_table_style(&mut table, StyleMode::Rich);
        let output = table.to_string();
        // Rich mode uses rounded style with curved corners
        assert!(output.contains("╭") || output.contains("│") || output.contains("─"));
    }

    #[test]
    fn test_apply_table_style_minimal() {
        use tabled::builder::Builder;

        let mut builder = Builder::default();
        builder.push_record(["Header1", "Header2"]);
        builder.push_record(["Value1", "Value2"]);
        let mut table = builder.build();

        apply_table_style(&mut table, StyleMode::Minimal);
        let output = table.to_string();
        // Minimal mode also uses rounded style
        assert!(output.contains("Value1"));
    }

    #[test]
    fn test_apply_table_style_plain() {
        use tabled::builder::Builder;

        let mut builder = Builder::default();
        builder.push_record(["Header1", "Header2"]);
        builder.push_record(["Value1", "Value2"]);
        let mut table = builder.build();

        apply_table_style(&mut table, StyleMode::Plain);
        let output = table.to_string();
        // Plain mode uses blank style - no border characters
        assert!(output.contains("Value1"));
        assert!(!output.contains("╭")); // No curved corners
    }

    // ==================== Progress Bar Creation Tests ====================

    #[test]
    fn test_scanning_spinner_creates_successfully() {
        let pb = scanning_spinner(30);
        // Just verify it creates without panicking
        pb.finish_and_clear();
    }

    #[test]
    fn test_connecting_spinner_creates_successfully() {
        let pb = connecting_spinner("test-device");
        pb.finish_and_clear();
    }

    #[test]
    fn test_download_progress_bar_creates_successfully() {
        let pb = download_progress_bar();
        pb.set_position(50);
        assert_eq!(pb.position(), 50);
        pb.finish_and_clear();
    }

    #[test]
    fn test_progress_bar_style_creates_successfully() {
        let style = progress_bar_style();
        // Just verify it creates without panicking
        let _ = style;
    }

    // ==================== Signal Bar Tests (additional) ====================

    #[test]
    fn test_format_signal_bar_boundary_values() {
        // Test at exact threshold boundaries
        let _ = format_signal_bar(Some(-50), true); // Strong/Good boundary
        let _ = format_signal_bar(Some(-70), true); // Good/Moderate boundary
        let _ = format_signal_bar(Some(-90), true); // Moderate/Weak boundary
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_format_co2_colored_zero() {
        let result = format_co2_colored(0, true);
        assert_eq!(result, "0");
    }

    #[test]
    fn test_format_battery_colored_zero() {
        let result = format_battery_colored(0, true);
        assert_eq!(result, "0%");
    }

    #[test]
    fn test_format_humidity_colored_zero() {
        let result = format_humidity_colored(0, true);
        assert_eq!(result, "0%");
    }

    #[test]
    fn test_format_humidity_colored_hundred() {
        let result = format_humidity_colored(100, true);
        assert_eq!(result, "100%");
    }

    #[test]
    fn test_format_temp_colored_negative() {
        let result = format_temp_colored(-5.0, true);
        assert_eq!(result, "-5.0");
    }
}
