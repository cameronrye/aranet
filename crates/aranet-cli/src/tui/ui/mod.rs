//! Main UI layout and rendering for the TUI dashboard.
//!
//! This module provides the primary layout structure and draw functions for
//! the Aranet TUI dashboard. The layout consists of:
//!
//! - **Header**: Title and current time display
//! - **Main content**: Device list (left) and readings panel (right)
//! - **Status bar**: Help text and status messages

pub mod colors;
pub mod theme;
pub mod widgets;

mod dashboard;
mod history;
mod overlays;
mod settings;

use chrono::Local;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::app::{App, Tab, Theme};
use colors::co2_color;
use theme::BORDER_TYPE;

/// Get signal strength bars based on RSSI value.
pub(crate) fn rssi_display(rssi: i16) -> (&'static str, Color) {
    // Typical RSSI ranges:
    // -30 to -50: Excellent (4 bars)
    // -50 to -60: Good (3 bars)
    // -60 to -70: Fair (2 bars)
    // -70 to -80: Weak (1 bar)
    // Below -80: Very weak (0 bars)
    if rssi >= -50 {
        ("▂▄▆█", Color::Green)
    } else if rssi >= -60 {
        ("▂▄▆░", Color::Green)
    } else if rssi >= -70 {
        ("▂▄░░", Color::Yellow)
    } else if rssi >= -80 {
        ("▂░░░", Color::Red)
    } else {
        ("░░░░", Color::DarkGray)
    }
}

/// Draw the complete TUI interface.
///
/// This function creates the main layout and delegates rendering to helper
/// functions for each area.
pub fn draw(frame: &mut Frame, app: &App) {
    // Apply theme background
    if matches!(app.theme, Theme::Light) {
        frame.render_widget(
            Block::default().style(Style::default().bg(app.theme.bg())),
            frame.area(),
        );
    }

    // Full-screen chart view
    if app.show_fullscreen_chart {
        overlays::draw_fullscreen_chart(frame, app);
        return; // Don't render anything else
    }

    // Comparison view
    if app.show_comparison {
        overlays::draw_comparison_view(frame, app);
        return; // Don't render anything else
    }

    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Header bar
            Constraint::Length(3), // Tab bar
            Constraint::Min(1),    // Main content
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    draw_header(frame, main_layout[0], app);
    draw_tab_bar(frame, main_layout[1], app);

    // Responsive layout: hide sidebar on narrow terminals or when toggled off
    let area = main_layout[2];
    let is_narrow = area.width < 80;
    let show_sidebar = app.show_sidebar && !is_narrow;

    let content_constraints = if show_sidebar {
        vec![
            Constraint::Length(app.sidebar_width), // Device list sidebar
            Constraint::Min(1),                    // Main content
        ]
    } else {
        vec![
            Constraint::Length(0), // Hidden sidebar
            Constraint::Min(1),    // Full width content
        ]
    };

    let content_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(content_constraints)
        .split(area);

    if show_sidebar {
        dashboard::draw_device_list(frame, content_layout[0], app);
    }

    // Render different content based on active tab
    match app.active_tab {
        Tab::Dashboard => dashboard::draw_readings_panel(frame, content_layout[1], app),
        Tab::History => history::draw_history_panel(frame, content_layout[1], app),
        Tab::Settings => settings::draw_settings_panel(frame, content_layout[1], app),
    }

    draw_status_bar(frame, main_layout[3], app);

    // Draw help overlay if active
    if app.show_help {
        overlays::draw_help_overlay(frame);
    }

    // Alert history overlay
    overlays::draw_alert_history(frame, app);

    // Alias editor overlay
    overlays::draw_alias_editor(frame, app);

    // Error details popup
    overlays::draw_error_popup(frame, app);

    // Confirmation dialog (on top of everything)
    overlays::draw_confirmation_dialog(frame, app);
}

/// Draw the header bar with app title, quick stats, and indicators.
fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    let mut spans = vec![
        Span::styled(
            " Aranet Monitor ",
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("v0.3.0 ", Style::default().fg(theme.text_muted)),
    ];

    // Connected count
    let connected = app.connected_count();
    let total = app.devices.len();
    let conn_color = if connected == 0 {
        theme.danger
    } else {
        theme.success
    };
    spans.push(Span::styled(
        format!(" *{}/{} ", connected, total),
        Style::default().fg(conn_color),
    ));

    // Average CO2 if available
    if let Some(avg_co2) = app.average_co2() {
        let co2_color = co2_color(avg_co2);
        spans.push(Span::styled(
            format!(" CO2:{} ", avg_co2),
            Style::default().fg(co2_color),
        ));
    }

    // Alert count
    let alert_count = app.alerts.len();
    if alert_count > 0 {
        spans.push(Span::styled(
            format!(" !{} ", alert_count),
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Sticky indicator
    if app.sticky_alerts {
        spans.push(Span::styled(" STICKY ", Style::default().fg(theme.warning)));
    }

    // Bell indicator
    if app.bell_enabled {
        spans.push(Span::styled(" BELL ", Style::default().fg(theme.warning)));
    }

    // Error indicator
    if app.last_error.is_some() && !app.show_error_details {
        spans.push(Span::styled(" ERR ", Style::default().fg(theme.danger)));
    }

    // Theme indicator
    if matches!(app.theme, Theme::Light) {
        spans.push(Span::styled(" LIGHT ", Style::default().fg(theme.warning)));
    } else {
        spans.push(Span::styled(" DARK ", Style::default().fg(theme.info)));
    }

    // Smart Home indicator
    if app.smart_home_enabled {
        spans.push(Span::styled(" HOME ", Style::default().fg(theme.success)));
    }

    let header = Paragraph::new(Line::from(spans)).style(theme.header_style());

    frame.render_widget(header, area);
}

/// Get context-sensitive help hints based on current state.
fn context_hints(app: &App) -> Vec<(&'static str, &'static str)> {
    let mut hints = Vec::new();

    // Always show help key
    hints.push(("?", "help"));

    match app.active_tab {
        Tab::Dashboard => {
            if app.devices.is_empty() {
                hints.push(("s", "scan"));
            } else {
                hints.push(("j/k", "select"));
                if app.selected_device().is_some() {
                    if app
                        .selected_device()
                        .map(|d| matches!(d.status, super::app::ConnectionStatus::Connected))
                        .unwrap_or(false)
                    {
                        hints.push(("r", "refresh"));
                        hints.push(("d", "disconnect"));
                        hints.push(("S", "sync"));
                    } else {
                        hints.push(("c", "connect"));
                    }
                }
                hints.push(("s", "scan"));
            }
        }
        Tab::History => {
            hints.push(("S", "sync"));
            hints.push(("0-4", "filter"));
            hints.push(("PgUp/Dn", "scroll"));
            hints.push(("g", "fullscreen"));
        }
        Tab::Settings => {
            hints.push(("+/-", "adjust"));
            hints.push(("n", "alias"));
        }
    }

    hints.push(("q", "quit"));
    hints
}

/// Draw the status bar with context-sensitive help.
fn draw_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();
    let time_str = Local::now().format("%H:%M:%S").to_string();

    // Build left content with context-sensitive hints
    let left_spans = if app.scanning {
        vec![
            Span::styled(
                format!("{} ", app.spinner_char()),
                Style::default().fg(theme.primary),
            ),
            Span::styled("Scanning...", Style::default().fg(theme.text_secondary)),
        ]
    } else if app.is_any_connecting() {
        vec![
            Span::styled(
                format!("{} ", app.spinner_char()),
                Style::default().fg(theme.primary),
            ),
            Span::styled("Connecting...", Style::default().fg(theme.text_secondary)),
        ]
    } else if app.is_syncing() {
        vec![
            Span::styled(
                format!("{} ", app.spinner_char()),
                Style::default().fg(theme.primary),
            ),
            Span::styled("Syncing...", Style::default().fg(theme.text_secondary)),
        ]
    } else if let Some(msg) = app.current_status_message() {
        vec![Span::styled(
            format!(" {}", msg),
            Style::default().fg(theme.text_secondary),
        )]
    } else {
        // Context-sensitive hints with styled keys
        let hints = context_hints(app);
        let mut spans = vec![Span::raw(" ")];
        for (i, (key, desc)) in hints.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" | ", Style::default().fg(theme.text_muted)));
            }
            spans.push(Span::styled(
                *key,
                Style::default()
                    .fg(theme.primary)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {}", desc),
                Style::default().fg(theme.text_muted),
            ));
        }
        spans
    };

    // Split status bar into left (hints), indicators, and right (time)
    let logging_width = if app.logging_enabled { 5 } else { 0 };
    let status_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(logging_width),
            Constraint::Length(10),
        ])
        .split(area);

    let left = Paragraph::new(Line::from(left_spans));
    frame.render_widget(left, status_layout[0]);

    // Logging indicator
    if app.logging_enabled {
        let log_indicator = Paragraph::new(" REC").style(
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(log_indicator, status_layout[1]);
    }

    let right = Paragraph::new(time_str)
        .style(Style::default().fg(theme.text_muted))
        .alignment(Alignment::Right);

    frame.render_widget(right, status_layout[2]);
}

/// Draw the tab bar with modern styling.
fn draw_tab_bar(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    let tabs = [
        ("Dashboard", Tab::Dashboard),
        ("History", Tab::History),
        ("Settings", Tab::Settings),
    ];

    // Build custom tab line with underline indicator for active tab
    let tab_titles: Vec<Line> = tabs
        .iter()
        .map(|(name, tab)| {
            let is_active = *tab == app.active_tab;
            let style = if is_active {
                Style::default()
                    .fg(theme.primary)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_muted)
            };
            // Add underline to active tab for visual emphasis
            let styled_name = if is_active {
                Span::styled(
                    format!(" {} ", name),
                    style.add_modifier(Modifier::UNDERLINED),
                )
            } else {
                Span::styled(format!(" {} ", name), style)
            };
            Line::from(styled_name)
        })
        .collect();

    let tabs_widget = ratatui::widgets::Tabs::new(tab_titles)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_type(BORDER_TYPE)
                .border_style(Style::default().fg(theme.border_inactive)),
        )
        .highlight_style(Style::default().fg(theme.primary))
        .divider(Span::styled(" | ", Style::default().fg(theme.text_muted)))
        .select(match app.active_tab {
            Tab::Dashboard => 0,
            Tab::History => 1,
            Tab::Settings => 2,
        });

    frame.render_widget(tabs_widget, area);
}
