//! Overlay rendering functions for the TUI.
//!
//! This module contains all overlay/popup/dialog rendering functions including:
//! - Help overlay
//! - Alert history
//! - Alias editor
//! - Error popup
//! - Confirmation dialog
//! - Fullscreen chart
//! - Comparison view

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Sparkline};

use aranet_types::DeviceType;

use super::colors::{battery_color, co2_color, radon_color};
use super::theme::{AppTheme, BORDER_TYPE};
use super::widgets::{resample_sparkline_data, sparkline_data};
use crate::tui::app::{App, DeviceState, PendingAction};
use crate::tui::errors::format_error_with_guidance;

/// Draw help overlay with keyboard shortcuts.
pub(super) fn draw_help_overlay(frame: &mut Frame) {
    let theme = AppTheme::dark(); // Help overlay uses dark theme for consistency

    let area = frame.area();
    // Use a moderate portion of the screen (70% width, 70% height)
    let width = (area.width * 70 / 100)
        .max(60)
        .min(area.width.saturating_sub(2));
    let height = (area.height * 70 / 100)
        .max(20)
        .min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;

    let help_area = Rect::new(x, y, width, height);
    frame.render_widget(Clear, help_area);

    // Create two-column layout for shortcuts
    let inner_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .margin(1)
        .split(help_area);

    // Left column
    let left_lines = vec![
        Line::from(Span::styled(
            "--- Navigation ---",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        shortcut_line("Tab/Shift+Tab", "Next/Prev tab", &theme),
        shortcut_line("j/k", "Next/Prev device", &theme),
        shortcut_line("l/h", "Next/Prev device", &theme),
        shortcut_line("Enter", "Connect/Disconnect", &theme),
        shortcut_line("PgUp/PgDn", "Scroll history", &theme),
        Line::from(""),
        Line::from(Span::styled(
            "--- Views ---",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        shortcut_line("?", "Toggle help", &theme),
        shortcut_line("g", "Full-screen chart", &theme),
        shortcut_line("v", "Comparison view", &theme),
        shortcut_line("a", "Alert history", &theme),
        shortcut_line("[", "Toggle sidebar", &theme),
        shortcut_line("]", "Toggle sidebar width", &theme),
        Line::from(""),
        Line::from(Span::styled(
            "--- Devices ---",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        shortcut_line("s", "Scan for devices", &theme),
        shortcut_line("r", "Refresh reading", &theme),
        shortcut_line("S", "Sync history", &theme),
        shortcut_line("C", "Connect all", &theme),
        shortcut_line("n", "Set device alias", &theme),
        shortcut_line("f", "Cycle device filter", &theme),
    ];

    // Right column
    let right_lines = vec![
        Line::from(Span::styled(
            "--- Charts ---",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        shortcut_line("T", "Toggle temp on chart", &theme),
        shortcut_line("H", "Toggle humidity on chart", &theme),
        shortcut_line("0-4", "Time filter (History)", &theme),
        Line::from(""),
        Line::from(Span::styled(
            "--- Alerts ---",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        shortcut_line("Esc", "Dismiss alert", &theme),
        shortcut_line("A", "Toggle sticky alerts", &theme),
        shortcut_line("b", "Toggle bell", &theme),
        shortcut_line("D", "Do Not Disturb", &theme),
        shortcut_line("+/-", "Adjust thresholds", &theme),
        Line::from(""),
        Line::from(Span::styled(
            "--- Settings ---",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        shortcut_line("t", "Toggle theme", &theme),
        shortcut_line("B", "Toggle BLE range", &theme),
        shortcut_line("I", "Toggle Smart Home mode", &theme),
        shortcut_line("Enter", "Change interval (Settings)", &theme),
        Line::from(""),
        Line::from(Span::styled(
            "--- Other ---",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        shortcut_line("e", "Export history", &theme),
        shortcut_line("F", "Toggle export format", &theme),
        shortcut_line("E", "Show error details", &theme),
        shortcut_line("q/Ctrl+C", "Quit", &theme),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "Press ? or Esc to close",
            Style::default().fg(theme.text_muted),
        )),
    ];

    let left_para = Paragraph::new(left_lines);
    let right_para = Paragraph::new(right_lines);

    // Draw container
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(theme.border_active_style())
        .title(Span::styled(" Keyboard Shortcuts ", theme.title_style()));

    frame.render_widget(block, help_area);
    frame.render_widget(left_para, inner_layout[0]);
    frame.render_widget(right_para, inner_layout[1]);
}

/// Create a shortcut line with key and description.
fn shortcut_line<'a>(key: &str, desc: &str, theme: &AppTheme) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{:>12} ", key), Style::default().fg(theme.warning)),
        Span::styled(desc.to_string(), Style::default().fg(theme.text_secondary)),
    ])
}

/// Draw alert history overlay.
pub(super) fn draw_alert_history(frame: &mut Frame, app: &App) {
    if !app.show_alert_history {
        return;
    }

    let theme = app.app_theme();

    let area = frame.area();
    let width = (area.width * 3 / 4).min(60);
    let height = (area.height * 3 / 4).min(20);
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;

    let overlay_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay_area);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Press ", Style::default().fg(theme.text_muted)),
            Span::styled(
                "a",
                Style::default()
                    .fg(theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to close ", Style::default().fg(theme.text_muted)),
        ]),
        Line::from(""),
    ];

    if app.alert_history.is_empty() {
        lines.push(Line::from(Span::styled(
            "No alerts recorded",
            Style::default().fg(theme.text_muted).italic(),
        )));
    } else {
        // Show most recent alerts first
        for record in app.alert_history.iter().rev().take(15) {
            let format = time::format_description::parse("[month]-[day] [hour]:[minute]").unwrap();
            let time_str = record.timestamp.format(&format).unwrap_or_default();

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", record.severity.icon()),
                    Style::default().fg(record.severity.color()),
                ),
                Span::styled(
                    format!("{} ", time_str),
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(&record.device_name, Style::default().fg(theme.primary)),
                Span::raw(": "),
                Span::styled(
                    &record.message,
                    Style::default().fg(record.severity.color()),
                ),
            ]));
        }

        if app.alert_history.len() > 15 {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("... and {} more", app.alert_history.len() - 15),
                Style::default().fg(theme.text_muted),
            )));
        }
    }

    let paragraph = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BORDER_TYPE)
            .border_style(Style::default().fg(theme.warning))
            .title(Span::styled(
                " Alert History ",
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            )),
    );

    frame.render_widget(paragraph, overlay_area);
}

/// Draw alias editing overlay.
pub(super) fn draw_alias_editor(frame: &mut Frame, app: &App) {
    if !app.editing_alias {
        return;
    }

    let theme = app.app_theme();

    let area = frame.area();
    let width = 40.min(area.width.saturating_sub(4));
    let height = 5;
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;

    let dialog_area = Rect::new(x, y, width, height);
    frame.render_widget(Clear, dialog_area);

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(" > ", Style::default().fg(theme.primary)),
            Span::styled(&app.alias_input, Style::default().fg(theme.text_primary)),
            Span::styled("_", Style::default().fg(theme.primary)), // Cursor
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Enter", Style::default().fg(theme.success)),
            Span::styled("=Save  ", Style::default().fg(theme.text_muted)),
            Span::styled("Esc", Style::default().fg(theme.danger)),
            Span::styled("=Cancel", Style::default().fg(theme.text_muted)),
        ]),
    ];

    let dialog = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BORDER_TYPE)
            .border_style(theme.border_active_style())
            .title(Span::styled(" Set Device Alias ", theme.title_style())),
    );

    frame.render_widget(dialog, dialog_area);
}

/// Draw error popup overlay.
pub(super) fn draw_error_popup(frame: &mut Frame, app: &App) {
    if !app.show_error_details {
        return;
    }

    let Some(error) = &app.last_error else {
        return;
    };

    let theme = app.app_theme();

    // Get formatted error with guidance
    let (short_message, suggestion) = format_error_with_guidance(error);

    let area = frame.area();
    let width = (area.width * 3 / 4).min(60);
    // Increase height to accommodate suggestion and technical details
    let height = (area.height / 2).min(14);
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;

    let popup_area = Rect::new(x, y, width, height);
    frame.render_widget(Clear, popup_area);

    let mut lines = vec![
        // Main error message (user-friendly)
        Line::from(Span::styled(
            &short_message,
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    // Add suggestion if available
    if let Some(ref suggestion_text) = suggestion {
        lines.push(Line::from(Span::styled(
            suggestion_text.as_str(),
            Style::default().fg(theme.warning),
        )));
        lines.push(Line::from(""));
    }

    // Add technical details (original error) if different from short message
    if short_message != *error {
        lines.push(Line::from(Span::styled(
            "Technical:",
            Style::default().fg(theme.text_muted),
        )));
        lines.push(Line::from(Span::styled(
            error.as_str(),
            Style::default().fg(theme.text_muted),
        )));
        lines.push(Line::from(""));
    }

    // Dismiss instruction
    lines.push(Line::from(vec![
        Span::styled("Press ", Style::default().fg(theme.text_muted)),
        Span::styled(
            "E",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to dismiss", Style::default().fg(theme.text_muted)),
    ]));

    let popup = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BORDER_TYPE)
            .border_style(Style::default().fg(theme.danger))
            .title(Span::styled(
                " Error ",
                Style::default()
                    .fg(theme.danger)
                    .add_modifier(Modifier::BOLD),
            )),
    );

    frame.render_widget(popup, popup_area);
}

/// Draw confirmation dialog overlay.
pub(super) fn draw_confirmation_dialog(frame: &mut Frame, app: &App) {
    if let Some(action) = &app.pending_confirmation {
        let theme = app.app_theme();

        let message = match action {
            PendingAction::Disconnect { device_name, .. } => {
                format!("Disconnect from '{}'?", device_name)
            }
        };

        let area = frame.area();
        let dialog_width = 40.min(area.width.saturating_sub(4));
        let dialog_height = 5;
        let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
        let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;

        let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);

        // Clear background
        frame.render_widget(Clear, dialog_area);

        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                message,
                Style::default().fg(theme.text_primary),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    " [Y]es ",
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    " [N]o ",
                    Style::default()
                        .fg(theme.danger)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ];

        let dialog = Paragraph::new(lines)
            .alignment(ratatui::layout::Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BORDER_TYPE)
                    .border_style(Style::default().fg(theme.warning))
                    .title(Span::styled(
                        " Confirm ",
                        Style::default()
                            .fg(theme.warning)
                            .add_modifier(Modifier::BOLD),
                    )),
            );

        frame.render_widget(dialog, dialog_area);
    }
}

/// Draw full-screen chart overlay.
pub(super) fn draw_fullscreen_chart(frame: &mut Frame, app: &App) {
    if !app.show_fullscreen_chart {
        return;
    }

    let Some(device) = app.selected_device() else {
        return;
    };

    if device.history.is_empty() {
        return;
    }

    let theme = app.app_theme();

    let area = frame.area();

    // Clear background
    frame.render_widget(Clear, area);

    // Get chart data
    let data = sparkline_data(&device.history, device.device_type);
    if data.is_empty() {
        return;
    }

    // Calculate min/max for labels
    let min_val = data.iter().copied().min().unwrap_or(0);
    let max_val = data.iter().copied().max().unwrap_or(0);

    // Determine chart color and title based on device type
    let (title, color) = match device.device_type {
        Some(DeviceType::AranetRadon) => ("Radon (Bq/m3)", theme.info),
        Some(DeviceType::AranetRadiation) => ("Radiation (uSv/h)", Color::Magenta),
        _ => ("CO2 (ppm)", theme.success),
    };

    // Layout: title row, chart area, legend row
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(5),    // Chart
            Constraint::Length(2), // Legend
        ])
        .split(area);

    // Title
    let device_name = device.name.as_deref().unwrap_or(&device.id);
    let title_text = format!(" {} - {} ", device_name, title);
    let title_para = Paragraph::new(title_text)
        .alignment(ratatui::layout::Alignment::Center)
        .style(Style::default().fg(color).add_modifier(Modifier::BOLD))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme.border_inactive)),
        );
    frame.render_widget(title_para, layout[0]);

    // Chart - resample data to fill the entire width (minus borders)
    let chart_width = layout[1].width.saturating_sub(2) as usize;
    let resampled_data = resample_sparkline_data(&data, chart_width);
    let sparkline = Sparkline::default()
        .data(&resampled_data)
        .style(Style::default().fg(color))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BORDER_TYPE)
                .border_style(Style::default().fg(theme.border_inactive)),
        );
    frame.render_widget(sparkline, layout[1]);

    // Legend
    let legend = Line::from(vec![
        Span::styled(
            format!(" Min: {} ", min_val),
            Style::default().fg(theme.success),
        ),
        Span::styled(
            format!(" Max: {} ", max_val),
            Style::default().fg(theme.danger),
        ),
        Span::styled(
            format!(" Points: {} ", data.len()),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled(" | Press ", Style::default().fg(theme.text_muted)),
        Span::styled(
            "g",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" to close ", Style::default().fg(theme.text_muted)),
    ]);
    let legend_para = Paragraph::new(legend).alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(legend_para, layout[2]);
}

/// Draw comparison view overlay.
pub(super) fn draw_comparison_view(frame: &mut Frame, app: &App) {
    let Some(device1) = app.selected_device() else {
        return;
    };

    let Some(device2) = app.comparison_device() else {
        return;
    };

    let theme = app.app_theme();

    let area = frame.area();

    // Clear background
    frame.render_widget(Clear, area);

    // Layout: header, two columns, footer
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Header
            Constraint::Min(10),   // Content
            Constraint::Length(2), // Footer
        ])
        .split(area);

    // Header
    let header = Paragraph::new(" Comparison View ")
        .style(
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(header, layout[0]);

    // Two columns
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(layout[1]);

    // Draw each device
    draw_comparison_device(frame, columns[0], device1, "< Selected", &theme);
    draw_comparison_device(frame, columns[1], device2, "Compare >", &theme);

    // Footer
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            "v",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("=close  ", Style::default().fg(theme.text_muted)),
        Span::styled(
            "</>",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("=change device ", Style::default().fg(theme.text_muted)),
    ]))
    .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(footer, layout[2]);
}

/// Draw a single device in comparison view.
fn draw_comparison_device(
    frame: &mut Frame,
    area: Rect,
    device: &DeviceState,
    label: &str,
    theme: &AppTheme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(Style::default().fg(theme.border_inactive))
        .title(Span::styled(
            format!(" {} - {} ", label, device.display_name()),
            Style::default().fg(theme.text_primary),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(reading) = &device.reading else {
        let no_reading = Paragraph::new("No reading")
            .style(Style::default().fg(theme.text_muted))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(no_reading, inner);
        return;
    };

    let mut lines = Vec::new();

    // CO2 or Radon
    match device.device_type {
        Some(DeviceType::AranetRadon) => {
            if let Some(radon) = reading.radon {
                let color = radon_color(radon);
                lines.push(Line::from(vec![
                    Span::styled("Radon: ", Style::default().fg(theme.text_secondary)),
                    Span::styled(
                        format!("{} Bq/m3", radon),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
        }
        _ => {
            if reading.co2 > 0 {
                let color = co2_color(reading.co2);
                lines.push(Line::from(vec![
                    Span::styled("CO2: ", Style::default().fg(theme.text_secondary)),
                    Span::styled(
                        format!("{} ppm", reading.co2),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
        }
    }

    // Temperature
    lines.push(Line::from(vec![
        Span::styled("Temp: ", Style::default().fg(theme.text_secondary)),
        Span::styled(
            format!("{:.1}C", reading.temperature),
            Style::default().fg(theme.text_primary),
        ),
    ]));

    // Humidity
    lines.push(Line::from(vec![
        Span::styled("Humidity: ", Style::default().fg(theme.text_secondary)),
        Span::styled(
            format!("{}%", reading.humidity),
            Style::default().fg(theme.text_primary),
        ),
    ]));

    // Battery
    let color = battery_color(reading.battery);
    lines.push(Line::from(vec![
        Span::styled("Battery: ", Style::default().fg(theme.text_secondary)),
        Span::styled(format!("{}%", reading.battery), Style::default().fg(color)),
    ]));

    let content = Paragraph::new(lines);
    frame.render_widget(content, inner);
}
