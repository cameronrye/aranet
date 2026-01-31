//! Device detail panel rendering.
//!
//! This module contains the device detail panel rendering logic,
//! showing device header, connection controls, and sensor readings.

use aranet_core::messages::Command;
use eframe::egui::{self, RichText};

use crate::gui::app::AranetApp;
use crate::gui::components;
use crate::gui::readings;
use crate::gui::types::{ConnectionState, DeviceState};

impl AranetApp {
    /// Render the device detail panel.
    pub(crate) fn render_device_panel(&self, ui: &mut egui::Ui, device: &DeviceState, idx: usize) {
        // Device header
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(device.display_name())
                        .size(self.theme.typography.heading)
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.horizontal(|ui| {
                    if let Some(device_type) = device.device_type {
                        components::status_badge(
                            ui,
                            &self.theme,
                            &format!("{:?}", device_type),
                            self.theme.info,
                        );
                    }
                    if let Some(rssi) = device.rssi {
                        let signal_color = if rssi > -60 {
                            self.theme.success
                        } else if rssi > -75 {
                            self.theme.warning
                        } else {
                            self.theme.danger
                        };
                        ui.add_space(self.theme.spacing.sm);
                        ui.label(
                            RichText::new(format!("{} dBm", rssi))
                                .size(self.theme.typography.caption)
                                .color(signal_color),
                        );
                    }

                    // Status badges for warnings
                    if let Some(ref reading) = device.reading {
                        // Low battery badge
                        if reading.battery < 20 {
                            ui.add_space(self.theme.spacing.sm);
                            let battery_color = if reading.battery < 10 {
                                self.theme.danger
                            } else {
                                self.theme.warning
                            };
                            components::status_badge(
                                ui,
                                &self.theme,
                                &format!("{}% battery", reading.battery),
                                battery_color,
                            );
                        }

                        // Stale reading badge (age > 2x interval means stale)
                        let is_stale = reading.interval > 0 && reading.age > reading.interval * 2;
                        if is_stale {
                            ui.add_space(self.theme.spacing.sm);
                            components::status_badge(
                                ui,
                                &self.theme,
                                "stale reading",
                                self.theme.caution,
                            );
                        }
                    }
                });
            });

            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::TOP),
                |ui| match &device.connection {
                    ConnectionState::Disconnected | ConnectionState::Error(_) => {
                        let btn = egui::Button::new(
                            RichText::new("Connect")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_on_accent),
                        )
                        .fill(self.theme.accent);
                        if ui.add(btn).clicked() {
                            self.send_command(Command::Connect {
                                device_id: device.id.clone(),
                            });
                        }
                    }
                    ConnectionState::Connecting => {
                        components::loading_indicator(ui, &self.theme, Some("Connecting..."));
                    }
                    ConnectionState::Reconnecting { attempt, .. } => {
                        let msg = format!("Reconnecting (attempt {})...", attempt);
                        components::loading_indicator(ui, &self.theme, Some(&msg));
                    }
                    ConnectionState::Connected => {
                        if ui
                            .add(egui::Button::new(
                                RichText::new("Refresh").size(self.theme.typography.body),
                            ))
                            .on_hover_text("Cmd+R")
                            .clicked()
                        {
                            self.send_command(Command::RefreshReading {
                                device_id: device.id.clone(),
                            });
                        }
                        ui.add_space(self.theme.spacing.sm);
                        if ui
                            .add(egui::Button::new(
                                RichText::new("Disconnect")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.danger),
                            ))
                            .clicked()
                        {
                            self.send_command(Command::Disconnect {
                                device_id: device.id.clone(),
                            });
                        }
                    }
                },
            );
        });

        ui.add_space(self.theme.spacing.lg);
        ui.separator();
        ui.add_space(self.theme.spacing.lg);

        // Readings content
        if device.reading.is_some() {
            readings::render_readings(
                ui,
                &self.theme,
                device,
                &self.gui_config.temperature_unit,
                &self.gui_config.pressure_unit,
            );
        } else if device.connection == ConnectionState::Connected {
            components::loading_indicator(ui, &self.theme, Some("Waiting for readings..."));
        } else {
            components::empty_state(
                ui,
                &self.theme,
                "No Readings",
                "Connect to the device to view sensor readings",
            );
        }

        let _ = idx;
    }
}
