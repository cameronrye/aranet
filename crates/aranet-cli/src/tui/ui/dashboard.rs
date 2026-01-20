//! Dashboard-related UI components for the TUI.
//!
//! This module contains functions for rendering the device list,
//! readings panel, and sparkline visualization on the main dashboard.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Sparkline};

use aranet_types::DeviceType;

use super::colors::{battery_color, co2_color, radon_color};
use super::rssi_display;
use super::theme::{AppTheme, BORDER_TYPE};
use super::widgets::{
    co2_trend, convert_radon_for_device, format_radon_for_device, format_temp_for_device,
    radon_unit_for_device, resample_sparkline_data, sparkline_data,
};
use crate::tui::app::{App, ConnectionStatus, DeviceFilter, calculate_radon_averages};

/// Create a bordered reading card with status-aware border color.
fn reading_card(
    title: &str,
    value: &str,
    color: Color,
    trend: Option<(&str, Color)>,
    theme: &AppTheme,
) -> Paragraph<'static> {
    let mut spans = vec![Span::styled(
        value.to_string(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )];

    if let Some((trend_str, trend_color)) = trend {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            trend_str.to_string(),
            Style::default().fg(trend_color),
        ));
    }

    // Use the value color for the border to create visual cohesion
    let border_color = color;

    Paragraph::new(Line::from(spans))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BORDER_TYPE)
                .border_style(Style::default().fg(border_color))
                .title(format!(" {} ", title))
                .title_style(Style::default().fg(theme.text_primary)),
        )
}

/// Draw the device list panel.
pub(super) fn draw_device_list(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();
    let filtered = app.filtered_devices();
    let filter_label = if app.device_filter != DeviceFilter::All {
        format!(" [{}]", app.device_filter.label())
    } else {
        String::new()
    };
    let title = format!(" Devices ({}){}  ", filtered.len(), filter_label);

    let items: Vec<ListItem> = filtered
        .iter()
        .enumerate()
        .map(|(i, device)| {
            let name = device.display_name().chars().take(18).collect::<String>();

            let (status_icon, icon_color) = match &device.status {
                ConnectionStatus::Connected => ("*", theme.success),
                ConnectionStatus::Connecting => (app.spinner_char(), theme.warning),
                ConnectionStatus::Error(_) => ("x", theme.danger),
                ConnectionStatus::Disconnected => ("o", theme.text_muted),
            };

            let is_selected = i == app.selected_device;
            let prefix = if is_selected { "> " } else { "  " };

            let name_style = if is_selected {
                Style::default()
                    .fg(theme.text_primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_secondary)
            };

            let mut spans = vec![
                Span::styled(prefix, Style::default().fg(theme.primary)),
                Span::styled(status_icon, Style::default().fg(icon_color)),
                Span::raw(" "),
                Span::styled(name, name_style),
            ];

            // Add RSSI indicator for connected devices
            if matches!(device.status, ConnectionStatus::Connected) {
                if let Some(rssi) = device.rssi {
                    let (bars, color) = rssi_display(rssi);
                    spans.push(Span::styled(
                        format!(" {}", bars),
                        Style::default().fg(color),
                    ));
                }
                // Add uptime for connected devices
                if let Some(uptime) = device.uptime() {
                    spans.push(Span::styled(
                        format!(" ({})", uptime),
                        Style::default().fg(theme.text_muted),
                    ));
                }
            }

            let line = Line::from(spans);

            let style = if is_selected {
                theme.selected_style()
            } else {
                Style::default()
            };

            ListItem::new(line).style(style)
        })
        .collect();

    let block = Block::default()
        .title(title)
        .title_style(theme.title_style())
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(theme.border_active_style());

    if items.is_empty() {
        // Show improved empty state
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "    No devices found",
                Style::default().fg(theme.text_muted),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("    Press ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    "s",
                    Style::default()
                        .fg(theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to scan", Style::default().fg(theme.text_muted)),
            ]),
        ];
        let hint = Paragraph::new(lines).block(block);
        frame.render_widget(hint, area);
        return;
    }

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Draw the readings panel for the selected device.
pub(super) fn draw_readings_panel(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    // Build device title
    let title = if let Some(device) = app.selected_device() {
        let name = device.display_name();
        let device_type = device
            .device_type
            .map(|dt| format!(" ({:?})", dt))
            .unwrap_or_default();
        format!(" {}{} ", name, device_type)
    } else {
        " Readings ".to_string()
    };

    let block = Block::default()
        .title(title)
        .title_style(theme.title_style())
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(theme.border_active_style());

    // No devices or no selection
    if app.devices.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                "No devices found",
                Style::default().fg(theme.text_muted),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Press ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    "s",
                    Style::default()
                        .fg(theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " to scan for devices",
                    Style::default().fg(theme.text_muted),
                ),
            ]),
        ];
        let msg = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    if app.selected_device >= app.devices.len() {
        let msg = Paragraph::new("Select a device")
            .style(Style::default().fg(theme.text_muted))
            .alignment(Alignment::Center)
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let device = &app.devices[app.selected_device];

    // Device has no reading
    let Some(reading) = &device.reading else {
        let status_msg = match &device.status {
            ConnectionStatus::Connecting => "Connecting...",
            ConnectionStatus::Error(e) => e.as_str(),
            _ => "Press [c] to connect",
        };
        let lines = vec![
            Line::from(""),
            Line::from(""),
            Line::from(Span::styled(
                "No data available",
                Style::default().fg(theme.text_muted),
            )),
            Line::from(""),
            Line::from(Span::styled(status_msg, Style::default().fg(theme.warning))),
        ];
        let msg = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(block);
        frame.render_widget(msg, area);
        return;
    };

    // Check if there's an active alert for this device
    let has_alert = app.alerts.iter().any(|a| a.device_id == device.id);
    let alert_height = if has_alert { 2 } else { 0 };

    // Split into CO2 display area, sparkline, other readings, and footer
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Check if we have history data for sparkline
    let has_history = !device.history.is_empty();
    let sparkline_height = if has_history { 3 } else { 0 };

    // Use card-based layout with 2 columns
    let readings_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(alert_height as u16), // Alert banner (if active)
            Constraint::Length(3),                   // Device name header
            Constraint::Length(5),                   // Row 1: Primary reading + Temperature
            Constraint::Length(5),                   // Row 2: Humidity + Pressure
            Constraint::Length(5),                   // Row 3: Battery + Age
            Constraint::Length(sparkline_height as u16), // Sparkline (if data available)
            Constraint::Length(1),                   // Radon averages (if applicable)
            Constraint::Length(1),                   // Session stats
            Constraint::Min(0),                      // Remaining space
        ])
        .split(inner);

    // Draw alert banner if active
    if has_alert && let Some(alert) = app.alerts.iter().find(|a| a.device_id == device.id) {
        let alert_style = Style::default()
            .fg(theme.text_primary)
            .bg(alert.severity.color())
            .add_modifier(Modifier::BOLD);
        let alert_text = if app.sticky_alerts {
            format!(" {} {} (sticky) ", alert.severity.icon(), alert.message)
        } else {
            format!(" {} {} ", alert.severity.icon(), alert.message)
        };
        let alert_para = Paragraph::new(alert_text)
            .style(alert_style)
            .alignment(Alignment::Center);
        frame.render_widget(alert_para, readings_layout[0]);
    }

    // Device name header with uptime
    let name = device.display_name();
    let header_text = if let Some(uptime) = device.uptime() {
        format!("{} ({})", name, uptime)
    } else {
        name.to_string()
    };
    let header = Paragraph::new(header_text)
        .style(theme.title_style())
        .alignment(Alignment::Center);
    frame.render_widget(header, readings_layout[1]);

    // Row 1: Primary reading (CO2/Radon/Radiation) + Temperature
    let row1_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(readings_layout[2]);

    // Get device settings for unit formatting
    let settings = device.settings.as_ref();

    // Primary reading card based on device type
    if let Some(radon) = reading.radon {
        // Radon device - use device's radon unit setting
        let color = radon_color(radon);
        let radon_display = format_radon_for_device(radon, settings);
        let card = reading_card("Radon", &radon_display, color, None, &theme);
        frame.render_widget(card, row1_cols[0]);
    } else if let Some(rate) = reading.radiation_rate {
        // Radiation device
        let card = reading_card(
            "Radiation",
            &format!("{:.2} uSv/h", rate),
            Color::Magenta,
            None,
            &theme,
        );
        frame.render_widget(card, row1_cols[0]);
    } else if reading.co2 > 0 {
        // Aranet4 - CO2
        let color = co2_color(reading.co2);
        let trend = co2_trend(reading.co2, device.previous_reading.as_ref().map(|r| r.co2));
        let card = reading_card("CO2", &format!("{} ppm", reading.co2), color, trend, &theme);
        frame.render_widget(card, row1_cols[0]);
    } else {
        // Aranet2 - Temperature is primary (show in first position)
        let temp_display = format_temp_for_device(reading.temperature, settings);
        let card = reading_card("Temperature", &temp_display, theme.warning, None, &theme);
        frame.render_widget(card, row1_cols[0]);
    }

    // Temperature card (only if not Aranet2, since it's already shown as primary)
    if reading.co2 > 0 || reading.radon.is_some() || reading.radiation_rate.is_some() {
        let temp_display = format_temp_for_device(reading.temperature, settings);
        let card = reading_card("Temperature", &temp_display, theme.warning, None, &theme);
        frame.render_widget(card, row1_cols[1]);
    } else {
        // For Aranet2, show humidity in the second slot of row 1
        let card = reading_card(
            "Humidity",
            &format!("{}%", reading.humidity),
            theme.info,
            None,
            &theme,
        );
        frame.render_widget(card, row1_cols[1]);
    }

    // Row 2: Humidity + Pressure
    let row2_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(readings_layout[3]);

    // Humidity card (skip if Aranet2 since it's already shown)
    if reading.co2 > 0 || reading.radon.is_some() || reading.radiation_rate.is_some() {
        let card = reading_card(
            "Humidity",
            &format!("{}%", reading.humidity),
            theme.info,
            None,
            &theme,
        );
        frame.render_widget(card, row2_cols[0]);
    } else {
        // For Aranet2, show battery in row 2 left slot
        let color = battery_color(reading.battery);
        let card = reading_card(
            "Battery",
            &format!("{}%", reading.battery),
            color,
            None,
            &theme,
        );
        frame.render_widget(card, row2_cols[0]);
    }

    // Pressure card (if available)
    if reading.pressure > 0.0 {
        let card = reading_card(
            "Pressure",
            &format!("{:.0} hPa", reading.pressure),
            theme.text_primary,
            None,
            &theme,
        );
        frame.render_widget(card, row2_cols[1]);
    } else if reading.co2 == 0 && reading.radon.is_none() && reading.radiation_rate.is_none() {
        // For Aranet2 without pressure, show age in row 2 right slot
        let age_str = if reading.age < 60 {
            format!("{}s ago", reading.age)
        } else if reading.age < 3600 {
            format!("{}m ago", reading.age / 60)
        } else {
            format!("{}h ago", reading.age / 3600)
        };
        let is_stale = reading.age > reading.interval * 2;
        let age_color = if is_stale {
            theme.danger
        } else {
            theme.text_muted
        };
        let card = reading_card("Age", &age_str, age_color, None, &theme);
        frame.render_widget(card, row2_cols[1]);
    }

    // Row 3: Battery + Age
    let row3_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(readings_layout[4]);

    // Battery card (skip if Aranet2 since it's already shown)
    if reading.co2 > 0 || reading.radon.is_some() || reading.radiation_rate.is_some() {
        let color = battery_color(reading.battery);
        let card = reading_card(
            "Battery",
            &format!("{}%", reading.battery),
            color,
            None,
            &theme,
        );
        frame.render_widget(card, row3_cols[0]);

        // Age card
        let age_str = if reading.age < 60 {
            format!("{}s ago", reading.age)
        } else if reading.age < 3600 {
            format!("{}m ago", reading.age / 60)
        } else {
            format!("{}h ago", reading.age / 3600)
        };
        let is_stale = reading.age > reading.interval * 2;
        let age_color = if is_stale {
            theme.danger
        } else {
            theme.text_muted
        };
        let card = reading_card("Age", &age_str, age_color, None, &theme);
        frame.render_widget(card, row3_cols[1]);
    }

    // Sparkline for history (CO2 for Aranet4, radon for AranetRadon)
    if has_history {
        let history_data = sparkline_data(&device.history, device.device_type);
        if !history_data.is_empty() {
            // Determine sparkline color based on device type and current reading
            let sparkline_color = match device.device_type {
                Some(DeviceType::AranetRadon) => {
                    // Use radon value from reading (stored in co2 field for current reading)
                    // or get from history if available
                    if let Some(last_radon) = device.history.last().and_then(|r| r.radon) {
                        radon_color(last_radon)
                    } else {
                        theme.info // Default for radon
                    }
                }
                Some(DeviceType::AranetRadiation) => Color::Magenta,
                _ => {
                    // CO2-based coloring for Aranet4
                    if reading.co2 > 0 {
                        co2_color(reading.co2)
                    } else {
                        theme.text_primary
                    }
                }
            };

            // Resample data to fill the entire width
            let sparkline_width = readings_layout[5].width as usize;
            let resampled_data = resample_sparkline_data(&history_data, sparkline_width);
            let sparkline = Sparkline::default()
                .data(&resampled_data)
                .style(Style::default().fg(sparkline_color));
            frame.render_widget(sparkline, readings_layout[5]);
        }
    }

    // Radon averages (for radon devices with history)
    if matches!(device.device_type, Some(DeviceType::AranetRadon)) && !device.history.is_empty() {
        let (day_avg, week_avg) = calculate_radon_averages(&device.history);
        let radon_unit = radon_unit_for_device(settings);

        let mut avg_spans = vec![Span::styled(
            "  Averages  ",
            Style::default().fg(theme.text_muted),
        )];

        if let Some(avg) = day_avg {
            let avg_display = convert_radon_for_device(avg, settings);
            avg_spans.push(Span::styled("24h: ", Style::default().fg(theme.text_muted)));
            avg_spans.push(Span::styled(
                format!("{:.1}", avg_display),
                Style::default().fg(radon_color(avg)),
            ));
            avg_spans.push(Span::raw("  "));
        }

        if let Some(avg) = week_avg {
            let avg_display = convert_radon_for_device(avg, settings);
            avg_spans.push(Span::styled("7d: ", Style::default().fg(theme.text_muted)));
            avg_spans.push(Span::styled(
                format!("{:.1}", avg_display),
                Style::default().fg(radon_color(avg)),
            ));
        }

        avg_spans.push(Span::styled(
            format!(" {}", radon_unit),
            Style::default().fg(theme.text_muted),
        ));

        let avg_line = Line::from(avg_spans);
        let avg_para = Paragraph::new(avg_line);
        frame.render_widget(avg_para, readings_layout[6]);
    }

    // Session statistics (if available)
    if device.session_stats.co2_count > 0 {
        let stats = &device.session_stats;
        let stats_line = Line::from(vec![
            Span::styled("  Stats  ", Style::default().fg(theme.text_muted)),
            Span::styled("Min: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}", stats.co2_min.unwrap_or(0)),
                Style::default().fg(theme.success),
            ),
            Span::styled("  Max: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}", stats.co2_max.unwrap_or(0)),
                Style::default().fg(theme.danger),
            ),
            Span::styled("  Avg: ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}", stats.co2_avg().unwrap_or(0)),
                Style::default().fg(theme.warning),
            ),
            Span::styled(" ppm", Style::default().fg(theme.text_muted)),
        ]);
        let stats_para = Paragraph::new(stats_line);
        frame.render_widget(stats_para, readings_layout[7]);
    }
}
