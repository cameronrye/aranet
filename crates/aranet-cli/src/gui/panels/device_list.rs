//! Device list side panel rendering.
//!
//! This module contains the device list sidebar rendering logic,
//! including device filtering, sorting, and selection.

use eframe::egui::{self, Color32, RichText};

use aranet_core::DeviceType;
use aranet_core::messages::Command;

use crate::gui::app::AranetApp;
use crate::gui::components;
use crate::gui::helpers::{format_radon, format_temperature};
use crate::gui::types::{ConnectionFilter, ConnectionState, DeviceTypeFilter};

impl AranetApp {
    /// Render the device list side panel.
    pub(crate) fn render_device_list(&mut self, ctx: &egui::Context) {
        // Collapsed sidebar - just show a thin expand button
        if self.sidebar_collapsed {
            egui::SidePanel::left("devices_collapsed")
                .exact_width(40.0)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(self.theme.bg_secondary)
                        .inner_margin(egui::Margin::symmetric(4, 8))
                        .stroke(egui::Stroke::new(1.0, self.theme.border_subtle)),
                )
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        // Expand button
                        let expand_btn = egui::Button::new(
                            RichText::new(">")
                                .size(16.0)
                                .color(self.theme.text_secondary),
                        )
                        .fill(Color32::TRANSPARENT)
                        .frame(false);
                        if ui
                            .add(expand_btn)
                            .on_hover_text("Expand sidebar ([)")
                            .clicked()
                        {
                            self.sidebar_collapsed = false;
                            self.gui_config.sidebar_collapsed = false;
                            self.save_gui_config();
                        }

                        ui.add_space(8.0);

                        // Show device count badge
                        let connected_count = self
                            .devices
                            .iter()
                            .filter(|d| matches!(d.connection, ConnectionState::Connected))
                            .count();
                        if connected_count > 0 {
                            ui.label(
                                RichText::new(format!("{}", connected_count))
                                    .size(12.0)
                                    .color(self.theme.success),
                            )
                            .on_hover_text(format!("{} connected", connected_count));
                        } else {
                            ui.label(
                                RichText::new(format!("{}", self.devices.len()))
                                    .size(12.0)
                                    .color(self.theme.text_muted),
                            )
                            .on_hover_text(format!("{} devices", self.devices.len()));
                        }
                    });
                });
            return;
        }

        // Full sidebar
        egui::SidePanel::left("devices")
            .exact_width(280.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg_secondary)
                    .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle)),
            )
            .show(ctx, |ui| {
                // Header with collapse button
                ui.horizontal(|ui| {
                    // Collapse button
                    let collapse_btn = egui::Button::new(
                        RichText::new("<")
                            .size(14.0)
                            .color(self.theme.text_secondary),
                    )
                    .fill(Color32::TRANSPARENT)
                    .frame(false);
                    if ui
                        .add(collapse_btn)
                        .on_hover_text("Collapse sidebar ([)")
                        .clicked()
                    {
                        self.sidebar_collapsed = true;
                        self.gui_config.sidebar_collapsed = true;
                        self.save_gui_config();
                    }

                    ui.label(
                        RichText::new("Devices")
                            .size(self.theme.typography.subheading)
                            .strong()
                            .color(self.theme.text_primary),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{}", self.devices.len()))
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
                });
                ui.add_space(self.theme.spacing.sm);
                ui.separator();
                ui.add_space(self.theme.spacing.sm);

                // Device filters (only show if we have devices)
                if !self.devices.is_empty() {
                    // Type filter row
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for filter in [
                            DeviceTypeFilter::All,
                            DeviceTypeFilter::Aranet4,
                            DeviceTypeFilter::AranetRadon,
                            DeviceTypeFilter::AranetRadiation,
                            DeviceTypeFilter::Aranet2,
                        ] {
                            let is_selected = self.device_type_filter == filter;
                            let (bg, text_color, stroke) = if is_selected {
                                (
                                    self.theme.accent,
                                    self.theme.text_on_accent,
                                    egui::Stroke::new(1.0, self.theme.accent),
                                )
                            } else {
                                (
                                    self.theme.bg_card,
                                    self.theme.text_secondary,
                                    egui::Stroke::new(1.0, self.theme.border_subtle),
                                )
                            };
                            let btn = egui::Button::new(
                                RichText::new(filter.label()).size(11.0).color(text_color),
                            )
                            .fill(bg)
                            .stroke(stroke)
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));
                            if ui.add(btn).clicked() {
                                self.device_type_filter = filter;
                            }
                        }
                    });

                    ui.add_space(self.theme.spacing.xs);

                    // Connection status filter row
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for filter in [
                            ConnectionFilter::All,
                            ConnectionFilter::Connected,
                            ConnectionFilter::Disconnected,
                        ] {
                            let is_selected = self.connection_filter == filter;
                            let (bg, text_color, stroke) = if is_selected {
                                (
                                    self.theme.info,
                                    self.theme.text_on_accent,
                                    egui::Stroke::new(1.0, self.theme.info),
                                )
                            } else {
                                (
                                    self.theme.bg_card,
                                    self.theme.text_secondary,
                                    egui::Stroke::new(1.0, self.theme.border_subtle),
                                )
                            };
                            let btn = egui::Button::new(
                                RichText::new(filter.label()).size(11.0).color(text_color),
                            )
                            .fill(bg)
                            .stroke(stroke)
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));
                            if ui.add(btn).clicked() {
                                self.connection_filter = filter;
                            }
                        }
                    });

                    ui.add_space(self.theme.spacing.sm);

                    // Bulk actions row
                    let disconnected_count = self
                        .devices
                        .iter()
                        .filter(|d| matches!(d.connection, ConnectionState::Disconnected))
                        .count();
                    let connected_count = self
                        .devices
                        .iter()
                        .filter(|d| matches!(d.connection, ConnectionState::Connected))
                        .count();

                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;

                        // Connect All button
                        let connect_enabled = disconnected_count > 0;
                        let (connect_bg, connect_text) = if connect_enabled {
                            (self.theme.bg_card, self.theme.text_secondary)
                        } else {
                            (self.theme.bg_disabled, self.theme.text_disabled)
                        };
                        ui.add_enabled_ui(connect_enabled, |ui| {
                            let btn = egui::Button::new(
                                RichText::new("Connect All").size(11.0).color(connect_text),
                            )
                            .fill(connect_bg)
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));
                            if ui
                                .add(btn)
                                .on_hover_text(format!(
                                    "Connect to all {} disconnected devices",
                                    disconnected_count
                                ))
                                .clicked()
                            {
                                self.status = "Connecting to all devices...".to_string();
                                for device in &self.devices {
                                    if matches!(device.connection, ConnectionState::Disconnected) {
                                        self.send_command(Command::Connect {
                                            device_id: device.id.clone(),
                                        });
                                    }
                                }
                            }
                        });

                        // Disconnect All button
                        let disconnect_enabled = connected_count > 0;
                        let (disconnect_bg, disconnect_text) = if disconnect_enabled {
                            (self.theme.bg_card, self.theme.text_secondary)
                        } else {
                            (self.theme.bg_disabled, self.theme.text_disabled)
                        };
                        ui.add_enabled_ui(disconnect_enabled, |ui| {
                            let btn = egui::Button::new(
                                RichText::new("Disconnect All")
                                    .size(11.0)
                                    .color(disconnect_text),
                            )
                            .fill(disconnect_bg)
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));
                            if ui
                                .add(btn)
                                .on_hover_text(format!(
                                    "Disconnect from all {} connected devices",
                                    connected_count
                                ))
                                .clicked()
                            {
                                self.status = "Disconnecting from all devices...".to_string();
                                for device in &self.devices {
                                    if matches!(device.connection, ConnectionState::Connected) {
                                        self.send_command(Command::Disconnect {
                                            device_id: device.id.clone(),
                                        });
                                    }
                                }
                            }
                        });
                    });

                    ui.add_space(self.theme.spacing.sm);
                }

                if self.devices.is_empty() {
                    components::empty_state(
                        ui,
                        &self.theme,
                        "No Devices",
                        "Click 'Scan' to discover nearby devices",
                    );
                } else {
                    // Build filtered device indices
                    let mut device_indices: Vec<usize> = (0..self.devices.len())
                        .filter(|&i| {
                            let device = &self.devices[i];
                            // Type filter
                            if !self.device_type_filter.matches(device.device_type) {
                                return false;
                            }
                            // Connection filter
                            match self.connection_filter {
                                ConnectionFilter::All => true,
                                ConnectionFilter::Connected => {
                                    matches!(device.connection, ConnectionState::Connected)
                                }
                                ConnectionFilter::Disconnected => {
                                    !matches!(device.connection, ConnectionState::Connected)
                                }
                            }
                        })
                        .collect();

                    // Sort: connected first, then alphabetically
                    device_indices.sort_by(|&a, &b| {
                        let dev_a = &self.devices[a];
                        let dev_b = &self.devices[b];
                        let conn_a = matches!(dev_a.connection, ConnectionState::Connected);
                        let conn_b = matches!(dev_b.connection, ConnectionState::Connected);
                        conn_b
                            .cmp(&conn_a)
                            .then_with(|| dev_a.display_name().cmp(dev_b.display_name()))
                    });

                    // Show filtered count if filters are active
                    let filters_active = self.device_type_filter != DeviceTypeFilter::All
                        || self.connection_filter != ConnectionFilter::All;
                    if filters_active {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Showing {} of {}",
                                    device_indices.len(),
                                    self.devices.len()
                                ))
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                            );
                        });
                        ui.add_space(self.theme.spacing.xs);
                    }

                    if device_indices.is_empty() && filters_active {
                        components::empty_state_with_kind(
                            ui,
                            &self.theme,
                            "No Matches",
                            "Adjust filters to see devices",
                            components::EmptyStateKind::NoMatch,
                        );
                    } else {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let mut new_selection = self.selected_device;
                            let mut comparison_changed = false;
                            for i in device_indices {
                                let device = &self.devices[i];
                                let is_in_comparison = self.comparison_devices.contains(&i);
                                let selected = if self.comparison_mode {
                                    is_in_comparison
                                } else {
                                    self.selected_device == Some(i)
                                };
                                let (frame_fill, border_color) = if selected {
                                    (self.theme.tint_bg(self.theme.accent, 20), self.theme.accent)
                                } else {
                                    (Color32::TRANSPARENT, self.theme.border_subtle)
                                };

                                let response = egui::Frame::new()
                                    .fill(frame_fill)
                                    .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                                    .corner_radius(egui::CornerRadius::same(
                                        self.theme.rounding.md as u8,
                                    ))
                                    .stroke(egui::Stroke::new(1.0, border_color))
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        ui.vertical(|ui| {
                                            // Device name row
                                            ui.horizontal(|ui| {
                                                let (dot_color, status_tip) = match &device
                                                    .connection
                                                {
                                                    ConnectionState::Disconnected => {
                                                        (self.theme.text_muted, "Disconnected")
                                                    }
                                                    ConnectionState::Connecting => {
                                                        (self.theme.warning, "Connecting...")
                                                    }
                                                    ConnectionState::Reconnecting { .. } => {
                                                        (self.theme.warning, "Reconnecting...")
                                                    }
                                                    ConnectionState::Connected => {
                                                        (self.theme.success, "Connected")
                                                    }
                                                    ConnectionState::Error(_) => {
                                                        (self.theme.danger, "Connection error")
                                                    }
                                                };
                                                // In comparison mode, show checkbox-like indicator
                                                if self.comparison_mode {
                                                    let checkbox_text = if is_in_comparison {
                                                        "☑"
                                                    } else {
                                                        "☐"
                                                    };
                                                    let checkbox_color = if is_in_comparison {
                                                        self.theme.accent
                                                    } else {
                                                        self.theme.text_muted
                                                    };
                                                    ui.label(
                                                        RichText::new(checkbox_text)
                                                            .size(14.0)
                                                            .color(checkbox_color),
                                                    );
                                                    ui.add_space(self.theme.spacing.xs);
                                                }

                                                components::status_dot(ui, dot_color, status_tip);
                                                ui.add_space(self.theme.spacing.sm);

                                                let name_color = if selected {
                                                    self.theme.accent
                                                } else {
                                                    self.theme.text_primary
                                                };
                                                ui.label(
                                                    RichText::new(device.display_name())
                                                        .color(name_color)
                                                        .size(self.theme.typography.body)
                                                        .strong(),
                                                );
                                            });

                                            // Device info row
                                            ui.add_space(self.theme.spacing.xs);
                                            ui.horizontal(|ui| {
                                                if let Some(device_type) = device.device_type {
                                                    let type_label = match device_type {
                                                        DeviceType::Aranet4 => "CO2",
                                                        DeviceType::Aranet2 => "T/H",
                                                        DeviceType::AranetRadon => "Rn",
                                                        DeviceType::AranetRadiation => "Rad",
                                                        _ => "?",
                                                    };
                                                    components::status_badge(
                                                        ui,
                                                        &self.theme,
                                                        type_label,
                                                        self.theme.info,
                                                    );
                                                    ui.add_space(self.theme.spacing.xs);
                                                }

                                                // Show primary sensor reading based on device type
                                                if let Some(ref reading) = device.reading {
                                                    if reading.co2 > 0 {
                                                        // Aranet4: Show CO2
                                                        let color =
                                                            self.theme.co2_color(reading.co2);
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "{} ppm",
                                                                reading.co2
                                                            ))
                                                            .size(self.theme.typography.caption)
                                                            .color(color),
                                                        )
                                                        .on_hover_text("CO2 level");
                                                    } else if let Some(radon) = reading.radon {
                                                        // AranetRadon: Show radon
                                                        let (value, unit) = format_radon(
                                                            radon,
                                                            device.settings.as_ref(),
                                                        );
                                                        let color = self.theme.radon_color(radon);
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "{} {}",
                                                                value, unit
                                                            ))
                                                            .size(self.theme.typography.caption)
                                                            .color(color),
                                                        )
                                                        .on_hover_text("Radon level");
                                                    } else if let Some(rate) =
                                                        reading.radiation_rate
                                                    {
                                                        // AranetRadiation: Show radiation rate
                                                        let color =
                                                            self.theme.radiation_color(rate);
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "{:.2} uSv/h",
                                                                rate
                                                            ))
                                                            .size(self.theme.typography.caption)
                                                            .color(color),
                                                        )
                                                        .on_hover_text("Radiation rate");
                                                    } else {
                                                        // Aranet2 or unknown: Show temperature
                                                        let (temp_val, temp_unit) =
                                                            format_temperature(
                                                                reading.temperature,
                                                                device.settings.as_ref(),
                                                                Some(
                                                                    &self
                                                                        .gui_config
                                                                        .temperature_unit,
                                                                ),
                                                            );
                                                        ui.label(
                                                            RichText::new(format!(
                                                                "{}{}",
                                                                temp_val, temp_unit
                                                            ))
                                                            .size(self.theme.typography.caption)
                                                            .color(self.theme.text_secondary),
                                                        )
                                                        .on_hover_text("Temperature");
                                                    }
                                                }

                                                if let Some(rssi) = device.rssi {
                                                    let signal_color = if rssi > -60 {
                                                        self.theme.success
                                                    } else if rssi > -75 {
                                                        self.theme.warning
                                                    } else {
                                                        self.theme.danger
                                                    };
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.label(
                                                                RichText::new(format!(
                                                                    "{}dB",
                                                                    rssi
                                                                ))
                                                                .size(self.theme.typography.caption)
                                                                .color(signal_color),
                                                            )
                                                            .on_hover_text("Signal strength");
                                                        },
                                                    );
                                                }
                                            });

                                            // Status badges row (battery low, stale reading)
                                            let has_badges = {
                                                let battery_low = device
                                                    .reading
                                                    .as_ref()
                                                    .map(|r| r.battery < 20)
                                                    .unwrap_or(false);
                                                let stale_reading = device
                                                    .reading
                                                    .as_ref()
                                                    .map(|r| {
                                                        r.interval > 0 && r.age > r.interval * 2
                                                    })
                                                    .unwrap_or(false);
                                                battery_low || stale_reading
                                            };

                                            if has_badges {
                                                ui.add_space(self.theme.spacing.xs);
                                                ui.horizontal(|ui| {
                                                    // Low battery badge
                                                    if let Some(ref reading) = device.reading
                                                        && reading.battery < 20
                                                    {
                                                        let battery_color = if reading.battery < 10
                                                        {
                                                            self.theme.danger
                                                        } else {
                                                            self.theme.warning
                                                        };
                                                        components::status_badge(
                                                            ui,
                                                            &self.theme,
                                                            &format!("{}% bat", reading.battery),
                                                            battery_color,
                                                        );
                                                        ui.add_space(self.theme.spacing.xs);
                                                    }

                                                    // Stale reading badge (age > 2x interval means stale)
                                                    if let Some(ref reading) = device.reading {
                                                        let is_stale = reading.interval > 0
                                                            && reading.age > reading.interval * 2;
                                                        if is_stale {
                                                            components::status_badge(
                                                                ui,
                                                                &self.theme,
                                                                "stale",
                                                                self.theme.caution,
                                                            );
                                                        }
                                                    }
                                                });
                                            }
                                        });
                                    })
                                    .response;

                                if response.interact(egui::Sense::click()).clicked() {
                                    if self.comparison_mode {
                                        // Toggle device in comparison list
                                        if let Some(pos) =
                                            self.comparison_devices.iter().position(|&x| x == i)
                                        {
                                            self.comparison_devices.remove(pos);
                                        } else {
                                            self.comparison_devices.push(i);
                                        }
                                        comparison_changed = true;
                                    } else {
                                        new_selection = Some(i);
                                    }
                                }

                                ui.add_space(self.theme.spacing.xs);
                            }
                            if !self.comparison_mode {
                                self.selected_device = new_selection;
                            }
                            // Force repaint if comparison changed
                            if comparison_changed {
                                ui.ctx().request_repaint();
                            }
                        });
                    }
                }
            });
    }
}
