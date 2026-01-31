//! Settings panel for the Aranet GUI application.
//!
//! This module contains the settings panel rendering logic, including
//! device configuration, measurement intervals, and application settings.

use aranet_core::{BluetoothRange, messages::Command};
use eframe::egui::{self, RichText};

use crate::gui::app::AranetApp;
use crate::gui::components;
use crate::gui::helpers::INTERVAL_OPTIONS;
use crate::gui::theme::Theme;
use crate::gui::types::DeviceState;

impl AranetApp {
    /// Render the settings panel with editable controls.
    pub(crate) fn render_settings_panel(&mut self, ui: &mut egui::Ui, device: &DeviceState) {
        // Header
        ui.label(
            RichText::new(format!("{} - Settings", device.display_name()))
                .size(self.theme.typography.heading)
                .strong()
                .color(self.theme.text_primary),
        );
        ui.add_space(self.theme.spacing.lg);

        // Collect commands to send after UI rendering
        let mut commands_to_send: Vec<Command> = Vec::new();

        // Device Name/Alias Section
        components::section_header(ui, &self.theme, "Device Name");

        egui::Frame::new()
            .fill(self.theme.bg_card)
            .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
            .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
            .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
            .show(ui, |ui| {
                // Check if we're editing this device's alias
                let is_editing = self
                    .alias_edit
                    .as_ref()
                    .is_some_and(|(id, _)| id == &device.id);

                // Track actions to take after UI (to avoid borrow conflicts)
                let mut should_save = false;
                let mut should_cancel = false;
                let mut should_start_edit = false;

                if is_editing {
                    // Edit mode: show text field and save/cancel buttons
                    // Get mutable reference to the alias text
                    if let Some((_, alias_text)) = &mut self.alias_edit {
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(alias_text)
                                    .hint_text("Enter device name...")
                                    .desired_width(200.0),
                            );

                            // Save on Enter
                            if response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                should_save = true;
                            }

                            ui.add_space(self.theme.spacing.sm);

                            // Save button
                            let save_btn = egui::Button::new(
                                RichText::new("Save")
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_on_accent),
                            )
                            .fill(self.theme.success);

                            if ui.add(save_btn).clicked() {
                                should_save = true;
                            }

                            // Cancel button
                            let cancel_btn = egui::Button::new(
                                RichText::new("Cancel")
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_secondary),
                            )
                            .fill(self.theme.bg_secondary);

                            if ui.add(cancel_btn).clicked() {
                                should_cancel = true;
                            }
                        });
                    }
                } else {
                    // Display mode: show current name and edit button
                    ui.horizontal(|ui| {
                        let display_name = device.name.as_deref().unwrap_or(&device.id);
                        ui.label(
                            RichText::new(display_name)
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );

                        ui.add_space(self.theme.spacing.md);

                        let edit_btn = egui::Button::new(
                            RichText::new("Rename")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        )
                        .fill(self.theme.bg_secondary);

                        ui.add_enabled_ui(!self.updating_settings, |ui| {
                            if ui.add(edit_btn).clicked() {
                                should_start_edit = true;
                            }
                        });
                    });
                }

                // Apply deferred actions
                if should_save {
                    if let Some((_, alias_text)) = &self.alias_edit {
                        let alias = if alias_text.trim().is_empty() {
                            None
                        } else {
                            Some(alias_text.trim().to_string())
                        };
                        commands_to_send.push(Command::SetAlias {
                            device_id: device.id.clone(),
                            alias,
                        });
                        self.updating_settings = true;
                    }
                    self.alias_edit = None;
                } else if should_cancel {
                    self.alias_edit = None;
                } else if should_start_edit {
                    let current_name = device.name.clone().unwrap_or_default();
                    self.alias_edit = Some((device.id.clone(), current_name));
                }

                ui.add_space(self.theme.spacing.sm);
                ui.label(
                    RichText::new("Set a friendly name for this device")
                        .size(self.theme.typography.caption)
                        .color(self.theme.text_muted),
                );
            });

        ui.add_space(self.theme.spacing.lg);

        egui::ScrollArea::vertical().show(ui, |ui| {
            // Measurement Interval Section
            if device.reading.is_some() {
                components::section_header(ui, &self.theme, "Measurement Interval");

                egui::Frame::new()
                    .fill(self.theme.bg_card)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
                    .show(ui, |ui| {
                        let current_interval =
                            device.reading.as_ref().map(|r| r.interval).unwrap_or(0);

                        ui.horizontal(|ui| {
                            for &(secs, label) in INTERVAL_OPTIONS {
                                let is_selected = current_interval == secs;
                                let (bg, text_color) = if is_selected {
                                    (self.theme.accent, self.theme.text_on_accent)
                                } else {
                                    (self.theme.bg_secondary, self.theme.text_secondary)
                                };

                                ui.add_enabled_ui(!self.updating_settings, |ui| {
                                    let btn = egui::Button::new(
                                        RichText::new(label)
                                            .size(self.theme.typography.caption)
                                            .color(text_color),
                                    )
                                    .fill(bg)
                                    .corner_radius(
                                        egui::CornerRadius::same(self.theme.rounding.sm as u8),
                                    );

                                    if ui.add(btn).clicked() && !is_selected {
                                        self.updating_settings = true;
                                        self.status = format!("Setting interval to {}...", label);
                                        commands_to_send.push(Command::SetInterval {
                                            device_id: device.id.clone(),
                                            interval_secs: secs,
                                        });
                                    }
                                });
                            }
                            if self.updating_settings {
                                ui.add_space(self.theme.spacing.sm);
                                components::loading_indicator(ui, &self.theme, None);
                            }
                        });

                        ui.add_space(self.theme.spacing.sm);
                        ui.label(
                            RichText::new("How often the sensor takes measurements")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });

                ui.add_space(self.theme.spacing.lg);
            }

            // Device Configuration Section
            if let Some(settings) = &device.settings {
                components::section_header(ui, &self.theme, "Device Configuration");

                egui::Frame::new()
                    .fill(self.theme.bg_card)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
                    .show(ui, |ui| {
                        // Smart Home toggle
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("Smart Home Integration")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_primary),
                                );
                                ui.label(
                                    RichText::new("Enable broadcasting to smart home systems")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.text_muted),
                                );
                            });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_enabled_ui(!self.updating_settings, |ui| {
                                        let current = settings.smart_home_enabled;
                                        for (val, text) in [(true, "On"), (false, "Off")] {
                                            let is_selected = current == val;
                                            let (bg, text_color) = if is_selected {
                                                (self.theme.accent, self.theme.text_on_accent)
                                            } else {
                                                (self.theme.bg_secondary, self.theme.text_secondary)
                                            };

                                            let btn = egui::Button::new(
                                                RichText::new(text)
                                                    .size(self.theme.typography.caption)
                                                    .color(text_color),
                                            )
                                            .fill(bg)
                                            .corner_radius(egui::CornerRadius::same(
                                                self.theme.rounding.sm as u8,
                                            ));

                                            if ui.add(btn).clicked() && !is_selected {
                                                self.updating_settings = true;
                                                self.status = if val {
                                                    "Enabling Smart Home...".to_string()
                                                } else {
                                                    "Disabling Smart Home...".to_string()
                                                };
                                                commands_to_send.push(Command::SetSmartHome {
                                                    device_id: device.id.clone(),
                                                    enabled: val,
                                                });
                                            }
                                        }
                                    });
                                },
                            );
                        });

                        ui.add_space(self.theme.spacing.md);

                        // Bluetooth Range toggle
                        let is_extended =
                            matches!(settings.bluetooth_range, BluetoothRange::Extended);
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("Bluetooth Range")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_primary),
                                );
                                ui.label(
                                    RichText::new("Extended range uses more battery")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.text_muted),
                                );
                            });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.add_enabled_ui(!self.updating_settings, |ui| {
                                        for (is_ext, label) in
                                            [(false, "Standard"), (true, "Extended")]
                                        {
                                            let is_selected = is_extended == is_ext;
                                            let (bg, text_color) = if is_selected {
                                                (self.theme.accent, self.theme.text_on_accent)
                                            } else {
                                                (self.theme.bg_secondary, self.theme.text_secondary)
                                            };

                                            let btn = egui::Button::new(
                                                RichText::new(label)
                                                    .size(self.theme.typography.caption)
                                                    .color(text_color),
                                            )
                                            .fill(bg)
                                            .corner_radius(egui::CornerRadius::same(
                                                self.theme.rounding.sm as u8,
                                            ));

                                            if ui.add(btn).clicked() && !is_selected {
                                                self.updating_settings = true;
                                                self.status =
                                                    format!("Setting range to {}...", label);
                                                commands_to_send.push(Command::SetBluetoothRange {
                                                    device_id: device.id.clone(),
                                                    extended: is_ext,
                                                });
                                            }
                                        }
                                    });
                                },
                            );
                        });

                        ui.add_space(self.theme.spacing.lg);
                        ui.separator();
                        ui.add_space(self.theme.spacing.md);

                        // Read-only settings grid
                        egui::Grid::new("settings_grid")
                            .num_columns(2)
                            .spacing([self.theme.spacing.xl, self.theme.spacing.sm])
                            .show(ui, |ui| {
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Temperature Unit",
                                    &format!("{:?}", settings.temperature_unit),
                                );
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Radon Unit",
                                    &format!("{:?}", settings.radon_unit),
                                );
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Buzzer",
                                    if settings.buzzer_enabled {
                                        "Enabled"
                                    } else {
                                        "Disabled"
                                    },
                                );
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Auto Calibration",
                                    if settings.auto_calibration_enabled {
                                        "Enabled"
                                    } else {
                                        "Disabled"
                                    },
                                );
                            });
                    });

                ui.add_space(self.theme.spacing.lg);
            } else if device.reading.is_none() {
                components::empty_state(
                    ui,
                    &self.theme,
                    "No Settings Available",
                    "Connect to the device to load settings",
                );
            }

            // Device Info Section
            components::section_header(ui, &self.theme, "Device Information");

            egui::Frame::new()
                .fill(self.theme.bg_card)
                .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
                .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
                .show(ui, |ui| {
                    egui::Grid::new("device_info_grid")
                        .num_columns(2)
                        .spacing([self.theme.spacing.xl, self.theme.spacing.sm])
                        .show(ui, |ui| {
                            Self::render_settings_row_static(
                                ui,
                                &self.theme,
                                "Device ID",
                                &device.id,
                            );

                            if let Some(name) = &device.name {
                                Self::render_settings_row_static(ui, &self.theme, "Name", name);
                            }

                            if let Some(device_type) = device.device_type {
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Type",
                                    &format!("{:?}", device_type),
                                );
                            }

                            if let Some(rssi) = device.rssi {
                                Self::render_settings_row_static(
                                    ui,
                                    &self.theme,
                                    "Signal Strength",
                                    &format!("{} dBm", rssi),
                                );
                            }

                            Self::render_settings_row_static(
                                ui,
                                &self.theme,
                                "History Records",
                                &format!("{}", device.history.len()),
                            );
                        });
                });

            ui.add_space(self.theme.spacing.xl);

            // Application Settings Section
            self.render_app_settings_section(ui);
        });

        // Send any queued commands
        for cmd in commands_to_send {
            self.send_command(cmd);
        }
    }

    fn render_settings_row_static(ui: &mut egui::Ui, theme: &Theme, label: &str, value: &str) {
        ui.label(
            RichText::new(label)
                .size(theme.typography.body)
                .color(theme.text_secondary),
        );
        ui.label(
            RichText::new(value)
                .size(theme.typography.body)
                .color(theme.text_primary),
        );
        ui.end_row();
    }
}
