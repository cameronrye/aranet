//! Service management panel for the TUI dashboard.
//!
//! This module renders the Service tab which allows users to:
//! - View the status of the aranet-service background collector
//! - Start/stop the collector
//! - View monitored devices and their collection statistics

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use super::theme::BORDER_TYPE;
use crate::tui::app::App;

/// Renders the service management panel.
pub(super) fn draw_service_panel(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    let block = Block::default()
        .title(Span::styled(" Service ", theme.title_style()))
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(theme.border_active_style());

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Layout: status section at top, device list below
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // Status section
            Constraint::Min(5),    // Device list
            Constraint::Length(2), // Help text
        ])
        .split(inner);

    draw_status_section(frame, layout[0], app);
    draw_device_list(frame, layout[1], app);
    draw_help_section(frame, layout[2], app);
}

/// Draw the service status section.
fn draw_status_section(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    let mut lines = vec![Line::from("")];

    match &app.service_status {
        Some(status) if status.reachable => {
            // Service is reachable
            let (status_text, status_color) = if status.collector_running {
                ("[RUNNING]", theme.success)
            } else {
                ("[STOPPED]", theme.warning)
            };

            lines.push(Line::from(vec![
                Span::styled("  Status:   ", Style::default().fg(theme.text_muted)),
                Span::styled(status_text, Style::default().fg(status_color).bold()),
            ]));

            // Uptime
            if let Some(uptime_secs) = status.uptime_seconds {
                let uptime_str = format_uptime(uptime_secs);
                lines.push(Line::from(vec![
                    Span::styled("  Uptime:   ", Style::default().fg(theme.text_muted)),
                    Span::styled(uptime_str, Style::default().fg(theme.text_primary)),
                ]));
            }

            // Service URL
            lines.push(Line::from(vec![
                Span::styled("  URL:      ", Style::default().fg(theme.text_muted)),
                Span::styled(&app.service_url, Style::default().fg(theme.info)),
            ]));

            // Device count
            let device_count = status.devices.len();
            lines.push(Line::from(vec![
                Span::styled("  Devices:  ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    format!("{} monitored", device_count),
                    Style::default().fg(theme.text_primary),
                ),
            ]));

            // Start/Stop button
            lines.push(Line::from(""));
            let is_selected = app.service_selected_item == 0;
            let button_style = if is_selected {
                theme.selected_style()
            } else {
                Style::default().fg(theme.primary)
            };
            let button_text = if status.collector_running {
                "[Stop Collector]"
            } else {
                "[Start Collector]"
            };
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(button_text, button_style),
                Span::styled(
                    if is_selected {
                        " <- Enter to toggle"
                    } else {
                        ""
                    },
                    Style::default().fg(theme.text_muted),
                ),
            ]));
        }
        Some(_status) => {
            // Service not reachable
            lines.push(Line::from(vec![
                Span::styled("  Status:   ", Style::default().fg(theme.text_muted)),
                Span::styled("[NOT REACHABLE]", Style::default().fg(theme.danger).bold()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  URL:      ", Style::default().fg(theme.text_muted)),
                Span::styled(&app.service_url, Style::default().fg(theme.text_muted)),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "  Service is not running or not reachable.",
                Style::default().fg(theme.text_muted).italic(),
            )]));
            lines.push(Line::from(vec![Span::styled(
                "  Start with: aranet service start",
                Style::default().fg(theme.text_muted).italic(),
            )]));
        }
        None => {
            // Status not yet fetched
            if app.service_refreshing {
                lines.push(Line::from(vec![
                    Span::styled("  Status:   ", Style::default().fg(theme.text_muted)),
                    Span::styled("Checking...", Style::default().fg(theme.info)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("  Status:   ", Style::default().fg(theme.text_muted)),
                    Span::styled("Unknown", Style::default().fg(theme.text_muted)),
                ]));
                lines.push(Line::from(vec![Span::styled(
                    "  Press 'r' to refresh",
                    Style::default().fg(theme.text_muted).italic(),
                )]));
            }
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Draw the device list section.
fn draw_device_list(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    let block = Block::default()
        .title(Span::styled(
            " Monitored Devices ",
            Style::default().fg(theme.text_muted),
        ))
        .borders(Borders::TOP)
        .border_type(BORDER_TYPE)
        .border_style(Style::default().fg(theme.border_inactive));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(status) = &app.service_status else {
        let msg = Paragraph::new("  No device information available")
            .style(Style::default().fg(theme.text_muted));
        frame.render_widget(msg, inner);
        return;
    };

    if !status.reachable || status.devices.is_empty() {
        let msg = if !status.reachable {
            "  Service not reachable"
        } else {
            "  No devices configured"
        };
        let paragraph = Paragraph::new(msg).style(Style::default().fg(theme.text_muted));
        frame.render_widget(paragraph, inner);
        return;
    }

    let items: Vec<ListItem> = status
        .devices
        .iter()
        .enumerate()
        .map(|(i, device)| {
            let is_selected = app.service_selected_item == i + 1;
            let style = if is_selected {
                theme.selected_style()
            } else {
                Style::default().fg(theme.text_primary)
            };

            // Device name/ID
            let name = device
                .alias
                .clone()
                .unwrap_or_else(|| device.device_id.clone());

            // Status indicator
            let (status_text, status_color) = if device.polling {
                ("[POLL]", theme.info)
            } else if device.last_error.is_some() {
                ("[FAIL]", theme.danger)
            } else if device.success_count > 0 {
                ("[PASS]", theme.success)
            } else {
                ("[WAIT]", theme.text_muted)
            };

            // Last poll time
            let last_poll = device
                .last_poll_at
                .map(format_time_ago)
                .unwrap_or_else(|| "never".to_string());

            // Build line
            let line = Line::from(vec![
                Span::styled(format!("  {:20}", name), style),
                Span::styled(
                    format!("Every {}s  ", device.poll_interval),
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(
                    format!("Last: {:8}  ", last_poll),
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(status_text, Style::default().fg(status_color)),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, inner);
}

/// Draw the help section at the bottom.
fn draw_help_section(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    let help = Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled("j/k", Style::default().fg(theme.primary)),
        Span::styled(" select  ", Style::default().fg(theme.text_muted)),
        Span::styled("Enter", Style::default().fg(theme.primary)),
        Span::styled(" toggle  ", Style::default().fg(theme.text_muted)),
        Span::styled("r", Style::default().fg(theme.primary)),
        Span::styled(" refresh", Style::default().fg(theme.text_muted)),
    ]);

    let paragraph = Paragraph::new(help);
    frame.render_widget(paragraph, area);
}

/// Format uptime in human-readable form.
fn format_uptime(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else if seconds < 86400 {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        format!("{}h {}m", hours, mins)
    } else {
        let days = seconds / 86400;
        let hours = (seconds % 86400) / 3600;
        format!("{}d {}h", days, hours)
    }
}

/// Format a timestamp as "X ago".
fn format_time_ago(timestamp: time::OffsetDateTime) -> String {
    let now = time::OffsetDateTime::now_utc();
    let duration = now - timestamp;
    let seconds = duration.whole_seconds().max(0) as u64;

    if seconds < 60 {
        format!("{}s ago", seconds)
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86400)
    }
}
