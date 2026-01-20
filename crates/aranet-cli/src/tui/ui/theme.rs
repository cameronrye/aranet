//! Centralized theme system for the TUI.
//!
//! This module provides a consistent color palette and styling based on
//! Tailwind CSS color conventions for a modern, cohesive look.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;

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
    pub danger: Color,
    pub info: Color,

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
            danger: Color::Rgb(248, 113, 113), // red-400
            info: Color::Rgb(96, 165, 250),    // blue-400

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
            danger: Color::Rgb(220, 38, 38),  // red-600
            info: Color::Rgb(37, 99, 235),    // blue-600

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
}

/// Default border type for all blocks (rounded for modern look).
pub const BORDER_TYPE: BorderType = BorderType::Rounded;
