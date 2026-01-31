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

/// Detect the system's current appearance (dark or light mode).
///
/// On macOS, this queries the system's effective appearance setting.
/// On other platforms, this returns Dark as a fallback.
#[cfg(target_os = "macos")]
pub fn detect_system_theme() -> ThemeMode {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::MainThreadMarker;

    // We need to be on the main thread to access NSApp
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::warn!("Cannot detect system theme: not on main thread");
        return ThemeMode::Dark;
    };

    let app = NSApplication::sharedApplication(mtm);

    // Get the effective appearance name
    // NSApp.effectiveAppearance.name contains the resolved appearance
    // Common values: "NSAppearanceNameAqua" (light), "NSAppearanceNameDarkAqua" (dark)
    let appearance = app.effectiveAppearance();
    // SAFETY: We're on the main thread (checked above), and the appearance object
    // is valid for the duration of this call. The name property returns a string
    // that describes the appearance.
    let name = unsafe { appearance.name() };
    let name_str = name.to_string();

    // Check if the appearance name contains "Dark"
    if name_str.contains("Dark") {
        tracing::debug!("System theme detected: Dark ({})", name_str);
        ThemeMode::Dark
    } else {
        tracing::debug!("System theme detected: Light ({})", name_str);
        ThemeMode::Light
    }
}

/// Detect the system's current appearance (dark or light mode).
///
/// On non-macOS platforms, this returns Dark as a fallback.
#[cfg(not(target_os = "macos"))]
pub fn detect_system_theme() -> ThemeMode {
    // On other platforms, we don't have a reliable way to detect system theme
    // Default to dark mode
    ThemeMode::Dark
}

/// Opacity levels for consistent transparency across the UI.
///
/// Use these named constants instead of magic numbers for alpha values.
#[derive(Debug, Clone, Copy)]
pub struct Opacity {
    /// Subtle hints and banners (15)
    pub subtle: u8,
    /// Measurement backgrounds, light tints (25)
    pub light: u8,
    /// Status badges, weak fills (35)
    pub medium: u8,
    /// Hover states, stronger tints (50)
    pub hover: u8,
    /// Selections, prominent highlights (70)
    pub strong: u8,
}

impl Default for Opacity {
    fn default() -> Self {
        Self {
            subtle: 15,
            light: 25,
            medium: 35,
            hover: 50,
            strong: 70,
        }
    }
}

/// Spacing constants for consistent layout using a 4px grid.
#[derive(Debug, Clone, Copy)]
pub struct Spacing {
    /// Extra small spacing (4px)
    pub xs: f32,
    /// Small spacing (8px)
    pub sm: f32,
    /// Medium spacing (16px)
    pub md: f32,
    /// Large spacing (24px)
    pub lg: f32,
    /// Extra large spacing (32px)
    pub xl: f32,
    /// Panel padding (16px)
    pub panel_padding: f32,
    /// Card padding (16px)
    pub card_padding: f32,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            xs: 4.0,
            sm: 8.0,
            md: 16.0,
            lg: 24.0,
            xl: 32.0,
            panel_padding: 16.0,
            card_padding: 16.0,
        }
    }
}

impl Spacing {
    /// Create compact spacing for denser layouts.
    pub fn compact() -> Self {
        Self {
            xs: 2.0,
            sm: 4.0,
            md: 8.0,
            lg: 16.0,
            xl: 24.0,
            panel_padding: 12.0,
            card_padding: 12.0,
        }
    }
}

/// Typography sizes using a harmonious 1.25 ratio scale.
#[derive(Debug, Clone, Copy)]
pub struct Typography {
    /// Caption/small text (11px)
    pub caption: f32,
    /// Body text (14px)
    pub body: f32,
    /// Subheading (18px)
    pub subheading: f32,
    /// Heading (22px)
    pub heading: f32,
    /// Large display text (28px)
    pub display: f32,
    /// Metric value text (35px)
    pub metric: f32,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            caption: 11.0,
            body: 14.0,
            subheading: 18.0,
            heading: 22.0,
            display: 28.0,
            metric: 35.0,
        }
    }
}

impl Typography {
    /// Create compact typography for denser layouts.
    pub fn compact() -> Self {
        Self {
            caption: 10.0,
            body: 12.0,
            subheading: 14.0,
            heading: 18.0,
            display: 22.0,
            metric: 28.0,
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

/// Button style variant for consistent button styling.
#[derive(Debug, Clone, Copy)]
pub struct ButtonStyle {
    /// Background fill color
    pub fill: Color32,
    /// Text/foreground color
    pub text: Color32,
    /// Border stroke (optional, use TRANSPARENT for no border)
    pub border: Color32,
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
    pub text_disabled: Color32,
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
    // Focus ring color for keyboard navigation
    pub focus_ring: Color32,
    // Disabled state colors
    pub bg_disabled: Color32,
    // Chart colors (distinct from semantic colors)
    pub chart_temperature: Color32,
    pub chart_humidity: Color32,
    pub chart_co2: Color32,
    pub chart_pressure: Color32,
    // Opacity levels
    pub opacity: Opacity,
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
            bg_primary: Color32::from_rgb(9, 9, 11), // zinc-950
            bg_secondary: Color32::from_rgb(24, 24, 27), // zinc-900
            bg_card: Color32::from_rgb(39, 39, 42),  // zinc-800
            bg_elevated: Color32::from_rgb(52, 52, 56), // zinc-700
            // Text
            text_primary: Color32::from_rgb(250, 250, 250), // zinc-50
            text_secondary: Color32::from_rgb(212, 212, 216), // zinc-300
            text_muted: Color32::from_rgb(161, 161, 170),   // zinc-400
            text_disabled: Color32::from_rgb(113, 113, 122), // zinc-500
            text_on_accent: Color32::WHITE,
            // Borders
            border: Color32::from_rgb(63, 63, 70), // zinc-700
            border_subtle: Color32::from_rgb(39, 39, 42), // zinc-800
            separator: Color32::from_rgb(52, 52, 56), // zinc-700
            // Accent (blue)
            accent: Color32::from_rgb(59, 130, 246), // blue-500
            accent_hover: Color32::from_rgb(96, 165, 250), // blue-400
            // Semantic colors
            success: Color32::from_rgb(34, 197, 94), // green-500
            warning: Color32::from_rgb(250, 204, 21), // yellow-400
            caution: Color32::from_rgb(251, 146, 60), // orange-400
            danger: Color32::from_rgb(239, 68, 68),  // red-500
            info: Color32::from_rgb(56, 189, 248),   // sky-400 (distinct from accent)
            // Focus ring
            focus_ring: Color32::from_rgb(147, 197, 253), // blue-300
            // Disabled
            bg_disabled: Color32::from_rgb(39, 39, 42), // zinc-800
            // Chart colors (distinct from semantic colors)
            chart_temperature: Color32::from_rgb(251, 191, 36), // amber-400
            chart_humidity: Color32::from_rgb(34, 211, 238),    // cyan-400
            chart_co2: Color32::from_rgb(74, 222, 128),         // green-400
            chart_pressure: Color32::from_rgb(192, 132, 252),   // purple-400
            // Opacity levels
            opacity: Opacity::default(),
            // Layout
            spacing: Spacing::default(),
            typography: Typography::default(),
            rounding: Rounding::default(),
        }
    }

    /// Create a light theme with clean neutral colors and visual depth.
    pub fn light() -> Self {
        Self {
            is_dark: false,
            // Light backgrounds with subtle hierarchy
            bg_primary: Color32::from_rgb(250, 250, 250), // neutral-50 (slight off-white)
            bg_secondary: Color32::from_rgb(244, 244, 245), // zinc-100
            bg_card: Color32::from_rgb(255, 255, 255),    // white (cards pop)
            bg_elevated: Color32::from_rgb(255, 255, 255), // white with shadow
            // Text
            text_primary: Color32::from_rgb(17, 24, 39), // gray-900
            text_secondary: Color32::from_rgb(55, 65, 81), // gray-700
            text_muted: Color32::from_rgb(107, 114, 128), // gray-500
            text_disabled: Color32::from_rgb(156, 163, 175), // gray-400
            text_on_accent: Color32::WHITE,
            // Borders
            border: Color32::from_rgb(209, 213, 219), // gray-300
            border_subtle: Color32::from_rgb(229, 231, 235), // gray-200
            separator: Color32::from_rgb(229, 231, 235), // gray-200
            // Accent (blue)
            accent: Color32::from_rgb(37, 99, 235), // blue-600
            accent_hover: Color32::from_rgb(29, 78, 216), // blue-700
            // Semantic colors
            success: Color32::from_rgb(22, 163, 74), // green-600
            warning: Color32::from_rgb(202, 138, 4), // yellow-600
            caution: Color32::from_rgb(234, 88, 12), // orange-600
            danger: Color32::from_rgb(220, 38, 38),  // red-600
            info: Color32::from_rgb(2, 132, 199),    // sky-600 (distinct from accent)
            // Focus ring
            focus_ring: Color32::from_rgb(59, 130, 246), // blue-500
            // Disabled
            bg_disabled: Color32::from_rgb(243, 244, 246), // gray-100
            // Chart colors (distinct from semantic colors)
            chart_temperature: Color32::from_rgb(217, 119, 6), // amber-600
            chart_humidity: Color32::from_rgb(8, 145, 178),    // cyan-600
            chart_co2: Color32::from_rgb(22, 163, 74),         // green-600
            chart_pressure: Color32::from_rgb(147, 51, 234),   // purple-600
            // Opacity levels
            opacity: Opacity::default(),
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

    /// Get theme for the specified mode with optional compact layout.
    pub fn for_mode_with_options(mode: ThemeMode, compact: bool) -> Self {
        let mut theme = Self::for_mode(mode);
        if compact {
            theme.spacing = Spacing::compact();
            theme.typography = Typography::compact();
        }
        theme
    }

    /// Apply compact mode to the current theme.
    pub fn with_compact(mut self, compact: bool) -> Self {
        if compact {
            self.spacing = Spacing::compact();
            self.typography = Typography::compact();
        }
        self
    }

    // -------------------------------------------------------------------------
    // Measurement-based color helpers
    // -------------------------------------------------------------------------

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

    /// Get CO2 background color (subtle for card backgrounds).
    pub fn co2_bg_color(&self, co2: u16) -> Color32 {
        self.tint_bg(self.co2_color(co2), self.opacity.light)
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

    /// Get radon background color (subtle for card backgrounds).
    pub fn radon_bg_color(&self, bq: u32) -> Color32 {
        self.tint_bg(self.radon_color(bq), self.opacity.light)
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

    /// Get radiation background color (subtle for card backgrounds).
    pub fn radiation_bg_color(&self, usv: f32) -> Color32 {
        self.tint_bg(self.radiation_color(usv), self.opacity.light)
    }

    // -------------------------------------------------------------------------
    // Color utility helpers
    // -------------------------------------------------------------------------

    /// Create a background tint from a color with specified alpha.
    pub fn tint_bg(&self, color: Color32, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
    }

    /// Create a subtle background tint (uses opacity.subtle).
    pub fn tint_subtle(&self, color: Color32) -> Color32 {
        self.tint_bg(color, self.opacity.subtle)
    }

    /// Create a light background tint (uses opacity.light).
    pub fn tint_light(&self, color: Color32) -> Color32 {
        self.tint_bg(color, self.opacity.light)
    }

    /// Create a medium background tint (uses opacity.medium).
    pub fn tint_medium(&self, color: Color32) -> Color32 {
        self.tint_bg(color, self.opacity.medium)
    }

    /// Create a hover background tint (uses opacity.hover).
    pub fn tint_hover(&self, color: Color32) -> Color32 {
        self.tint_bg(color, self.opacity.hover)
    }

    // -------------------------------------------------------------------------
    // Shadow helpers
    // -------------------------------------------------------------------------

    /// Get a card shadow for elevation effect.
    pub fn card_shadow(&self) -> Shadow {
        let alpha = if self.is_dark { 50 } else { 30 };
        Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(alpha),
        }
    }

    /// Get a subtle shadow for slight elevation.
    pub fn subtle_shadow(&self) -> Shadow {
        let alpha = if self.is_dark { 25 } else { 15 };
        Shadow {
            offset: [0, 1],
            blur: 4,
            spread: 0,
            color: Color32::from_black_alpha(alpha),
        }
    }

    /// Get a toast notification shadow.
    pub fn toast_shadow(&self) -> Shadow {
        let alpha = if self.is_dark { 60 } else { 40 };
        Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(alpha),
        }
    }

    // -------------------------------------------------------------------------
    // Button style helpers
    // -------------------------------------------------------------------------

    /// Primary button style (filled accent color).
    pub fn button_primary(&self) -> ButtonStyle {
        ButtonStyle {
            fill: self.accent,
            text: self.text_on_accent,
            border: Color32::TRANSPARENT,
        }
    }

    /// Secondary button style (outlined/subtle).
    pub fn button_secondary(&self) -> ButtonStyle {
        ButtonStyle {
            fill: self.bg_card,
            text: self.text_secondary,
            border: self.border,
        }
    }

    /// Ghost button style (transparent background).
    pub fn button_ghost(&self) -> ButtonStyle {
        ButtonStyle {
            fill: Color32::TRANSPARENT,
            text: self.text_secondary,
            border: Color32::TRANSPARENT,
        }
    }

    /// Danger button style (for destructive actions).
    pub fn button_danger(&self) -> ButtonStyle {
        ButtonStyle {
            fill: self.danger,
            text: self.text_on_accent,
            border: Color32::TRANSPARENT,
        }
    }

    /// Success button style (for confirmations).
    pub fn button_success(&self) -> ButtonStyle {
        ButtonStyle {
            fill: self.success,
            text: self.text_on_accent,
            border: Color32::TRANSPARENT,
        }
    }

    /// Disabled button style.
    pub fn button_disabled(&self) -> ButtonStyle {
        ButtonStyle {
            fill: self.bg_disabled,
            text: self.text_disabled,
            border: Color32::TRANSPARENT,
        }
    }

    // -------------------------------------------------------------------------
    // Toast styling helpers
    // -------------------------------------------------------------------------

    /// Get toast background color based on type.
    pub fn toast_bg(&self, is_success: bool, is_error: bool) -> Color32 {
        if is_error {
            if self.is_dark {
                Color32::from_rgb(127, 29, 29) // red-900
            } else {
                Color32::from_rgb(254, 226, 226) // red-100
            }
        } else if is_success {
            if self.is_dark {
                Color32::from_rgb(20, 83, 45) // green-900
            } else {
                Color32::from_rgb(220, 252, 231) // green-100
            }
        } else {
            // Info toast
            if self.is_dark {
                Color32::from_rgb(30, 58, 138) // blue-900
            } else {
                Color32::from_rgb(219, 234, 254) // blue-100
            }
        }
    }

    /// Get toast text color based on type.
    pub fn toast_text(&self, is_success: bool, is_error: bool) -> Color32 {
        if is_error {
            if self.is_dark {
                Color32::from_rgb(254, 202, 202) // red-200
            } else {
                Color32::from_rgb(153, 27, 27) // red-800
            }
        } else if is_success {
            if self.is_dark {
                Color32::from_rgb(187, 247, 208) // green-200
            } else {
                Color32::from_rgb(22, 101, 52) // green-800
            }
        } else {
            // Info toast
            if self.is_dark {
                Color32::from_rgb(191, 219, 254) // blue-200
            } else {
                Color32::from_rgb(30, 64, 175) // blue-800
            }
        }
    }

    // -------------------------------------------------------------------------
    // egui Style integration
    // -------------------------------------------------------------------------

    /// Create egui Style from this theme.
    pub fn to_style(&self) -> Style {
        Style {
            visuals: self.to_visuals(),
            spacing: eframe::egui::style::Spacing {
                item_spacing: eframe::egui::vec2(self.spacing.sm, self.spacing.sm),
                window_margin: Margin::same(self.spacing.md as i8),
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

        // Window shadow - more prominent in light mode for depth
        visuals.window_shadow = self.card_shadow();
        visuals.popup_shadow = self.card_shadow();

        // Widget styling
        visuals.widgets.noninteractive.bg_fill = self.bg_secondary;
        visuals.widgets.noninteractive.weak_bg_fill = self.bg_secondary;
        visuals.widgets.inactive.bg_fill = self.bg_card;
        visuals.widgets.inactive.weak_bg_fill = self.bg_card;
        visuals.widgets.hovered.bg_fill = self.accent_hover;
        visuals.widgets.hovered.weak_bg_fill = self.tint_hover(self.accent);
        visuals.widgets.active.bg_fill = self.accent;
        visuals.widgets.active.weak_bg_fill = self.accent;

        // Open widgets (dropdowns, etc.)
        visuals.widgets.open.bg_fill = self.bg_elevated;
        visuals.widgets.open.weak_bg_fill = self.bg_elevated;

        // Selection
        visuals.selection.bg_fill = self.tint_bg(self.accent, self.opacity.strong);
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
