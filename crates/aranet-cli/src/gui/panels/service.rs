//! Service management panel.
//!
//! This module contains the service panel rendering logic,
//! including service status display, control buttons, and device collection statistics.

use aranet_core::messages::Command;
use eframe::egui::{self, RichText};

use crate::gui::app::AranetApp;
use crate::gui::components;
use crate::gui::helpers::format_uptime;

impl AranetApp {
    /// Render the service management panel.
    pub(crate) fn render_service_panel(&mut self, ui: &mut egui::Ui) {
        let mut commands_to_send: Vec<Command> = Vec::new();
        let mut should_start_refreshing = false;
        let mut close_add_dialog = false;

        ui.label(
            RichText::new("Service Management")
                .size(self.theme.typography.heading)
                .strong()
                .color(self.theme.text_primary),
        );
        ui.add_space(self.theme.spacing.md);

        egui::ScrollArea::vertical().show(ui, |ui| {
            // Service Status Section
            components::section_header(ui, &self.theme, "Service Status");

            egui::Frame::new()
                .fill(self.theme.bg_secondary)
                .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                .show(ui, |ui| {
                    if let Some(ref status) = self.service_status {
                        // Status grid
                        egui::Grid::new("service_status_grid")
                            .num_columns(2)
                            .spacing([self.theme.spacing.xl, self.theme.spacing.sm])
                            .show(ui, |ui| {
                                // Reachability
                                ui.label(
                                    RichText::new("Service")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_secondary),
                                );
                                let (status_text, color) = if status.reachable {
                                    ("Reachable", self.theme.success)
                                } else {
                                    ("Not Reachable", self.theme.danger)
                                };
                                components::status_badge(ui, &self.theme, status_text, color);
                                ui.end_row();

                                // Collector status
                                ui.label(
                                    RichText::new("Collector")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_secondary),
                                );
                                let (coll_text, coll_color) = if status.collector_running {
                                    ("Running", self.theme.success)
                                } else {
                                    ("Stopped", self.theme.text_muted)
                                };
                                components::status_badge(ui, &self.theme, coll_text, coll_color);
                                ui.end_row();

                                // Uptime
                                if let Some(uptime) = status.uptime_seconds {
                                    ui.label(
                                        RichText::new("Uptime")
                                            .size(self.theme.typography.body)
                                            .color(self.theme.text_secondary),
                                    );
                                    let uptime_str = format_uptime(uptime);
                                    ui.label(
                                        RichText::new(uptime_str)
                                            .size(self.theme.typography.body)
                                            .color(self.theme.text_primary),
                                    );
                                    ui.end_row();
                                }

                                // Device count
                                ui.label(
                                    RichText::new("Devices")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_secondary),
                                );
                                ui.label(
                                    RichText::new(format!("{}", status.devices.len()))
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_primary),
                                );
                                ui.end_row();
                            });

                        ui.add_space(self.theme.spacing.md);

                        // Control buttons
                        ui.horizontal(|ui| {
                            // Refresh button
                            let refresh_btn = egui::Button::new(
                                RichText::new(if self.service_refreshing {
                                    "Refreshing..."
                                } else {
                                    "Refresh"
                                })
                                .size(self.theme.typography.body)
                                .color(self.theme.text_on_accent),
                            )
                            .fill(self.theme.accent)
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui
                                .add_enabled(!self.service_refreshing, refresh_btn)
                                .clicked()
                            {
                                commands_to_send.push(Command::RefreshServiceStatus);
                                should_start_refreshing = true;
                            }

                            ui.add_space(self.theme.spacing.sm);

                            // Start/Stop button
                            if status.reachable {
                                let (btn_text, btn_color) = if status.collector_running {
                                    ("Stop Collector", self.theme.danger)
                                } else {
                                    ("Start Collector", self.theme.success)
                                };

                                let toggle_btn = egui::Button::new(
                                    RichText::new(btn_text)
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_on_accent),
                                )
                                .fill(btn_color)
                                .corner_radius(
                                    egui::CornerRadius::same(self.theme.rounding.sm as u8),
                                );

                                if ui.add(toggle_btn).clicked() {
                                    if status.collector_running {
                                        commands_to_send.push(Command::StopServiceCollector);
                                    } else {
                                        commands_to_send.push(Command::StartServiceCollector);
                                    }
                                }
                            }
                        });
                    } else {
                        // No status yet - show loading or prompt to refresh
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Service status not loaded")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.text_muted),
                            );

                            ui.add_space(self.theme.spacing.md);

                            let refresh_btn = egui::Button::new(
                                RichText::new(if self.service_refreshing {
                                    "Loading..."
                                } else {
                                    "Load Status"
                                })
                                .size(self.theme.typography.body)
                                .color(self.theme.text_on_accent),
                            )
                            .fill(self.theme.accent)
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui
                                .add_enabled(!self.service_refreshing, refresh_btn)
                                .clicked()
                            {
                                commands_to_send.push(Command::RefreshServiceStatus);
                                should_start_refreshing = true;
                            }
                        });
                    }
                });

            // Show helpful message when service is not reachable
            if let Some(ref status) = self.service_status
                && !status.reachable
            {
                ui.add_space(self.theme.spacing.sm);
                egui::Frame::new()
                    .fill(self.theme.bg_elevated)
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8))
                    .inner_margin(egui::Margin::same(self.theme.spacing.sm as i8))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("Tip: Run 'aranet-service run' in a terminal to start the background service.")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
            }

            ui.add_space(self.theme.spacing.lg);

            // System Service Installation Section
            components::section_header(ui, &self.theme, "System Service");

            egui::Frame::new()
                .fill(self.theme.bg_secondary)
                .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Install aranet-service as a system service to run at startup.")
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_muted),
                    );
                    ui.add_space(self.theme.spacing.sm);

                    // System service status
                    if let Some((installed, running)) = self.system_service_status {
                        egui::Grid::new("system_service_status_grid")
                            .num_columns(2)
                            .spacing([self.theme.spacing.xl, self.theme.spacing.xs])
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new("Installed")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_secondary),
                                );
                                let (text, color) = if installed {
                                    ("Yes", self.theme.success)
                                } else {
                                    ("No", self.theme.text_muted)
                                };
                                components::status_badge(ui, &self.theme, text, color);
                                ui.end_row();

                                ui.label(
                                    RichText::new("Running")
                                        .size(self.theme.typography.body)
                                        .color(self.theme.text_secondary),
                                );
                                let (text, color) = if running {
                                    ("Yes", self.theme.success)
                                } else {
                                    ("No", self.theme.text_muted)
                                };
                                components::status_badge(ui, &self.theme, text, color);
                                ui.end_row();
                            });

                        ui.add_space(self.theme.spacing.md);

                        // Action buttons based on current state
                        ui.horizontal(|ui| {
                            let pending = self.system_service_pending;

                            // Refresh status button
                            let refresh_btn = egui::Button::new(
                                RichText::new(if pending { "..." } else { "Refresh" })
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_primary),
                            )
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add_enabled(!pending, refresh_btn).clicked() {
                                commands_to_send.push(Command::CheckSystemServiceStatus {
                                    user_level: true,
                                });
                                self.system_service_pending = true;
                            }

                            if installed {
                                // Start/Stop buttons
                                if running {
                                    let stop_btn = egui::Button::new(
                                        RichText::new("Stop")
                                            .size(self.theme.typography.caption)
                                            .color(self.theme.text_on_accent),
                                    )
                                    .fill(self.theme.danger)
                                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                                    if ui.add_enabled(!pending, stop_btn).clicked() {
                                        commands_to_send.push(Command::StopSystemService {
                                            user_level: true,
                                        });
                                        self.system_service_pending = true;
                                    }
                                } else {
                                    let start_btn = egui::Button::new(
                                        RichText::new("Start")
                                            .size(self.theme.typography.caption)
                                            .color(self.theme.text_on_accent),
                                    )
                                    .fill(self.theme.success)
                                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                                    if ui.add_enabled(!pending, start_btn).clicked() {
                                        commands_to_send.push(Command::StartSystemService {
                                            user_level: true,
                                        });
                                        self.system_service_pending = true;
                                    }
                                }

                                // Uninstall button
                                let uninstall_btn = egui::Button::new(
                                    RichText::new("Uninstall")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.danger),
                                )
                                .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                                if ui.add_enabled(!pending, uninstall_btn).clicked() {
                                    commands_to_send.push(Command::UninstallSystemService {
                                        user_level: true,
                                    });
                                    self.system_service_pending = true;
                                }
                            } else {
                                // Install button
                                let install_btn = egui::Button::new(
                                    RichText::new("Install")
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.text_on_accent),
                                )
                                .fill(self.theme.accent)
                                .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                                if ui.add_enabled(!pending, install_btn).clicked() {
                                    commands_to_send.push(Command::InstallSystemService {
                                        user_level: true,
                                    });
                                    self.system_service_pending = true;
                                }
                            }
                        });
                    } else {
                        // Status not loaded yet
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Status not checked")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.text_muted),
                            );

                            ui.add_space(self.theme.spacing.md);

                            let check_btn = egui::Button::new(
                                RichText::new(if self.system_service_pending {
                                    "Checking..."
                                } else {
                                    "Check Status"
                                })
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_on_accent),
                            )
                            .fill(self.theme.accent)
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui
                                .add_enabled(!self.system_service_pending, check_btn)
                                .clicked()
                            {
                                commands_to_send.push(Command::CheckSystemServiceStatus {
                                    user_level: true,
                                });
                                self.system_service_pending = true;
                            }
                        });
                    }

                    ui.add_space(self.theme.spacing.sm);
                    ui.label(
                        RichText::new("Note: User-level service does not require admin privileges.")
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_muted)
                            .italics(),
                    );
                });

            ui.add_space(self.theme.spacing.lg);

            // Device Collection Stats Section
            if let Some(ref status) = self.service_status
                && !status.devices.is_empty()
            {
                components::section_header(ui, &self.theme, "Collection Statistics");

                for device in &status.devices {
                    egui::Frame::new()
                        .fill(self.theme.bg_secondary)
                        .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                        .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                // Device name/ID
                                let display_name =
                                    device.alias.as_ref().unwrap_or(&device.device_id).clone();

                                ui.label(
                                    RichText::new(&display_name)
                                        .size(self.theme.typography.body)
                                        .strong()
                                        .color(self.theme.text_primary),
                                );

                                ui.add_space(self.theme.spacing.sm);

                                // Status indicator
                                let (status_text, status_color) = if device.polling {
                                    ("POLL", self.theme.accent)
                                } else if device.last_error.is_some() {
                                    ("FAIL", self.theme.danger)
                                } else if device.success_count > 0 {
                                    ("PASS", self.theme.success)
                                } else {
                                    ("WAIT", self.theme.text_muted)
                                };
                                components::status_badge(
                                    ui,
                                    &self.theme,
                                    status_text,
                                    status_color,
                                );
                            });

                            ui.add_space(self.theme.spacing.xs);

                            // Stats row
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "Success: {} | Failures: {}",
                                        device.success_count, device.failure_count
                                    ))
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_secondary),
                                );

                                ui.label(
                                    RichText::new(format!(
                                        " | Interval: {}s",
                                        device.poll_interval
                                    ))
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_secondary),
                                );
                            });

                            // Last error if present
                            if let Some(ref error) = device.last_error {
                                ui.add_space(self.theme.spacing.xs);
                                ui.label(
                                    RichText::new(format!("Last error: {}", error))
                                        .size(self.theme.typography.caption)
                                        .color(self.theme.danger),
                                );
                            }
                        });

                    ui.add_space(self.theme.spacing.sm);
                }
            }

            ui.add_space(self.theme.spacing.lg);

            // Device Configuration Section (add/remove devices from service)
            if let Some(ref status) = self.service_status
                && status.reachable
            {
                components::section_header(ui, &self.theme, "Monitored Devices Configuration");

                egui::Frame::new()
                    .fill(self.theme.bg_secondary)
                    .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
                    .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
                    .show(ui, |ui| {
                        // Load config button if not loaded
                        if self.service_monitored_devices.is_empty() && !self.service_config_loading {
                            if ui
                                .button(
                                    RichText::new("Load Device Configuration")
                                        .size(self.theme.typography.body),
                                )
                                .clicked()
                            {
                                commands_to_send.push(Command::FetchServiceConfig);
                                self.service_config_loading = true;
                            }
                        } else if self.service_config_loading {
                            ui.label(
                                RichText::new("Loading configuration...")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.text_muted),
                            );
                        } else {
                            // Show monitored devices
                            ui.label(
                                RichText::new(format!(
                                    "{} device(s) configured for monitoring",
                                    self.service_monitored_devices.len()
                                ))
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                            );

                            ui.add_space(self.theme.spacing.sm);

                            // List each monitored device with remove button
                            let mut device_to_remove: Option<String> = None;

                            for device in &self.service_monitored_devices {
                                ui.horizontal(|ui| {
                                    // Device info
                                    let display = device
                                        .alias
                                        .as_ref()
                                        .map(|a| format!("{} ({})", a, device.address))
                                        .unwrap_or_else(|| device.address.clone());

                                    ui.label(
                                        RichText::new(&display)
                                            .size(self.theme.typography.caption)
                                            .color(self.theme.text_primary),
                                    );

                                    ui.label(
                                        RichText::new(format!(" - {}s interval", device.poll_interval))
                                            .size(self.theme.typography.caption)
                                            .color(self.theme.text_muted),
                                    );

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .small_button(RichText::new("X").color(self.theme.danger))
                                                .on_hover_text("Remove from monitoring")
                                                .clicked()
                                            {
                                                device_to_remove = Some(device.address.clone());
                                            }
                                        },
                                    );
                                });
                            }

                            if let Some(addr) = device_to_remove {
                                commands_to_send.push(Command::RemoveServiceDevice { address: addr });
                            }

                            ui.add_space(self.theme.spacing.md);

                            // Add device form
                            if let Some((ref mut addr, ref mut alias, ref mut interval)) =
                                self.add_device_dialog
                            {
                                ui.separator();
                                ui.add_space(self.theme.spacing.sm);

                                ui.label(
                                    RichText::new("Add Device to Monitoring")
                                        .size(self.theme.typography.body)
                                        .strong(),
                                );

                                egui::Grid::new("add_device_grid")
                                    .num_columns(2)
                                    .spacing([self.theme.spacing.md, self.theme.spacing.xs])
                                    .show(ui, |ui| {
                                        ui.label("Address:");
                                        ui.text_edit_singleline(addr);
                                        ui.end_row();

                                        ui.label("Alias:");
                                        ui.text_edit_singleline(alias);
                                        ui.end_row();

                                        ui.label("Interval (s):");
                                        ui.add(egui::DragValue::new(interval).range(10..=3600));
                                        ui.end_row();
                                    });

                                ui.add_space(self.theme.spacing.sm);

                                let can_add = !addr.is_empty();
                                let mut clicked_add = false;
                                let mut clicked_cancel = false;

                                ui.horizontal(|ui| {
                                    clicked_add = ui.add_enabled(can_add, egui::Button::new("Add")).clicked();
                                    clicked_cancel = ui.button("Cancel").clicked();
                                });

                                if clicked_add {
                                    commands_to_send.push(Command::AddServiceDevice {
                                        address: addr.clone(),
                                        alias: if alias.is_empty() {
                                            None
                                        } else {
                                            Some(alias.clone())
                                        },
                                        poll_interval: *interval,
                                    });
                                    close_add_dialog = true;
                                }

                                if clicked_cancel {
                                    close_add_dialog = true;
                                }
                            } else {
                                // Show "Add Device" button
                                if ui
                                    .button(
                                        RichText::new("+ Add Device")
                                            .size(self.theme.typography.caption)
                                            .color(self.theme.accent),
                                    )
                                    .clicked()
                                {
                                    self.add_device_dialog =
                                        Some((String::new(), String::new(), 60));
                                }
                            }
                        }
                    });
            }
        });

        // Update refreshing state before sending commands
        if should_start_refreshing {
            self.service_refreshing = true;
        }

        // Close add device dialog if requested
        if close_add_dialog {
            self.add_device_dialog = None;
        }

        // Send any queued commands
        for cmd in commands_to_send {
            self.send_command(cmd);
        }
    }
}
