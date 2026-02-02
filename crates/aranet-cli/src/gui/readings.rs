//! Sensor readings display for the Aranet GUI.
//!
//! This module provides rendering for current sensor readings with styled cards.

use eframe::egui::{self, RichText};

use super::components;
use super::helpers::{format_pressure, format_radon, format_temperature};
use super::theme::Theme;
use super::types::{
    calculate_radon_averages, Co2Level, DeviceState, RadiationLevel, RadonLevel, Trend,
};

/// Render sensor readings with styled cards.
///
/// Displays the current readings from a device including CO2, radon, radiation,
/// temperature, humidity, pressure, and battery levels with appropriate color coding.
pub fn render_readings(
    ui: &mut egui::Ui,
    theme: &Theme,
    device: &DeviceState,
    temperature_unit: &str,
    pressure_unit: &str,
) {
    let reading = match device.reading.as_ref() {
        Some(r) => r,
        None => return,
    };

    components::section_header(ui, theme, "Current Readings");

    // Show cached data banner if device is offline but we have cached readings
    if device.is_showing_cached_data() {
        let is_stale = components::is_reading_stale(reading.captured_at, reading.interval);
        components::cached_data_banner(ui, theme, reading.captured_at, is_stale);
        ui.add_space(theme.spacing.md);
    }

    egui::ScrollArea::vertical().show(ui, |ui| {
        // CO2 with color-coded card (only for Aranet4)
        if reading.co2 > 0 {
            render_co2_card(ui, theme, device, reading.co2);
            ui.add_space(theme.spacing.lg);
        }

        // Radon with color-coded card (only for AranetRadon)
        if let Some(radon) = reading.radon {
            render_radon_card(ui, theme, device, radon);
            ui.add_space(theme.spacing.lg);
        }

        // Radiation with color-coded card (only for AranetRadiation)
        if let Some(rate) = reading.radiation_rate {
            render_radiation_card(ui, theme, rate, reading.radiation_total.map(|t| t as f32));
            ui.add_space(theme.spacing.lg);
        }

        // Metrics grid
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(theme.spacing.md, theme.spacing.md);

            // Temperature (use device settings for unit, fall back to app preference)
            let (temp_value, temp_unit) = format_temperature(
                reading.temperature,
                device.settings.as_ref(),
                Some(temperature_unit),
            );
            components::metric_card(
                ui,
                theme,
                "Temperature",
                &temp_value,
                temp_unit,
                device.temperature_trend(),
                theme.info,
            );

            // Humidity
            components::metric_card(
                ui,
                theme,
                "Humidity",
                &format!("{}", reading.humidity),
                "%",
                device.humidity_trend(),
                theme.info,
            );

            // Pressure (if available)
            if reading.pressure > 0.0 {
                let (pressure_value, pressure_unit_str) =
                    format_pressure(reading.pressure, pressure_unit);
                components::metric_card(
                    ui,
                    theme,
                    "Pressure",
                    &pressure_value,
                    pressure_unit_str,
                    None,
                    theme.text_secondary,
                );
            }

            // Battery
            let battery_color = theme.battery_color(reading.battery);
            components::metric_card(
                ui,
                theme,
                "Battery",
                &format!("{}", reading.battery),
                "%",
                None,
                battery_color,
            );
        });
    });
}

/// Render CO2 reading card with gauge.
fn render_co2_card(ui: &mut egui::Ui, theme: &Theme, device: &DeviceState, co2: u16) {
    let level = Co2Level::from_ppm(co2);
    let (status_text, color) = match level {
        Co2Level::Good => ("Good", theme.success),
        Co2Level::Moderate => ("Moderate", theme.warning),
        Co2Level::Poor => ("Poor", theme.caution),
        Co2Level::Bad => ("Bad", theme.danger),
    };
    let bg_color = theme.co2_bg_color(co2);

    egui::Frame::new()
        .fill(bg_color)
        .inner_margin(egui::Margin::same(theme.spacing.lg as i8))
        .corner_radius(egui::CornerRadius::same(theme.rounding.lg as u8))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
        .shadow(theme.subtle_shadow())
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().min(320.0));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("CO2")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.xs);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{}", co2))
                                    .color(color)
                                    .size(theme.typography.metric)
                                    .strong(),
                            );
                            ui.add_space(theme.spacing.xs);
                            ui.label(
                                RichText::new("ppm")
                                    .color(theme.text_muted)
                                    .size(theme.typography.body),
                            );
                            if let Some(trend) = device.co2_trend() {
                                let trend_color = match trend {
                                    Trend::Rising => theme.danger,
                                    Trend::Falling => theme.success,
                                    Trend::Stable => theme.text_muted,
                                };
                                ui.add_space(theme.spacing.sm);
                                ui.label(
                                    RichText::new(trend.indicator())
                                        .color(trend_color)
                                        .size(theme.typography.heading),
                                );
                            }
                        });
                        ui.add_space(theme.spacing.xs);
                        components::status_badge(ui, theme, status_text, color);
                    });
                });
                ui.add_space(theme.spacing.md);
                components::co2_gauge(ui, theme, co2);

                // Session statistics (if we have any readings tracked)
                if device.session_stats.co2_count > 0 {
                    ui.add_space(theme.spacing.md);
                    ui.separator();
                    ui.add_space(theme.spacing.sm);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Session:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.md);
                        ui.label(
                            RichText::new("Min:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{}",
                                device.session_stats.co2_min.unwrap_or(0)
                            ))
                            .color(theme.success)
                            .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.sm);
                        ui.label(
                            RichText::new("Max:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{}",
                                device.session_stats.co2_max.unwrap_or(0)
                            ))
                            .color(theme.danger)
                            .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.sm);
                        ui.label(
                            RichText::new("Avg:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.label(
                            RichText::new(format!("{}", device.session_stats.co2_avg().unwrap_or(0)))
                                .color(theme.warning)
                                .size(theme.typography.caption),
                        );
                    });
                }
            });
        });
}

/// Render radon reading card.
fn render_radon_card(ui: &mut egui::Ui, theme: &Theme, device: &DeviceState, radon: u32) {
    let level = RadonLevel::from_bq(radon);
    let color = theme.radon_color(radon);
    let bg_color = theme.radon_bg_color(radon);
    let (radon_value, radon_unit) = format_radon(radon, device.settings.as_ref());

    egui::Frame::new()
        .fill(bg_color)
        .inner_margin(egui::Margin::same(theme.spacing.lg as i8))
        .corner_radius(egui::CornerRadius::same(theme.rounding.lg as u8))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
        .shadow(theme.subtle_shadow())
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().min(320.0));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Radon")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.xs);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(&radon_value)
                                    .color(color)
                                    .size(theme.typography.metric)
                                    .strong(),
                            );
                            ui.add_space(theme.spacing.xs);
                            ui.label(
                                RichText::new(radon_unit)
                                    .color(theme.text_muted)
                                    .size(theme.typography.body),
                            );
                        });
                        ui.add_space(theme.spacing.xs);
                        components::status_badge(ui, theme, level.status_text(), color);
                    });
                });

                // Session statistics for radon (if we have any readings tracked)
                if device.session_stats.radon_count > 0 {
                    ui.add_space(theme.spacing.md);
                    ui.separator();
                    ui.add_space(theme.spacing.sm);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Session:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.md);
                        ui.label(
                            RichText::new("Min:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{}",
                                device.session_stats.radon_min.unwrap_or(0)
                            ))
                            .color(theme.success)
                            .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.sm);
                        ui.label(
                            RichText::new("Max:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{}",
                                device.session_stats.radon_max.unwrap_or(0)
                            ))
                            .color(theme.danger)
                            .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.sm);
                        ui.label(
                            RichText::new("Avg:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{}",
                                device.session_stats.radon_avg().unwrap_or(0)
                            ))
                            .color(theme.warning)
                            .size(theme.typography.caption),
                        );
                    });
                }

                // Historical radon averages (24h, 7d, 30d) if history is available
                if !device.history.is_empty() {
                    let (day_avg, week_avg, month_avg) = calculate_radon_averages(&device.history);
                    if day_avg.is_some() || week_avg.is_some() || month_avg.is_some() {
                        ui.add_space(theme.spacing.sm);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Averages:")
                                    .color(theme.text_muted)
                                    .size(theme.typography.caption),
                            );
                            ui.add_space(theme.spacing.sm);
                            if let Some(avg) = day_avg {
                                ui.label(
                                    RichText::new("24h:")
                                        .color(theme.text_muted)
                                        .size(theme.typography.caption),
                                );
                                ui.label(
                                    RichText::new(format!("{}", avg))
                                        .color(theme.radon_color(avg))
                                        .size(theme.typography.caption),
                                );
                                ui.add_space(theme.spacing.sm);
                            }
                            if let Some(avg) = week_avg {
                                ui.label(
                                    RichText::new("7d:")
                                        .color(theme.text_muted)
                                        .size(theme.typography.caption),
                                );
                                ui.label(
                                    RichText::new(format!("{}", avg))
                                        .color(theme.radon_color(avg))
                                        .size(theme.typography.caption),
                                );
                                ui.add_space(theme.spacing.sm);
                            }
                            if let Some(avg) = month_avg {
                                ui.label(
                                    RichText::new("30d:")
                                        .color(theme.text_muted)
                                        .size(theme.typography.caption),
                                );
                                ui.label(
                                    RichText::new(format!("{}", avg))
                                        .color(theme.radon_color(avg))
                                        .size(theme.typography.caption),
                                );
                            }
                        });
                    }
                }
            });
        });
}

/// Render radiation reading card.
fn render_radiation_card(ui: &mut egui::Ui, theme: &Theme, rate: f32, total: Option<f32>) {
    let level = RadiationLevel::from_usv(rate);
    let color = theme.radiation_color(rate);
    let bg_color = theme.radiation_bg_color(rate);

    egui::Frame::new()
        .fill(bg_color)
        .inner_margin(egui::Margin::same(theme.spacing.lg as i8))
        .corner_radius(egui::CornerRadius::same(theme.rounding.lg as u8))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
        .shadow(theme.subtle_shadow())
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width().min(320.0));
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Radiation")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.add_space(theme.spacing.xs);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{:.3}", rate))
                                    .color(color)
                                    .size(theme.typography.metric)
                                    .strong(),
                            );
                            ui.add_space(theme.spacing.xs);
                            ui.label(
                                RichText::new("uSv/h")
                                    .color(theme.text_muted)
                                    .size(theme.typography.body),
                            );
                        });
                        ui.add_space(theme.spacing.xs);
                        components::status_badge(ui, theme, level.status_text(), color);
                    });
                });
                // Show total dose if available
                if let Some(total_dose) = total {
                    ui.add_space(theme.spacing.sm);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Total Dose:")
                                .color(theme.text_muted)
                                .size(theme.typography.caption),
                        );
                        ui.label(
                            RichText::new(format!("{:.2} uSv", total_dose))
                                .color(theme.text_secondary)
                                .size(theme.typography.caption),
                        );
                    });
                }
            });
        });
}
