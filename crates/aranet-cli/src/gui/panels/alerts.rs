//! Alert history popup rendering.
//!
//! This module contains the alert history popup rendering logic,
//! displaying past alerts with severity levels and timestamps.

use eframe::egui::{self, RichText};

use crate::gui::app::AranetApp;
use crate::gui::types::AlertSeverity;

impl AranetApp {
    /// Render the alert history popup.
    pub(crate) fn render_alert_history_popup(&mut self, ctx: &egui::Context) {
        egui::Window::new("Alert History")
            .collapsible(false)
            .resizable(true)
            .default_width(400.0)
            .default_height(300.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                // Header with close button
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} alerts", self.alert_history.len()))
                            .color(self.theme.text_muted)
                            .size(self.theme.typography.caption),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Clear All")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.text_secondary),
                                )
                                .fill(self.theme.bg_secondary),
                            )
                            .clicked()
                        {
                            self.alert_history.clear();
                        }

                        ui.add_space(self.theme.spacing.sm);

                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Close")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.text_on_accent),
                                )
                                .fill(self.theme.accent),
                            )
                            .clicked()
                        {
                            self.alert_history_visible = false;
                        }
                    });
                });

                ui.separator();

                if self.alert_history.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(self.theme.spacing.xl);
                        ui.label(
                            RichText::new("No alerts yet")
                                .color(self.theme.text_muted)
                                .size(self.theme.typography.body),
                        );
                        ui.label(
                            RichText::new("Alerts will appear when thresholds are exceeded")
                                .color(self.theme.text_muted)
                                .size(self.theme.typography.caption),
                        );
                    });
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(250.0)
                        .show(ui, |ui| {
                            for alert in &self.alert_history {
                                let (severity_color, severity_bg) = match alert.severity {
                                    AlertSeverity::Info => {
                                        (self.theme.info, self.theme.tint_bg(self.theme.info, 15))
                                    }
                                    AlertSeverity::Warning => (
                                        self.theme.warning,
                                        self.theme.tint_bg(self.theme.warning, 15),
                                    ),
                                    AlertSeverity::Critical => (
                                        self.theme.danger,
                                        self.theme.tint_bg(self.theme.danger, 15),
                                    ),
                                };

                                egui::Frame::new()
                                    .fill(severity_bg)
                                    .inner_margin(egui::Margin::same(self.theme.spacing.sm as i8))
                                    .corner_radius(egui::CornerRadius::same(
                                        self.theme.rounding.sm as u8,
                                    ))
                                    .stroke(egui::Stroke::new(
                                        1.0,
                                        severity_color.gamma_multiply(0.3),
                                    ))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            // Severity icon
                                            ui.label(
                                                RichText::new(alert.severity.icon())
                                                    .color(severity_color)
                                                    .size(self.theme.typography.body)
                                                    .strong(),
                                            );

                                            ui.vertical(|ui| {
                                                ui.horizontal(|ui| {
                                                    // Time and device
                                                    ui.label(
                                                        RichText::new(&alert.time_str)
                                                            .color(self.theme.text_secondary)
                                                            .size(self.theme.typography.caption),
                                                    );
                                                    ui.label(
                                                        RichText::new("-")
                                                            .color(self.theme.text_muted)
                                                            .size(self.theme.typography.caption),
                                                    );
                                                    ui.label(
                                                        RichText::new(&alert.device_name)
                                                            .color(self.theme.text_primary)
                                                            .size(self.theme.typography.caption)
                                                            .strong(),
                                                    );

                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.label(
                                                                RichText::new(alert.age_str())
                                                                    .color(self.theme.text_muted)
                                                                    .size(
                                                                        self.theme
                                                                            .typography
                                                                            .caption,
                                                                    ),
                                                            );
                                                        },
                                                    );
                                                });

                                                ui.label(
                                                    RichText::new(&alert.message)
                                                        .color(self.theme.text_secondary)
                                                        .size(self.theme.typography.caption),
                                                );
                                            });
                                        });
                                    });

                                ui.add_space(self.theme.spacing.xs);
                            }
                        });
                }
            });
    }
}
