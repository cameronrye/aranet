//! History panel rendering for the TUI dashboard.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Sparkline};

use aranet_types::{DeviceType, HistoryRecord};

use super::theme::BORDER_TYPE;
use super::widgets::{
    format_radon_for_device, format_temp_for_device, resample_sparkline_data, sparkline_data,
};
use crate::tui::app::{App, HistoryFilter};

/// Get data for a specific chart metric from history.
/// Returns (data vector, color, label) tuple.
fn get_chart_metric_data(
    history: &[HistoryRecord],
    metric: u8,
    device_type: Option<DeviceType>,
) -> (Vec<u64>, Color, &'static str) {
    match metric {
        App::METRIC_TEMP => {
            // Temperature data (scaled by 10 to preserve decimal places in u64)
            let data: Vec<u64> = history
                .iter()
                .map(|r| ((r.temperature + 40.0) * 10.0) as u64) // Offset to handle negatives
                .collect();
            (data, Color::Yellow, "Temp")
        }
        App::METRIC_HUMIDITY => {
            // Humidity data
            let data: Vec<u64> = history.iter().map(|r| r.humidity as u64).collect();
            (data, Color::Cyan, "Humidity")
        }
        _ => {
            // Primary metric (CO2/Radon/Radiation)
            let data = sparkline_data(history, device_type);
            let (color, label) = match device_type {
                Some(DeviceType::AranetRadon) => (Color::Cyan, "Radon"),
                Some(DeviceType::AranetRadiation) => (Color::Magenta, "Radiation"),
                _ => (Color::Green, "CO2"),
            };
            (data, color, label)
        }
    }
}

/// Draw X-axis time labels for the sparkline (oldest left, newest right).
fn draw_sparkline_x_axis(
    frame: &mut Frame,
    area: Rect,
    oldest: time::OffsetDateTime,
    newest: time::OffsetDateTime,
    text_muted: Color,
) {
    let format = time::format_description::parse("[month]/[day] [hour]:[minute]")
        .unwrap_or_else(|_| Vec::new());

    let oldest_str = oldest.format(&format).unwrap_or_else(|_| "-".to_string());
    let newest_str = newest.format(&format).unwrap_or_else(|_| "-".to_string());

    // Calculate padding between labels
    let label_len = oldest_str.len() + newest_str.len();
    let padding = (area.width as usize).saturating_sub(label_len);

    let line = Line::from(vec![
        Span::styled(oldest_str, Style::default().fg(text_muted)),
        Span::raw(" ".repeat(padding)),
        Span::styled(newest_str, Style::default().fg(text_muted)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

/// Draw the history panel with detailed historical data.
pub(super) fn draw_history_panel(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    let block = Block::default()
        .title(Span::styled(" History ", theme.title_style()))
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(theme.border_active_style());

    if app.devices.is_empty() || app.selected_device >= app.devices.len() {
        let msg = Paragraph::new("Select a device to view history")
            .style(Style::default().fg(theme.text_muted))
            .alignment(Alignment::Center)
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let device = &app.devices[app.selected_device];

    if device.history.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "No history data available",
                Style::default().fg(theme.text_muted),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Press ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    "S",
                    Style::default()
                        .fg(theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " to sync history from device",
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

    // Show history stats and sparkline
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Calculate how many metrics to show on the chart
    let metrics_to_show: Vec<u8> = [App::METRIC_PRIMARY, App::METRIC_TEMP, App::METRIC_HUMIDITY]
        .into_iter()
        .filter(|&m| app.chart_shows(m))
        .collect();
    let chart_count = metrics_to_show.len();
    // Height per metric: 2 lines for sparkline + 1 for label, plus borders
    let sparkline_height = (chart_count as u16 * 3).max(3) + 2;

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),                // Stats
            Constraint::Length(sparkline_height), // Sparkline(s)
            Constraint::Min(1),                   // Recent records table
        ])
        .split(inner);

    // Stats line with last sync info
    let record_count = device.history.len();
    let mut info_lines = vec![Line::from(Span::styled(
        format!("  {} records stored", record_count),
        Style::default().fg(theme.text_muted),
    ))];

    // Show last sync time
    if let Some(sync_time) = device.last_sync {
        let format = time::format_description::parse("[hour]:[minute]:[second]").unwrap();
        let sync_str = sync_time.format(&format).unwrap_or_default();
        let age = (time::OffsetDateTime::now_utc() - sync_time).whole_minutes();
        let age_str = if age < 1 {
            "just now".to_string()
        } else if age < 60 {
            format!("{}m ago", age)
        } else {
            format!("{}h ago", age / 60)
        };

        info_lines.push(Line::from(vec![
            Span::styled("  Last sync: ", Style::default().fg(theme.text_muted)),
            Span::styled(sync_str, Style::default().fg(theme.text_primary)),
            Span::styled(
                format!(" ({})", age_str),
                Style::default().fg(theme.text_muted),
            ),
        ]));
    } else {
        info_lines.push(Line::from(vec![
            Span::styled("  Last sync: ", Style::default().fg(theme.text_muted)),
            Span::styled("Never", Style::default().fg(theme.warning)),
        ]));
    }

    let stats_para = Paragraph::new(info_lines);
    frame.render_widget(stats_para, layout[0]);

    // Sparkline with multiple metrics stacked vertically
    if !device.history.is_empty() {
        // Build title with legend
        let mut title_spans = vec![Span::styled(" Trend ", theme.title_style())];
        for &metric in &metrics_to_show {
            let (_, color, label) =
                get_chart_metric_data(&device.history, metric, device.device_type);
            title_spans.push(Span::styled(
                format!("[{}] ", label),
                Style::default().fg(color),
            ));
        }
        title_spans.push(Span::styled(
            "(T/H toggle) ",
            Style::default().fg(theme.text_muted),
        ));

        let sparkline_block = Block::default()
            .title(Line::from(title_spans))
            .borders(Borders::ALL)
            .border_type(BORDER_TYPE)
            .border_style(theme.border_inactive_style());
        let sparkline_inner = sparkline_block.inner(layout[1]);
        frame.render_widget(sparkline_block, layout[1]);

        // Split into chart area and X-axis labels
        let sparkline_vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // Chart area
                Constraint::Length(1), // X-axis labels
            ])
            .split(sparkline_inner);

        // Split chart area into rows for each metric
        let chart_constraints: Vec<Constraint> = metrics_to_show
            .iter()
            .map(|_| Constraint::Ratio(1, chart_count as u32))
            .collect();

        let chart_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(chart_constraints)
            .split(sparkline_vertical[0]);

        // Draw each metric's sparkline, resampled to fit the available width
        let chart_width = chart_rows.first().map(|r| r.width as usize).unwrap_or(0);
        for (i, &metric) in metrics_to_show.iter().enumerate() {
            let (data, color, _label) =
                get_chart_metric_data(&device.history, metric, device.device_type);
            if !data.is_empty() {
                // Resample data to fill the entire width
                let resampled = resample_sparkline_data(&data, chart_width);
                let sparkline = Sparkline::default()
                    .data(&resampled)
                    .style(Style::default().fg(color));
                frame.render_widget(sparkline, chart_rows[i]);
            }
        }

        // Draw X-axis time labels
        if let (Some(oldest), Some(newest)) = (device.history.first(), device.history.last()) {
            draw_sparkline_x_axis(
                frame,
                sparkline_vertical[1],
                oldest.timestamp,
                newest.timestamp,
                theme.text_muted,
            );
        }
    }

    // Filter records by time range
    let now = time::OffsetDateTime::now_utc();
    let filtered_history: Vec<_> = device
        .history
        .iter()
        .filter(|record| match app.history_filter {
            HistoryFilter::All => true,
            HistoryFilter::Today => {
                let today_start = now.date().midnight().assume_utc();
                record.timestamp >= today_start
            }
            HistoryFilter::Last24Hours => {
                let cutoff = now - time::Duration::hours(24);
                record.timestamp >= cutoff
            }
            HistoryFilter::Last7Days => {
                let cutoff = now - time::Duration::days(7);
                record.timestamp >= cutoff
            }
            HistoryFilter::Last30Days => {
                let cutoff = now - time::Duration::days(30);
                record.timestamp >= cutoff
            }
            HistoryFilter::Custom { start, end } => {
                let record_date = record.timestamp.date();
                let after_start = start.is_none_or(|s| record_date >= s);
                let before_end = end.is_none_or(|e| record_date <= e);
                after_start && before_end
            }
        })
        .collect();

    // Calculate visible records with scroll
    let visible_count = (layout[2].height as usize).saturating_sub(2); // Account for borders
    let total_records = filtered_history.len();
    let scroll_offset = app
        .history_scroll
        .min(total_records.saturating_sub(visible_count));

    // Get device settings for unit formatting
    let settings = device.settings.as_ref();

    // Get records in reverse order (newest first), then apply scroll
    let records: Vec<Line> = filtered_history
        .iter()
        .rev()
        .skip(scroll_offset)
        .take(visible_count)
        .map(|record| {
            let time = record
                .timestamp
                .format(
                    &time::format_description::parse("[month]/[day] [hour]:[minute]")
                        .expect("valid format"),
                )
                .unwrap_or_else(|_| "Unknown".to_string());
            let value = if let Some(radon) = record.radon {
                format!("Radon: {}", format_radon_for_device(radon, settings))
            } else if record.co2 > 0 {
                format!("CO2: {} ppm", record.co2)
            } else {
                format!(
                    "Temp: {}",
                    format_temp_for_device(record.temperature, settings)
                )
            };
            Line::from(vec![
                Span::styled(
                    format!("  {}  ", time),
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(value, Style::default().fg(theme.text_secondary)),
            ])
        })
        .collect();

    // Show scroll indicator in title with filter label
    let filter_label = app.history_filter.label();
    let scroll_info = if total_records > visible_count {
        format!(
            " [{}] [{}-{}/{}] ",
            filter_label,
            scroll_offset + 1,
            (scroll_offset + visible_count).min(total_records),
            total_records
        )
    } else {
        format!(" [{}] [{}] ", filter_label, total_records)
    };

    let recent_block = Block::default()
        .title(Line::from(vec![
            Span::styled(" Records", theme.title_style()),
            Span::styled(scroll_info, Style::default().fg(theme.text_muted)),
        ]))
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(theme.border_inactive_style());
    let recent_para = Paragraph::new(records).block(recent_block);
    frame.render_widget(recent_para, layout[2]);
}
