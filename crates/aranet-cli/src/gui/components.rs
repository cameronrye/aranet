//! Reusable UI components for the Aranet GUI.
//!
//! This module provides styled, consistent UI components that can be used
//! throughout the application.

use eframe::egui::{self, Color32, RichText, Sense, Ui};

use super::theme::Theme;
use super::types::Trend;

/// Render a styled metric card with value, unit, and optional trend.
pub fn metric_card(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &str,
    unit: &str,
    trend: Option<Trend>,
    accent: Color32,
) {
    egui::Frame::new()
        .fill(theme.bg_card)
        .inner_margin(egui::Margin::same(theme.spacing.card_padding as i8))
        .corner_radius(egui::CornerRadius::same(theme.rounding.md as u8))
        .stroke(egui::Stroke::new(1.0, theme.border_subtle))
        .show(ui, |ui| {
            ui.set_min_width(100.0);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(label)
                        .color(theme.text_muted)
                        .size(theme.typography.caption),
                );
                ui.add_space(theme.spacing.xs);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(value)
                            .color(accent)
                            .size(theme.typography.display)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(unit)
                            .color(theme.text_muted)
                            .size(theme.typography.body),
                    );
                    if let Some(t) = trend {
                        let trend_color = match t {
                            Trend::Rising => theme.caution,
                            Trend::Falling => theme.info,
                            Trend::Stable => theme.text_muted,
                        };
                        ui.label(
                            RichText::new(t.indicator())
                                .color(trend_color)
                                .size(theme.typography.subheading),
                        );
                    }
                });
            });
        });
}

/// Kind of empty state for visual differentiation.
#[derive(Debug, Clone, Copy, Default)]
pub enum EmptyStateKind {
    /// No data available (neutral)
    #[default]
    NoData,
    /// No matches for current filters
    NoMatch,
}

/// Render an empty state with icon and message.
pub fn empty_state(ui: &mut Ui, theme: &Theme, title: &str, description: &str) {
    empty_state_with_kind(ui, theme, title, description, EmptyStateKind::NoData);
}

/// Render an empty state with a specific visual kind.
pub fn empty_state_with_kind(
    ui: &mut Ui,
    theme: &Theme,
    title: &str,
    description: &str,
    kind: EmptyStateKind,
) {
    let (icon, icon_color, title_color) = match kind {
        EmptyStateKind::NoData => ("○", theme.text_muted, theme.text_secondary),
        EmptyStateKind::NoMatch => ("◇", theme.info, theme.text_secondary),
    };

    ui.vertical_centered(|ui| {
        ui.add_space(theme.spacing.xl * 2.0);
        ui.label(
            RichText::new(icon)
                .color(icon_color)
                .size(theme.typography.display),
        );
        ui.add_space(theme.spacing.md);
        ui.label(
            RichText::new(title)
                .color(title_color)
                .size(theme.typography.subheading)
                .strong(),
        );
        ui.add_space(theme.spacing.xs);
        ui.label(
            RichText::new(description)
                .color(theme.text_secondary)
                .size(theme.typography.body),
        );
    });
}

/// Render a section header with left accent border for visual separation.
pub fn section_header(ui: &mut Ui, theme: &Theme, title: &str) {
    ui.horizontal(|ui| {
        // Left accent bar
        let bar_height = theme.typography.subheading + 4.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, bar_height), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(1), theme.accent);
        ui.add_space(theme.spacing.sm);
        ui.label(
            RichText::new(title)
                .color(theme.text_primary)
                .size(theme.typography.subheading)
                .strong(),
        );
    });
    ui.add_space(theme.spacing.sm);
}

/// Render a styled status badge (pill-shaped).
pub fn status_badge(ui: &mut Ui, theme: &Theme, text: &str, color: Color32) {
    let bg = theme.tint_medium(color);
    egui::Frame::new()
        .fill(bg)
        .inner_margin(egui::Margin::symmetric(
            theme.spacing.sm as i8,
            theme.spacing.xs as i8,
        ))
        .corner_radius(egui::CornerRadius::same(theme.rounding.full as u8))
        .show(ui, |ui| {
            ui.label(
                RichText::new(text)
                    .color(color)
                    .size(theme.typography.caption),
            );
        });
}

/// Render a connection status indicator dot.
pub fn status_dot(ui: &mut Ui, color: Color32, tooltip: &str) -> egui::Response {
    let size = 8.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.circle_filled(rect.center(), size / 2.0, color);
    }
    response.on_hover_text(tooltip)
}

/// Render a CO2 level gauge bar with current value indicator.
pub fn co2_gauge(ui: &mut Ui, theme: &Theme, co2: u16) {
    let max_ppm = 2500.0_f32;
    let pct = (co2 as f32 / max_ppm).min(1.0);

    let available_width = ui.available_width().min(280.0);
    let bar_height = 14.0;
    let indicator_height = 20.0; // Space above bar for value indicator
    let label_height = 18.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(
            available_width,
            indicator_height + bar_height + label_height,
        ),
        Sense::hover(),
    );

    let painter = ui.painter();
    let bar_rect = egui::Rect::from_min_size(
        rect.min + egui::vec2(0.0, indicator_height),
        egui::vec2(available_width, bar_height),
    );

    // Draw zone backgrounds
    let zones = [
        (800.0 / max_ppm, theme.success),
        (200.0 / max_ppm, theme.warning),
        (500.0 / max_ppm, theme.caution),
        (1.0 - 1500.0 / max_ppm, theme.danger),
    ];
    let mut x_offset = 0.0;
    for (width_pct, color) in zones {
        let width = width_pct * available_width;
        painter.rect_filled(
            egui::Rect::from_min_size(
                bar_rect.min + egui::vec2(x_offset, 0.0),
                egui::vec2(width, bar_height),
            ),
            egui::CornerRadius::ZERO,
            color.gamma_multiply(0.2),
        );
        x_offset += width;
    }

    // Draw border
    painter.rect_stroke(
        bar_rect,
        egui::CornerRadius::same(theme.rounding.sm as u8),
        egui::Stroke::new(1.0, theme.border),
        egui::StrokeKind::Outside,
    );

    // Draw filled portion
    let fill_width = pct * available_width;
    let fill_color = theme.co2_color(co2);
    painter.rect_filled(
        egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_width, bar_height)),
        egui::CornerRadius::same(theme.rounding.sm as u8),
        fill_color.gamma_multiply(0.85),
    );

    // Draw current value indicator (triangle pointer + value label above bar)
    let indicator_x = bar_rect.min.x + pct * available_width;

    // Draw triangle pointer pointing down at the bar
    let triangle_size = 6.0;
    let triangle_y = bar_rect.min.y - 2.0;
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(indicator_x, triangle_y),
            egui::pos2(
                indicator_x - triangle_size / 2.0,
                triangle_y - triangle_size,
            ),
            egui::pos2(
                indicator_x + triangle_size / 2.0,
                triangle_y - triangle_size,
            ),
        ],
        fill_color,
        egui::Stroke::NONE,
    ));

    // Draw value label above the triangle
    painter.text(
        egui::pos2(indicator_x, triangle_y - triangle_size - 2.0),
        egui::Align2::CENTER_BOTTOM,
        format!("{}", co2),
        egui::FontId::proportional(theme.typography.caption),
        fill_color,
    );

    // Draw tick marks and labels below bar
    let label_y = bar_rect.max.y + 3.0;
    let ticks = [(800.0, "800"), (1000.0, "1k"), (1500.0, "1.5k")];
    for (ppm, label) in ticks {
        let x = bar_rect.min.x + (ppm / max_ppm) * available_width;
        painter.line_segment(
            [egui::pos2(x, bar_rect.min.y), egui::pos2(x, bar_rect.max.y)],
            egui::Stroke::new(1.0, theme.text_muted.gamma_multiply(0.4)),
        );
        painter.text(
            egui::pos2(x, label_y),
            egui::Align2::CENTER_TOP,
            label,
            egui::FontId::proportional(theme.typography.caption),
            theme.text_muted,
        );
    }
}

/// Render a loading indicator with optional message.
pub fn loading_indicator(ui: &mut Ui, theme: &Theme, message: Option<&str>) {
    ui.horizontal(|ui| {
        ui.spinner();
        if let Some(msg) = message {
            ui.add_space(theme.spacing.sm);
            ui.label(RichText::new(msg).color(theme.text_muted));
        }
    });
}

/// Render a banner for cached/offline data.
///
/// Shows a warning banner indicating that the displayed readings are from cache
/// and not live from the device, along with the timestamp of when the data was captured.
pub fn cached_data_banner(
    ui: &mut Ui,
    theme: &Theme,
    captured_at: Option<time::OffsetDateTime>,
    is_stale: bool,
) {
    let (bg_color, border_color, icon, message) = if is_stale {
        (
            theme.tint_light(theme.warning),
            theme.warning,
            "⚠",
            "Cached data - reading may be outdated",
        )
    } else {
        (
            theme.tint_light(theme.info),
            theme.info,
            "ℹ",
            "Showing cached data - device offline",
        )
    };

    egui::Frame::new()
        .fill(bg_color)
        .inner_margin(egui::Margin::symmetric(
            theme.spacing.md as i8,
            theme.spacing.sm as i8,
        ))
        .corner_radius(egui::CornerRadius::same(theme.rounding.md as u8))
        .stroke(egui::Stroke::new(2.0, border_color))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let icon_color = if is_stale { theme.warning } else { theme.info };
                ui.label(
                    RichText::new(icon)
                        .color(icon_color)
                        .size(theme.typography.subheading),
                );
                ui.add_space(theme.spacing.sm);

                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(message)
                            .color(theme.text_primary)
                            .size(theme.typography.body)
                            .strong(),
                    );

                    if let Some(ts) = captured_at {
                        let age = format_reading_age(ts);
                        ui.label(
                            RichText::new(format!("Last reading: {}", age))
                                .color(theme.text_secondary)
                                .size(theme.typography.caption),
                        );
                    }
                });
            });
        });
}

/// Format the age of a reading in human-readable form.
fn format_reading_age(captured_at: time::OffsetDateTime) -> String {
    let now = time::OffsetDateTime::now_utc();
    let duration = now - captured_at;

    let total_seconds = duration.whole_seconds();
    if total_seconds < 0 {
        return "just now".to_string();
    }

    let minutes = duration.whole_minutes();
    let hours = duration.whole_hours();
    let days = duration.whole_days();

    if days > 0 {
        if days == 1 {
            "1 day ago".to_string()
        } else {
            format!("{} days ago", days)
        }
    } else if hours > 0 {
        if hours == 1 {
            "1 hour ago".to_string()
        } else {
            format!("{} hours ago", hours)
        }
    } else if minutes > 0 {
        if minutes == 1 {
            "1 minute ago".to_string()
        } else {
            format!("{} minutes ago", minutes)
        }
    } else {
        "just now".to_string()
    }
}

/// Check if a reading is considered stale (older than threshold).
///
/// A reading is stale if it's older than 2x the measurement interval,
/// or older than 30 minutes if no interval is known.
pub fn is_reading_stale(captured_at: Option<time::OffsetDateTime>, interval_secs: u16) -> bool {
    let Some(ts) = captured_at else {
        return false; // Can't determine staleness without timestamp
    };

    let now = time::OffsetDateTime::now_utc();
    let age_secs = (now - ts).whole_seconds();

    if age_secs < 0 {
        return false;
    }

    // Stale if older than 2x the interval, or 30 minutes if no interval
    let threshold = if interval_secs > 0 {
        (interval_secs as i64) * 2
    } else {
        30 * 60 // 30 minutes default
    };

    age_secs > threshold
}
