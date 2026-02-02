//! Application settings panel for the Aranet GUI.
//!
//! This module contains the settings UI including theme, display units,
//! alert thresholds, data export, and behavior configuration options.

use eframe::egui::{self, Color32, RichText};

use crate::config::Config;
use crate::gui::app::AranetApp;
use crate::gui::components;
use crate::gui::helpers::ToastType;
use crate::gui::theme::{Theme, ThemeMode};

impl AranetApp {
    /// Render the application settings section.
    pub(crate) fn render_app_settings_section(&mut self, ui: &mut egui::Ui) {
        components::section_header(ui, &self.theme, "Application Settings");

        egui::Frame::new()
            .fill(self.theme.bg_card)
            .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8))
            .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
            .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
            .show(ui, |ui| {
                let mut config_changed = false;

                // Theme selection
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Theme")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Choose light or dark appearance")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (mode, label) in [("dark", "Dark"), ("light", "Light")] {
                            let is_selected = self.gui_config.theme == mode;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                self.gui_config.theme = mode.to_string();
                                self.theme_mode = match mode {
                                    "light" => ThemeMode::Light,
                                    _ => ThemeMode::Dark,
                                };
                                self.theme = Theme::for_mode_with_options(
                                    self.theme_mode,
                                    self.gui_config.compact_mode,
                                );
                                if let Some(ref menu) = self.menu_manager {
                                    menu.set_dark_mode(self.theme_mode == ThemeMode::Dark);
                                }
                                config_changed = true;
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Compact mode toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Compact Mode")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Denser layout for smaller screens")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (val, text) in [(true, "On"), (false, "Off")] {
                            let is_selected = self.gui_config.compact_mode == val;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                self.gui_config.compact_mode = val;
                                // Rebuild theme with new compact setting
                                self.theme = Theme::for_mode_with_options(self.theme_mode, val);
                                ui.ctx().set_style(self.theme.to_style());
                                config_changed = true;
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Colored tray icon toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Colored Menu Bar Icon")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Show colored icon when CO2 is elevated")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (val, text) in [(true, "On"), (false, "Off")] {
                            let is_selected = self.gui_config.colored_tray_icon == val;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                self.gui_config.colored_tray_icon = val;
                                // Update tray state
                                if let Ok(mut state) = self.tray_state.lock() {
                                    state.colored_tray_icon = val;
                                }
                                // Trigger tray icon update
                                if let Some(ref tray) = self.tray_manager {
                                    tray.update_tooltip();
                                }
                                config_changed = true;
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Notifications toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Desktop Notifications")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Alert when CO2 reaches threshold levels")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (val, text) in [(true, "On"), (false, "Off")] {
                            let is_selected = self.gui_config.notifications_enabled == val;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                self.gui_config.notifications_enabled = val;
                                // Update tray state
                                if let Ok(mut state) = self.tray_state.lock() {
                                    state.notifications_enabled = val;
                                }
                                config_changed = true;
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Notification sound toggle (only show if notifications are enabled)
                if self.gui_config.notifications_enabled {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("Notification Sound")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.text_primary),
                            );
                            ui.label(
                                RichText::new("Play sound with notifications")
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_secondary),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (val, text) in [(true, "On"), (false, "Off")] {
                                let is_selected = self.gui_config.notification_sound == val;
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
                                .corner_radius(
                                    egui::CornerRadius::same(self.theme.rounding.sm as u8),
                                );

                                if ui.add(btn).clicked() && !is_selected {
                                    self.gui_config.notification_sound = val;
                                    // Update tray state
                                    if let Ok(mut state) = self.tray_state.lock() {
                                        state.notification_sound = val;
                                    }
                                    config_changed = true;
                                }
                            }
                        });
                    });

                    ui.add_space(self.theme.spacing.md);

                    // Do Not Disturb toggle
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("Do Not Disturb")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.text_primary),
                            );
                            ui.label(
                                RichText::new("Temporarily silence all notifications")
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_secondary),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (val, text) in [(true, "On"), (false, "Off")] {
                                let is_selected = self.do_not_disturb == val;
                                let (bg, text_color) = if is_selected {
                                    if val {
                                        // When DND is active, show with warning color
                                        (self.theme.warning, Color32::BLACK)
                                    } else {
                                        (self.theme.accent, self.theme.text_on_accent)
                                    }
                                } else {
                                    (self.theme.bg_secondary, self.theme.text_secondary)
                                };

                                let btn = egui::Button::new(
                                    RichText::new(text)
                                        .size(self.theme.typography.caption)
                                        .color(text_color),
                                )
                                .fill(bg)
                                .corner_radius(
                                    egui::CornerRadius::same(self.theme.rounding.sm as u8),
                                );

                                if ui.add(btn).clicked() && !is_selected {
                                    self.do_not_disturb = val;
                                    // Persist to config
                                    self.gui_config.do_not_disturb = val;
                                    config_changed = true;
                                    // Update tray state
                                    if let Ok(mut state) = self.tray_state.lock() {
                                        state.do_not_disturb = val;
                                    }
                                    // Update menu if available
                                    if let Some(ref menu) = self.menu_manager {
                                        menu.set_do_not_disturb(val);
                                    }
                                    // Show toast to confirm
                                    if val {
                                        self.add_toast(
                                            "Do Not Disturb enabled".to_string(),
                                            ToastType::Info,
                                        );
                                    } else {
                                        self.add_toast(
                                            "Do Not Disturb disabled".to_string(),
                                            ToastType::Info,
                                        );
                                    }
                                }
                            }
                        });
                    });

                    ui.add_space(self.theme.spacing.md);
                }

                // Close to tray toggle (only show if tray is available)
                if self.tray_manager.is_some() {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("Close to Menu Bar")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.text_primary),
                            );
                            ui.label(
                                RichText::new("Keep running in background when window closes")
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_secondary),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (val, text) in [(true, "On"), (false, "Off")] {
                                let is_selected = self.gui_config.close_to_tray == val;
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
                                .corner_radius(
                                    egui::CornerRadius::same(self.theme.rounding.sm as u8),
                                );

                                if ui.add(btn).clicked() && !is_selected {
                                    self.gui_config.close_to_tray = val;
                                    self.close_to_tray = val;
                                    config_changed = true;
                                }
                            }
                        });
                    });

                    ui.add_space(self.theme.spacing.md);

                    // Start minimized toggle
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("Start Minimized")
                                    .size(self.theme.typography.body)
                                    .color(self.theme.text_primary),
                            );
                            ui.label(
                                RichText::new("Launch hidden in menu bar")
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_secondary),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (val, text) in [(true, "On"), (false, "Off")] {
                                let is_selected = self.gui_config.start_minimized == val;
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
                                .corner_radius(
                                    egui::CornerRadius::same(self.theme.rounding.sm as u8),
                                );

                                if ui.add(btn).clicked() && !is_selected {
                                    self.gui_config.start_minimized = val;
                                    config_changed = true;
                                }
                            }
                        });
                    });
                }

                ui.add_space(self.theme.spacing.lg);
                ui.separator();
                ui.add_space(self.theme.spacing.md);

                // Units section header
                ui.label(
                    RichText::new("Display Units")
                        .size(self.theme.typography.body)
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.add_space(self.theme.spacing.sm);

                // Temperature unit toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Temperature")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Used when device preference is unavailable")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (unit, label) in [("fahrenheit", "°F"), ("celsius", "°C")] {
                            let is_selected = self.gui_config.temperature_unit == unit;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                self.gui_config.temperature_unit = unit.to_string();
                                config_changed = true;
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Pressure unit toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Pressure")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Atmospheric pressure display unit")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (unit, label) in [("inhg", "inHg"), ("hpa", "hPa")] {
                            let is_selected = self.gui_config.pressure_unit == unit;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                self.gui_config.pressure_unit = unit.to_string();
                                config_changed = true;
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.lg);
                ui.separator();
                ui.add_space(self.theme.spacing.md);

                // Alert Thresholds section
                ui.label(
                    RichText::new("Alert Thresholds")
                        .size(self.theme.typography.body)
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.add_space(self.theme.spacing.sm);

                // CO2 Warning Threshold slider
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("CO2 Warning")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Amber indicator threshold (ppm)")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut co2_warning = self.gui_config.co2_warning_threshold as f32;
                        // Current value (rightmost)
                        ui.label(
                            RichText::new(format!("{} ppm", self.gui_config.co2_warning_threshold))
                                .size(self.theme.typography.caption)
                                .color(self.theme.warning),
                        );
                        ui.add_space(self.theme.spacing.sm);
                        // Max label
                        ui.label(
                            RichText::new("1200")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                        let slider = egui::Slider::new(&mut co2_warning, 600.0..=1200.0)
                            .show_value(false)
                            .step_by(50.0);
                        if ui.add(slider).changed() {
                            self.gui_config.co2_warning_threshold = co2_warning as u16;
                            // Ensure warning < danger (maintain at least 50 ppm gap)
                            if self.gui_config.co2_warning_threshold
                                >= self.gui_config.co2_danger_threshold
                            {
                                self.gui_config.co2_danger_threshold =
                                    (self.gui_config.co2_warning_threshold + 50).min(2000);
                            }
                            config_changed = true;
                        }
                        // Min label (leftmost)
                        ui.label(
                            RichText::new("600")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
                });

                ui.add_space(self.theme.spacing.sm);

                // CO2 Danger Threshold slider
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("CO2 Danger")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Red indicator threshold (ppm)")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut co2_danger = self.gui_config.co2_danger_threshold as f32;
                        // Current value (rightmost)
                        ui.label(
                            RichText::new(format!("{} ppm", self.gui_config.co2_danger_threshold))
                                .size(self.theme.typography.caption)
                                .color(self.theme.danger),
                        );
                        ui.add_space(self.theme.spacing.sm);
                        // Max label
                        ui.label(
                            RichText::new("2000")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                        let slider = egui::Slider::new(&mut co2_danger, 1000.0..=2000.0)
                            .show_value(false)
                            .step_by(50.0);
                        if ui.add(slider).changed() {
                            self.gui_config.co2_danger_threshold = co2_danger as u16;
                            // Ensure danger > warning (maintain at least 50 ppm gap)
                            if self.gui_config.co2_danger_threshold
                                <= self.gui_config.co2_warning_threshold
                            {
                                self.gui_config.co2_warning_threshold =
                                    self.gui_config.co2_danger_threshold.saturating_sub(50).max(600);
                            }
                            config_changed = true;
                        }
                        // Min label (leftmost)
                        ui.label(
                            RichText::new("1000")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
                });

                ui.add_space(self.theme.spacing.sm);

                // Radon Warning Threshold slider
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Radon Warning")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Amber indicator threshold (Bq/m³)")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut radon_warning = self.gui_config.radon_warning_threshold as f32;
                        // Current value (rightmost)
                        ui.label(
                            RichText::new(format!(
                                "{} Bq/m³",
                                self.gui_config.radon_warning_threshold
                            ))
                            .size(self.theme.typography.caption)
                            .color(self.theme.warning),
                        );
                        ui.add_space(self.theme.spacing.sm);
                        // Max label
                        ui.label(
                            RichText::new("200")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                        let slider = egui::Slider::new(&mut radon_warning, 50.0..=200.0)
                            .show_value(false)
                            .step_by(10.0);
                        if ui.add(slider).changed() {
                            self.gui_config.radon_warning_threshold = radon_warning as u32;
                            // Ensure warning < danger (maintain at least 10 Bq/m³ gap)
                            if self.gui_config.radon_warning_threshold
                                >= self.gui_config.radon_danger_threshold
                            {
                                self.gui_config.radon_danger_threshold =
                                    (self.gui_config.radon_warning_threshold + 10).min(300);
                            }
                            config_changed = true;
                        }
                        // Min label (leftmost)
                        ui.label(
                            RichText::new("50")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
                });

                ui.add_space(self.theme.spacing.sm);

                // Radon Danger Threshold slider
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Radon Danger")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Red indicator threshold (Bq/m³)")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut radon_danger = self.gui_config.radon_danger_threshold as f32;
                        // Current value (rightmost)
                        ui.label(
                            RichText::new(format!(
                                "{} Bq/m³",
                                self.gui_config.radon_danger_threshold
                            ))
                            .size(self.theme.typography.caption)
                            .color(self.theme.danger),
                        );
                        ui.add_space(self.theme.spacing.sm);
                        // Max label
                        ui.label(
                            RichText::new("300")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                        let slider = egui::Slider::new(&mut radon_danger, 100.0..=300.0)
                            .show_value(false)
                            .step_by(10.0);
                        if ui.add(slider).changed() {
                            self.gui_config.radon_danger_threshold = radon_danger as u32;
                            // Ensure danger > warning (maintain at least 10 Bq/m³ gap)
                            if self.gui_config.radon_danger_threshold
                                <= self.gui_config.radon_warning_threshold
                            {
                                self.gui_config.radon_warning_threshold =
                                    self.gui_config.radon_danger_threshold.saturating_sub(10).max(50);
                            }
                            config_changed = true;
                        }
                        // Min label (leftmost)
                        ui.label(
                            RichText::new("100")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
                });

                ui.add_space(self.theme.spacing.lg);
                ui.separator();
                ui.add_space(self.theme.spacing.md);

                // Data Export section
                ui.label(
                    RichText::new("Data Export")
                        .size(self.theme.typography.body)
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.add_space(self.theme.spacing.sm);

                // Default export format
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Default Format")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Format used for history exports")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (fmt, label) in [("json", "JSON"), ("csv", "CSV")] {
                            let is_selected = self.gui_config.default_export_format == fmt;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                self.gui_config.default_export_format = fmt.to_string();
                                config_changed = true;
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Export directory display
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Export Location")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        let display_path = if self.gui_config.export_directory.is_empty() {
                            dirs::download_dir()
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|| "Downloads folder".to_string())
                        } else {
                            self.gui_config.export_directory.clone()
                        };
                        ui.label(
                            RichText::new(&display_path)
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Reset to default button (only show if custom path is set)
                        if !self.gui_config.export_directory.is_empty() {
                            let reset_btn = egui::Button::new(
                                RichText::new("Reset")
                                    .size(self.theme.typography.caption)
                                    .color(self.theme.text_secondary),
                            )
                            .fill(self.theme.bg_secondary)
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui
                                .add(reset_btn)
                                .on_hover_text("Reset to Downloads folder")
                                .clicked()
                            {
                                self.gui_config.export_directory = String::new();
                                config_changed = true;
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.lg);
                ui.separator();
                ui.add_space(self.theme.spacing.md);

                // Behavior section
                ui.label(
                    RichText::new("Behavior")
                        .size(self.theme.typography.body)
                        .strong()
                        .color(self.theme.text_primary),
                );
                ui.add_space(self.theme.spacing.sm);

                // Load the behavior config for display
                let behavior_config = Config::load().behavior;

                // Auto-connect toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Auto-Connect")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Connect to known devices on startup")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (val, text) in [(true, "On"), (false, "Off")] {
                            let is_selected = behavior_config.auto_connect == val;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                // Save behavior config separately
                                let mut full_config = Config::load();
                                full_config.behavior.auto_connect = val;
                                let _ = full_config.save();
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Auto-sync toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Auto-Sync History")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Download history when connecting to device")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (val, text) in [(true, "On"), (false, "Off")] {
                            let is_selected = behavior_config.auto_sync == val;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                let mut full_config = Config::load();
                                full_config.behavior.auto_sync = val;
                                let _ = full_config.save();
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Remember devices toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Remember Devices")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Save connected devices to database")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (val, text) in [(true, "On"), (false, "Off")] {
                            let is_selected = behavior_config.remember_devices == val;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                let mut full_config = Config::load();
                                full_config.behavior.remember_devices = val;
                                let _ = full_config.save();
                            }
                        }
                    });
                });

                ui.add_space(self.theme.spacing.md);

                // Load cache toggle
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Load Cached Data")
                                .size(self.theme.typography.body)
                                .color(self.theme.text_primary),
                        );
                        ui.label(
                            RichText::new("Load devices and readings from database on startup")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (val, text) in [(true, "On"), (false, "Off")] {
                            let is_selected = behavior_config.load_cache == val;
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
                            .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                            if ui.add(btn).clicked() && !is_selected {
                                let mut full_config = Config::load();
                                full_config.behavior.load_cache = val;
                                let _ = full_config.save();
                            }
                        }
                    });
                });

                // Save config if any setting changed
                if config_changed {
                    self.save_gui_config();
                }
            });
    }
}
