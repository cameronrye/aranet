//! Main application state and UI rendering for the Aranet GUI.
//!
//! This module contains the [`AranetApp`] struct which implements the egui application,
//! handling user input, rendering, and coordinating with the background BLE worker.

use std::collections::VecDeque;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aranet_core::messages::{Command, SensorEvent};
use aranet_core::service_client::DeviceCollectionStats;
use eframe::egui::{self, RichText, UserData, ViewportCommand};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::config::{Config, GuiConfig};

use super::components;
use super::export;
use super::helpers::{SCAN_DURATION, TOAST_DURATION, Toast, ToastType};
use super::theme::{Theme, ThemeMode};
use super::tray::{
    TrayCommand, TrayManager, TrayState, check_co2_threshold, hide_dock_icon, show_dock_icon,
};
use super::types::{
    ConnectionFilter, ConnectionState, DeviceState, DeviceTypeFilter, HistoryFilter, Tab,
};

/// State of the aranet-service.
#[derive(Debug, Clone)]
pub struct ServiceState {
    /// Whether the service is reachable.
    pub reachable: bool,
    /// Whether the collector is running.
    pub collector_running: bool,
    /// Uptime in seconds.
    pub uptime_seconds: Option<u64>,
    /// Per-device collection statistics.
    pub devices: Vec<DeviceCollectionStats>,
    /// Last status fetch time (for staleness detection).
    #[allow(dead_code)]
    pub fetched_at: Instant,
}

/// Main application state.
pub struct AranetApp {
    /// Channel to send commands to the worker.
    pub(crate) command_tx: mpsc::Sender<Command>,
    /// Channel to receive events from the worker (via std mpsc for non-async).
    pub(crate) event_rx: std_mpsc::Receiver<SensorEvent>,
    /// List of discovered/connected devices.
    pub(crate) devices: Vec<DeviceState>,
    /// Currently selected device index.
    pub(crate) selected_device: Option<usize>,
    /// Whether a scan is in progress.
    pub(crate) scanning: bool,
    /// Status message.
    pub(crate) status: String,
    /// Active tab/view.
    pub(crate) active_tab: Tab,
    /// History time filter.
    pub(crate) history_filter: HistoryFilter,
    /// Custom date range start (YYYY-MM-DD string for input).
    pub(crate) custom_date_start: String,
    /// Custom date range end (YYYY-MM-DD string for input).
    pub(crate) custom_date_end: String,
    /// Device type filter for device list.
    pub(crate) device_type_filter: DeviceTypeFilter,
    /// Device connection filter for device list.
    pub(crate) connection_filter: ConnectionFilter,
    /// Whether a settings update is in progress.
    pub(crate) updating_settings: bool,
    /// When the last auto-refresh was triggered.
    pub(crate) last_auto_refresh: Option<Instant>,
    /// Whether auto-refresh is enabled.
    pub(crate) auto_refresh_enabled: bool,
    /// Current theme mode (dark/light).
    pub(crate) theme_mode: ThemeMode,
    /// Current theme colors.
    pub(crate) theme: Theme,
    /// Active toast notifications.
    pub(crate) toasts: Vec<Toast>,
    /// Shared tray state for system tray integration.
    pub(crate) tray_state: Arc<Mutex<TrayState>>,
    /// System tray manager (if tray is available).
    pub(crate) tray_manager: Option<TrayManager>,
    /// Native menu bar manager (if available).
    pub(crate) menu_manager: Option<super::MenuManager>,
    /// Whether the main window is visible (for close-to-tray behavior).
    pub(crate) window_visible: bool,
    /// Whether to minimize to tray instead of quitting when closing window.
    pub(crate) close_to_tray: bool,
    /// Whether running in demo mode with mock data.
    #[allow(dead_code)]
    pub(crate) demo_mode: bool,
    /// Path to save screenshot (if taking screenshot).
    pub(crate) screenshot_path: Option<std::path::PathBuf>,
    /// Frame counter for screenshot delay.
    pub(crate) frame_count: u32,
    /// Number of frames to wait before taking screenshot.
    pub(crate) screenshot_delay_frames: u32,
    // -------------------------------------------------------------------------
    // Service State
    // -------------------------------------------------------------------------
    /// Last known service status.
    pub(crate) service_status: Option<ServiceState>,
    /// Whether the service status is being refreshed.
    pub(crate) service_refreshing: bool,
    /// System service status (installed/running at OS level).
    pub(crate) system_service_status: Option<(bool, bool)>, // (installed, running)
    /// Whether a system service operation is in progress.
    pub(crate) system_service_pending: bool,
    /// Monitored devices in the service configuration.
    pub(crate) service_monitored_devices: Vec<aranet_core::messages::ServiceMonitoredDevice>,
    /// Whether the service config is loading.
    pub(crate) service_config_loading: bool,
    /// Add device dialog state: (address, alias, poll_interval).
    pub(crate) add_device_dialog: Option<(String, String, u64)>,
    // -------------------------------------------------------------------------
    // Application Settings
    // -------------------------------------------------------------------------
    /// GUI-specific configuration (persisted to config file).
    pub(crate) gui_config: GuiConfig,
    // -------------------------------------------------------------------------
    // UI State
    // -------------------------------------------------------------------------
    /// Whether the sidebar is collapsed.
    pub(crate) sidebar_collapsed: bool,
    /// Last known window size for saving on exit.
    pub(crate) last_window_size: Option<egui::Vec2>,
    /// Last known window position for saving on exit.
    pub(crate) last_window_pos: Option<egui::Pos2>,
    /// Alias edit state: (device_id, current_text).
    pub(crate) alias_edit: Option<(String, String)>,
    // -------------------------------------------------------------------------
    // Alert History
    // -------------------------------------------------------------------------
    /// History of alerts for the current session (newest first).
    pub(crate) alert_history: VecDeque<super::types::AlertEntry>,
    /// Maximum number of alerts to keep in history.
    pub(crate) alert_history_max: usize,
    /// Whether the alert history popup is visible.
    pub(crate) alert_history_visible: bool,
    /// Do Not Disturb mode - temporarily suppresses all notifications (per-session).
    pub(crate) do_not_disturb: bool,
    /// Whether to show combined Temperature & Humidity overlay chart.
    pub(crate) show_temp_humidity_overlay: bool,
    /// Whether comparison mode is active (side-by-side device readings).
    pub(crate) comparison_mode: bool,
    /// Indices of devices selected for comparison.
    pub(crate) comparison_devices: Vec<usize>,
    // -------------------------------------------------------------------------
    // Data Logging
    // -------------------------------------------------------------------------
    /// Path to log file for data logging.
    pub(crate) log_file: Option<std::path::PathBuf>,
    /// Whether data logging is enabled.
    pub(crate) logging_enabled: bool,
    // -------------------------------------------------------------------------
    // Alert Settings (feature parity with TUI)
    // -------------------------------------------------------------------------
    /// Whether alerts are sticky (don't auto-clear when condition improves).
    pub(crate) sticky_alerts: bool,
    // -------------------------------------------------------------------------
    // Logo
    // -------------------------------------------------------------------------
    /// Texture handle for the app logo displayed in the header.
    pub(crate) logo_texture: Option<egui::TextureHandle>,
}

impl AranetApp {
    /// Create a new AranetApp instance.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        command_tx: mpsc::Sender<Command>,
        event_rx: std_mpsc::Receiver<SensorEvent>,
        tray_state: Arc<Mutex<TrayState>>,
        tray_manager: Option<TrayManager>,
        menu_manager: Option<super::MenuManager>,
    ) -> Self {
        Self::new_with_options(
            cc,
            command_tx,
            event_rx,
            tray_state,
            tray_manager,
            menu_manager,
            false,
            None,
            3,
        )
    }

    /// Create a new AranetApp instance with demo/screenshot options.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_options(
        cc: &eframe::CreationContext<'_>,
        command_tx: mpsc::Sender<Command>,
        event_rx: std_mpsc::Receiver<SensorEvent>,
        tray_state: Arc<Mutex<TrayState>>,
        tray_manager: Option<TrayManager>,
        menu_manager: Option<super::MenuManager>,
        demo_mode: bool,
        screenshot_path: Option<std::path::PathBuf>,
        screenshot_delay_frames: u32,
    ) -> Self {
        // Load GUI configuration from config file
        let config = Config::load();
        let gui_config = config.gui.clone();

        // Initialize theme based on saved preferences (including compact mode)
        let theme_mode = match gui_config.theme.as_str() {
            "light" => ThemeMode::Light,
            "system" => super::theme::detect_system_theme(),
            _ => ThemeMode::Dark,
        };
        let theme = Theme::for_mode_with_options(theme_mode, gui_config.compact_mode);
        cc.egui_ctx.set_style(theme.to_style());

        // Close-to-tray is enabled only when tray is available and config allows it
        let close_to_tray = tray_manager.is_some() && gui_config.close_to_tray;

        // Sync tray state with config settings
        if let Ok(mut state) = tray_state.lock() {
            state.colored_tray_icon = gui_config.colored_tray_icon;
            state.notifications_enabled = gui_config.notifications_enabled;
            state.notification_sound = gui_config.notification_sound;
        }

        // Sync menu state with initial app state
        if let Some(ref menu) = menu_manager {
            menu.set_dark_mode(theme_mode == ThemeMode::Dark);
            menu.set_auto_refresh(!demo_mode);
        }

        // Load demo devices if in demo mode
        let devices = if demo_mode {
            super::demo::create_demo_devices()
        } else {
            Vec::new()
        };

        // Select first device in demo mode
        let selected_device = if demo_mode && !devices.is_empty() {
            Some(0)
        } else {
            None
        };

        let status = if demo_mode {
            "Demo Mode - 3 devices loaded".to_string()
        } else {
            "Ready - Click 'Scan' to discover devices".to_string()
        };

        Self {
            command_tx,
            event_rx,
            devices,
            selected_device,
            scanning: false,
            status,
            active_tab: Tab::Dashboard,
            history_filter: HistoryFilter::All,
            custom_date_start: String::new(),
            custom_date_end: String::new(),
            device_type_filter: DeviceTypeFilter::All,
            connection_filter: ConnectionFilter::All,
            updating_settings: false,
            last_auto_refresh: None,
            auto_refresh_enabled: !demo_mode, // Disable auto-refresh in demo mode
            theme_mode,
            theme,
            toasts: Vec::new(),
            // Read initial window visibility from tray state (for start_minimized support)
            window_visible: tray_state.lock().map(|s| s.window_visible).unwrap_or(true),
            tray_state,
            tray_manager,
            menu_manager,
            close_to_tray,
            demo_mode,
            screenshot_path,
            frame_count: 0,
            screenshot_delay_frames,
            // Service state
            service_status: None,
            service_refreshing: false,
            system_service_status: None,
            system_service_pending: false,
            service_monitored_devices: Vec::new(),
            service_config_loading: false,
            add_device_dialog: None,
            // Application settings
            sidebar_collapsed: gui_config.sidebar_collapsed,
            last_window_size: None,
            last_window_pos: None,
            // Do Not Disturb mode (persisted in config, read before moving gui_config)
            do_not_disturb: gui_config.do_not_disturb,
            gui_config,
            // Alias edit state
            alias_edit: None,
            // Alert history
            alert_history: VecDeque::new(),
            alert_history_max: 100, // Keep last 100 alerts
            alert_history_visible: false,
            // Temperature & Humidity overlay chart (off by default)
            show_temp_humidity_overlay: false,
            // Comparison mode (off by default)
            comparison_mode: false,
            comparison_devices: Vec::new(),
            // Data logging (off by default)
            log_file: None,
            logging_enabled: false,
            // Alert settings (feature parity with TUI)
            sticky_alerts: false,
            // Logo texture (loaded on first frame)
            logo_texture: None,
        }
    }

    /// Set the menu manager after app creation.
    ///
    /// This is needed because on macOS, the menu must be created AFTER
    /// eframe has initialized NSApp inside the run_native callback.
    pub fn set_menu_manager(&mut self, menu_manager: Option<super::MenuManager>) {
        // Sync menu state with current app state
        if let Some(ref menu) = menu_manager {
            // Theme
            menu.set_theme(&self.gui_config.theme);

            // Auto-refresh
            menu.set_auto_refresh(self.auto_refresh_enabled);

            // Notifications
            menu.set_notifications_enabled(self.gui_config.notifications_enabled);

            // Display options (from config)
            menu.set_display_options(
                self.gui_config.show_co2,
                self.gui_config.show_temperature,
                self.gui_config.show_humidity,
                self.gui_config.show_pressure,
            );

            // Do Not Disturb (from config)
            menu.set_do_not_disturb(self.do_not_disturb);

            // Export enabled based on whether we have devices with history
            let has_history = self.devices.iter().any(|d| !d.history.is_empty());
            menu.set_export_enabled(has_history);
        }
        self.menu_manager = menu_manager;
    }

    /// Add a toast notification.
    pub(crate) fn add_toast(&mut self, message: impl Into<String>, toast_type: ToastType) {
        self.toasts.push(Toast::new(message, toast_type));
    }

    /// Remove expired toasts.
    fn cleanup_toasts(&mut self) {
        self.toasts.retain(|t| !t.is_expired());
    }

    /// Add an alert to the history log.
    fn log_alert(&mut self, alert: super::types::AlertEntry) {
        self.alert_history.push_front(alert); // Add to front (most recent first), O(1)
        // Trim to max size by removing from back (oldest)
        while self.alert_history.len() > self.alert_history_max {
            self.alert_history.pop_back();
        }
    }

    /// Check CO2 level and log alert if threshold exceeded.
    fn check_and_log_co2_alert(&mut self, device_name: &str, co2_ppm: u16) {
        use super::types::{AlertEntry, Co2Level};

        let level = Co2Level::from_ppm(co2_ppm);

        // Get the last alert level for this specific check to avoid duplicate alerts
        let last_co2_alert = self.alert_history.iter().find(|a| {
            a.device_name == device_name && matches!(a.alert_type, super::types::AlertType::Co2)
        });

        let should_log = match last_co2_alert {
            None => matches!(level, Co2Level::Poor | Co2Level::Bad),
            Some(last) => {
                // Log when transitioning to a worse level
                let last_level = if last.message.contains("dangerous") {
                    Co2Level::Bad
                } else if last.message.contains("poor") {
                    Co2Level::Poor
                } else if last.message.contains("moderate") {
                    Co2Level::Moderate
                } else {
                    Co2Level::Good
                };

                matches!(
                    (&last_level, &level),
                    (Co2Level::Good, Co2Level::Poor | Co2Level::Bad)
                        | (Co2Level::Moderate, Co2Level::Poor | Co2Level::Bad)
                        | (Co2Level::Poor, Co2Level::Bad)
                        | (Co2Level::Bad | Co2Level::Poor, Co2Level::Good)
                )
            }
        };

        if should_log {
            let alert = AlertEntry::co2(device_name, co2_ppm, level);
            self.log_alert(alert);
        }
    }

    /// Process system tray events and handle commands.
    fn process_tray_events(&mut self, ctx: &egui::Context) {
        let Some(ref tray_manager) = self.tray_manager else {
            return;
        };

        // Process tray events
        for command in tray_manager.process_events() {
            match command {
                TrayCommand::ShowWindow => {
                    debug!("Tray command: ShowWindow");
                    self.window_visible = true;
                    // Order matters: make window visible first, then show dock icon
                    // (which also activates the app), then focus the window
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    show_dock_icon();
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                TrayCommand::HideWindow => {
                    debug!("Tray command: HideWindow");
                    self.window_visible = false;
                    ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                    hide_dock_icon();
                }
                TrayCommand::ToggleWindow => {
                    debug!(
                        "Tray command: ToggleWindow, visible={}",
                        self.window_visible
                    );
                    self.window_visible = !self.window_visible;
                    if self.window_visible {
                        // Order matters: make window visible first, then show dock icon
                        // (which also activates the app), then focus the window
                        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                        show_dock_icon();
                        ctx.send_viewport_cmd(ViewportCommand::Focus);
                    } else {
                        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
                        hide_dock_icon();
                    }
                }
                TrayCommand::Scan => {
                    debug!("Tray command: Scan");
                    if !self.scanning {
                        self.scanning = true;
                        self.status = "Scanning...".to_string();
                        self.send_command(Command::Scan {
                            duration: SCAN_DURATION,
                        });
                    }
                }
                TrayCommand::RefreshAll => {
                    debug!("Tray command: RefreshAll");
                    self.status = "Refreshing all devices...".to_string();
                    self.send_command(Command::RefreshAll);
                }
                TrayCommand::OpenSettings => {
                    debug!("Tray command: OpenSettings");
                    self.active_tab = Tab::Settings;
                }
                TrayCommand::Quit => {
                    debug!("Tray command: Quit");
                    show_dock_icon(); // Restore dock icon before quitting
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }

        // Sync window visibility to tray state and update menu item states
        if let Ok(mut state) = self.tray_state.lock() {
            state.window_visible = self.window_visible;
        }

        // Update tray menu item enabled states based on window visibility
        if let Some(ref tray_manager) = self.tray_manager {
            tray_manager.update_tooltip();
        }
    }

    /// Process native menu bar events and handle commands.
    fn process_menu_events(&mut self, ctx: &egui::Context) {
        // Collect commands first to avoid borrow conflicts
        let commands: Vec<super::MenuCommand> = self
            .menu_manager
            .as_ref()
            .map(|m| m.process_events())
            .unwrap_or_default();

        if commands.is_empty() {
            return;
        }

        for command in commands {
            match command {
                // === File menu ===
                super::MenuCommand::Scan => {
                    if !self.scanning {
                        self.scanning = true;
                        self.status = "Scanning...".to_string();
                        let _ = self.command_tx.try_send(Command::Scan {
                            duration: std::time::Duration::from_secs(5),
                        });
                    }
                }
                super::MenuCommand::RefreshAll => {
                    self.status = "Refreshing all devices...".to_string();
                    let _ = self.command_tx.try_send(Command::RefreshAll);
                }
                super::MenuCommand::ExportCsv => {
                    self.export_selected_device_history("csv");
                }
                super::MenuCommand::ExportJson => {
                    self.export_selected_device_history("json");
                }

                // === View menu - appearance ===
                super::MenuCommand::ToggleTheme => {
                    self.theme_mode.toggle();
                    self.apply_theme_change(ctx);
                }
                super::MenuCommand::ThemeSystem => {
                    // Detect system preference using platform APIs
                    self.theme_mode = super::theme::detect_system_theme();
                    self.gui_config.theme = "system".to_string();
                    self.apply_theme_change(ctx);
                    self.save_gui_config();
                }
                super::MenuCommand::ThemeLight => {
                    self.theme_mode = ThemeMode::Light;
                    self.gui_config.theme = "light".to_string();
                    self.apply_theme_change(ctx);
                    self.save_gui_config();
                }
                super::MenuCommand::ThemeDark => {
                    self.theme_mode = ThemeMode::Dark;
                    self.gui_config.theme = "dark".to_string();
                    self.apply_theme_change(ctx);
                    self.save_gui_config();
                }

                // === View menu - auto refresh ===
                super::MenuCommand::ToggleAutoRefresh => {
                    self.auto_refresh_enabled = !self.auto_refresh_enabled;
                    if let Some(ref menu) = self.menu_manager {
                        menu.set_auto_refresh(self.auto_refresh_enabled);
                    }
                }
                super::MenuCommand::SetRefreshInterval(seconds) => {
                    // Store in a field if we want to use a custom interval
                    // For now just update the menu state
                    if let Some(ref menu) = self.menu_manager {
                        menu.set_refresh_interval(seconds);
                    }
                    debug!("Refresh interval set to {} seconds", seconds);
                }

                // === View menu - notifications ===
                super::MenuCommand::ToggleNotifications => {
                    self.gui_config.notifications_enabled = !self.gui_config.notifications_enabled;
                    if let Some(ref menu) = self.menu_manager {
                        menu.set_notifications_enabled(self.gui_config.notifications_enabled);
                    }
                    self.save_gui_config();
                }

                // === View menu - alerts ===
                super::MenuCommand::ToggleDoNotDisturb => {
                    self.do_not_disturb = !self.do_not_disturb;
                    if let Some(ref menu) = self.menu_manager {
                        menu.set_do_not_disturb(self.do_not_disturb);
                    }
                    let msg = if self.do_not_disturb {
                        "Do Not Disturb enabled - alerts silenced"
                    } else {
                        "Do Not Disturb disabled"
                    };
                    self.add_toast(msg.to_string(), ToastType::Info);
                }
                super::MenuCommand::ToggleStickyAlerts => {
                    self.toggle_sticky_alerts();
                    if let Some(ref menu) = self.menu_manager {
                        menu.set_sticky_alerts(self.sticky_alerts);
                    }
                }

                // === View menu - data logging ===
                super::MenuCommand::ToggleDataLogging => {
                    self.toggle_logging();
                    if let Some(ref menu) = self.menu_manager {
                        menu.set_data_logging(self.logging_enabled);
                    }
                }

                // === View menu - display toggles ===
                super::MenuCommand::ToggleCo2Display => {
                    self.gui_config.show_co2 = !self.gui_config.show_co2;
                    self.sync_display_toggles_to_menu();
                    self.save_gui_config();
                }
                super::MenuCommand::ToggleTemperatureDisplay => {
                    self.gui_config.show_temperature = !self.gui_config.show_temperature;
                    self.sync_display_toggles_to_menu();
                    self.save_gui_config();
                }
                super::MenuCommand::ToggleHumidityDisplay => {
                    self.gui_config.show_humidity = !self.gui_config.show_humidity;
                    self.sync_display_toggles_to_menu();
                    self.save_gui_config();
                }
                super::MenuCommand::TogglePressureDisplay => {
                    self.gui_config.show_pressure = !self.gui_config.show_pressure;
                    self.sync_display_toggles_to_menu();
                    self.save_gui_config();
                }

                // === View menu - tabs ===
                super::MenuCommand::ShowDashboard => {
                    self.active_tab = Tab::Dashboard;
                }
                super::MenuCommand::ShowHistory => {
                    self.active_tab = Tab::History;
                }
                super::MenuCommand::ShowSettings => {
                    self.active_tab = Tab::Settings;
                }
                super::MenuCommand::ShowService => {
                    self.active_tab = Tab::Service;
                }

                // === Device menu ===
                super::MenuCommand::ConnectDevice(idx) => {
                    if let Some(device) = self.devices.get(idx) {
                        let _ = self.command_tx.try_send(Command::Connect {
                            device_id: device.id.clone(),
                        });
                    }
                }
                super::MenuCommand::DisconnectDevice(idx) => {
                    if let Some(device) = self.devices.get(idx) {
                        let _ = self.command_tx.try_send(Command::Disconnect {
                            device_id: device.id.clone(),
                        });
                    }
                }
                super::MenuCommand::ManageAliases => {
                    // Switch to settings tab where aliases can be managed
                    self.active_tab = Tab::Settings;
                    self.add_toast(
                        "Manage device aliases in Settings".to_string(),
                        ToastType::Info,
                    );
                }
                super::MenuCommand::ForgetDevice(idx) => {
                    if let Some(device) = self.devices.get(idx) {
                        let _ = self.command_tx.try_send(Command::ForgetDevice {
                            device_id: device.id.clone(),
                        });
                    }
                }

                // === Help menu ===
                super::MenuCommand::OpenDocumentation => {
                    if let Err(e) = open::that("https://aranet.dev/docs") {
                        debug!("Failed to open documentation: {}", e);
                    }
                }
                super::MenuCommand::ReportIssue => {
                    if let Err(e) = open::that("https://github.com/cameronrye/aranet/issues/new") {
                        debug!("Failed to open issues page: {}", e);
                    }
                }
                super::MenuCommand::CheckForUpdates => {
                    // Open releases page for now
                    if let Err(e) = open::that("https://github.com/cameronrye/aranet/releases") {
                        debug!("Failed to open releases page: {}", e);
                    }
                }
                super::MenuCommand::ShowAbout => {
                    // Show about info via toast for now
                    self.add_toast(
                        format!(
                            "Aranet v{}\nMade with ❤️ by Cameron Rye\nrye.dev",
                            env!("CARGO_PKG_VERSION")
                        ),
                        ToastType::Info,
                    );
                }

                super::MenuCommand::Quit => {
                    show_dock_icon(); // Restore dock icon before quitting
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }

    /// Apply theme changes to the UI and sync with menu.
    fn apply_theme_change(&mut self, ctx: &egui::Context) {
        self.theme = Theme::for_mode_with_options(self.theme_mode, self.gui_config.compact_mode);
        ctx.set_style(self.theme.to_style());
        if let Some(ref menu) = self.menu_manager {
            menu.set_dark_mode(self.theme_mode == ThemeMode::Dark);
        }
    }

    /// Sync display toggle settings to the menu.
    fn sync_display_toggles_to_menu(&self) {
        if let Some(ref menu) = self.menu_manager {
            menu.set_display_options(
                self.gui_config.show_co2,
                self.gui_config.show_temperature,
                self.gui_config.show_humidity,
                self.gui_config.show_pressure,
            );
        }
    }

    /// Export history for the currently selected device.
    fn export_selected_device_history(&mut self, format: &str) {
        let Some(idx) = self.selected_device else {
            self.add_toast("No device selected".to_string(), ToastType::Error);
            return;
        };

        let Some(device) = self.devices.get(idx) else {
            return;
        };

        if device.history.is_empty() {
            self.add_toast("No history to export".to_string(), ToastType::Error);
            return;
        }

        // Clone the data we need to avoid borrow issues
        let records: Vec<aranet_types::HistoryRecord> = device.history.clone();
        let device_name = device.display_name().to_string();

        let records_refs: Vec<&aranet_types::HistoryRecord> = records.iter().collect();
        self.export_history(&records_refs, &device_name, format);
    }

    /// Update tray state with current sensor readings.
    fn update_tray_state(&self, device_name: &str, co2_ppm: Option<u16>) {
        if let Ok(mut state) = self.tray_state.lock() {
            state.device_name = Some(device_name.to_string());
            if let Some(co2) = co2_ppm {
                check_co2_threshold(&mut state, co2, device_name);
            }
        }

        // Update tray tooltip
        if let Some(ref tray_manager) = self.tray_manager {
            tray_manager.update_tooltip();
        }
    }

    /// Process all pending events from the worker.
    fn process_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
        }
    }

    /// Send a command to the worker.
    pub(crate) fn send_command(&self, cmd: Command) {
        let _ = self.command_tx.try_send(cmd);
    }

    /// Check if auto-refresh is due and refresh connected devices.
    fn check_auto_refresh(&mut self) {
        if !self.auto_refresh_enabled {
            return;
        }

        // Get the shortest interval from connected devices (or default 60s)
        let interval_secs = self
            .devices
            .iter()
            .filter(|d| matches!(d.connection, ConnectionState::Connected))
            .filter_map(|d| d.reading.as_ref())
            .map(|r| r.interval)
            .filter(|&i| i > 0)
            .min()
            .unwrap_or(60) as u64;

        let interval = Duration::from_secs(interval_secs);
        let now = Instant::now();

        let should_refresh = match self.last_auto_refresh {
            Some(last) => now.duration_since(last) >= interval,
            None => {
                self.last_auto_refresh = Some(now);
                false
            }
        };

        if should_refresh {
            self.last_auto_refresh = Some(now);
            let connected_ids: Vec<_> = self
                .devices
                .iter()
                .filter(|d| matches!(d.connection, ConnectionState::Connected))
                .map(|d| d.id.clone())
                .collect();

            for device_id in connected_ids {
                self.send_command(Command::RefreshReading { device_id });
            }
        }
    }

    /// Handle a single event from the worker.
    fn handle_event(&mut self, event: SensorEvent) {
        match event {
            SensorEvent::ScanStarted => {
                self.scanning = true;
                self.status = "Scanning for devices...".to_string();
            }
            SensorEvent::ScanComplete { devices } => {
                self.scanning = false;
                self.status = format!("Found {} device(s)", devices.len());
                for discovered in devices {
                    if !self.devices.iter().any(|d| d.id == discovered.identifier) {
                        self.devices.push(DeviceState::from_discovered(&discovered));
                    }
                }
                // Auto-select first device if none selected
                if self.selected_device.is_none() && !self.devices.is_empty() {
                    self.selected_device = Some(0);
                }
            }
            SensorEvent::ScanError { error } => {
                self.scanning = false;
                self.add_toast(format!("Scan failed: {}", error), ToastType::Error);
            }
            SensorEvent::DeviceConnecting { device_id } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.connection = ConnectionState::Connecting;
                }
                self.status = "Connecting...".to_string();
            }
            SensorEvent::DeviceConnected {
                device_id,
                name,
                device_type,
                rssi,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.connection = ConnectionState::Connected;
                    device.connected_at = Some(std::time::Instant::now());
                    if name.is_some() {
                        device.name = name;
                    }
                    if device_type.is_some() {
                        device.device_type = device_type;
                    }
                    if rssi.is_some() {
                        device.rssi = rssi;
                    }
                }
                self.status = "Connected".to_string();
            }
            SensorEvent::DeviceDisconnected { device_id } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.connection = ConnectionState::Disconnected;
                    device.connected_at = None;
                }
            }
            SensorEvent::ConnectionError {
                device_id,
                error,
                context,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.connection = ConnectionState::Error(error.clone());
                }
                // Show suggestion if available
                let msg = if let Some(ctx) = context
                    && let Some(suggestion) = ctx.suggestion
                {
                    format!("{}\n{}", error, suggestion)
                } else {
                    format!("Connection failed: {}", error)
                };
                self.add_toast(msg, ToastType::Error);
            }
            SensorEvent::ReadingUpdated { device_id, reading } => {
                // Extract CO2 for tray notification before consuming reading
                let co2_ppm = if reading.co2 > 0 {
                    Some(reading.co2)
                } else {
                    None
                };
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.display_name().to_string())
                    .unwrap_or_else(|| device_id.clone());

                // Log reading to file if logging is enabled
                self.log_reading(&device_id, &reading);

                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.update_reading(reading);
                }

                // Update tray state with new reading
                self.update_tray_state(&device_name, co2_ppm);

                // Log alert if CO2 threshold exceeded
                if let Some(co2) = co2_ppm {
                    self.check_and_log_co2_alert(&device_name, co2);
                }

                self.status = "Reading updated".to_string();
            }
            SensorEvent::ReadingError {
                device_id,
                error,
                context,
            } => {
                // Show suggestion if available
                let msg = if let Some(ctx) = context
                    && let Some(suggestion) = ctx.suggestion
                {
                    format!("{}: {}\n{}", device_id, error, suggestion)
                } else {
                    format!("Reading error for {}: {}", device_id, error)
                };
                self.add_toast(msg, ToastType::Error);
            }
            SensorEvent::HistorySyncStarted {
                device_id,
                total_records,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.syncing_history = true;
                    device.sync_progress = Some((0, total_records.unwrap_or(0) as usize));
                }
                if let Some(total) = total_records {
                    self.status = format!("Syncing {} records...", total);
                } else {
                    self.status = "Syncing history...".to_string();
                }
            }
            SensorEvent::HistoryLoaded { device_id, records } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.history = records;
                    device.syncing_history = false;
                }
            }
            SensorEvent::HistorySynced { device_id, count } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.syncing_history = false;
                    device.sync_progress = None;
                    device.last_sync = Some(time::OffsetDateTime::now_utc());
                }
                self.status = format!("Synced {} history records", count);
            }
            SensorEvent::HistorySyncError {
                device_id,
                error,
                context,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.syncing_history = false;
                    device.sync_progress = None;
                }
                // Show suggestion if available
                let msg = if let Some(ctx) = context
                    && let Some(suggestion) = ctx.suggestion
                {
                    format!("History sync failed: {}\n{}", error, suggestion)
                } else {
                    format!("History sync failed: {}", error)
                };
                self.add_toast(msg, ToastType::Error);
            }
            SensorEvent::SettingsLoaded {
                device_id,
                settings,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.settings = Some(settings);
                }
            }
            SensorEvent::IntervalChanged {
                device_id,
                interval_secs,
            } => {
                self.updating_settings = false;
                self.status = format!("Interval set to {} min", interval_secs / 60);
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id)
                    && let Some(reading) = &mut device.reading
                {
                    reading.interval = interval_secs;
                }
            }
            SensorEvent::IntervalError {
                device_id: _,
                error,
                context,
            } => {
                self.updating_settings = false;
                // Include suggestion from context if available
                let msg = if let Some(ctx) = context {
                    if let Some(suggestion) = ctx.suggestion {
                        format!("Failed to set interval: {}. {}", error, suggestion)
                    } else {
                        format!("Failed to set interval: {}", error)
                    }
                } else {
                    format!("Failed to set interval: {}", error)
                };
                self.add_toast(msg, ToastType::Error);
            }
            SensorEvent::BluetoothRangeChanged {
                device_id: _,
                extended,
            } => {
                self.updating_settings = false;
                let range = if extended { "Extended" } else { "Standard" };
                self.status = format!("Bluetooth range set to {}", range);
            }
            SensorEvent::BluetoothRangeError {
                device_id: _,
                error,
                context,
            } => {
                self.updating_settings = false;
                // Include suggestion from context if available
                let msg = if let Some(ctx) = context {
                    if let Some(suggestion) = ctx.suggestion {
                        format!("Failed to set BT range: {}. {}", error, suggestion)
                    } else {
                        format!("Failed to set BT range: {}", error)
                    }
                } else {
                    format!("Failed to set BT range: {}", error)
                };
                self.add_toast(msg, ToastType::Error);
            }
            SensorEvent::SmartHomeChanged {
                device_id: _,
                enabled,
            } => {
                self.updating_settings = false;
                let mode = if enabled { "enabled" } else { "disabled" };
                self.add_toast(format!("Smart Home {}", mode), ToastType::Success);
            }
            SensorEvent::SmartHomeError {
                device_id: _,
                error,
                context,
            } => {
                self.updating_settings = false;
                // Include suggestion from context if available
                let msg = if let Some(ctx) = context {
                    if let Some(suggestion) = ctx.suggestion {
                        format!("Failed to set Smart Home: {}. {}", error, suggestion)
                    } else {
                        format!("Failed to set Smart Home: {}", error)
                    }
                } else {
                    format!("Failed to set Smart Home: {}", error)
                };
                self.add_toast(msg, ToastType::Error);
            }
            SensorEvent::AliasChanged { device_id, alias } => {
                self.updating_settings = false;
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.name = alias.clone();
                }
                let msg = if alias.is_some() {
                    "Device renamed"
                } else {
                    "Device name cleared"
                };
                self.add_toast(msg.to_string(), ToastType::Success);
            }
            SensorEvent::AliasError {
                device_id: _,
                error,
            } => {
                self.updating_settings = false;
                self.add_toast(format!("Failed to rename: {}", error), ToastType::Error);
            }
            SensorEvent::CachedDataLoaded { devices } => {
                for cached in devices {
                    if !self.devices.iter().any(|d| d.id == cached.id) {
                        self.devices.push(DeviceState::from_cached(&cached));
                    }
                }
                if !self.devices.is_empty() {
                    self.status = format!("Loaded {} cached device(s)", self.devices.len());
                    // Auto-select first device if none selected
                    if self.selected_device.is_none() {
                        self.selected_device = Some(0);
                    }
                }
            }
            // Service events
            SensorEvent::ServiceStatusRefreshed {
                reachable,
                collector_running,
                uptime_seconds,
                devices,
            } => {
                self.service_refreshing = false;
                self.service_status = Some(ServiceState {
                    reachable,
                    collector_running,
                    uptime_seconds,
                    devices: devices
                        .into_iter()
                        .map(|d| DeviceCollectionStats {
                            device_id: d.device_id,
                            alias: d.alias,
                            poll_interval: d.poll_interval,
                            last_poll_at: d.last_poll_at,
                            last_error_at: None,
                            last_error: d.last_error,
                            success_count: d.success_count,
                            failure_count: d.failure_count,
                            polling: d.polling,
                        })
                        .collect(),
                    fetched_at: Instant::now(),
                });
                if reachable {
                    self.status = if collector_running {
                        "Service running".to_string()
                    } else {
                        "Service stopped".to_string()
                    };
                } else {
                    self.status = "Service not reachable".to_string();
                }
            }
            SensorEvent::ServiceStatusError { error } => {
                self.service_refreshing = false;
                // Clear stale status and mark as unreachable
                self.service_status = Some(ServiceState {
                    reachable: false,
                    collector_running: false,
                    uptime_seconds: None,
                    devices: vec![],
                    fetched_at: Instant::now(),
                });
                self.status = "Service not reachable".to_string();
                self.add_toast(format!("Service error: {}", error), ToastType::Error);
            }
            SensorEvent::ServiceCollectorStarted => {
                self.add_toast("Collector started", ToastType::Success);
            }
            SensorEvent::ServiceCollectorStopped => {
                self.add_toast("Collector stopped", ToastType::Success);
            }
            SensorEvent::ServiceCollectorError { error } => {
                self.add_toast(format!("Collector error: {}", error), ToastType::Error);
            }
            SensorEvent::SystemServiceStatus { installed, running } => {
                self.system_service_pending = false;
                self.system_service_status = Some((installed, running));
            }
            SensorEvent::SystemServiceInstalled => {
                self.system_service_pending = false;
                self.system_service_status = Some((true, false));
                self.add_toast("Service installed successfully", ToastType::Success);
            }
            SensorEvent::SystemServiceUninstalled => {
                self.system_service_pending = false;
                self.system_service_status = Some((false, false));
                self.add_toast("Service uninstalled", ToastType::Success);
            }
            SensorEvent::SystemServiceStarted => {
                self.system_service_pending = false;
                if let Some((installed, _)) = self.system_service_status {
                    self.system_service_status = Some((installed, true));
                }
                self.add_toast("Service started", ToastType::Success);
            }
            SensorEvent::SystemServiceStopped => {
                self.system_service_pending = false;
                if let Some((installed, _)) = self.system_service_status {
                    self.system_service_status = Some((installed, false));
                }
                self.add_toast("Service stopped", ToastType::Success);
            }
            SensorEvent::SystemServiceError { operation, error } => {
                self.system_service_pending = false;
                self.add_toast(
                    format!("Service {} failed: {}", operation, error),
                    ToastType::Error,
                );
            }
            SensorEvent::ServiceConfigFetched { devices } => {
                self.service_config_loading = false;
                self.service_monitored_devices = devices;
            }
            SensorEvent::ServiceConfigError { error } => {
                self.service_config_loading = false;
                self.add_toast(format!("Config error: {}", error), ToastType::Error);
            }
            SensorEvent::ServiceDeviceAdded { device } => {
                self.add_device_dialog = None;
                self.service_monitored_devices.push(device);
                self.add_toast("Device added to monitoring", ToastType::Success);
            }
            SensorEvent::ServiceDeviceUpdated { device } => {
                if let Some(existing) = self
                    .service_monitored_devices
                    .iter_mut()
                    .find(|d| d.address == device.address)
                {
                    *existing = device;
                }
                self.add_toast("Device updated", ToastType::Success);
            }
            SensorEvent::ServiceDeviceRemoved { address } => {
                self.service_monitored_devices
                    .retain(|d| d.address != address);
                self.add_toast("Device removed from monitoring", ToastType::Success);
            }
            SensorEvent::ServiceDeviceError { operation, error } => {
                self.add_toast(
                    format!("Device {} failed: {}", operation, error),
                    ToastType::Error,
                );
            }
            SensorEvent::DeviceForgotten { device_id } => {
                // Remove device from list
                if let Some(pos) = self.devices.iter().position(|d| d.id == device_id) {
                    let name = self.devices[pos].display_name().to_string();
                    self.devices.remove(pos);

                    // Adjust selected device if needed
                    if let Some(selected) = self.selected_device {
                        if selected == pos {
                            // Selected device was removed, clear selection
                            self.selected_device = None;
                        } else if selected > pos {
                            // Adjust index for removed device
                            self.selected_device = Some(selected - 1);
                        }
                    }

                    // Remove from comparison if present
                    self.comparison_devices.retain(|&i| i != pos);
                    // Adjust comparison indices for removed device
                    for idx in &mut self.comparison_devices {
                        if *idx > pos {
                            *idx -= 1;
                        }
                    }

                    self.add_toast(format!("Forgot device: {}", name), ToastType::Success);
                }
            }
            SensorEvent::ForgetDeviceError { device_id, error } => {
                self.add_toast(
                    format!("Failed to forget device {}: {}", device_id, error),
                    ToastType::Error,
                );
            }
            SensorEvent::HistorySyncProgress {
                device_id,
                downloaded,
                total,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.sync_progress = Some((downloaded, total));
                }
                // Update status with progress percentage
                if total > 0 {
                    let percent = (downloaded as f32 / total as f32 * 100.0) as u32;
                    self.status = format!("Syncing history... {}%", percent);
                }
            }
            SensorEvent::OperationCancelled { operation } => {
                self.scanning = false;
                // Reset any syncing states
                for device in &mut self.devices {
                    device.syncing_history = false;
                    device.sync_progress = None;
                }
                self.status = format!("{} cancelled", operation);
            }
            SensorEvent::BackgroundPollingStarted {
                device_id,
                interval_secs,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.background_polling = Some(interval_secs);
                }
                self.add_toast(
                    format!("Auto-refresh enabled (every {}s)", interval_secs),
                    ToastType::Success,
                );
            }
            SensorEvent::BackgroundPollingStopped { device_id } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.background_polling = None;
                }
                self.add_toast("Auto-refresh disabled".to_string(), ToastType::Success);
            }
            SensorEvent::SignalStrengthUpdate {
                device_id,
                rssi,
                quality,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.rssi = Some(rssi);
                    device.signal_quality = Some(quality);
                }
            }
        }
    }
}

impl eframe::App for AranetApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request repaint at the start to ensure the event loop keeps running
        // even when the window is hidden (important for tray icon events on macOS)
        ctx.request_repaint_after(Duration::from_millis(100));

        // Track window size and position for saving on exit
        ctx.input(|i| {
            if let Some(rect) = i.viewport().inner_rect {
                self.last_window_size = Some(rect.size());
            }
            if let Some(pos) = i.viewport().outer_rect {
                self.last_window_pos = Some(pos.min);
            }
        });

        self.process_events();
        self.check_auto_refresh();
        self.cleanup_toasts();
        self.process_tray_events(ctx);
        self.process_menu_events(ctx);

        // Load logo texture on first frame
        if self.logo_texture.is_none() {
            if let Ok(image) = image::load_from_memory(super::ICON_PNG) {
                let image = image.into_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                let pixels = image.into_flat_samples();
                self.logo_texture = Some(ctx.load_texture(
                    "logo",
                    egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice()),
                    egui::TextureOptions::LINEAR,
                ));
            }
        }

        // Handle screenshot capture in demo mode
        if self.screenshot_path.is_some() {
            self.frame_count += 1;

            // Check if we received a screenshot event
            let screenshot_image = ctx.input(|i| {
                for event in &i.events {
                    if let egui::Event::Screenshot { image, .. } = event {
                        return Some(image.clone());
                    }
                }
                None
            });

            if let Some(image) = screenshot_image
                && let Some(ref path) = self.screenshot_path
            {
                // Save the screenshot
                let pixels = image.as_raw();
                let size = image.size;
                if let Err(e) = image::save_buffer(
                    path,
                    pixels,
                    size[0] as u32,
                    size[1] as u32,
                    image::ColorType::Rgba8,
                ) {
                    tracing::error!("Failed to save screenshot: {}", e);
                } else {
                    info!("Screenshot saved to {:?}", path);
                }
                // Exit after saving screenshot
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }

            // Request screenshot after delay frames
            if self.frame_count == self.screenshot_delay_frames {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
            }
        }

        // Handle close-to-tray behavior
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested && self.close_to_tray {
            // Cancel the close and hide to tray instead
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.window_visible = false;
            ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            hide_dock_icon();
            debug!("Window close intercepted - minimizing to tray");
        }

        // Handle keyboard shortcuts
        let mut toggle_theme = false;
        let mut toggle_sidebar = false;
        let mut sync_history = false;
        let mut export_history_csv = false;
        let mut navigate_device: Option<i32> = None;
        let mut close_dialogs = false;
        ctx.input(|i| {
            // F5: Scan for devices
            if i.key_pressed(egui::Key::F5) && !self.scanning {
                self.send_command(Command::Scan {
                    duration: SCAN_DURATION,
                });
            }
            // Cmd+R: Refresh all connected devices
            if i.modifiers.command && i.key_pressed(egui::Key::R) {
                for device in &self.devices {
                    if matches!(device.connection, ConnectionState::Connected) {
                        self.send_command(Command::RefreshReading {
                            device_id: device.id.clone(),
                        });
                    }
                }
            }
            // Cmd+S: Sync history for selected device
            if i.modifiers.command && i.key_pressed(egui::Key::S) {
                sync_history = true;
            }
            // Cmd+E: Export history to CSV (when on History tab)
            if i.modifiers.command && i.key_pressed(egui::Key::E) {
                export_history_csv = true;
            }
            // Cmd+,: Open settings tab
            if i.modifiers.command && i.key_pressed(egui::Key::Comma) {
                self.active_tab = Tab::Settings;
            }
            // 1/2/3/4: Switch tabs
            if i.key_pressed(egui::Key::Num1) {
                self.active_tab = Tab::Dashboard;
            }
            if i.key_pressed(egui::Key::Num2) {
                self.active_tab = Tab::History;
            }
            if i.key_pressed(egui::Key::Num3) {
                self.active_tab = Tab::Settings;
            }
            if i.key_pressed(egui::Key::Num4) {
                self.active_tab = Tab::Service;
            }
            // T: Toggle theme (when not in text input)
            if i.key_pressed(egui::Key::T) && !i.modifiers.command && !i.modifiers.ctrl {
                toggle_theme = true;
            }
            // A: Toggle auto-refresh
            if i.key_pressed(egui::Key::A) && !i.modifiers.command && !i.modifiers.ctrl {
                self.auto_refresh_enabled = !self.auto_refresh_enabled;
            }
            // [: Toggle sidebar
            if i.key_pressed(egui::Key::OpenBracket) && !i.modifiers.command && !i.modifiers.ctrl {
                toggle_sidebar = true;
            }
            // Up/Down: Navigate device list (with Cmd modifier to avoid text conflicts)
            if i.modifiers.command && i.key_pressed(egui::Key::ArrowUp) {
                navigate_device = Some(-1);
            }
            if i.modifiers.command && i.key_pressed(egui::Key::ArrowDown) {
                navigate_device = Some(1);
            }
            // Escape: Close dialogs/popups and cancel editing
            if i.key_pressed(egui::Key::Escape) {
                close_dialogs = true;
            }
        });

        // Apply deferred actions
        if toggle_theme {
            self.theme_mode.toggle();
            self.theme =
                Theme::for_mode_with_options(self.theme_mode, self.gui_config.compact_mode);
            ctx.set_style(self.theme.to_style());
        }
        if toggle_sidebar {
            self.sidebar_collapsed = !self.sidebar_collapsed;
            self.gui_config.sidebar_collapsed = self.sidebar_collapsed;
            self.save_gui_config();
        }
        // Handle Escape: close dialogs/popups and cancel editing
        if close_dialogs {
            // Close alert history popup if visible
            if self.alert_history_visible {
                self.alert_history_visible = false;
            }
            // Cancel alias editing if in progress
            if self.alias_edit.is_some() {
                self.alias_edit = None;
            }
        }
        if sync_history
            && let Some(idx) = self.selected_device
            && let Some(device) = self.devices.get(idx)
            && matches!(device.connection, ConnectionState::Connected)
        {
            self.send_command(Command::SyncHistory {
                device_id: device.id.clone(),
            });
            self.add_toast(
                format!("Syncing history for {}", device.display_name()),
                ToastType::Info,
            );
        }
        // Handle Cmd+E export (only when on History tab with selected device)
        if export_history_csv
            && self.active_tab == Tab::History
            && let Some(idx) = self.selected_device
        {
            // Parse custom date range if needed
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
                    .map(|d| d + time::Duration::days(1) - time::Duration::seconds(1));
                (start, end)
            } else {
                (None, None)
            };

            // Clone necessary data to avoid borrow checker issues
            let export_data = self.devices.get(idx).and_then(|device| {
                if device.history.is_empty() {
                    return None;
                }
                let now = time::OffsetDateTime::now_utc();
                let filtered: Vec<_> = device
                    .history
                    .iter()
                    .filter(|r| match self.history_filter {
                        HistoryFilter::All => true,
                        HistoryFilter::Last24Hours => {
                            (now - r.timestamp) < time::Duration::hours(24)
                        }
                        HistoryFilter::Last7Days => (now - r.timestamp) < time::Duration::days(7),
                        HistoryFilter::Last30Days => (now - r.timestamp) < time::Duration::days(30),
                        HistoryFilter::Custom => {
                            let after_start = custom_start.is_none_or(|s| r.timestamp >= s);
                            let before_end = custom_end.is_none_or(|e| r.timestamp <= e);
                            after_start && before_end
                        }
                    })
                    .cloned()
                    .collect();
                Some((filtered, device.display_name().to_string()))
            });
            if let Some((filtered, name)) = export_data {
                self.export_history(&filtered.iter().collect::<Vec<_>>(), &name, "csv");
            }
        }
        if let Some(delta) = navigate_device
            && !self.devices.is_empty()
        {
            let current = self.selected_device.unwrap_or(0) as i32;
            let new_idx = (current + delta).clamp(0, self.devices.len() as i32 - 1) as usize;
            self.selected_device = Some(new_idx);
        }

        // Render toast notifications
        if !self.toasts.is_empty() {
            egui::Area::new(egui::Id::new("toasts"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
                .show(ctx, |ui| {
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
                        for toast in &self.toasts {
                            let (is_success, is_error) = match toast.toast_type {
                                ToastType::Success => (true, false),
                                ToastType::Error => (false, true),
                                ToastType::Info => (false, false),
                            };
                            let icon = match toast.toast_type {
                                ToastType::Success => "[OK]",
                                ToastType::Error => "[!]",
                                ToastType::Info => "[i]",
                            };
                            let elapsed = toast.created_at.elapsed().as_secs_f32();
                            let fade_start = TOAST_DURATION.as_secs_f32() - 0.5;
                            let alpha = if elapsed > fade_start {
                                1.0 - (elapsed - fade_start) / 0.5
                            } else {
                                1.0
                            };

                            let bg_color = self.theme.toast_bg(is_success, is_error);
                            let text_color = self
                                .theme
                                .toast_text(is_success, is_error)
                                .gamma_multiply(alpha);
                            let shadow = self.theme.toast_shadow();
                            let shadow_with_alpha = egui::Shadow {
                                color: shadow.color.gamma_multiply(alpha),
                                ..shadow
                            };

                            egui::Frame::new()
                                .fill(bg_color.gamma_multiply(alpha))
                                .inner_margin(egui::Margin::symmetric(12, 8))
                                .corner_radius(egui::CornerRadius::same(
                                    self.theme.rounding.md as u8,
                                ))
                                .shadow(shadow_with_alpha)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(icon).color(text_color).strong());
                                        ui.label(RichText::new(&toast.message).color(text_color));
                                    });
                                });
                            ui.add_space(self.theme.spacing.xs);
                        }
                    });
                });
        }

        // Top panel with title, tabs, and scan button
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(
                        self.theme.spacing.lg as i8,
                        self.theme.spacing.md as i8,
                    ))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // App logo
                    if let Some(texture) = &self.logo_texture {
                        let logo_size = egui::vec2(24.0, 24.0);
                        ui.image(egui::load::SizedTexture::new(texture.id(), logo_size));
                        ui.add_space(self.theme.spacing.sm);
                    }

                    // App title
                    ui.label(
                        RichText::new("Aranet")
                            .size(self.theme.typography.heading)
                            .strong()
                            .color(self.theme.text_primary),
                    );

                    ui.add_space(self.theme.spacing.lg);
                    ui.separator();
                    ui.add_space(self.theme.spacing.sm);

                    // Tab navigation
                    for (tab, label, shortcut) in [
                        (Tab::Dashboard, "Dashboard", "1"),
                        (Tab::History, "History", "2"),
                        (Tab::Settings, "Settings", "3"),
                        (Tab::Service, "Service", "4"),
                    ] {
                        let is_selected = self.active_tab == tab;
                        let text_color = if is_selected {
                            self.theme.accent
                        } else {
                            self.theme.text_secondary
                        };

                        let response = ui.add(
                            egui::Label::new(
                                RichText::new(label)
                                    .size(self.theme.typography.body)
                                    .color(text_color),
                            )
                            .selectable(false)
                            .sense(egui::Sense::click()),
                        );

                        if response.clicked() {
                            self.active_tab = tab;
                        }
                        let rect = response.rect;
                        response.on_hover_text(format!("Press {}", shortcut));

                        // Underline for selected tab
                        if is_selected {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(rect.min.x, rect.max.y + 2.0),
                                    egui::vec2(rect.width(), 2.0),
                                ),
                                egui::CornerRadius::same(1),
                                self.theme.accent,
                            );
                        }

                        ui.add_space(self.theme.spacing.md);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Theme toggle
                        if ui
                            .add(egui::Button::new(
                                RichText::new(self.theme_mode.icon())
                                    .size(self.theme.typography.caption),
                            ))
                            .on_hover_text("Press T to toggle theme")
                            .clicked()
                        {
                            self.theme_mode.toggle();
                            self.theme = Theme::for_mode_with_options(
                                self.theme_mode,
                                self.gui_config.compact_mode,
                            );
                            ctx.set_style(self.theme.to_style());
                        }

                        ui.add_space(self.theme.spacing.sm);

                        // Auto-refresh toggle
                        let auto_color = if self.auto_refresh_enabled {
                            self.theme.success
                        } else {
                            self.theme.text_muted
                        };
                        if ui
                            .add(egui::Button::new(
                                RichText::new(if self.auto_refresh_enabled {
                                    "Auto: On"
                                } else {
                                    "Auto: Off"
                                })
                                .size(self.theme.typography.caption)
                                .color(auto_color),
                            ))
                            .on_hover_text("Press A to toggle auto-refresh")
                            .clicked()
                        {
                            self.auto_refresh_enabled = !self.auto_refresh_enabled;
                        }

                        ui.add_space(self.theme.spacing.sm);

                        // Scan button
                        ui.add_enabled_ui(!self.scanning, |ui| {
                            let btn_style = self.theme.button_primary();
                            let scan_btn = egui::Button::new(
                                RichText::new("Scan")
                                    .size(self.theme.typography.body)
                                    .color(btn_style.text),
                            )
                            .fill(btn_style.fill);
                            if ui.add(scan_btn).on_hover_text("F5").clicked() {
                                self.send_command(Command::Scan {
                                    duration: SCAN_DURATION,
                                });
                            }
                        });
                        if self.scanning {
                            ui.spinner();
                        }
                    });
                });
            });

        // Bottom panel with status
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg_secondary)
                    .inner_margin(egui::Margin::symmetric(
                        self.theme.spacing.lg as i8,
                        self.theme.spacing.sm as i8,
                    ))
                    .stroke(egui::Stroke::new(1.0, self.theme.border_subtle)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Status indicator dot
                    let status_color = if self.scanning {
                        self.theme.warning
                    } else if self
                        .devices
                        .iter()
                        .any(|d| matches!(d.connection, ConnectionState::Connected))
                    {
                        self.theme.success
                    } else {
                        self.theme.text_muted
                    };
                    components::status_dot(ui, status_color, "Connection status");
                    ui.add_space(self.theme.spacing.sm);

                    ui.label(
                        RichText::new(&self.status)
                            .color(self.theme.text_muted)
                            .size(self.theme.typography.caption),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Alert history button
                        let alert_count = self.alert_history.len();
                        let has_recent_alerts = self.alert_history.iter().any(|a| {
                            a.timestamp.elapsed() < Duration::from_secs(300) // 5 minutes
                                && matches!(
                                    a.severity,
                                    super::types::AlertSeverity::Warning
                                        | super::types::AlertSeverity::Critical
                                )
                        });

                        let alert_color = if has_recent_alerts {
                            self.theme.warning
                        } else {
                            self.theme.text_muted
                        };

                        let alert_text = if alert_count > 0 {
                            format!("Alerts ({})", alert_count)
                        } else {
                            "Alerts".to_string()
                        };

                        let ghost_style = self.theme.button_ghost();
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(alert_text)
                                        .size(self.theme.typography.caption)
                                        .color(alert_color),
                                )
                                .fill(ghost_style.fill),
                            )
                            .on_hover_text("View alert history")
                            .clicked()
                        {
                            self.alert_history_visible = !self.alert_history_visible;
                        }

                        ui.add_space(self.theme.spacing.sm);

                        // Comparison mode button (only show if we have multiple connected devices)
                        let connected_count = self
                            .devices
                            .iter()
                            .filter(|d| matches!(d.connection, ConnectionState::Connected))
                            .count();

                        if connected_count >= 2 {
                            let compare_text = if self.comparison_mode {
                                format!("Compare ({})", self.comparison_devices.len())
                            } else {
                                "Compare".to_string()
                            };
                            let btn_style = if self.comparison_mode {
                                self.theme.button_primary()
                            } else {
                                self.theme.button_ghost()
                            };
                            let text_color = if self.comparison_mode {
                                btn_style.text
                            } else {
                                self.theme.text_muted
                            };

                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(compare_text)
                                            .size(self.theme.typography.caption)
                                            .color(text_color),
                                    )
                                    .fill(btn_style.fill),
                                )
                                .on_hover_text(
                                    "Compare readings from multiple devices side-by-side",
                                )
                                .clicked()
                            {
                                self.comparison_mode = !self.comparison_mode;
                                if !self.comparison_mode {
                                    self.comparison_devices.clear();
                                }
                            }
                        }

                        ui.add_space(self.theme.spacing.md);

                        ui.label(
                            RichText::new("F5: Scan | Cmd+R: Refresh | T: Theme | A: Auto")
                                .color(self.theme.text_muted)
                                .size(self.theme.typography.caption),
                        );
                    });
                });
            });

        // Alert history popup
        if self.alert_history_visible {
            self.render_alert_history_popup(ctx);
        }

        // Left panel with device list
        self.render_device_list(ctx);

        // Central panel
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(self.theme.bg_primary)
                    .inner_margin(egui::Margin::same(self.theme.spacing.lg as i8)),
            )
            .show(ctx, |ui| {
                // Service tab is independent of device selection
                if self.active_tab == Tab::Service {
                    self.render_service_panel(ui);
                    return;
                }

                // Comparison mode with devices selected
                if self.comparison_mode && self.comparison_devices.len() >= 2 {
                    self.render_comparison_panel(ui);
                    return;
                }

                // Comparison mode prompt (no devices selected yet)
                if self.comparison_mode {
                    components::empty_state(
                        ui,
                        &self.theme,
                        "Select Devices to Compare",
                        "Click on 2+ devices in the sidebar to compare them side-by-side",
                    );
                    return;
                }

                if let Some(idx) = self.selected_device {
                    let device = self.devices.get(idx).cloned();
                    if let Some(device) = device {
                        match self.active_tab {
                            Tab::Dashboard => self.render_device_panel(ui, &device, idx),
                            Tab::History => self.render_history_panel(ui, &device),
                            Tab::Settings => self.render_settings_panel(ui, &device),
                            Tab::Service => {} // Handled above
                        }
                    }
                } else {
                    components::empty_state(
                        ui,
                        &self.theme,
                        "Select a Device",
                        "Choose a device from the sidebar to view readings",
                    );
                }
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        info!("Sending shutdown command");
        let _ = self.command_tx.try_send(Command::Shutdown);

        // Save window size and position
        let mut config_changed = false;
        if let Some(size) = self.last_window_size
            && size.x > 100.0
            && size.y > 100.0
        {
            self.gui_config.window_width = Some(size.x);
            self.gui_config.window_height = Some(size.y);
            config_changed = true;
        }
        if let Some(pos) = self.last_window_pos {
            // Only save reasonable positions (not off-screen)
            if pos.x >= -1000.0 && pos.y >= -1000.0 && pos.x < 10000.0 && pos.y < 10000.0 {
                self.gui_config.window_x = Some(pos.x);
                self.gui_config.window_y = Some(pos.y);
                config_changed = true;
            }
        }
        if config_changed {
            debug!(
                "Saving window geometry: size={:?}, pos={:?}",
                self.last_window_size, self.last_window_pos
            );
            self.save_gui_config();
        }
    }
}

impl AranetApp {
    /// Save GUI configuration to the config file.
    pub(crate) fn save_gui_config(&self) {
        let mut config = Config::load();
        config.gui = self.gui_config.clone();
        if let Err(e) = config.save() {
            debug!("Failed to save GUI config: {}", e);
        }
    }

    /// Export history records to a file (CSV or JSON).
    pub(crate) fn export_history(
        &mut self,
        records: &[&aranet_types::HistoryRecord],
        device_name: &str,
        format: &str,
    ) {
        match export::export_history(
            records,
            &self.gui_config.export_directory,
            device_name,
            format,
        ) {
            Ok(filename) => {
                self.add_toast(
                    format!("Exported {} records to {}", records.len(), filename),
                    ToastType::Success,
                );
            }
            Err(e) => {
                self.add_toast(format!("Export failed: {}", e), ToastType::Error);
            }
        }
    }

    /// Toggle data logging on/off.
    pub(crate) fn toggle_logging(&mut self) {
        if self.logging_enabled {
            self.logging_enabled = false;
            self.add_toast("Data logging disabled".to_string(), ToastType::Info);
        } else {
            // Create log file path
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let log_dir = dirs::data_local_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("aranet")
                .join("logs");

            // Create directory if needed
            if let Err(e) = std::fs::create_dir_all(&log_dir) {
                self.add_toast(
                    format!("Failed to create log directory: {}", e),
                    ToastType::Error,
                );
                return;
            }

            let log_path = log_dir.join(format!("readings_{}.csv", timestamp));
            self.log_file = Some(log_path.clone());
            self.logging_enabled = true;
            self.add_toast(
                format!("Logging to {}", log_path.display()),
                ToastType::Success,
            );
        }
    }

    /// Log a reading to file if logging is enabled.
    pub(crate) fn log_reading(&self, device_id: &str, reading: &aranet_types::CurrentReading) {
        if !self.logging_enabled {
            return;
        }

        let Some(log_path) = &self.log_file else {
            return;
        };

        use std::io::Write;

        let file_exists = log_path.exists();
        let file = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
        {
            Ok(f) => f,
            Err(_) => return,
        };

        let mut writer = std::io::BufWriter::new(file);

        // Write header if new file
        if !file_exists {
            let _ = writeln!(
                writer,
                "timestamp,device_id,co2,temperature,humidity,pressure,battery,status,radon,radiation_rate"
            );
        }

        let timestamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
        let radon = reading.radon.map(|r| r.to_string()).unwrap_or_default();
        let radiation = reading
            .radiation_rate
            .map(|r| format!("{:.3}", r))
            .unwrap_or_default();

        let _ = writeln!(
            writer,
            "{},{},{},{:.1},{},{:.1},{},{:?},{},{}",
            timestamp,
            device_id,
            reading.co2,
            reading.temperature,
            reading.humidity,
            reading.pressure,
            reading.battery,
            reading.status,
            radon,
            radiation
        );
    }

    /// Toggle sticky alerts mode.
    pub(crate) fn toggle_sticky_alerts(&mut self) {
        self.sticky_alerts = !self.sticky_alerts;
        let msg = if self.sticky_alerts {
            "Sticky alerts enabled - alerts won't auto-clear"
        } else {
            "Sticky alerts disabled"
        };
        self.add_toast(msg.to_string(), ToastType::Info);
    }
}
