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
