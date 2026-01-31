//! History charts panel rendering.
//!
//! This module contains the history panel rendering logic,
//! including time-series charts for CO2, radon, radiation, temperature, and humidity.

use aranet_core::messages::Command;
use aranet_core::settings::{RadonUnit, TemperatureUnit};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{HLine, Legend, Line, Plot, PlotPoints};

use crate::gui::app::AranetApp;
use crate::gui::components;
use crate::gui::helpers::{bq_to_pci, celsius_to_fahrenheit};
use crate::gui::types::{DeviceState, HistoryFilter};

impl AranetApp {
    /// Render the history panel with charts.
    pub(crate) fn render_history_panel(&mut self, ui: &mut egui::Ui, device: &DeviceState) {
        // Header with title and sync button
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} - History", device.display_name()))
                    .size(self.theme.typography.heading)
                    .strong()
                    .color(self.theme.text_primary),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if device.syncing_history {
                    components::loading_indicator(ui, &self.theme, Some("Syncing..."));
                } else {
                    let btn = egui::Button::new(
                        RichText::new("Sync History")
                            .size(self.theme.typography.body)
                            .color(self.theme.text_on_accent),
                    )
                    .fill(self.theme.accent);
                    if ui
                        .add(btn)
                        .on_hover_text("Download history from device")
                        .clicked()
                    {
                        self.send_command(Command::SyncHistory {
                            device_id: device.id.clone(),
                        });
                    }
                }
            });
        });

        ui.add_space(self.theme.spacing.md);

        // Filter segmented control
        let filter_options = [
            (HistoryFilter::All, HistoryFilter::All.label()),
            (
                HistoryFilter::Last24Hours,
                HistoryFilter::Last24Hours.label(),
            ),
            (HistoryFilter::Last7Days, HistoryFilter::Last7Days.label()),
            (HistoryFilter::Last30Days, HistoryFilter::Last30Days.label()),
            (HistoryFilter::Custom, HistoryFilter::Custom.label()),
        ];

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Time Range:")
                    .size(self.theme.typography.body)
                    .color(self.theme.text_secondary),
            );
            ui.add_space(self.theme.spacing.sm);

            for (filter, label) in filter_options {
                let is_selected = self.history_filter == filter;
                let (bg, text_color) = if is_selected {
                    (self.theme.accent, self.theme.text_on_accent)
                } else {
                    (self.theme.bg_card, self.theme.text_secondary)
                };

                let btn = egui::Button::new(
                    RichText::new(label)
                        .size(self.theme.typography.caption)
                        .color(text_color),
                )
                .fill(bg)
                .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                if ui.add(btn).clicked() {
                    self.history_filter = filter;
                    // Initialize date fields with sensible defaults when switching to Custom
                    if filter == HistoryFilter::Custom
                        && self.custom_date_start.is_empty()
                        && self.custom_date_end.is_empty()
                    {
                        let now = time::OffsetDateTime::now_utc();
                        let week_ago = now - time::Duration::days(7);
                        self.custom_date_start = format!(
                            "{:04}-{:02}-{:02}",
                            week_ago.year(),
                            week_ago.month() as u8,
                            week_ago.day()
                        );
                        self.custom_date_end = format!(
                            "{:04}-{:02}-{:02}",
                            now.year(),
                            now.month() as u8,
                            now.day()
                        );
                    }
                }
            }

            // Sync status indicator (right-aligned)
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(last_sync) = device.last_sync {
                    let now = time::OffsetDateTime::now_utc();
                    let duration = now - last_sync;
                    let sync_text = if duration < time::Duration::minutes(1) {
                        "just now".to_string()
                    } else if duration < time::Duration::hours(1) {
                        format!("{} min ago", duration.whole_minutes())
                    } else if duration < time::Duration::hours(24) {
                        format!("{} hr ago", duration.whole_hours())
                    } else {
                        format!("{} days ago", duration.whole_days())
                    };
                    ui.label(
                        RichText::new(format!("Last synced: {}", sync_text))
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_muted),
                    );
                }
            });
        });

        // Custom date range inputs (only shown when Custom filter is selected)
        if self.history_filter == HistoryFilter::Custom {
            ui.add_space(self.theme.spacing.sm);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("From:")
                        .size(self.theme.typography.caption)
                        .color(self.theme.text_secondary),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.custom_date_start)
                        .hint_text("YYYY-MM-DD")
                        .desired_width(90.0)
                        .font(egui::FontId::proportional(self.theme.typography.caption)),
                );
                ui.add_space(self.theme.spacing.sm);
                ui.label(
                    RichText::new("To:")
                        .size(self.theme.typography.caption)
                        .color(self.theme.text_secondary),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.custom_date_end)
                        .hint_text("YYYY-MM-DD")
                        .desired_width(90.0)
                        .font(egui::FontId::proportional(self.theme.typography.caption)),
                );
                ui.add_space(self.theme.spacing.sm);
                ui.label(
                    RichText::new("(YYYY-MM-DD format)")
                        .size(self.theme.typography.caption)
                        .color(self.theme.text_muted),
                );
            });
        }

        ui.add_space(self.theme.spacing.lg);
        ui.separator();
        ui.add_space(self.theme.spacing.md);

        if device.history.is_empty() {
            components::empty_state(
                ui,
                &self.theme,
                "No History Data",
                "Click 'Sync History' to download data from your device",
            );
            return;
        }

        let now = time::OffsetDateTime::now_utc();

        // Parse custom date range if Custom filter is selected
        let (custom_start, custom_end) = if self.history_filter == HistoryFilter::Custom {
            let parse_date = |s: &str| -> Option<time::OffsetDateTime> {
                let parts: Vec<&str> = s.trim().split('-').collect();
                if parts.len() != 3 {
                    return None;
                }
                let year: i32 = parts[0].parse().ok()?;
                let month: u8 = parts[1].parse().ok()?;
                let day: u8 = parts[2].parse().ok()?;
                let month = time::Month::try_from(month).ok()?;
                let date = time::Date::from_calendar_date(year, month, day).ok()?;
                Some(date.with_hms(0, 0, 0).ok()?.assume_utc())
            };
            let start = parse_date(&self.custom_date_start);
            let end = parse_date(&self.custom_date_end)
                .map(|d| d + time::Duration::days(1) - time::Duration::seconds(1)); // End of day
            (start, end)
        } else {
            (None, None)
        };

        let filtered: Vec<_> = device
            .history
            .iter()
            .filter(|r| match self.history_filter {
                HistoryFilter::All => true,
                HistoryFilter::Last24Hours => (now - r.timestamp) < time::Duration::hours(24),
                HistoryFilter::Last7Days => (now - r.timestamp) < time::Duration::days(7),
                HistoryFilter::Last30Days => (now - r.timestamp) < time::Duration::days(30),
                HistoryFilter::Custom => {
                    let after_start = custom_start.is_none_or(|s| r.timestamp >= s);
                    let before_end = custom_end.is_none_or(|e| r.timestamp <= e);
                    after_start && before_end
                }
            })
            .collect();

        // Record count badge and export buttons
        ui.horizontal(|ui| {
            components::status_badge(
                ui,
                &self.theme,
                &format!("{} records", filtered.len()),
                self.theme.info,
            );
            if filtered.len() != device.history.len() {
                ui.add_space(self.theme.spacing.sm);
                ui.label(
                    RichText::new(format!("of {} total", device.history.len()))
                        .size(self.theme.typography.caption)
                        .color(self.theme.text_muted),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Export JSON button
                if ui
                    .add(egui::Button::new(
                        RichText::new("Export JSON")
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_secondary),
                    ))
                    .on_hover_text("Export filtered history to JSON file")
                    .clicked()
                {
                    self.export_history(&filtered, device.display_name(), "json");
                }

                ui.add_space(self.theme.spacing.sm);

                // Export CSV button
                if ui
                    .add(egui::Button::new(
                        RichText::new("Export CSV")
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_secondary),
                    ))
                    .on_hover_text("Export filtered history to CSV file")
                    .clicked()
                {
                    self.export_history(&filtered, device.display_name(), "csv");
                }
            });
        });
        ui.add_space(self.theme.spacing.md);

        let has_co2 = filtered.iter().any(|r| r.co2 > 0);
        let has_radon = filtered.iter().any(|r| r.radon.is_some());
        let has_radiation = filtered.iter().any(|r| r.radiation_rate.is_some());

        let now_secs = time::OffsetDateTime::now_utc().unix_timestamp() as f64;
        let to_hours_ago = |ts: time::OffsetDateTime| -> f64 {
            let secs = ts.unix_timestamp() as f64;
            (now_secs - secs) / 3600.0
        };

        // Plot styling constants
        let plot_height = 160.0;
        let data_count = filtered.len();

        // Calculate max hours based on filter for X-axis bounds
        let max_hours = match self.history_filter {
            HistoryFilter::All => {
                // For "All", calculate actual data range or use 24h default
                filtered
                    .first()
                    .map(|oldest| to_hours_ago(oldest.timestamp).max(24.0))
                    .unwrap_or(24.0)
            }
            HistoryFilter::Last24Hours => 24.0,
            HistoryFilter::Last7Days => 24.0 * 7.0, // 168 hours
            HistoryFilter::Last30Days => 24.0 * 30.0, // 720 hours
            HistoryFilter::Custom => {
                // For custom date range, use oldest timestamp in filtered data
                filtered
                    .first()
                    .map(|oldest| to_hours_ago(oldest.timestamp).max(24.0))
                    .unwrap_or(24.0)
            }
        };

        egui::ScrollArea::vertical().show(ui, |ui| {
            if has_co2 {
                self.render_chart_section(
                    ui,
                    "CO2",
                    "ppm",
                    || {
                        let co2_points: PlotPoints = filtered
                            .iter()
                            .map(|r| [-to_hours_ago(r.timestamp), r.co2 as f64])
                            .collect();
                        (co2_points, self.theme.info)
                    },
                    plot_height,
                    Some(vec![
                        (800.0, "Good", self.theme.success),
                        (1000.0, "Moderate", self.theme.warning),
                        (1500.0, "Poor", self.theme.danger),
                    ]),
                    max_hours,
                    data_count,
                );
            }

            if has_radon {
                // Use device settings for radon unit
                let use_pci = device
                    .settings
                    .as_ref()
                    .map(|s| s.radon_unit == RadonUnit::PciL)
                    .unwrap_or(false);
                let radon_unit_label = if use_pci { "pCi/L" } else { "Bq/m3" };
                // Threshold lines (convert if using pCi/L)
                let thresholds = if use_pci {
                    vec![
                        (100.0 * 0.027, "Action", self.theme.warning),
                        (300.0 * 0.027, "High", self.theme.danger),
                    ]
                } else {
                    vec![
                        (100.0, "Action", self.theme.warning),
                        (300.0, "High", self.theme.danger),
                    ]
                };
                self.render_chart_section(
                    ui,
                    "Radon",
                    radon_unit_label,
                    || {
                        let radon_points: PlotPoints = filtered
                            .iter()
                            .filter_map(|r| {
                                r.radon.map(|v| {
                                    let value = if use_pci {
                                        bq_to_pci(v) as f64
                                    } else {
                                        v as f64
                                    };
                                    [-to_hours_ago(r.timestamp), value]
                                })
                            })
                            .collect();
                        (radon_points, self.theme.warning)
                    },
                    plot_height,
                    Some(thresholds),
                    max_hours,
                    data_count,
                );
            }

            if has_radiation {
                self.render_chart_section(
                    ui,
                    "Radiation Rate",
                    "uSv/h",
                    || {
                        let radiation_points: PlotPoints = filtered
                            .iter()
                            .filter_map(|r| {
                                r.radiation_rate
                                    .map(|v| [-to_hours_ago(r.timestamp), v as f64])
                            })
                            .collect();
                        (radiation_points, self.theme.danger)
                    },
                    plot_height,
                    None,
                    max_hours,
                    data_count,
                );
            }

            // Use device settings for temperature unit
            let use_fahrenheit = device
                .settings
                .as_ref()
                .map(|s| s.temperature_unit == TemperatureUnit::Fahrenheit)
                .unwrap_or(false);
            let temp_unit_label = if use_fahrenheit { "F" } else { "C" };

            // Toggle for overlay mode
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Temperature & Humidity")
                        .size(self.theme.typography.body)
                        .color(self.theme.text_secondary),
                );
                ui.add_space(self.theme.spacing.sm);

                let overlay_text = if self.show_temp_humidity_overlay {
                    "Separate Charts"
                } else {
                    "Overlay Charts"
                };
                let toggle_btn = egui::Button::new(
                    RichText::new(overlay_text)
                        .size(self.theme.typography.caption)
                        .color(self.theme.text_secondary),
                )
                .fill(self.theme.bg_secondary)
                .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                if ui
                    .add(toggle_btn)
                    .on_hover_text("Toggle between separate and combined charts")
                    .clicked()
                {
                    self.show_temp_humidity_overlay = !self.show_temp_humidity_overlay;
                }
            });
            ui.add_space(self.theme.spacing.sm);

            if self.show_temp_humidity_overlay {
                // Combined Temperature & Humidity overlay chart
                self.render_temp_humidity_overlay(
                    ui,
                    &filtered,
                    use_fahrenheit,
                    temp_unit_label,
                    plot_height,
                    max_hours,
                    data_count,
                    &to_hours_ago,
                );
            } else {
                // Separate charts
                self.render_chart_section(
                    ui,
                    "Temperature",
                    temp_unit_label,
                    || {
                        let temp_points: PlotPoints = filtered
                            .iter()
                            .map(|r| {
                                let value = if use_fahrenheit {
                                    celsius_to_fahrenheit(r.temperature) as f64
                                } else {
                                    r.temperature as f64
                                };
                                [-to_hours_ago(r.timestamp), value]
                            })
                            .collect();
                        (temp_points, self.theme.chart_temperature)
                    },
                    plot_height,
                    None,
                    max_hours,
                    data_count,
                );

                self.render_chart_section(
                    ui,
                    "Humidity",
                    "%",
                    || {
                        let humidity_points: PlotPoints = filtered
                            .iter()
                            .map(|r| [-to_hours_ago(r.timestamp), r.humidity as f64])
                            .collect();
                        (humidity_points, self.theme.chart_humidity)
                    },
                    plot_height,
                    None,
                    max_hours,
                    data_count,
                );
            }
        });
    }

    /// Render a chart section with consistent styling.
    ///
    /// # Arguments
    /// * `ui` - The egui UI context
    /// * `title` - Chart title (e.g., "CO2")
    /// * `unit` - Unit label (e.g., "ppm")
    /// * `data_fn` - Function returning plot points and line color
    /// * `height` - Chart height in pixels
    /// * `thresholds` - Optional threshold lines
    /// * `max_hours` - Maximum hours range for X-axis bounds
    /// * `data_count` - Number of data points for display
    #[allow(clippy::too_many_arguments)]
    fn render_chart_section<F>(
        &self,
        ui: &mut egui::Ui,
        title: &str,
        unit: &str,
        data_fn: F,
        height: f32,
        thresholds: Option<Vec<(f64, &str, Color32)>>,
        max_hours: f64,
        data_count: usize,
    ) where
        F: FnOnce() -> (PlotPoints<'static>, Color32),
    {
        let plot_id = format!("{}_plot", title.to_lowercase().replace(' ', "_"));

        egui::Frame::new()
            .fill(self.theme.bg_card)
            .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
            .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
            .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
            .show(ui, |ui| {
                // Header row with title, unit, data count indicator, and reset button
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(title)
                            .size(self.theme.typography.subheading)
                            .strong()
                            .color(self.theme.text_primary),
                    );
                    ui.label(
                        RichText::new(format!("({})", unit))
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_muted),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Reset View button
                        let reset_btn = egui::Button::new(
                            RichText::new("Reset View")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        )
                        .fill(self.theme.bg_secondary)
                        .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                        if ui
                            .add(reset_btn)
                            .on_hover_text("Double-click chart to reset view")
                            .clicked()
                        {
                            // Reset plot bounds by enabling auto-bounds
                            if let Some(mut plot_memory) =
                                egui_plot::PlotMemory::load(ui.ctx(), egui::Id::new(&plot_id))
                            {
                                plot_memory.auto_bounds = egui::Vec2b::TRUE;
                                plot_memory.store(ui.ctx(), egui::Id::new(&plot_id));
                            }
                        }

                        // Data count indicator
                        ui.label(
                            RichText::new(format!("{} points", data_count))
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
                });
                ui.add_space(self.theme.spacing.sm);

                let (points, line_color) = data_fn();
                let has_thresholds = thresholds.is_some();

                let mut plot = Plot::new(&plot_id)
                    .height(height)
                    .show_axes(true)
                    .show_grid(true)
                    // Disable scroll - prevents losing data in infinite scroll
                    .allow_scroll(false)
                    // Allow drag on X-axis only (time navigation)
                    .allow_drag([true, false])
                    // Allow zoom on X-axis only, Y auto-fits
                    .allow_zoom([true, false])
                    // Keep boxed zoom for precise selection
                    .allow_boxed_zoom(true)
                    // Enable double-click to reset view
                    .allow_double_click_reset(true)
                    // Set default X bounds based on filter (negative hours ago)
                    .default_x_bounds(-max_hours, 0.0)
                    // Add margin so edge points aren't clipped
                    .set_margin_fraction(egui::vec2(0.02, 0.1))
                    // X fixed to filter range, Y auto-fits data
                    .auto_bounds([false, true])
                    // Clamp grid to data range
                    .clamp_grid(true)
                    // Include origin (current time) for context
                    .include_x(0.0)
                    .x_axis_label("Hours ago");

                // Add legend if we have threshold lines
                if has_thresholds {
                    plot = plot.legend(Legend::default());
                }

                plot.show(ui, |plot_ui| {
                    if let Some(ref thresh) = thresholds {
                        for (value, label, color) in thresh {
                            plot_ui.hline(
                                HLine::new(*label, *value)
                                    .color(*color)
                                    .style(egui_plot::LineStyle::dashed_dense()),
                            );
                        }
                    }
                    plot_ui.line(Line::new(title, points).color(line_color).width(2.0));
                });
            });
        ui.add_space(self.theme.spacing.md);
    }

    /// Render a combined Temperature & Humidity overlay chart.
    ///
    /// This shows both temperature and humidity on the same chart with normalized Y-axis
    /// (temperature scaled to 0-100 range to match humidity percentage).
    #[allow(clippy::too_many_arguments)]
    fn render_temp_humidity_overlay<F>(
        &self,
        ui: &mut egui::Ui,
        filtered: &[&aranet_types::HistoryRecord],
        use_fahrenheit: bool,
        temp_unit_label: &str,
        height: f32,
        max_hours: f64,
        data_count: usize,
        to_hours_ago: &F,
    ) where
        F: Fn(time::OffsetDateTime) -> f64,
    {
        let plot_id = "temp_humidity_overlay_plot";

        egui::Frame::new()
            .fill(self.theme.bg_card)
            .inner_margin(egui::Margin::same(self.theme.spacing.md as i8))
            .corner_radius(egui::CornerRadius::same(self.theme.rounding.md as u8))
            .stroke(egui::Stroke::new(1.0, self.theme.border_subtle))
            .show(ui, |ui| {
                // Header row
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Temperature & Humidity")
                            .size(self.theme.typography.subheading)
                            .strong()
                            .color(self.theme.text_primary),
                    );
                    ui.label(
                        RichText::new(format!("(°{} / %)", temp_unit_label))
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_muted),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Reset View button
                        let reset_btn = egui::Button::new(
                            RichText::new("Reset View")
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_secondary),
                        )
                        .fill(self.theme.bg_secondary)
                        .corner_radius(egui::CornerRadius::same(self.theme.rounding.sm as u8));

                        if ui
                            .add(reset_btn)
                            .on_hover_text("Double-click chart to reset view")
                            .clicked()
                            && let Some(mut plot_memory) =
                                egui_plot::PlotMemory::load(ui.ctx(), egui::Id::new(plot_id))
                        {
                            plot_memory.auto_bounds = egui::Vec2b::TRUE;
                            plot_memory.store(ui.ctx(), egui::Id::new(plot_id));
                        }

                        // Data count indicator
                        ui.label(
                            RichText::new(format!("{} points", data_count))
                                .size(self.theme.typography.caption)
                                .color(self.theme.text_muted),
                        );
                    });
                });
                ui.add_space(self.theme.spacing.sm);

                // Prepare data points
                let temp_points: PlotPoints = filtered
                    .iter()
                    .map(|r| {
                        let value = if use_fahrenheit {
                            celsius_to_fahrenheit(r.temperature) as f64
                        } else {
                            r.temperature as f64
                        };
                        [-to_hours_ago(r.timestamp), value]
                    })
                    .collect();

                let humidity_points: PlotPoints = filtered
                    .iter()
                    .map(|r| [-to_hours_ago(r.timestamp), r.humidity as f64])
                    .collect();

                let plot = Plot::new(plot_id)
                    .height(height + 40.0) // Taller for combined view
                    .show_axes(true)
                    .show_grid(true)
                    .allow_scroll(false)
                    .allow_drag([true, false])
                    .allow_zoom([true, false])
                    .allow_boxed_zoom(true)
                    .allow_double_click_reset(true)
                    .default_x_bounds(-max_hours, 0.0)
                    .set_margin_fraction(egui::vec2(0.02, 0.1))
                    .auto_bounds([false, true])
                    .clamp_grid(true)
                    .include_x(0.0)
                    .x_axis_label("Hours ago")
                    .legend(Legend::default());

                plot.show(ui, |plot_ui| {
                    plot_ui.line(
                        Line::new(format!("Temp (°{})", temp_unit_label), temp_points)
                            .color(self.theme.chart_temperature)
                            .width(2.0),
                    );
                    plot_ui.line(
                        Line::new("Humidity (%)", humidity_points)
                            .color(self.theme.chart_humidity)
                            .width(2.0),
                    );
                });

                // Legend explanation
                ui.add_space(self.theme.spacing.xs);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("●")
                            .color(self.theme.chart_temperature)
                            .size(self.theme.typography.caption),
                    );
                    ui.label(
                        RichText::new(format!("Temperature (°{})", temp_unit_label))
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_muted),
                    );
                    ui.add_space(self.theme.spacing.md);
                    ui.label(
                        RichText::new("●")
                            .color(self.theme.chart_humidity)
                            .size(self.theme.typography.caption),
                    );
                    ui.label(
                        RichText::new("Humidity (%)")
                            .size(self.theme.typography.caption)
                            .color(self.theme.text_muted),
                    );
                });
            });
        ui.add_space(self.theme.spacing.md);
    }
}
