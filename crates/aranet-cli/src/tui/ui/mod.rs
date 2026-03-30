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
mod service;
mod settings;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::app::{App, Tab, Theme};
use colors::co2_color;
use theme::BORDER_TYPE;

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
        Tab::Service => service::draw_service_panel(frame, content_layout[1], app),
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
    let width = area.width;

    let mut spans = vec![Span::styled(
        " Aranet ",
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::BOLD),
    )];

    if width >= 32 {
        spans.push(Span::styled(
            format!("v{} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme.text_muted),
        ));
    }

    // Connected count
    let connected = app.connected_count();
    let total = app.devices.len();
    let conn_color = if connected == 0 {
        theme.danger
    } else {
        theme.success
    };
    if width >= 22 {
        spans.push(Span::styled(
            format!(" {}/{} online ", connected, total),
            Style::default().fg(conn_color),
        ));
    }

    // Average CO2 if available
    if width >= 40
        && let Some(avg_co2) = app.average_co2()
    {
        let co2_color = co2_color(&theme, avg_co2);
        spans.push(Span::styled(
            format!(" CO2 {} ", avg_co2),
            Style::default().fg(co2_color),
        ));
    }

    // Alert count
    let alert_count = app.alerts.len();
    if width >= 50 && alert_count > 0 {
        spans.push(Span::styled(
            format!(" Alerts {} ", alert_count),
            Style::default()
                .fg(theme.danger)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Theme indicator
    if width >= 62 {
        let (theme_label, theme_color) = if matches!(app.theme, Theme::Light) {
            (" Light ", theme.warning)
        } else {
            (" Dark ", theme.info)
        };
        spans.push(Span::styled(theme_label, Style::default().fg(theme_color)));
    }

    if width >= 76 {
        if app.sticky_alerts {
            spans.push(Span::styled(" Sticky ", Style::default().fg(theme.warning)));
        }
        if app.bell_enabled {
            spans.push(Span::styled(" Bell ", Style::default().fg(theme.warning)));
        }
        if app.last_error.is_some() && !app.show_error_details {
            spans.push(Span::styled(" Error ", Style::default().fg(theme.danger)));
        }
        if app.smart_home_enabled {
            spans.push(Span::styled(" Home ", Style::default().fg(theme.success)));
        }
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
        Tab::Service => {
            hints.push(("r", "refresh"));
            hints.push(("Enter", "start/stop"));
            hints.push(("j/k", "select"));
        }
    }

    hints.push(("q", "quit"));
    hints
}

/// Draw the status bar with context-sensitive help.
fn draw_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();
    let width = area.width;
    let time_str = {
        let now =
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        now.format(&time::format_description::parse("[hour]:[minute]:[second]").unwrap_or_default())
            .unwrap_or_default()
    };

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
        let hints: Vec<_> = if width < 46 {
            let mut compact = vec![hints[0]];
            if hints.len() > 2 {
                compact.push(hints[1]);
            }
            if let Some(last) = hints.last().copied()
                && compact.last().copied() != Some(last)
            {
                compact.push(last);
            }
            compact
        } else if width < 72 {
            let mut compact = vec![hints[0]];
            compact.extend(hints.iter().skip(1).take(2).copied());
            if let Some(last) = hints.last().copied()
                && compact.last().copied() != Some(last)
            {
                compact.push(last);
            }
            compact
        } else {
            hints
        };
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
        ("Service", Tab::Service),
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
            Tab::Service => 3,
        });

    frame.render_widget(tabs_widget, area);
}
