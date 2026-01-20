//! Settings panel rendering for the TUI dashboard.

use aranet_core::settings::{BluetoothRange, RadonUnit, TemperatureUnit};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::app::App;
use super::colors::battery_color;
use super::rssi_display;
use super::theme::BORDER_TYPE;

/// Renders the settings panel with device info and configuration options.
pub(super) fn draw_settings_panel(frame: &mut Frame, area: Rect, app: &App) {
    let theme = app.app_theme();

    let block = Block::default()
        .title(Span::styled(" Settings ", theme.title_style()))
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(theme.border_active_style());

    if app.devices.is_empty() || app.selected_device >= app.devices.len() {
        let msg = Paragraph::new("Select a device to view settings")
            .style(Style::default().fg(theme.text_muted))
            .alignment(Alignment::Center)
            .block(block);
        frame.render_widget(msg, area);
        return;
    }

    let device = &app.devices[app.selected_device];
    let device_name = device.name.as_deref().unwrap_or("Unknown");
    let device_type = device
        .device_type
        .map(|dt| format!("{:?}", dt))
        .unwrap_or_else(|| "Unknown".to_string());

    let mut info_lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Device ID:    ", Style::default().fg(theme.text_muted)),
            Span::styled(&device.id, Style::default().fg(theme.text_primary)),
        ]),
        Line::from(vec![
            Span::styled("  Name:         ", Style::default().fg(theme.text_muted)),
            Span::styled(device_name, Style::default().fg(theme.text_primary)),
        ]),
        Line::from(vec![
            Span::styled("  Type:         ", Style::default().fg(theme.text_muted)),
            Span::styled(device_type, Style::default().fg(theme.primary)),
        ]),
    ];

    // Show RSSI signal strength if available
    if let Some(rssi) = device.rssi {
        let (bars, color) = rssi_display(rssi);
        info_lines.push(Line::from(vec![
            Span::styled("  Signal:       ", Style::default().fg(theme.text_muted)),
            Span::styled(bars, Style::default().fg(color)),
            Span::styled(format!(" ({}dBm)", rssi), Style::default().fg(theme.text_muted)),
        ]));
    }

    // Show uptime if connected
    if let Some(uptime) = device.uptime() {
        info_lines.push(Line::from(vec![
            Span::styled("  Connected:    ", Style::default().fg(theme.text_muted)),
            Span::styled(uptime, Style::default().fg(theme.success)),
        ]));
    }

    info_lines.push(Line::from(""));

    // Show current reading interval if available (setting 0)
    if let Some(reading) = &device.reading {
        let interval_style = if app.selected_setting == 0 {
            theme.selected_style()
        } else {
            Style::default().fg(theme.text_primary)
        };
        if reading.interval > 0 {
            let interval_mins = reading.interval / 60;
            info_lines.push(Line::from(vec![
                Span::styled("  Interval:     ", Style::default().fg(theme.text_muted)),
                Span::styled(format!("[{}m]", interval_mins), interval_style),
                Span::styled(" (Enter to change)", Style::default().fg(theme.text_muted)),
            ]));
        }
        info_lines.push(Line::from(vec![
            Span::styled("  Battery:      ", Style::default().fg(theme.text_muted)),
            Span::styled(
                format!("{}%", reading.battery),
                Style::default().fg(battery_color(reading.battery)),
            ),
        ]));
    }

    info_lines.push(Line::from(""));
    info_lines.push(Line::from(Span::styled(
        "  Alert Thresholds:",
        Style::default().fg(theme.primary),
    )));
    info_lines.push(Line::from(""));

    // CO2 Alert Threshold (setting 1)
    let co2_threshold_style = if app.selected_setting == 1 {
        theme.selected_style()
    } else {
        Style::default().fg(theme.text_primary)
    };
    info_lines.push(Line::from(vec![
        Span::styled("  CO2 Alert:    ", Style::default().fg(theme.text_muted)),
        Span::styled(format!("[{} ppm]", app.co2_alert_threshold), co2_threshold_style),
        Span::styled(" (+/- to adjust)", Style::default().fg(theme.text_muted)),
    ]));

    // Radon Alert Threshold (setting 2)
    let radon_threshold_style = if app.selected_setting == 2 {
        theme.selected_style()
    } else {
        Style::default().fg(theme.text_primary)
    };
    info_lines.push(Line::from(vec![
        Span::styled("  Radon Alert:  ", Style::default().fg(theme.text_muted)),
        Span::styled(format!("[{} Bq/m3]", app.radon_alert_threshold), radon_threshold_style),
        Span::styled(" (+/- to adjust)", Style::default().fg(theme.text_muted)),
    ]));

    info_lines.push(Line::from(""));
    info_lines.push(Line::from(Span::styled(
        "  Device Settings:",
        Style::default().fg(theme.primary),
    )));
    info_lines.push(Line::from(""));

    // Show device settings if available
    if let Some(settings) = &device.settings {
        // Temperature unit
        let temp_unit_text = match settings.temperature_unit {
            TemperatureUnit::Celsius => "Celsius",
            TemperatureUnit::Fahrenheit => "Fahrenheit",
        };
        info_lines.push(Line::from(vec![
            Span::styled("  Temp Unit:    ", Style::default().fg(theme.text_muted)),
            Span::styled(temp_unit_text, Style::default().fg(theme.text_primary)),
        ]));

        // Radon unit (only show for radon devices)
        if device.device_type == Some(aranet_types::DeviceType::AranetRadon) {
            let radon_unit_text = match settings.radon_unit {
                RadonUnit::BqM3 => "Bq/m3",
                RadonUnit::PciL => "pCi/L",
            };
            info_lines.push(Line::from(vec![
                Span::styled("  Radon Unit:   ", Style::default().fg(theme.text_muted)),
                Span::styled(radon_unit_text, Style::default().fg(theme.text_primary)),
            ]));
        }

        // Smart Home setting
        let smart_home_text = if settings.smart_home_enabled { "Enabled" } else { "Disabled" };
        let smart_home_color = if settings.smart_home_enabled { theme.success } else { theme.text_muted };
        info_lines.push(Line::from(vec![
            Span::styled("  Smart Home:   ", Style::default().fg(theme.text_muted)),
            Span::styled(smart_home_text, Style::default().fg(smart_home_color)),
        ]));

        // BLE Range setting
        let (range_text, range_color) = match settings.bluetooth_range {
            BluetoothRange::Standard => ("Standard", theme.success),
            BluetoothRange::Extended => ("Extended", theme.info),
        };
        info_lines.push(Line::from(vec![
            Span::styled("  BLE Range:    ", Style::default().fg(theme.text_muted)),
            Span::styled(range_text, Style::default().fg(range_color)),
        ]));

        // Buzzer setting
        let buzzer_text = if settings.buzzer_enabled { "Enabled" } else { "Disabled" };
        let buzzer_color = if settings.buzzer_enabled { theme.success } else { theme.text_muted };
        info_lines.push(Line::from(vec![
            Span::styled("  Buzzer:       ", Style::default().fg(theme.text_muted)),
            Span::styled(buzzer_text, Style::default().fg(buzzer_color)),
        ]));

        // Auto calibration (Aranet4 only)
        if device.device_type == Some(aranet_types::DeviceType::Aranet4) {
            let auto_cal_text = if settings.auto_calibration_enabled { "Enabled" } else { "Disabled" };
            let auto_cal_color = if settings.auto_calibration_enabled { theme.success } else { theme.text_muted };
            info_lines.push(Line::from(vec![
                Span::styled("  Auto Calib:   ", Style::default().fg(theme.text_muted)),
                Span::styled(auto_cal_text, Style::default().fg(auto_cal_color)),
            ]));
        }
    } else {
        info_lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled("(Connect to view device settings)", Style::default().fg(theme.text_muted).italic()),
        ]));
    }

    info_lines.push(Line::from(""));
    info_lines.push(Line::from(vec![
        Span::styled("  Use ", Style::default().fg(theme.text_muted).italic()),
        Span::styled("j/k", Style::default().fg(theme.primary)),
        Span::styled(" to select, ", Style::default().fg(theme.text_muted).italic()),
        Span::styled("+/-", Style::default().fg(theme.primary)),
        Span::styled(" to adjust", Style::default().fg(theme.text_muted).italic()),
    ]));

    let settings_para = Paragraph::new(info_lines).block(block);
    frame.render_widget(settings_para, area);
}

