//! Comparison panel for side-by-side device comparison.

use eframe::egui::{self, Color32, RichText};

use crate::gui::app::AranetApp;
use crate::gui::components;
use crate::gui::helpers::{format_pressure, format_radon, format_temperature};
use crate::gui::types::ConnectionState;

impl AranetApp {
    pub(crate) fn render_comparison_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Device Comparison")
                .size(self.theme.typography.heading)
                .strong()
                .color(self.theme.text_primary),
        );
        ui.add_space(self.theme.spacing.sm);

        // Get the devices to compare
        let devices_to_compare: Vec<_> = self
            .comparison_devices
            .iter()
            .filter_map(|&idx| self.devices.get(idx).cloned())
            .collect();

        if devices_to_compare.len() < 2 {
            components::empty_state(
                ui,
                &self.theme,
                "Select 2+ Devices",
                "Click on devices in the sidebar to compare them",
            );
            return;
        }

        // Header with device names
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Comparing {} devices", devices_to_compare.len()))
                    .size(self.theme.typography.body)
                    .color(self.theme.text_secondary),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(
                        RichText::new("Exit Comparison")
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_secondary),
                    )
                    .clicked()
                {
                    self.comparison_mode = false;
                    self.comparison_devices.clear();
                }
            });
        });
        ui.add_space(self.theme.spacing.md);

        egui::ScrollArea::vertical().show(ui, |ui| {
            // Side-by-side cards using a Grid
            let card_width =
                (ui.available_width() / devices_to_compare.len() as f32) - self.theme.spacing.sm;

            ui.horizontal(|ui| {
                for device in &devices_to_compare {
                    egui::Frame::new()
                        .fill(self.theme.bg_card)
                        .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                        .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                        .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
                        .show(ui, |ui| {
                            ui.set_width(card_width.max(180.0));

                            // Device name
                            ui.label(
                                RichText::new(device.display_name())
                                    .size(self.theme.typography.subheading)
                                    .strong()
                                    .color(self.theme.text_primary),
                            );

                            // Connection status
                            let (status_text, status_color) = match &device.connection {
                                ConnectionState::Connected => ("Connected", self.theme.success),
                                ConnectionState::Connecting => {
                                    ("Connecting...", self.theme.warning)
                                }
                                ConnectionState::Reconnecting { .. } => {
                                    ("Reconnecting...", self.theme.warning)
                                }
                                ConnectionState::Disconnected => {
                                    ("Disconnected", self.theme.text_muted)
                                }
                                ConnectionState::Error(_) => ("Error", self.theme.danger),
                            };
                            ui.label(
                                RichText::new(status_text)
                                    .size(self.theme.typography.caption)
                                    .color(status_color),
                            );

                            ui.add_space(self.theme.spacing.md);
                            ui.separator();
                            ui.add_space(self.theme.spacing.sm);

                            // Readings
                            if let Some(ref reading) = device.reading {
                                // CO2 (if available)
                                if reading.co2 > 0 {
                                    self.render_comparison_metric(
                                        ui,
                                        "CO2",
                                        &format!("{}", reading.co2),
                                        "ppm",
                                        self.theme.co2_color(reading.co2),
                                    );
                                }

                                // Radon (if available)
                                if let Some(radon) = reading.radon {
                                    let (value, unit) =
                                        format_radon(radon, device.settings.as_ref());
                                    self.render_comparison_metric(
                                        ui,
                                        "Radon",
                                        &value,
                                        unit,
                                        self.theme.radon_color(radon),
                                    );
                                }

                                // Radiation (if available)
                                if let Some(rate) = reading.radiation_rate {
                                    self.render_comparison_metric(
                                        ui,
                                        "Radiation",
                                        &format!("{:.2}", rate),
                                        "µSv/h",
                                        self.theme.radiation_color(rate),
                                    );
                                }

                                // Temperature
                                let (temp_val, temp_unit) = format_temperature(
                                    reading.temperature,
                                    device.settings.as_ref(),
                                    Some(&self.gui_config.temperature_unit),
                                );
                                self.render_comparison_metric(
                                    ui,
                                    "Temperature",
                                    &temp_val,
                                    temp_unit,
                                    self.theme.chart_temperature,
                                );

                                // Humidity
                                self.render_comparison_metric(
                                    ui,
                                    "Humidity",
                                    &format!("{}", reading.humidity),
                                    "%",
                                    self.theme.chart_humidity,
                                );

                                // Pressure
                                let (pressure_val, pressure_unit) = format_pressure(
                                    reading.pressure,
                                    &self.gui_config.pressure_unit,
                                );
                                self.render_comparison_metric(
                                    ui,
                                    "Pressure",
                                    &pressure_val,
                                    pressure_unit,
                                    self.theme.text_secondary,
                                );

                                ui.add_space(self.theme.spacing.sm);
                                ui.separator();
                                ui.add_space(self.theme.spacing.xs);

                                // Battery
                                let battery_color = if reading.battery < 20 {
                                    self.theme.danger
                                } else if reading.battery < 40 {
                                    self.theme.warning
                                } else {
                                    self.theme.success
                                };
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Battery:")
                                            .size(self.theme.typography.caption)
                                            .color(self.theme.text_muted),
                                    );
                                    ui.label(
                                        RichText::new(format!("{}%", reading.battery))
                                            .size(self.theme.typography.caption)
                                            .color(battery_color),
                                    );
                                });
                            } else {
                                ui.label(
                                    RichText::new("No readings available")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.text_muted),
                                );
                            }
                        });

                    ui.add_space(self.theme.spacing.sm);
                }
            });
        });
    }

    /// Render a single metric row in the comparison view.
    fn render_comparison_metric(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        value: &str,
        unit: &str,
        color: Color32,
    ) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}:", label))
                    .size(self.theme.typography.caption)
                    .color(self.theme.text_muted),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(unit)
                        .size(self.theme.typography.caption)
                        .color(self.theme.text_muted),
                );
                ui.label(
                    RichText::new(value)
                        .size(self.theme.typography.body)
                        .strong()
                        .color(color),
                );
            });
        });
        ui.add_space(self.theme.spacing.xs);
    }
}
