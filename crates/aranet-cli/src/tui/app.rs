//! Application state for the TUI.
//!
//! This module contains the core state management for the terminal user interface,
//! including device tracking, connection status, and UI navigation.

use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use aranet_core::settings::DeviceSettings;
use aranet_types::{CurrentReading, DeviceType, HistoryRecord};

use super::messages::{CachedDevice, Command, SensorEvent};

/// Maximum number of alert history entries to retain.
const MAX_ALERT_HISTORY: usize = 1000;

/// Bluetooth range mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BleRange {
    #[default]
    Standard,
    Extended,
}

impl BleRange {
    /// Get display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Extended => "Extended",
        }
    }

    /// Toggle between modes.
    pub fn toggle(self) -> Self {
        match self {
            Self::Standard => Self::Extended,
            Self::Extended => Self::Standard,
        }
    }
}

/// UI theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// Get background color for this theme.
    pub fn bg(self) -> ratatui::style::Color {
        match self {
            Self::Dark => ratatui::style::Color::Reset,
            Self::Light => ratatui::style::Color::White,
        }
    }
}

/// Connection status for a device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Device is not connected.
    #[default]
    Disconnected,
    /// Device is currently connecting.
    Connecting,
    /// Device is connected and ready.
    Connected,
    /// Connection error occurred.
    Error(String),
}

/// State for a single Aranet device.
#[derive(Debug, Clone)]
pub struct DeviceState {
    /// Unique device identifier.
    pub id: String,
    /// Device name if known.
    pub name: Option<String>,
    /// User-defined alias for the device.
    pub alias: Option<String>,
    /// Device type if detected.
    pub device_type: Option<DeviceType>,
    /// Most recent sensor reading.
    pub reading: Option<CurrentReading>,
    /// Historical readings for sparkline display.
    pub history: Vec<HistoryRecord>,
    /// Current connection status.
    pub status: ConnectionStatus,
    /// When the device state was last updated.
    pub last_updated: Option<Instant>,
    /// Error message if an error occurred.
    pub error: Option<String>,
    /// Previous reading for trend calculation.
    pub previous_reading: Option<CurrentReading>,
    /// Session statistics for this device.
    pub session_stats: SessionStats,
    /// When history was last synced from the device.
    pub last_sync: Option<time::OffsetDateTime>,
    /// RSSI signal strength (dBm) if available.
    pub rssi: Option<i16>,
    /// When the device was connected (for uptime calculation).
    pub connected_at: Option<std::time::Instant>,
    /// Device settings read from the device.
    pub settings: Option<DeviceSettings>,
}

impl DeviceState {
    /// Create a new device state with the given ID.
    pub fn new(id: String) -> Self {
        Self {
            id,
            name: None,
            alias: None,
            device_type: None,
            reading: None,
            history: Vec::new(),
            status: ConnectionStatus::Disconnected,
            last_updated: None,
            error: None,
            previous_reading: None,
            session_stats: SessionStats::default(),
            last_sync: None,
            rssi: None,
            connected_at: None,
            settings: None,
        }
    }

    /// Get the display name (alias > name > id).
    pub fn display_name(&self) -> &str {
        self.alias
            .as_deref()
            .or(self.name.as_deref())
            .unwrap_or(&self.id)
    }

    /// Get uptime as formatted string if connected.
    pub fn uptime(&self) -> Option<String> {
        let connected_at = self.connected_at?;
        let elapsed = connected_at.elapsed();
        let secs = elapsed.as_secs();

        if secs < 60 {
            Some(format!("{}s", secs))
        } else if secs < 3600 {
            Some(format!("{}m {}s", secs / 60, secs % 60))
        } else {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            Some(format!("{}h {}m", hours, mins))
        }
    }
}

/// UI tab selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    /// Main dashboard showing current readings.
    #[default]
    Dashboard,
    /// Historical data view.
    History,
    /// Application settings.
    Settings,
    /// Service management.
    Service,
}

/// Time range filter for history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HistoryFilter {
    /// Show all history.
    #[default]
    All,
    /// Show today's records only.
    Today,
    /// Show last 24 hours.
    Last24Hours,
    /// Show last 7 days.
    Last7Days,
    /// Show last 30 days.
    Last30Days,
}

impl HistoryFilter {
    /// Get display label for the filter.
    pub fn label(&self) -> &'static str {
        match self {
            HistoryFilter::All => "All",
            HistoryFilter::Today => "Today",
            HistoryFilter::Last24Hours => "24h",
            HistoryFilter::Last7Days => "7d",
            HistoryFilter::Last30Days => "30d",
        }
    }
}

/// Filter for device list display.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DeviceFilter {
    /// Show all devices.
    #[default]
    All,
    /// Show only Aranet4 devices.
    Aranet4Only,
    /// Show only Aranet Radon devices.
    RadonOnly,
    /// Show only Aranet Radiation devices.
    RadiationOnly,
    /// Show only connected devices.
    ConnectedOnly,
}

impl DeviceFilter {
    /// Get display label for the filter.
    pub fn label(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Aranet4Only => "Aranet4",
            Self::RadonOnly => "Radon",
            Self::RadiationOnly => "Radiation",
            Self::ConnectedOnly => "Connected",
        }
    }

    /// Cycle to next filter.
    pub fn next(&self) -> Self {
        match self {
            Self::All => Self::Aranet4Only,
            Self::Aranet4Only => Self::RadonOnly,
            Self::RadonOnly => Self::RadiationOnly,
            Self::RadiationOnly => Self::ConnectedOnly,
            Self::ConnectedOnly => Self::All,
        }
    }
}

/// Alert severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    /// Informational alert (blue).
    Info,
    /// Warning alert (yellow).
    Warning,
    /// Critical alert (red).
    Critical,
}

impl AlertSeverity {
    /// Get the color for this severity.
    pub fn color(self) -> ratatui::style::Color {
        match self {
            Self::Info => ratatui::style::Color::Blue,
            Self::Warning => ratatui::style::Color::Yellow,
            Self::Critical => ratatui::style::Color::Red,
        }
    }

    /// Get the icon for this severity.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Info => "(i)",
            Self::Warning => "(!)",
            Self::Critical => "(X)",
        }
    }
}

/// An active alert for a device.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Alert {
    /// Device ID that triggered the alert.
    pub device_id: String,
    /// Device name for display.
    pub device_name: Option<String>,
    /// Alert message.
    pub message: String,
    /// CO2 level that triggered the alert.
    pub level: aranet_core::Co2Level,
    /// When the alert was triggered.
    pub triggered_at: Instant,
    /// Severity level of the alert.
    pub severity: AlertSeverity,
}

/// Record of a past alert for history viewing.
#[derive(Debug, Clone)]
pub struct AlertRecord {
    /// Device name or ID.
    pub device_name: String,
    /// Alert message.
    pub message: String,
    /// When the alert was triggered.
    pub timestamp: time::OffsetDateTime,
    /// Severity level of the alert.
    pub severity: AlertSeverity,
}

/// Session statistics for a device.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    /// Minimum CO2 reading in session.
    pub co2_min: Option<u16>,
    /// Maximum CO2 reading in session.
    pub co2_max: Option<u16>,
    /// Sum of CO2 readings for average calculation.
    pub co2_sum: u64,
    /// Count of CO2 readings.
    pub co2_count: u32,
    /// Minimum temperature in session.
    pub temp_min: Option<f32>,
    /// Maximum temperature in session.
    pub temp_max: Option<f32>,
}

impl SessionStats {
    /// Update statistics with a new reading.
    pub fn update(&mut self, reading: &CurrentReading) {
        // Only track non-zero CO2 (Aranet4)
        if reading.co2 > 0 {
            self.co2_min = Some(self.co2_min.map_or(reading.co2, |m| m.min(reading.co2)));
            self.co2_max = Some(self.co2_max.map_or(reading.co2, |m| m.max(reading.co2)));
            self.co2_sum += reading.co2 as u64;
            self.co2_count += 1;
        }

        // Temperature
        self.temp_min = Some(
            self.temp_min
                .map_or(reading.temperature, |m| m.min(reading.temperature)),
        );
        self.temp_max = Some(
            self.temp_max
                .map_or(reading.temperature, |m| m.max(reading.temperature)),
        );
    }

    /// Get average CO2.
    pub fn co2_avg(&self) -> Option<u16> {
        if self.co2_count > 0 {
            Some((self.co2_sum / self.co2_count as u64) as u16)
        } else {
            None
        }
    }
}

/// Calculate radon averages from history records.
pub fn calculate_radon_averages(history: &[HistoryRecord]) -> (Option<u32>, Option<u32>) {
    use time::OffsetDateTime;

    let now = OffsetDateTime::now_utc();
    let day_ago = now - time::Duration::days(1);
    let week_ago = now - time::Duration::days(7);

    let mut day_sum: u64 = 0;
    let mut day_count: u32 = 0;
    let mut week_sum: u64 = 0;
    let mut week_count: u32 = 0;

    for record in history {
        if let Some(radon) = record.radon
            && record.timestamp >= week_ago
        {
            week_sum += radon as u64;
            week_count += 1;

            if record.timestamp >= day_ago {
                day_sum += radon as u64;
                day_count += 1;
            }
        }
    }

    let day_avg = if day_count > 0 {
        Some((day_sum / day_count as u64) as u32)
    } else {
        None
    };

    let week_avg = if week_count > 0 {
        Some((week_sum / week_count as u64) as u32)
    } else {
        None
    };

    (day_avg, week_avg)
}

/// Actions that require user confirmation.
#[derive(Debug, Clone)]
pub enum PendingAction {
    /// Disconnect from device.
    Disconnect {
        device_id: String,
        device_name: String,
    },
}

/// Main application state for the TUI.
pub struct App {
    /// Whether the application should exit.
    pub should_quit: bool,
    /// Currently active UI tab.
    pub active_tab: Tab,
    /// Index of the currently selected device.
    pub selected_device: usize,
    /// List of all known devices.
    pub devices: Vec<DeviceState>,
    /// Whether a device scan is in progress.
    pub scanning: bool,
    /// Queue of status messages with their creation time.
    pub status_messages: Vec<(String, Instant)>,
    /// How long to show each status message (in seconds).
    pub status_message_timeout: u64,
    /// Whether to show the help overlay.
    pub show_help: bool,
    /// Channel for sending commands to the background worker.
    #[allow(dead_code)]
    pub command_tx: mpsc::Sender<Command>,
    /// Channel for receiving events from the background worker.
    pub event_rx: mpsc::Receiver<SensorEvent>,
    /// Threshold evaluator for CO2 levels.
    pub thresholds: aranet_core::Thresholds,
    /// Active alerts for devices.
    pub alerts: Vec<Alert>,
    /// History of all alerts (for viewing).
    pub alert_history: Vec<AlertRecord>,
    /// Whether to show alert history overlay.
    pub show_alert_history: bool,
    /// Path to log file for data logging.
    pub log_file: Option<std::path::PathBuf>,
    /// Whether logging is enabled.
    pub logging_enabled: bool,
    /// When the last auto-refresh was triggered.
    pub last_auto_refresh: Option<Instant>,
    /// Auto-refresh interval (uses device interval or 60s default).
    pub auto_refresh_interval: Duration,
    /// Scroll offset for history list in History tab.
    pub history_scroll: usize,
    /// Time range filter for history display.
    pub history_filter: HistoryFilter,
    /// Spinner animation frame counter.
    pub spinner_frame: usize,
    /// Currently selected setting in the Settings tab.
    pub selected_setting: usize,
    /// Available interval options in seconds.
    pub interval_options: Vec<u16>,
    /// Custom CO2 alert threshold (ppm). Default is 1500 (Poor level).
    pub co2_alert_threshold: u16,
    /// Custom radon alert threshold (Bq/m³). Default is 300.
    pub radon_alert_threshold: u16,
    /// Whether to ring terminal bell on alerts.
    pub bell_enabled: bool,
    /// Device list filter.
    pub device_filter: DeviceFilter,
    /// Pending confirmation action.
    pub pending_confirmation: Option<PendingAction>,
    /// Whether to show the device sidebar (can be hidden on narrow terminals).
    pub show_sidebar: bool,
    /// Whether to show full-screen chart view.
    pub show_fullscreen_chart: bool,
    /// Whether currently editing device alias.
    pub editing_alias: bool,
    /// Current alias input buffer.
    pub alias_input: String,
    /// Whether alerts are sticky (don't auto-clear when condition improves).
    pub sticky_alerts: bool,
    /// Last error message (full details).
    pub last_error: Option<String>,
    /// Whether to show error details popup.
    pub show_error_details: bool,
    /// Whether comparison view is active.
    pub show_comparison: bool,
    /// Index of second device for comparison (first is selected_device).
    pub comparison_device_index: Option<usize>,
    /// Sidebar width (default 28, wide 40).
    pub sidebar_width: u16,
    /// Current UI theme.
    pub theme: Theme,
    /// Which metrics to show on sparkline (bitmask: 1=primary, 2=temp, 4=humidity).
    pub chart_metrics: u8,
    /// Whether Smart Home integration mode is enabled.
    pub smart_home_enabled: bool,
    /// Bluetooth range setting.
    pub ble_range: BleRange,
    /// Whether a history sync is in progress.
    pub syncing: bool,
    /// Service client for aranet-service communication.
    /// Currently unused as communication goes through the worker.
    #[allow(dead_code)]
    pub service_client: Option<aranet_core::service_client::ServiceClient>,
    /// Service URL (default: http://localhost:8080).
    pub service_url: String,
    /// Last known service status.
    pub service_status: Option<ServiceState>,
    /// Whether the service is being refreshed.
    pub service_refreshing: bool,
    /// Selected item in service tab (0=start/stop, 1+=devices).
    pub service_selected_item: usize,
}

/// State of the aranet-service.
#[derive(Debug, Clone)]
pub struct ServiceState {
    /// Whether the service is reachable.
    pub reachable: bool,
    /// Whether the collector is running.
    pub collector_running: bool,
    /// When the collector was started (for display purposes).
    #[allow(dead_code)]
    pub started_at: Option<time::OffsetDateTime>,
    /// Uptime in seconds.
    pub uptime_seconds: Option<u64>,
    /// Per-device collection statistics.
    pub devices: Vec<aranet_core::service_client::DeviceCollectionStats>,
    /// Last status fetch time (for staleness detection).
    #[allow(dead_code)]
    pub fetched_at: Instant,
}

impl App {
    /// Create a new application with the given command and event channels.
    pub fn new(command_tx: mpsc::Sender<Command>, event_rx: mpsc::Receiver<SensorEvent>) -> Self {
        Self {
            should_quit: false,
            active_tab: Tab::default(),
            selected_device: 0,
            devices: Vec::new(),
            scanning: false,
            status_messages: Vec::new(),
            status_message_timeout: 5, // 5 seconds
            show_help: false,
            command_tx,
            event_rx,
            thresholds: aranet_core::Thresholds::default(),
            alerts: Vec::new(),
            alert_history: Vec::new(),
            show_alert_history: false,
            log_file: None,
            logging_enabled: false,
            last_auto_refresh: None,
            auto_refresh_interval: Duration::from_secs(60),
            history_scroll: 0,
            history_filter: HistoryFilter::default(),
            spinner_frame: 0,
            selected_setting: 0,
            interval_options: vec![60, 120, 300, 600], // 1, 2, 5, 10 minutes
            co2_alert_threshold: 1500,
            radon_alert_threshold: 300,
            bell_enabled: true,
            device_filter: DeviceFilter::default(),
            pending_confirmation: None,
            show_sidebar: true,
            show_fullscreen_chart: false,
            editing_alias: false,
            alias_input: String::new(),
            sticky_alerts: false,
            last_error: None,
            show_error_details: false,
            show_comparison: false,
            comparison_device_index: None,
            sidebar_width: 28,
            theme: Theme::default(),
            chart_metrics: Self::METRIC_PRIMARY, // Primary metric only by default
            smart_home_enabled: false,
            ble_range: BleRange::default(),
            syncing: false,
            service_client: aranet_core::service_client::ServiceClient::new(
                "http://localhost:8080",
            )
            .ok(),
            service_url: "http://localhost:8080".to_string(),
            service_status: None,
            service_refreshing: false,
            service_selected_item: 0,
        }
    }

    /// Toggle Bluetooth range.
    pub fn toggle_ble_range(&mut self) {
        self.ble_range = self.ble_range.toggle();
        self.push_status_message(format!("BLE range: {}", self.ble_range.name()));
    }

    /// Bitmask constant for primary metric (CO2/Radon/Radiation).
    pub const METRIC_PRIMARY: u8 = 0b001;
    /// Bitmask constant for temperature metric.
    pub const METRIC_TEMP: u8 = 0b010;
    /// Bitmask constant for humidity metric.
    pub const METRIC_HUMIDITY: u8 = 0b100;

    /// Toggle a metric on the chart.
    pub fn toggle_chart_metric(&mut self, metric: u8) {
        self.chart_metrics ^= metric;
        // Ensure at least one metric is shown
        if self.chart_metrics == 0 {
            self.chart_metrics = Self::METRIC_PRIMARY;
        }
    }

    /// Check if a metric is enabled on chart.
    pub fn chart_shows(&self, metric: u8) -> bool {
        self.chart_metrics & metric != 0
    }

    /// Toggle between light and dark theme.
    pub fn toggle_theme(&mut self) {
        self.theme = match self.theme {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        };
    }

    /// Get the current AppTheme based on the theme setting.
    #[must_use]
    pub fn app_theme(&self) -> super::ui::theme::AppTheme {
        match self.theme {
            Theme::Dark => super::ui::theme::AppTheme::dark(),
            Theme::Light => super::ui::theme::AppTheme::light(),
        }
    }

    /// Toggle Smart Home mode.
    pub fn toggle_smart_home(&mut self) {
        self.smart_home_enabled = !self.smart_home_enabled;
        let status = if self.smart_home_enabled {
            "enabled"
        } else {
            "disabled"
        };
        self.push_status_message(format!("Smart Home mode {}", status));
    }

    /// Toggle full-screen chart view.
    pub fn toggle_fullscreen_chart(&mut self) {
        self.show_fullscreen_chart = !self.show_fullscreen_chart;
    }

    /// Returns whether the application should quit.
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Add a status message to the queue.
    pub fn push_status_message(&mut self, message: String) {
        self.status_messages.push((message, Instant::now()));
        // Keep at most 5 messages
        while self.status_messages.len() > 5 {
            self.status_messages.remove(0);
        }
    }

    /// Remove expired status messages.
    pub fn clean_expired_messages(&mut self) {
        let timeout = std::time::Duration::from_secs(self.status_message_timeout);
        self.status_messages
            .retain(|(_, created)| created.elapsed() < timeout);
    }

    /// Get the current status message to display.
    pub fn current_status_message(&self) -> Option<&str> {
        self.status_messages.last().map(|(msg, _)| msg.as_str())
    }

    /// Handle an incoming sensor event and update state accordingly.
    ///
    /// Returns a list of commands to send to the worker (for auto-connect, auto-sync, etc.).
    pub fn handle_sensor_event(&mut self, event: SensorEvent) -> Vec<Command> {
        let mut commands = Vec::new();

        match event {
            SensorEvent::CachedDataLoaded { devices } => {
                // Collect device IDs before handling (for auto-connect)
                let device_ids: Vec<String> = devices.iter().map(|d| d.id.clone()).collect();
                self.handle_cached_data(devices);

                // Auto-connect to all cached devices
                for device_id in device_ids {
                    commands.push(Command::Connect { device_id });
                }
            }
            SensorEvent::ScanStarted => {
                self.scanning = true;
                self.push_status_message("Scanning for devices...".to_string());
            }
            SensorEvent::ScanComplete { devices } => {
                self.scanning = false;
                self.push_status_message(format!("Found {} device(s)", devices.len()));
                // Add discovered devices to our list
                for discovered in devices {
                    let id_str = discovered.id.to_string();
                    if !self.devices.iter().any(|d| d.id == id_str) {
                        let mut device = DeviceState::new(id_str);
                        device.name = discovered.name;
                        device.device_type = discovered.device_type;
                        self.devices.push(device);
                    }
                }
            }
            SensorEvent::ScanError { error } => {
                self.scanning = false;
                let error_msg = format!("Scan: {}", error);
                self.set_error(error_msg);
                self.push_status_message(format!(
                    "Scan error: {} (press E for details)",
                    error.chars().take(40).collect::<String>()
                ));
            }
            SensorEvent::DeviceConnecting { device_id } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.status = ConnectionStatus::Connecting;
                    device.last_updated = Some(Instant::now());
                }
                self.push_status_message("Connecting...".to_string());
            }
            SensorEvent::DeviceConnected {
                device_id,
                name,
                device_type,
                rssi,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.status = ConnectionStatus::Connected;
                    device.name = name.or(device.name.take());
                    device.device_type = device_type.or(device.device_type);
                    device.rssi = rssi;
                    device.last_updated = Some(Instant::now());
                    device.error = None;
                    device.connected_at = Some(Instant::now());
                }
                self.push_status_message("Connected".to_string());

                // Auto-sync history after successful connection
                commands.push(Command::SyncHistory {
                    device_id: device_id.clone(),
                });
            }
            SensorEvent::DeviceDisconnected { device_id } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.status = ConnectionStatus::Disconnected;
                    device.last_updated = Some(Instant::now());
                    device.connected_at = None;
                }
            }
            SensorEvent::ConnectionError { device_id, error } => {
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.display_name().to_string())
                    .unwrap_or_else(|| device_id.clone());
                let error_msg = format!("{}: {}", device_name, error);
                self.set_error(error_msg);
                self.push_status_message(format!(
                    "Connection error: {} (press E for details)",
                    error.chars().take(40).collect::<String>()
                ));
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.status = ConnectionStatus::Error(error.clone());
                    device.error = Some(error);
                    device.last_updated = Some(Instant::now());
                }
            }
            SensorEvent::ReadingUpdated { device_id, reading } => {
                // Check thresholds for alerts
                self.check_thresholds(&device_id, &reading);

                // Log reading to file if enabled
                self.log_reading(&device_id, &reading);

                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    // Update session statistics
                    device.session_stats.update(&reading);
                    // Store previous reading for trend calculation
                    device.previous_reading = device.reading.take();
                    device.reading = Some(reading);
                    device.last_updated = Some(Instant::now());
                    device.error = None;
                }
            }
            SensorEvent::ReadingError { device_id, error } => {
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.display_name().to_string())
                    .unwrap_or_else(|| device_id.clone());
                let error_msg = format!("{}: {}", device_name, error);
                self.set_error(error_msg);
                self.push_status_message(format!(
                    "Reading error: {} (press E for details)",
                    error.chars().take(40).collect::<String>()
                ));
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.error = Some(error);
                    device.last_updated = Some(Instant::now());
                }
            }
            SensorEvent::HistoryLoaded { device_id, records } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.history = records;
                    device.last_updated = Some(Instant::now());
                }
            }
            SensorEvent::HistorySyncStarted { device_id } => {
                self.syncing = true;
                self.push_status_message(format!("Syncing history for {}...", device_id));
            }
            SensorEvent::HistorySynced { device_id, count } => {
                self.syncing = false;
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.last_sync = Some(time::OffsetDateTime::now_utc());
                }
                self.push_status_message(format!("Synced {} records for {}", count, device_id));
            }
            SensorEvent::HistorySyncError { device_id, error } => {
                self.syncing = false;
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.display_name().to_string())
                    .unwrap_or_else(|| device_id.clone());
                let error_msg = format!("{}: {}", device_name, error);
                self.set_error(error_msg);
                self.push_status_message(format!(
                    "History sync failed: {} (press E for details)",
                    error.chars().take(40).collect::<String>()
                ));
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.error = Some(error);
                }
            }
            SensorEvent::IntervalChanged {
                device_id,
                interval_secs,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id)
                    && let Some(reading) = &mut device.reading
                {
                    reading.interval = interval_secs;
                }
                self.push_status_message(format!("Interval set to {}m", interval_secs / 60));
            }
            SensorEvent::IntervalError { device_id, error } => {
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .map(|d| d.display_name().to_string())
                    .unwrap_or_else(|| device_id.clone());
                let error_msg = format!("{}: {}", device_name, error);
                self.set_error(error_msg);
                self.push_status_message(format!(
                    "Set interval failed: {} (press E for details)",
                    error.chars().take(40).collect::<String>()
                ));
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.error = Some(error);
                }
            }
            SensorEvent::SettingsLoaded {
                device_id,
                settings,
            } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.settings = Some(settings);
                    device.last_updated = Some(Instant::now());
                }
            }
            SensorEvent::BluetoothRangeChanged {
                device_id: _,
                extended,
            } => {
                let range = if extended { "Extended" } else { "Standard" };
                self.push_status_message(format!("Bluetooth range set to {}", range));
            }
            SensorEvent::BluetoothRangeError { device_id, error } => {
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .and_then(|d| d.name.clone())
                    .unwrap_or_else(|| device_id.clone());
                let error_msg = format!("{}: {}", device_name, error);
                self.set_error(error_msg);
                self.push_status_message(format!(
                    "Set BT range failed: {} (press E for details)",
                    error.chars().take(40).collect::<String>()
                ));
            }
            SensorEvent::SmartHomeChanged {
                device_id: _,
                enabled,
            } => {
                let mode = if enabled { "enabled" } else { "disabled" };
                self.push_status_message(format!("Smart Home {}", mode));
            }
            SensorEvent::SmartHomeError { device_id, error } => {
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .and_then(|d| d.name.clone())
                    .unwrap_or_else(|| device_id.clone());
                let error_msg = format!("{}: {}", device_name, error);
                self.set_error(error_msg);
                self.push_status_message(format!(
                    "Set Smart Home failed: {} (press E for details)",
                    error.chars().take(40).collect::<String>()
                ));
            }
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
                    started_at: None, // We could compute from uptime if needed
                    uptime_seconds,
                    devices: devices
                        .into_iter()
                        .map(|d| aranet_core::service_client::DeviceCollectionStats {
                            device_id: d.device_id,
                            alias: d.alias,
                            poll_interval: d.poll_interval,
                            polling: d.polling,
                            success_count: d.success_count,
                            failure_count: d.failure_count,
                            last_poll_at: d.last_poll_at,
                            last_error_at: None, // Not tracked in messages, derived from last_error
                            last_error: d.last_error,
                        })
                        .collect(),
                    fetched_at: Instant::now(),
                });
                if reachable {
                    let status = if collector_running {
                        "running"
                    } else {
                        "stopped"
                    };
                    self.push_status_message(format!("Service collector: {}", status));
                } else {
                    self.push_status_message("Service not reachable".to_string());
                }
            }
            SensorEvent::ServiceStatusError { error } => {
                self.service_refreshing = false;
                self.push_status_message(format!("Service error: {}", error));
            }
            SensorEvent::ServiceCollectorStarted => {
                self.push_status_message("Collector started".to_string());
            }
            SensorEvent::ServiceCollectorStopped => {
                self.push_status_message("Collector stopped".to_string());
            }
            SensorEvent::ServiceCollectorError { error } => {
                self.push_status_message(format!("Collector error: {}", error));
            }
            SensorEvent::AliasChanged { device_id, alias } => {
                if let Some(device) = self.devices.iter_mut().find(|d| d.id == device_id) {
                    device.name = alias;
                }
                self.push_status_message("Device renamed".to_string());
            }
            SensorEvent::AliasError {
                device_id: _,
                error,
            } => {
                self.push_status_message(format!("Rename failed: {}", error));
            }
            SensorEvent::DeviceForgotten { device_id } => {
                if let Some(pos) = self.devices.iter().position(|d| d.id == device_id) {
                    self.devices.remove(pos);
                    if self.selected_device >= self.devices.len() && !self.devices.is_empty() {
                        self.selected_device = self.devices.len() - 1;
                    }
                }
                self.push_status_message("Device forgotten".to_string());
            }
            SensorEvent::ForgetDeviceError {
                device_id: _,
                error,
            } => {
                self.push_status_message(format!("Forget failed: {}", error));
            }
        }

        commands
    }

    /// Get a reference to the currently selected device, if any.
    pub fn selected_device(&self) -> Option<&DeviceState> {
        self.devices.get(self.selected_device)
    }

    /// Select the next device in the list.
    pub fn select_next_device(&mut self) {
        if !self.devices.is_empty() {
            self.selected_device = (self.selected_device + 1) % self.devices.len();
            self.reset_history_scroll();
        }
    }

    /// Select the previous device in the list.
    pub fn select_previous_device(&mut self) {
        if !self.devices.is_empty() {
            self.selected_device = self
                .selected_device
                .checked_sub(1)
                .unwrap_or(self.devices.len() - 1);
            self.reset_history_scroll();
        }
    }

    /// Scroll history list up by one page.
    pub fn scroll_history_up(&mut self) {
        self.history_scroll = self.history_scroll.saturating_sub(5);
    }

    /// Scroll history list down by one page.
    pub fn scroll_history_down(&mut self) {
        if let Some(device) = self.selected_device() {
            let max_scroll = device.history.len().saturating_sub(10);
            self.history_scroll = (self.history_scroll + 5).min(max_scroll);
        }
    }

    /// Reset history scroll when device changes.
    pub fn reset_history_scroll(&mut self) {
        self.history_scroll = 0;
    }

    /// Advance the spinner animation frame.
    pub fn tick_spinner(&mut self) {
        self.spinner_frame = (self.spinner_frame + 1) % 10;
    }

    /// Get the current spinner character.
    pub fn spinner_char(&self) -> &'static str {
        const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        SPINNER[self.spinner_frame]
    }

    /// Set history filter.
    pub fn set_history_filter(&mut self, filter: HistoryFilter) {
        self.history_filter = filter;
        self.history_scroll = 0; // Reset scroll when filter changes
    }

    /// Get devices matching current filter.
    pub fn filtered_devices(&self) -> Vec<&DeviceState> {
        self.devices
            .iter()
            .filter(|d| match self.device_filter {
                DeviceFilter::All => true,
                DeviceFilter::Aranet4Only => {
                    matches!(d.device_type, Some(DeviceType::Aranet4))
                }
                DeviceFilter::RadonOnly => {
                    matches!(d.device_type, Some(DeviceType::AranetRadon))
                }
                DeviceFilter::RadiationOnly => {
                    matches!(d.device_type, Some(DeviceType::AranetRadiation))
                }
                DeviceFilter::ConnectedOnly => {
                    matches!(d.status, ConnectionStatus::Connected)
                }
            })
            .collect()
    }

    /// Cycle device filter to next option.
    pub fn cycle_device_filter(&mut self) {
        self.device_filter = self.device_filter.next();
        self.push_status_message(format!("Filter: {}", self.device_filter.label()));
    }

    /// Select the next setting in the Settings tab.
    pub fn select_next_setting(&mut self) {
        self.selected_setting = (self.selected_setting + 1) % 3; // 3 settings now
    }

    /// Select the previous setting in the Settings tab.
    pub fn select_previous_setting(&mut self) {
        self.selected_setting = self.selected_setting.checked_sub(1).unwrap_or(2);
    }

    /// Increase CO2 threshold by 100 ppm.
    pub fn increase_co2_threshold(&mut self) {
        self.co2_alert_threshold = (self.co2_alert_threshold + 100).min(3000);
    }

    /// Decrease CO2 threshold by 100 ppm.
    pub fn decrease_co2_threshold(&mut self) {
        self.co2_alert_threshold = self.co2_alert_threshold.saturating_sub(100).max(500);
    }

    /// Increase radon threshold by 50 Bq/m³.
    pub fn increase_radon_threshold(&mut self) {
        self.radon_alert_threshold = (self.radon_alert_threshold + 50).min(1000);
    }

    /// Decrease radon threshold by 50 Bq/m³.
    pub fn decrease_radon_threshold(&mut self) {
        self.radon_alert_threshold = self.radon_alert_threshold.saturating_sub(50).max(100);
    }

    /// Cycle to next interval option.
    pub fn cycle_interval(&mut self) -> Option<(String, u16)> {
        let device = self.selected_device()?;
        let reading = device.reading.as_ref()?;
        let current_idx = self
            .interval_options
            .iter()
            .position(|&i| i == reading.interval)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % self.interval_options.len();
        let new_interval = self.interval_options[next_idx];
        Some((device.id.clone(), new_interval))
    }

    /// Handle cached device data loaded from the store on startup.
    fn handle_cached_data(&mut self, cached_devices: Vec<CachedDevice>) {
        let count = cached_devices.len();
        if count > 0 {
            self.push_status_message(format!("Loaded {} cached device(s)", count));
        }

        for cached in cached_devices {
            // Check if device already exists (e.g., from live scan)
            if let Some(device) = self.devices.iter_mut().find(|d| d.id == cached.id) {
                // Update with cached data if we don't have live data
                if device.reading.is_none() {
                    device.reading = cached.reading;
                }
                if device.name.is_none() {
                    device.name = cached.name;
                }
                if device.device_type.is_none() {
                    device.device_type = cached.device_type;
                }
                // Always set last_sync from cache if we don't have it
                if device.last_sync.is_none() {
                    device.last_sync = cached.last_sync;
                }
            } else {
                // Add new device from cache
                let mut device = DeviceState::new(cached.id);
                device.name = cached.name;
                device.device_type = cached.device_type;
                device.reading = cached.reading;
                device.last_sync = cached.last_sync;
                // Mark as disconnected since it's from cache
                device.status = ConnectionStatus::Disconnected;
                self.devices.push(device);
            }
        }
    }

    /// Check if a reading exceeds thresholds and create an alert if needed.
    pub fn check_thresholds(&mut self, device_id: &str, reading: &CurrentReading) {
        // Check CO2 against custom threshold
        if reading.co2 > 0 && reading.co2 >= self.co2_alert_threshold {
            let level = self.thresholds.evaluate_co2(reading.co2);

            // Determine severity based on how far above threshold
            let severity = if reading.co2 >= self.co2_alert_threshold * 2 {
                AlertSeverity::Critical
            } else if reading.co2 >= (self.co2_alert_threshold * 3) / 2 {
                AlertSeverity::Warning
            } else {
                AlertSeverity::Info
            };

            // Check if we already have a CO2 alert for this device
            if !self
                .alerts
                .iter()
                .any(|a| a.device_id == device_id && a.message.contains("CO2"))
            {
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .and_then(|d| d.name.clone());

                let message = format!("CO2 at {} ppm - {}", reading.co2, level.action());

                self.alerts.push(Alert {
                    device_id: device_id.to_string(),
                    device_name: device_name.clone(),
                    message: message.clone(),
                    level,
                    triggered_at: Instant::now(),
                    severity,
                });

                // Add to alert history
                self.alert_history.push(AlertRecord {
                    device_name: device_name.unwrap_or_else(|| device_id.to_string()),
                    message,
                    timestamp: time::OffsetDateTime::now_utc(),
                    severity,
                });

                // Keep history limited to last MAX_ALERT_HISTORY entries
                while self.alert_history.len() > MAX_ALERT_HISTORY {
                    self.alert_history.remove(0);
                }

                // Ring terminal bell if enabled
                if self.bell_enabled {
                    print!("\x07"); // ASCII BEL character
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
            }
        } else if reading.co2 > 0 && !self.sticky_alerts {
            // Clear CO2 alert if level improved below threshold (unless sticky)
            self.alerts
                .retain(|a| !(a.device_id == device_id && a.message.contains("CO2")));
        }

        // Check battery level
        if reading.battery > 0 && reading.battery < 20 {
            // Check if we already have a battery alert for this device
            let has_battery_alert = self
                .alerts
                .iter()
                .any(|a| a.device_id == device_id && a.message.contains("Battery"));

            if !has_battery_alert {
                let device_name = self
                    .devices
                    .iter()
                    .find(|d| d.id == device_id)
                    .and_then(|d| d.name.clone());

                // Determine severity: < 10% is Critical, 10-20% is Warning
                let (message, severity) = if reading.battery < 10 {
                    (
                        format!("Battery critically low: {}%", reading.battery),
                        AlertSeverity::Critical,
                    )
                } else {
                    (
                        format!("Battery low: {}%", reading.battery),
                        AlertSeverity::Warning,
                    )
                };

                self.alerts.push(Alert {
                    device_id: device_id.to_string(),
                    device_name: device_name.clone(),
                    message: message.clone(),
                    level: aranet_core::Co2Level::Good, // Not applicable, just a placeholder
                    triggered_at: Instant::now(),
                    severity,
                });

                // Add to alert history
                self.alert_history.push(AlertRecord {
                    device_name: device_name.unwrap_or_else(|| device_id.to_string()),
                    message,
                    timestamp: time::OffsetDateTime::now_utc(),
                    severity,
                });

                // Keep history limited to last MAX_ALERT_HISTORY entries
                while self.alert_history.len() > MAX_ALERT_HISTORY {
                    self.alert_history.remove(0);
                }

                // Ring terminal bell if enabled
                if self.bell_enabled {
                    print!("\x07"); // ASCII BEL character
                    use std::io::Write;
                    std::io::stdout().flush().ok();
                }
            }
        } else if reading.battery >= 20 && !self.sticky_alerts {
            // Clear battery alert if battery improved (unless sticky)
            self.alerts
                .retain(|a| !(a.device_id == device_id && a.message.contains("Battery")));
        }

        // Check radon against custom threshold
        if let Some(radon) = reading.radon {
            if radon >= self.radon_alert_threshold as u32 {
                // Check if we already have a radon alert for this device
                let has_radon_alert = self
                    .alerts
                    .iter()
                    .any(|a| a.device_id == device_id && a.message.contains("Radon"));

                if !has_radon_alert {
                    let device_name = self
                        .devices
                        .iter()
                        .find(|d| d.id == device_id)
                        .and_then(|d| d.name.clone());

                    // Determine severity: 2x threshold is Critical, at threshold is Warning
                    let severity = if radon >= (self.radon_alert_threshold as u32) * 2 {
                        AlertSeverity::Critical
                    } else {
                        AlertSeverity::Warning
                    };

                    let message = format!("Radon high: {} Bq/m³", radon);

                    self.alerts.push(Alert {
                        device_id: device_id.to_string(),
                        device_name: device_name.clone(),
                        message: message.clone(),
                        level: aranet_core::Co2Level::Good, // Not applicable, just a placeholder
                        triggered_at: Instant::now(),
                        severity,
                    });

                    // Add to alert history
                    self.alert_history.push(AlertRecord {
                        device_name: device_name.unwrap_or_else(|| device_id.to_string()),
                        message,
                        timestamp: time::OffsetDateTime::now_utc(),
                        severity,
                    });

                    // Keep history limited to last MAX_ALERT_HISTORY entries
                    while self.alert_history.len() > MAX_ALERT_HISTORY {
                        self.alert_history.remove(0);
                    }

                    // Ring terminal bell if enabled
                    if self.bell_enabled {
                        print!("\x07"); // ASCII BEL character
                        use std::io::Write;
                        std::io::stdout().flush().ok();
                    }
                }
            } else if !self.sticky_alerts {
                // Clear radon alert if level improved (unless sticky)
                self.alerts
                    .retain(|a| !(a.device_id == device_id && a.message.contains("Radon")));
            }
        }
    }

    /// Dismiss an alert for a device.
    pub fn dismiss_alert(&mut self, device_id: &str) {
        self.alerts.retain(|a| a.device_id != device_id);
    }

    /// Toggle alert history view.
    pub fn toggle_alert_history(&mut self) {
        self.show_alert_history = !self.show_alert_history;
    }

    /// Toggle sticky alerts mode.
    pub fn toggle_sticky_alerts(&mut self) {
        self.sticky_alerts = !self.sticky_alerts;
        self.push_status_message(format!(
            "Sticky alerts {}",
            if self.sticky_alerts {
                "enabled"
            } else {
                "disabled"
            }
        ));
    }

    /// Toggle data logging on/off.
    pub fn toggle_logging(&mut self) {
        if self.logging_enabled {
            self.logging_enabled = false;
            self.push_status_message("Logging disabled".to_string());
        } else {
            // Create log file path
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let log_dir = dirs::data_local_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join("aranet")
                .join("logs");

            // Create directory if needed
            if let Err(e) = std::fs::create_dir_all(&log_dir) {
                self.push_status_message(format!("Failed to create log dir: {}", e));
                return;
            }

            let log_path = log_dir.join(format!("readings_{}.csv", timestamp));
            self.log_file = Some(log_path.clone());
            self.logging_enabled = true;
            self.push_status_message(format!("Logging to {}", log_path.display()));
        }
    }

    /// Log a reading to file.
    pub fn log_reading(&self, device_id: &str, reading: &CurrentReading) {
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

    /// Export visible history to CSV file.
    pub fn export_history(&self) -> Option<String> {
        use std::io::Write;

        let device = self.selected_device()?;
        if device.history.is_empty() {
            return None;
        }

        // Filter history based on current filter
        let filtered: Vec<_> = device
            .history
            .iter()
            .filter(|r| self.filter_matches_record(r))
            .collect();

        if filtered.is_empty() {
            return None;
        }

        // Create export directory
        let export_dir = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("aranet")
            .join("exports");
        std::fs::create_dir_all(&export_dir).ok()?;

        // Generate filename with timestamp
        let now =
            time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let filename = format!(
            "history_{}_{}.csv",
            device
                .name
                .as_deref()
                .unwrap_or(&device.id)
                .replace(" ", "_"),
            now.format(
                &time::format_description::parse("[year][month][day]_[hour][minute][second]")
                    .unwrap()
            )
            .unwrap_or_default()
        );
        let path = export_dir.join(&filename);

        // Write CSV
        let mut file = std::fs::File::create(&path).ok()?;

        // Header
        writeln!(
            file,
            "timestamp,co2,temperature,humidity,pressure,radon,radiation_rate"
        )
        .ok()?;

        // Records
        for record in filtered {
            writeln!(
                file,
                "{},{},{:.1},{},{:.1},{},{}",
                record
                    .timestamp
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                record.co2,
                record.temperature,
                record.humidity,
                record.pressure,
                record.radon.map(|v| v.to_string()).unwrap_or_default(),
                record
                    .radiation_rate
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_default(),
            )
            .ok()?;
        }

        Some(path.to_string_lossy().to_string())
    }

    /// Check if a record matches the current history filter.
    fn filter_matches_record(&self, record: &HistoryRecord) -> bool {
        use time::OffsetDateTime;

        match self.history_filter {
            HistoryFilter::All => true,
            HistoryFilter::Today => {
                let now = OffsetDateTime::now_utc();
                record.timestamp.date() == now.date()
            }
            HistoryFilter::Last24Hours => {
                let cutoff = OffsetDateTime::now_utc() - time::Duration::hours(24);
                record.timestamp >= cutoff
            }
            HistoryFilter::Last7Days => {
                let cutoff = OffsetDateTime::now_utc() - time::Duration::days(7);
                record.timestamp >= cutoff
            }
            HistoryFilter::Last30Days => {
                let cutoff = OffsetDateTime::now_utc() - time::Duration::days(30);
                record.timestamp >= cutoff
            }
        }
    }

    /// Check if auto-refresh is due and return list of connected device IDs to refresh.
    pub fn check_auto_refresh(&mut self) -> Vec<String> {
        let now = Instant::now();

        // Determine refresh interval based on first connected device's reading interval
        // or use default of 60 seconds
        let interval = self
            .devices
            .iter()
            .find(|d| d.status == ConnectionStatus::Connected)
            .and_then(|d| d.reading.as_ref())
            .map(|r| Duration::from_secs(r.interval as u64))
            .unwrap_or(Duration::from_secs(60));

        self.auto_refresh_interval = interval;

        // Check if enough time has passed since last refresh
        let should_refresh = match self.last_auto_refresh {
            Some(last) => now.duration_since(last) >= interval,
            None => true, // First refresh
        };

        if should_refresh {
            self.last_auto_refresh = Some(now);
            // Return IDs of all connected devices
            self.devices
                .iter()
                .filter(|d| d.status == ConnectionStatus::Connected)
                .map(|d| d.id.clone())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Request confirmation for an action.
    pub fn request_confirmation(&mut self, action: PendingAction) {
        self.pending_confirmation = Some(action);
    }

    /// Confirm the pending action.
    pub fn confirm_action(&mut self) -> Option<Command> {
        if let Some(action) = self.pending_confirmation.take() {
            match action {
                PendingAction::Disconnect { device_id, .. } => {
                    return Some(Command::Disconnect { device_id });
                }
            }
        }
        None
    }

    /// Cancel the pending action.
    pub fn cancel_confirmation(&mut self) {
        self.pending_confirmation = None;
        self.push_status_message("Cancelled".to_string());
    }

    /// Toggle sidebar visibility.
    pub fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
    }

    /// Toggle between normal and wide sidebar.
    pub fn toggle_sidebar_width(&mut self) {
        self.sidebar_width = if self.sidebar_width == 28 { 40 } else { 28 };
    }

    /// Start editing alias for selected device.
    pub fn start_alias_edit(&mut self) {
        if let Some(device) = self.selected_device() {
            self.alias_input = device
                .alias
                .clone()
                .or_else(|| device.name.clone())
                .unwrap_or_default();
            self.editing_alias = true;
        }
    }

    /// Cancel alias editing.
    pub fn cancel_alias_edit(&mut self) {
        self.editing_alias = false;
        self.alias_input.clear();
    }

    /// Save the alias.
    pub fn save_alias(&mut self) {
        let display_name = if let Some(device) = self.devices.get_mut(self.selected_device) {
            if self.alias_input.trim().is_empty() {
                device.alias = None;
            } else {
                device.alias = Some(self.alias_input.trim().to_string());
            }
            Some(device.display_name().to_string())
        } else {
            None
        };
        if let Some(name) = display_name {
            self.push_status_message(format!("Alias set: {}", name));
        }
        self.editing_alias = false;
        self.alias_input.clear();
    }

    /// Handle character input for alias editing.
    pub fn alias_input_char(&mut self, c: char) {
        if self.alias_input.len() < 20 {
            self.alias_input.push(c);
        }
    }

    /// Handle backspace for alias editing.
    pub fn alias_input_backspace(&mut self) {
        self.alias_input.pop();
    }

    /// Store an error for later display.
    pub fn set_error(&mut self, error: String) {
        self.last_error = Some(error);
    }

    /// Toggle error details popup.
    pub fn toggle_error_details(&mut self) {
        if self.last_error.is_some() {
            self.show_error_details = !self.show_error_details;
        } else {
            self.push_status_message("No error to display".to_string());
        }
    }

    /// Get average CO2 across all connected devices with readings.
    pub fn average_co2(&self) -> Option<u16> {
        let values: Vec<u16> = self
            .devices
            .iter()
            .filter(|d| matches!(d.status, ConnectionStatus::Connected))
            .filter_map(|d| d.reading.as_ref())
            .filter_map(|r| if r.co2 > 0 { Some(r.co2) } else { None })
            .collect();

        if values.is_empty() {
            None
        } else {
            Some((values.iter().map(|&v| v as u32).sum::<u32>() / values.len() as u32) as u16)
        }
    }

    /// Get count of connected devices.
    pub fn connected_count(&self) -> usize {
        self.devices
            .iter()
            .filter(|d| matches!(d.status, ConnectionStatus::Connected))
            .count()
    }

    /// Check if any device is currently connecting.
    pub fn is_any_connecting(&self) -> bool {
        self.devices
            .iter()
            .any(|d| matches!(d.status, ConnectionStatus::Connecting))
    }

    /// Check if a history sync is in progress.
    pub fn is_syncing(&self) -> bool {
        self.syncing
    }

    /// Toggle comparison view.
    pub fn toggle_comparison(&mut self) {
        if self.devices.len() < 2 {
            self.push_status_message("Need at least 2 devices for comparison".to_string());
            return;
        }

        self.show_comparison = !self.show_comparison;

        if self.show_comparison {
            // Pick the next device as comparison target
            let next = (self.selected_device + 1) % self.devices.len();
            self.comparison_device_index = Some(next);
            self.push_status_message(
                "Comparison view: use </> to change second device".to_string(),
            );
        } else {
            self.comparison_device_index = None;
        }
    }

    /// Cycle the comparison device.
    pub fn cycle_comparison_device(&mut self, forward: bool) {
        if !self.show_comparison || self.devices.len() < 2 {
            return;
        }

        let current = self.comparison_device_index.unwrap_or(0);
        let mut next = if forward {
            (current + 1) % self.devices.len()
        } else {
            current.checked_sub(1).unwrap_or(self.devices.len() - 1)
        };

        // Skip the selected device
        if next == self.selected_device {
            next = if forward {
                (next + 1) % self.devices.len()
            } else {
                next.checked_sub(1).unwrap_or(self.devices.len() - 1)
            };
        }

        self.comparison_device_index = Some(next);
    }

    /// Get the comparison device.
    pub fn comparison_device(&self) -> Option<&DeviceState> {
        self.comparison_device_index
            .and_then(|i| self.devices.get(i))
    }
}
