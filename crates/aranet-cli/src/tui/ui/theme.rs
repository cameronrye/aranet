//! Centralized theme system for the TUI.

use aranet_types::Status;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;

use crate::tui::app::ConnectionStatus;

/// Application theme with all UI colors.
///
/// Colors are based on the Tailwind CSS palette for consistency.
#[derive(Debug, Clone, Copy)]
pub struct AppTheme {
    // Primary colors
    pub primary: Color,

    // Status colors
    pub success: Color,
    pub warning: Color,
    pub caution: Color,
    pub danger: Color,
    pub info: Color,

    // Sensor/series colors
    pub sensor_temperature: Color,
    pub sensor_humidity: Color,
    pub sensor_pressure: Color,
    pub sensor_radiation: Color,
    pub series_co2: Color,
    pub series_radon: Color,
    pub series_radiation: Color,

    // Trend and signal colors
    pub trend_rising: Color,
    pub trend_falling: Color,
    pub trend_stable: Color,
    pub signal_excellent: Color,
    pub signal_good: Color,
    pub signal_fair: Color,
    pub signal_weak: Color,
    pub signal_offline: Color,

    // Text colors
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,

    // Border colors
    pub border_active: Color,
    pub border_inactive: Color,

    // Background colors
    pub bg_selected: Color,
    pub bg_header: Color,
}

impl Default for AppTheme {
    fn default() -> Self {
        Self::dark()
    }
}

impl AppTheme {
    /// Dark theme using Tailwind-inspired colors.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            // Primary: Cyan/Teal
            primary: Color::Rgb(34, 211, 238), // cyan-400

            // Status colors
            success: Color::Rgb(74, 222, 128), // green-400
            warning: Color::Rgb(251, 191, 36), // amber-400
            caution: Color::Rgb(251, 146, 60), // orange-400
            danger: Color::Rgb(248, 113, 113), // red-400
            info: Color::Rgb(96, 165, 250),    // blue-400

            // Sensor/series colors
            sensor_temperature: Color::Rgb(251, 191, 36), // amber-400
            sensor_humidity: Color::Rgb(96, 165, 250),    // blue-400
            sensor_pressure: Color::Rgb(248, 250, 252),   // slate-50
            sensor_radiation: Color::Rgb(217, 70, 239),   // fuchsia-500
            series_co2: Color::Rgb(74, 222, 128),         // green-400
            series_radon: Color::Rgb(34, 211, 238),       // cyan-400
            series_radiation: Color::Rgb(217, 70, 239),   // fuchsia-500

            // Trend and signal colors
            trend_rising: Color::Rgb(248, 113, 113), // red-400
            trend_falling: Color::Rgb(74, 222, 128), // green-400
            trend_stable: Color::Rgb(100, 116, 139), // slate-500
            signal_excellent: Color::Rgb(74, 222, 128), // green-400
            signal_good: Color::Rgb(74, 222, 128),   // green-400
            signal_fair: Color::Rgb(251, 191, 36),   // amber-400
            signal_weak: Color::Rgb(248, 113, 113),  // red-400
            signal_offline: Color::Rgb(100, 116, 139), // slate-500

            // Text
            text_primary: Color::Rgb(248, 250, 252), // slate-50
            text_secondary: Color::Rgb(148, 163, 184), // slate-400
            text_muted: Color::Rgb(100, 116, 139),   // slate-500

            // Borders
            border_active: Color::Rgb(34, 211, 238), // cyan-400
            border_inactive: Color::Rgb(71, 85, 105), // slate-600

            // Backgrounds
            bg_selected: Color::Rgb(51, 65, 85), // slate-700
            bg_header: Color::Rgb(30, 41, 59),   // slate-800
        }
    }

    /// Light theme using Tailwind-inspired colors.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            // Primary: Cyan/Teal (darker for light theme)
            primary: Color::Rgb(6, 182, 212), // cyan-500

            // Status colors (darker for readability)
            success: Color::Rgb(22, 163, 74), // green-600
            warning: Color::Rgb(217, 119, 6), // amber-600
            caution: Color::Rgb(234, 88, 12), // orange-600
            danger: Color::Rgb(220, 38, 38),  // red-600
            info: Color::Rgb(37, 99, 235),    // blue-600

            // Sensor/series colors
            sensor_temperature: Color::Rgb(217, 119, 6), // amber-600
            sensor_humidity: Color::Rgb(37, 99, 235),    // blue-600
            sensor_pressure: Color::Rgb(15, 23, 42),     // slate-900
            sensor_radiation: Color::Rgb(147, 51, 234),  // violet-600
            series_co2: Color::Rgb(22, 163, 74),         // green-600
            series_radon: Color::Rgb(8, 145, 178),       // cyan-600
            series_radiation: Color::Rgb(147, 51, 234),  // violet-600

            // Trend and signal colors
            trend_rising: Color::Rgb(220, 38, 38),   // red-600
            trend_falling: Color::Rgb(22, 163, 74),  // green-600
            trend_stable: Color::Rgb(148, 163, 184), // slate-400
            signal_excellent: Color::Rgb(22, 163, 74), // green-600
            signal_good: Color::Rgb(22, 163, 74),    // green-600
            signal_fair: Color::Rgb(217, 119, 6),    // amber-600
            signal_weak: Color::Rgb(220, 38, 38),    // red-600
            signal_offline: Color::Rgb(148, 163, 184), // slate-400

            // Text (dark for light backgrounds)
            text_primary: Color::Rgb(15, 23, 42),    // slate-900
            text_secondary: Color::Rgb(71, 85, 105), // slate-600
            text_muted: Color::Rgb(148, 163, 184),   // slate-400

            // Borders
            border_active: Color::Rgb(6, 182, 212), // cyan-500
            border_inactive: Color::Rgb(203, 213, 225), // slate-300

            // Backgrounds
            bg_selected: Color::Rgb(226, 232, 240), // slate-200
            bg_header: Color::Rgb(241, 245, 249),   // slate-100
        }
    }

    // Style helpers

    /// Style for active/focused borders.
    #[inline]
    #[must_use]
    pub fn border_active_style(&self) -> Style {
        Style::default().fg(self.border_active)
    }

    /// Style for inactive borders.
    #[inline]
    #[must_use]
    pub fn border_inactive_style(&self) -> Style {
        Style::default().fg(self.border_inactive)
    }

    /// Style for selected items (inverted/highlighted).
    #[inline]
    #[must_use]
    pub fn selected_style(&self) -> Style {
        Style::default()
            .bg(self.bg_selected)
            .fg(self.text_primary)
            .add_modifier(Modifier::BOLD)
    }

    /// Style for titles.
    #[inline]
    #[must_use]
    pub fn title_style(&self) -> Style {
        Style::default()
            .fg(self.primary)
            .add_modifier(Modifier::BOLD)
    }

    /// Style for header/app bar.
    #[inline]
    #[must_use]
    pub fn header_style(&self) -> Style {
        Style::default().bg(self.bg_header)
    }

    /// Semantic color for CO2 levels.
    #[must_use]
    pub fn co2_level_color(&self, ppm: u16) -> Color {
        match ppm {
            0..=800 => self.success,
            801..=1000 => self.warning,
            1001..=1500 => self.caution,
            _ => self.danger,
        }
    }

    /// Semantic color for radon levels.
    #[must_use]
    pub fn radon_level_color(&self, bq_m3: u32) -> Color {
        match bq_m3 {
            0..=100 => self.success,
            101..=150 => self.warning,
            151..=300 => self.caution,
            _ => self.danger,
        }
    }

    /// Semantic color for battery levels.
    #[must_use]
    pub fn battery_level_color(&self, percent: u8) -> Color {
        match percent {
            0..=20 => self.danger,
            21..=50 => self.warning,
            _ => self.success,
        }
    }

    /// Semantic color for a sensor-reported status.
    #[must_use]
    pub fn sensor_status_color(&self, status: &Status) -> Color {
        match status {
            Status::Green => self.success,
            Status::Yellow => self.warning,
            Status::Red => self.danger,
            Status::Error => self.signal_offline,
            _ => self.signal_offline,
        }
    }

    /// Semantic color for connection state.
    #[must_use]
    pub fn connection_color(&self, status: &ConnectionStatus) -> Color {
        match status {
            ConnectionStatus::Disconnected => self.signal_offline,
            ConnectionStatus::Connecting => self.warning,
            ConnectionStatus::Connected => self.success,
            ConnectionStatus::Error(_) => self.danger,
        }
    }

    /// Signal bars and color for RSSI strength.
    #[must_use]
    pub fn signal_strength_display(&self, rssi: i16) -> (&'static str, Color) {
        if rssi >= -50 {
            ("▂▄▆█", self.signal_excellent)
        } else if rssi >= -60 {
            ("▂▄▆░", self.signal_good)
        } else if rssi >= -70 {
            ("▂▄░░", self.signal_fair)
        } else if rssi >= -80 {
            ("▂░░░", self.signal_weak)
        } else {
            ("░░░░", self.signal_offline)
        }
    }

    /// Semantic color for trend direction.
    #[must_use]
    pub fn trend_color(&self, diff: i32, threshold: i32) -> Color {
        if diff > threshold {
            self.trend_rising
        } else if diff < -threshold {
            self.trend_falling
        } else {
            self.trend_stable
        }
    }
}

/// Default border type for all blocks (rounded for modern look).
pub const BORDER_TYPE: BorderType = BorderType::Rounded;
