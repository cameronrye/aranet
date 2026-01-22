//! Theme and styling for the Aranet GUI.
//!
//! Provides a consistent visual theme with dark/light mode support,
//! including colors, spacing, typography, and rounding constants.

use eframe::egui::{Color32, CornerRadius, Margin, Shadow, Stroke, Style, Visuals};

/// Theme mode for the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

impl ThemeMode {
    /// Toggle between light and dark mode.
    pub fn toggle(&mut self) {
        *self = match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        };
    }

    /// Get the icon/label for the current theme.
    pub fn icon(&self) -> &'static str {
        match self {
            ThemeMode::Dark => "Light Mode",
            ThemeMode::Light => "Dark Mode",
        }
    }
}

/// Spacing constants for consistent layout.
#[derive(Debug, Clone, Copy)]
pub struct Spacing {
    /// Extra small spacing (4px)
    pub xs: f32,
    /// Small spacing (8px)
    pub sm: f32,
    /// Medium spacing (12px)
    pub md: f32,
    /// Large spacing (16px)
    pub lg: f32,
    /// Extra large spacing (24px)
    pub xl: f32,
    /// Panel padding
    pub panel_padding: f32,
    /// Card padding
    pub card_padding: f32,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            xs: 4.0,
            sm: 8.0,
            md: 12.0,
            lg: 16.0,
            xl: 24.0,
            panel_padding: 16.0,
            card_padding: 14.0,
        }
    }
}

/// Typography sizes for consistent text hierarchy.
#[derive(Debug, Clone, Copy)]
pub struct Typography {
    /// Caption/small text (11px)
    pub caption: f32,
    /// Body text (14px)
    pub body: f32,
    /// Subheading (16px)
    pub subheading: f32,
    /// Heading (20px)
    pub heading: f32,
    /// Large display text (28px)
    pub display: f32,
    /// Metric value text (32px)
    pub metric: f32,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            caption: 11.0,
            body: 14.0,
            subheading: 16.0,
            heading: 20.0,
            display: 28.0,
            metric: 32.0,
        }
    }
}

/// Rounding constants for consistent corner radii.
#[derive(Debug, Clone, Copy)]
pub struct Rounding {
    /// Small rounding (4px)
    pub sm: f32,
    /// Medium rounding (8px)
    pub md: f32,
    /// Large rounding (12px)
    pub lg: f32,
    /// Full/pill rounding (999px)
    pub full: f32,
}

impl Default for Rounding {
    fn default() -> Self {
        Self {
            sm: 4.0,
            md: 8.0,
            lg: 12.0,
            full: 999.0,
        }
    }
}

/// Application color theme.
#[derive(Debug, Clone)]
pub struct Theme {
    // Theme mode
    pub is_dark: bool,
    // Background colors
    pub bg_primary: Color32,
    pub bg_secondary: Color32,
    pub bg_card: Color32,
    pub bg_elevated: Color32,
    // Text colors
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub text_on_accent: Color32,
    // Border and separator
    pub border: Color32,
    pub border_subtle: Color32,
    pub separator: Color32,
    // Accent and semantic colors
    pub accent: Color32,
    pub accent_hover: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub caution: Color32,
    pub danger: Color32,
    pub info: Color32,
    // Chart colors
    pub chart_temperature: Color32,
    pub chart_humidity: Color32,
    // Layout constants
    pub spacing: Spacing,
    pub typography: Typography,
    pub rounding: Rounding,
}

impl Theme {
    /// Create a dark theme with modern zinc/slate colors.
    pub fn dark() -> Self {
        Self {
            is_dark: true,
            // Zinc-based dark backgrounds
            bg_primary: Color32::from_rgb(9, 9, 11),      // zinc-950
            bg_secondary: Color32::from_rgb(24, 24, 27),  // zinc-900
            bg_card: Color32::from_rgb(39, 39, 42),       // zinc-800
            bg_elevated: Color32::from_rgb(52, 52, 56),   // zinc-700
            // Text
            text_primary: Color32::from_rgb(250, 250, 250),  // zinc-50
            text_secondary: Color32::from_rgb(212, 212, 216), // zinc-300
            text_muted: Color32::from_rgb(161, 161, 170),    // zinc-400
            text_on_accent: Color32::WHITE,                   // white text on accent buttons
            // Borders
            border: Color32::from_rgb(63, 63, 70),        // zinc-700
            border_subtle: Color32::from_rgb(39, 39, 42), // zinc-800
            separator: Color32::from_rgb(52, 52, 56),     // zinc-700
            // Accent (blue)
            accent: Color32::from_rgb(59, 130, 246),      // blue-500
            accent_hover: Color32::from_rgb(96, 165, 250), // blue-400
            // Semantic colors
            success: Color32::from_rgb(34, 197, 94),      // green-500
            warning: Color32::from_rgb(250, 204, 21),     // yellow-400
            caution: Color32::from_rgb(251, 146, 60),     // orange-400
            danger: Color32::from_rgb(239, 68, 68),       // red-500
            info: Color32::from_rgb(96, 165, 250),        // blue-400
            // Chart colors
            chart_temperature: Color32::from_rgb(251, 146, 60), // orange-400
            chart_humidity: Color32::from_rgb(96, 165, 250),    // blue-400
            // Layout
            spacing: Spacing::default(),
            typography: Typography::default(),
            rounding: Rounding::default(),
        }
    }

    /// Create a light theme with clean neutral colors.
    pub fn light() -> Self {
        Self {
            is_dark: false,
            // Light backgrounds
            bg_primary: Color32::from_rgb(255, 255, 255),   // white
            bg_secondary: Color32::from_rgb(249, 250, 251), // gray-50
            bg_card: Color32::from_rgb(255, 255, 255),      // white (cards pop on gray bg)
            bg_elevated: Color32::from_rgb(255, 255, 255),  // white with shadow for elevation
            // Text
            text_primary: Color32::from_rgb(17, 24, 39),    // gray-900
            text_secondary: Color32::from_rgb(55, 65, 81),  // gray-700
            text_muted: Color32::from_rgb(107, 114, 128),   // gray-500
            text_on_accent: Color32::WHITE,                  // white text on accent buttons
            // Borders
            border: Color32::from_rgb(209, 213, 219),       // gray-300
            border_subtle: Color32::from_rgb(229, 231, 235), // gray-200
            separator: Color32::from_rgb(229, 231, 235),    // gray-200
            // Accent (blue)
            accent: Color32::from_rgb(37, 99, 235),         // blue-600
            accent_hover: Color32::from_rgb(29, 78, 216),   // blue-700
            // Semantic colors
            success: Color32::from_rgb(22, 163, 74),        // green-600
            warning: Color32::from_rgb(202, 138, 4),        // yellow-600
            caution: Color32::from_rgb(234, 88, 12),        // orange-600
            danger: Color32::from_rgb(220, 38, 38),         // red-600
            info: Color32::from_rgb(37, 99, 235),           // blue-600
            // Chart colors
            chart_temperature: Color32::from_rgb(234, 88, 12), // orange-600
            chart_humidity: Color32::from_rgb(37, 99, 235),    // blue-600
            // Layout
            spacing: Spacing::default(),
            typography: Typography::default(),
            rounding: Rounding::default(),
        }
    }

    /// Get theme for the specified mode.
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
    }

    /// Get color for CO2 level.
    pub fn co2_color(&self, co2: u16) -> Color32 {
        if co2 < 800 {
            self.success
        } else if co2 < 1000 {
            self.warning
        } else if co2 < 1500 {
            self.caution
        } else {
            self.danger
        }
    }

    /// Get color for battery level.
    pub fn battery_color(&self, battery: u8) -> Color32 {
        if battery > 50 {
            self.success
        } else if battery > 20 {
            self.warning
        } else {
            self.danger
        }
    }

    /// Get CO2 background color (more subtle for card backgrounds).
    pub fn co2_bg_color(&self, co2: u16) -> Color32 {
        let base = self.co2_color(co2);
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 25)
    }

    /// Get radon color based on level (Bq/m³).
    pub fn radon_color(&self, bq: u32) -> Color32 {
        if bq < 100 {
            self.success
        } else if bq < 300 {
            self.warning
        } else {
            self.danger
        }
    }

    /// Get radon background color (more subtle for card backgrounds).
    pub fn radon_bg_color(&self, bq: u32) -> Color32 {
        let base = self.radon_color(bq);
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 25)
    }

    /// Get radiation color based on level (µSv/h).
    pub fn radiation_color(&self, usv: f32) -> Color32 {
        if usv < 0.3 {
            self.success
        } else if usv < 1.0 {
            self.warning
        } else {
            self.danger
        }
    }

    /// Get radiation background color (more subtle for card backgrounds).
    pub fn radiation_bg_color(&self, usv: f32) -> Color32 {
        let base = self.radiation_color(usv);
        Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 25)
    }

    /// Create a subtle background tint from a color.
    pub fn tint_bg(&self, color: Color32, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    }

    /// Get a card shadow for elevation effect.
    /// Adapts opacity based on light/dark mode.
    pub fn card_shadow(&self) -> Shadow {
        let alpha = if self.is_dark { 40 } else { 25 };
        Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(alpha),
        }
    }

    /// Get a subtle shadow for slight elevation.
    /// Adapts opacity based on light/dark mode.
    pub fn subtle_shadow(&self) -> Shadow {
        let alpha = if self.is_dark { 20 } else { 15 };
        Shadow {
            offset: [0, 1],
            blur: 4,
            spread: 0,
            color: Color32::from_black_alpha(alpha),
        }
    }

    /// Create egui Style from this theme.
    pub fn to_style(&self) -> Style {
        Style {
            visuals: self.to_visuals(),
            spacing: eframe::egui::style::Spacing {
                item_spacing: eframe::egui::vec2(self.spacing.sm, self.spacing.sm),
                window_margin: Margin::same(self.spacing.lg as i8),
                button_padding: eframe::egui::vec2(12.0, 6.0),
                interact_size: eframe::egui::vec2(40.0, 24.0),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Create egui Visuals from this theme.
    pub fn to_visuals(&self) -> Visuals {
        // Start from the appropriate base visuals
        let mut visuals = if self.is_dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };

        // Set dark_mode flag explicitly
        visuals.dark_mode = self.is_dark;

        // Background colors
        visuals.panel_fill = self.bg_primary;
        visuals.window_fill = self.bg_secondary;
        visuals.extreme_bg_color = self.bg_card;
        visuals.faint_bg_color = self.bg_secondary;

        // Window shadow - more prominent in light mode
        visuals.window_shadow = self.card_shadow();
        visuals.popup_shadow = self.card_shadow();

        // Widget styling
        visuals.widgets.noninteractive.bg_fill = self.bg_secondary;
        visuals.widgets.noninteractive.weak_bg_fill = self.bg_secondary;
        visuals.widgets.inactive.bg_fill = self.bg_card;
        visuals.widgets.inactive.weak_bg_fill = self.bg_card;
        visuals.widgets.hovered.bg_fill = self.accent_hover;
        visuals.widgets.hovered.weak_bg_fill = self.tint_bg(self.accent, 40);
        visuals.widgets.active.bg_fill = self.accent;
        visuals.widgets.active.weak_bg_fill = self.accent;

        // Open widgets (dropdowns, etc.)
        visuals.widgets.open.bg_fill = self.bg_elevated;
        visuals.widgets.open.weak_bg_fill = self.bg_elevated;

        // Selection
        visuals.selection.bg_fill = self.tint_bg(self.accent, if self.is_dark { 80 } else { 60 });
        visuals.selection.stroke = Stroke::new(1.0, self.accent);

        // Text/foreground strokes
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text_primary);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, self.text_secondary);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.5, self.text_on_accent);
        visuals.widgets.active.fg_stroke = Stroke::new(1.5, self.text_on_accent);
        visuals.widgets.open.fg_stroke = Stroke::new(1.0, self.text_primary);

        // Border strokes
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border_subtle);
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, self.accent);
        visuals.widgets.active.bg_stroke = Stroke::new(1.5, self.accent);
        visuals.widgets.open.bg_stroke = Stroke::new(1.0, self.border);

        // Rounding
        let rounding = CornerRadius::same(self.rounding.md as u8);
        visuals.widgets.noninteractive.corner_radius = rounding;
        visuals.widgets.inactive.corner_radius = rounding;
        visuals.widgets.hovered.corner_radius = rounding;
        visuals.widgets.active.corner_radius = rounding;
        visuals.widgets.open.corner_radius = rounding;

        // Expansion on interaction
        visuals.widgets.hovered.expansion = 1.0;
        visuals.widgets.active.expansion = 0.0;

        // Hyperlink color
        visuals.hyperlink_color = self.accent;

        // Override colors - used for things like error text
        visuals.error_fg_color = self.danger;
        visuals.warn_fg_color = self.warning;

        visuals
    }
}

