//! Keyboard input handling for the TUI.
//!
//! This module provides key mapping and action handling for the terminal
//! user interface. It translates keyboard events into high-level actions
//! and applies those actions to the application state.
//!
//! # Key Bindings
//!
//! | Key       | Action            |
//! |-----------|-------------------|
//! | `q`       | Quit              |
//! | `s`       | Scan              |
//! | `r`       | Refresh           |
//! | `c`       | Connect           |
//! | `d`       | Disconnect        |
//! | `y`       | Sync history      |
//! | `↓` / `j` | Select next       |
//! | `↑` / `k` | Select previous   |
//! | `Tab` / `l` | Next tab        |
//! | `BackTab` / `h` | Previous tab |
//! | `?`       | Toggle help       |
//! | `D`       | Do Not Disturb    |
//! | `F`       | Toggle export fmt |

use std::time::Duration;

use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use tokio::sync::mpsc;

use super::app::{App, ConnectionStatus, HistoryFilter, PendingAction, Tab, Theme};
use super::messages::Command;

/// User actions that can be triggered by keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Quit the application.
    Quit,
    /// Start scanning for devices.
    Scan,
    /// Refresh readings for all connected devices.
    Refresh,
    /// Connect to the currently selected device.
    Connect,
    /// Connect to all devices.
    ConnectAll,
    /// Disconnect from the currently selected device.
    Disconnect,
    /// Sync history from the currently selected device.
    SyncHistory,
    /// Select the next item in the list.
    SelectNext,
    /// Select the previous item in the list.
    SelectPrevious,
    /// Switch to the next tab.
    NextTab,
    /// Switch to the previous tab.
    PreviousTab,
    /// Toggle the help overlay.
    ToggleHelp,
    /// Toggle data logging.
    ToggleLogging,
    /// Toggle terminal bell for alerts.
    ToggleBell,
    /// Dismiss current alert.
    DismissAlert,
    /// Scroll history up.
    ScrollUp,
    /// Scroll history down.
    ScrollDown,
    /// Set history filter.
    SetHistoryFilter(HistoryFilter),
    /// Increase threshold value.
    IncreaseThreshold,
    /// Decrease threshold value.
    DecreaseThreshold,
    /// Change setting value (in Settings tab).
    ChangeSetting,
    /// Export history to CSV file.
    ExportHistory,
    /// Toggle alert history view.
    ToggleAlertHistory,
    /// Cycle device filter.
    CycleDeviceFilter,
    /// Toggle sidebar visibility.
    ToggleSidebar,
    /// Toggle sidebar width.
    ToggleSidebarWidth,
    /// Mouse click at coordinates.
    MouseClick { x: u16, y: u16 },
    /// Confirm pending action.
    Confirm,
    /// Cancel pending action.
    Cancel,
    /// Toggle full-screen chart view.
    ToggleChart,
    /// Start editing device alias.
    EditAlias,
    /// Input character for text input.
    TextInput(char),
    /// Backspace for text input.
    TextBackspace,
    /// Submit text input.
    TextSubmit,
    /// Cancel text input.
    TextCancel,
    /// Toggle sticky alerts.
    ToggleStickyAlerts,
    /// Toggle comparison view.
    ToggleComparison,
    /// Cycle comparison device forward.
    NextComparisonDevice,
    /// Cycle comparison device backward.
    PrevComparisonDevice,
    /// Show error details popup.
    ShowErrorDetails,
    /// Toggle theme.
    ToggleTheme,
    /// Toggle temperature on chart.
    ToggleChartTemp,
    /// Toggle humidity on chart.
    ToggleChartHumidity,
    /// Toggle Bluetooth range.
    ToggleBleRange,
    /// Toggle Smart Home mode.
    ToggleSmartHome,
    /// Toggle Do Not Disturb mode.
    ToggleDoNotDisturb,
    /// Toggle export format (CSV/JSON).
    ToggleExportFormat,
    /// No action (unrecognized key).
    None,
}

/// Map a key code to an action.
///
/// # Arguments
///
/// * `key` - The key code from a keyboard event
/// * `editing_text` - Whether the user is currently editing text input
/// * `has_pending_confirmation` - Whether there is a pending confirmation dialog
///
/// # Returns
///
/// The corresponding action for the key, or [`Action::None`] if the key
/// is not mapped to any action.
pub fn handle_key(key: KeyCode, editing_text: bool, has_pending_confirmation: bool) -> Action {
    // If editing text, handle text input specially
    if editing_text {
        return match key {
            KeyCode::Enter => Action::TextSubmit,
            KeyCode::Esc => Action::TextCancel,
            KeyCode::Backspace => Action::TextBackspace,
            KeyCode::Char(c) => Action::TextInput(c),
            _ => Action::None,
        };
    }

    // When a confirmation dialog is active, only handle Y/N keys
    if has_pending_confirmation {
        return match key {
            KeyCode::Char('y') | KeyCode::Char('Y') => Action::Confirm,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::Cancel,
            _ => Action::None,
        };
    }

    match key {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('s') => Action::Scan,
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('c') => Action::Connect,
        KeyCode::Char('C') => Action::ConnectAll,
        KeyCode::Char('d') => Action::Disconnect,
        KeyCode::Char('S') | KeyCode::Char('y') => Action::SyncHistory,
        KeyCode::Down | KeyCode::Char('j') => Action::SelectNext,
        KeyCode::Up | KeyCode::Char('k') => Action::SelectPrevious,
        KeyCode::Tab | KeyCode::Char('l') => Action::NextTab,
        KeyCode::BackTab | KeyCode::Char('h') => Action::PreviousTab,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char('L') => Action::ToggleLogging,
        KeyCode::Char('b') => Action::ToggleBell,
        KeyCode::Char('n') => Action::EditAlias,
        KeyCode::Esc => Action::DismissAlert,
        KeyCode::PageUp => Action::ScrollUp,
        KeyCode::PageDown => Action::ScrollDown,
        KeyCode::Char('0') => Action::SetHistoryFilter(HistoryFilter::All),
        KeyCode::Char('1') => Action::SetHistoryFilter(HistoryFilter::Today),
        KeyCode::Char('2') => Action::SetHistoryFilter(HistoryFilter::Last24Hours),
        KeyCode::Char('3') => Action::SetHistoryFilter(HistoryFilter::Last7Days),
        KeyCode::Char('4') => Action::SetHistoryFilter(HistoryFilter::Last30Days),
        KeyCode::Char('+') | KeyCode::Char('=') => Action::IncreaseThreshold,
        KeyCode::Char('-') | KeyCode::Char('_') => Action::DecreaseThreshold,
        KeyCode::Enter => Action::ChangeSetting,
        KeyCode::Char('e') => Action::ExportHistory,
        KeyCode::Char('a') => Action::ToggleAlertHistory,
        KeyCode::Char('f') => Action::CycleDeviceFilter,
        KeyCode::Char('[') => Action::ToggleSidebar,
        KeyCode::Char(']') => Action::ToggleSidebarWidth,
        KeyCode::Char('g') => Action::ToggleChart,
        KeyCode::Char('A') => Action::ToggleStickyAlerts,
        KeyCode::Char('v') => Action::ToggleComparison,
        KeyCode::Char('<') => Action::PrevComparisonDevice,
        KeyCode::Char('>') => Action::NextComparisonDevice,
        KeyCode::Char('E') => Action::ShowErrorDetails,
        KeyCode::Char('t') => Action::ToggleTheme,
        KeyCode::Char('T') => Action::ToggleChartTemp,
        KeyCode::Char('H') => Action::ToggleChartHumidity,
        KeyCode::Char('B') => Action::ToggleBleRange,
        KeyCode::Char('I') => Action::ToggleSmartHome,
        KeyCode::Char('D') => Action::ToggleDoNotDisturb,
        KeyCode::Char('F') => Action::ToggleExportFormat,
        _ => Action::None,
    }
}

/// Handle mouse events and return corresponding action.
///
/// # Arguments
///
/// * `event` - The mouse event from crossterm
///
/// # Returns
///
/// The corresponding action for the mouse event, or [`Action::None`] if the event
/// is not mapped to any action.
pub fn handle_mouse(event: MouseEvent) -> Action {
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => Action::MouseClick {
            x: event.column,
            y: event.row,
        },
        _ => Action::None,
    }
}

/// Apply an action to the application state.
///
/// This function handles both UI-only actions (which modify app state directly)
/// and command actions (which return a command to be sent to the background worker).
///
/// # Arguments
///
/// * `app` - Mutable reference to the application state
/// * `action` - The action to apply
/// * `_command_tx` - Channel for sending commands (used for reference, actual sending done by caller)
///
/// # Returns
///
/// `Some(Command)` if an async command should be sent to the background worker,
/// `None` if the action was handled entirely within the UI.
pub fn apply_action(
    app: &mut App,
    action: Action,
    _command_tx: &mpsc::Sender<Command>,
) -> Option<Command> {
    match action {
        Action::Quit => {
            app.should_quit = true;
            None
        }
        Action::Scan => Some(Command::Scan {
            duration: Duration::from_secs(5),
        }),
        Action::Refresh => {
            if app.active_tab == Tab::Service {
                // In Service tab, refresh service status
                Some(Command::RefreshServiceStatus)
            } else {
                // In other tabs, refresh sensor readings
                Some(Command::RefreshAll)
            }
        }
        Action::Connect => app.selected_device().map(|device| Command::Connect {
            device_id: device.id.clone(),
        }),
        Action::ConnectAll => {
            // Connect to all disconnected devices one by one
            // Find first disconnected device and connect
            let first_disconnected = app
                .devices
                .iter()
                .find(|d| matches!(d.status, ConnectionStatus::Disconnected))
                .map(|d| d.id.clone());
            let count = app
                .devices
                .iter()
                .filter(|d| matches!(d.status, ConnectionStatus::Disconnected))
                .count();

            if let Some(device_id) = first_disconnected {
                app.push_status_message(format!("Connecting... ({} remaining)", count));
                return Some(Command::Connect { device_id });
            } else {
                app.push_status_message("All devices already connected".to_string());
            }
            None
        }
        Action::Disconnect => {
            if let Some(device) = app.selected_device()
                && matches!(device.status, ConnectionStatus::Connected)
            {
                let action = PendingAction::Disconnect {
                    device_id: device.id.clone(),
                    device_name: device.name.clone().unwrap_or_else(|| device.id.clone()),
                };
                app.request_confirmation(action);
            }
            None
        }
        Action::SyncHistory => app.selected_device().map(|device| Command::SyncHistory {
            device_id: device.id.clone(),
        }),
        Action::SelectNext => {
            if app.active_tab == Tab::Settings {
                app.select_next_setting();
            } else {
                app.select_next_device();
            }
            None
        }
        Action::SelectPrevious => {
            if app.active_tab == Tab::Settings {
                app.select_previous_setting();
            } else {
                app.select_previous_device();
            }
            None
        }
        Action::NextTab => {
            app.active_tab = match app.active_tab {
                Tab::Dashboard => Tab::History,
                Tab::History => Tab::Settings,
                Tab::Settings => Tab::Service,
                Tab::Service => Tab::Dashboard,
            };
            None
        }
        Action::PreviousTab => {
            app.active_tab = match app.active_tab {
                Tab::Dashboard => Tab::Service,
                Tab::History => Tab::Dashboard,
                Tab::Settings => Tab::History,
                Tab::Service => Tab::Settings,
            };
            None
        }
        Action::ToggleHelp => {
            app.show_help = !app.show_help;
            None
        }
        Action::ToggleLogging => {
            app.toggle_logging();
            None
        }
        Action::ToggleBell => {
            app.bell_enabled = !app.bell_enabled;
            app.push_status_message(format!(
                "Bell notifications {}",
                if app.bell_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ));
            None
        }
        Action::DismissAlert => {
            // Close help overlay if open
            if app.show_help {
                app.show_help = false;
            // Close error popup if open
            } else if app.show_error_details {
                app.show_error_details = false;
            } else if let Some(device) = app.selected_device() {
                let device_id = device.id.clone();
                app.dismiss_alert(&device_id);
            }
            None
        }
        Action::ScrollUp => {
            if app.active_tab == Tab::History {
                app.scroll_history_up();
            }
            None
        }
        Action::ScrollDown => {
            if app.active_tab == Tab::History {
                app.scroll_history_down();
            }
            None
        }
        Action::SetHistoryFilter(filter) => {
            if app.active_tab == Tab::History {
                app.set_history_filter(filter);
            }
            None
        }
        Action::IncreaseThreshold => {
            if app.active_tab == Tab::Settings {
                match app.selected_setting {
                    1 => app.increase_co2_threshold(),
                    2 => app.increase_radon_threshold(),
                    _ => {}
                }
                app.push_status_message(format!(
                    "CO2: {} ppm, Radon: {} Bq/m³",
                    app.co2_alert_threshold, app.radon_alert_threshold
                ));
            }
            None
        }
        Action::DecreaseThreshold => {
            if app.active_tab == Tab::Settings {
                match app.selected_setting {
                    1 => app.decrease_co2_threshold(),
                    2 => app.decrease_radon_threshold(),
                    _ => {}
                }
                app.push_status_message(format!(
                    "CO2: {} ppm, Radon: {} Bq/m³",
                    app.co2_alert_threshold, app.radon_alert_threshold
                ));
            }
            None
        }
        Action::ChangeSetting => {
            if app.active_tab == Tab::Service {
                // In Service tab, Enter toggles collector start/stop
                if let Some(ref status) = app.service_status {
                    if status.reachable {
                        if status.collector_running {
                            return Some(Command::StopServiceCollector);
                        } else {
                            return Some(Command::StartServiceCollector);
                        }
                    } else {
                        app.push_status_message("Service not reachable".to_string());
                    }
                } else {
                    app.push_status_message(
                        "Service status unknown - press 'r' to refresh".to_string(),
                    );
                }
                None
            } else if app.active_tab == Tab::Settings && app.selected_setting == 0 {
                // Interval setting
                if let Some((device_id, new_interval)) = app.cycle_interval() {
                    return Some(Command::SetInterval {
                        device_id,
                        interval_secs: new_interval,
                    });
                }
                None
            } else {
                None
            }
        }
        Action::ExportHistory => {
            if let Some(path) = app.export_history() {
                app.push_status_message(format!("Exported to {}", path));
            } else {
                app.push_status_message("No history to export".to_string());
            }
            None
        }
        Action::ToggleAlertHistory => {
            app.toggle_alert_history();
            None
        }
        Action::ToggleStickyAlerts => {
            app.toggle_sticky_alerts();
            None
        }
        Action::CycleDeviceFilter => {
            app.cycle_device_filter();
            None
        }
        Action::ToggleSidebar => {
            app.toggle_sidebar();
            app.push_status_message(
                if app.show_sidebar {
                    "Sidebar shown"
                } else {
                    "Sidebar hidden"
                }
                .to_string(),
            );
            None
        }
        Action::ToggleSidebarWidth => {
            app.toggle_sidebar_width();
            app.push_status_message(format!("Sidebar width: {}", app.sidebar_width));
            None
        }
        Action::MouseClick { x, y } => {
            // Tab bar is at y=1-3, clicking on a tab switches to it
            if (1..=3).contains(&y) {
                // Simple tab detection based on x position
                if x < 15 {
                    app.active_tab = Tab::Dashboard;
                } else if x < 30 {
                    app.active_tab = Tab::History;
                } else if x < 45 {
                    app.active_tab = Tab::Settings;
                } else if x < 60 {
                    app.active_tab = Tab::Service;
                }
            }
            // Device list is in the left sidebar (x < ~25, y > 4)
            else if x < 25 && y > 4 {
                let device_row = (y as usize).saturating_sub(5);
                if device_row < app.devices.len() {
                    app.selected_device = device_row;
                }
            }
            None
        }
        Action::Confirm => {
            if app.pending_confirmation.is_some() {
                return app.confirm_action();
            }
            None
        }
        Action::Cancel => {
            if app.pending_confirmation.is_some() {
                app.cancel_confirmation();
            }
            None
        }
        Action::ToggleChart => {
            app.toggle_fullscreen_chart();
            None
        }
        Action::EditAlias => {
            app.start_alias_edit();
            None
        }
        Action::TextInput(c) => {
            if app.editing_alias {
                app.alias_input_char(c);
            }
            None
        }
        Action::TextBackspace => {
            if app.editing_alias {
                app.alias_input_backspace();
            }
            None
        }
        Action::TextSubmit => {
            if app.editing_alias {
                app.save_alias();
            }
            None
        }
        Action::TextCancel => {
            if app.editing_alias {
                app.cancel_alias_edit();
            }
            None
        }
        Action::ToggleComparison => {
            app.toggle_comparison();
            None
        }
        Action::NextComparisonDevice => {
            app.cycle_comparison_device(true);
            None
        }
        Action::PrevComparisonDevice => {
            app.cycle_comparison_device(false);
            None
        }
        Action::ShowErrorDetails => {
            app.toggle_error_details();
            None
        }
        Action::ToggleTheme => {
            app.toggle_theme();
            let theme_name = match app.theme {
                Theme::Dark => "dark",
                Theme::Light => "light",
            };
            app.push_status_message(format!("Theme: {}", theme_name));
            None
        }
        Action::ToggleChartTemp => {
            app.toggle_chart_metric(App::METRIC_TEMP);
            let status = if app.chart_shows(App::METRIC_TEMP) {
                "shown"
            } else {
                "hidden"
            };
            app.push_status_message(format!("Temperature on chart: {}", status));
            None
        }
        Action::ToggleChartHumidity => {
            app.toggle_chart_metric(App::METRIC_HUMIDITY);
            let status = if app.chart_shows(App::METRIC_HUMIDITY) {
                "shown"
            } else {
                "hidden"
            };
            app.push_status_message(format!("Humidity on chart: {}", status));
            None
        }
        Action::ToggleBleRange => {
            app.toggle_ble_range();
            None
        }
        Action::ToggleSmartHome => {
            app.toggle_smart_home();
            None
        }
        Action::ToggleDoNotDisturb => {
            app.toggle_do_not_disturb();
            None
        }
        Action::ToggleExportFormat => {
            app.toggle_export_format();
            None
        }
        Action::None => None,
    }
}
