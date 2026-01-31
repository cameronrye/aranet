//! Type definitions for the GUI module.

use aranet_core::messages::CachedDevice;
use aranet_core::scan::DiscoveredDevice;
use aranet_core::settings::DeviceSettings;
use aranet_types::{CurrentReading, DeviceType, HistoryRecord};

/// Connection state for a device.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    /// Reconnecting after a connection failure.
    /// Contains the attempt number and optional error message from the last attempt.
    Reconnecting {
        attempt: u32,
        last_error: Option<String>,
    },
    Error(String),
}

impl ConnectionState {
    /// Check if the device is currently in a connecting state (including reconnecting).
    pub fn is_connecting(&self) -> bool {
        matches!(self, Self::Connecting | Self::Reconnecting { .. })
    }

    /// Get a user-friendly status message.
    pub fn status_message(&self) -> String {
        match self {
            Self::Disconnected => "Disconnected".to_string(),
            Self::Connecting => "Connecting...".to_string(),
            Self::Connected => "Connected".to_string(),
            Self::Reconnecting {
                attempt,
                last_error,
            } => {
                if let Some(err) = last_error {
                    format!("Reconnecting (attempt {})... Last error: {}", attempt, err)
                } else {
                    format!("Reconnecting (attempt {})...", attempt)
                }
            }
            Self::Error(msg) => format!("Error: {}", msg),
        }
    }

    /// Get a short status label for display.
    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Disconnected => "Offline",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Reconnecting { .. } => "Reconnecting",
            Self::Error(_) => "Error",
        }
    }
}

/// Active view/tab in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Dashboard,
    History,
    Settings,
    Service,
}

/// Time filter for history display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryFilter {
    #[default]
    All,
    Last24Hours,
    Last7Days,
    Last30Days,
    /// Custom date range (dates stored separately in app state)
    Custom,
}

/// Filter for device list by type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceTypeFilter {
    #[default]
    All,
    Aranet4,
    AranetRadon,
    AranetRadiation,
    Aranet2,
}

impl DeviceTypeFilter {
    /// Get display label for the filter.
    pub fn label(&self) -> &'static str {
        match self {
            DeviceTypeFilter::All => "All",
            DeviceTypeFilter::Aranet4 => "CO2",
            DeviceTypeFilter::AranetRadon => "Radon",
            DeviceTypeFilter::AranetRadiation => "Rad",
            DeviceTypeFilter::Aranet2 => "T/H",
        }
    }

    /// Check if a device type matches this filter.
    pub fn matches(&self, device_type: Option<DeviceType>) -> bool {
        match self {
            DeviceTypeFilter::All => true,
            DeviceTypeFilter::Aranet4 => device_type == Some(DeviceType::Aranet4),
            DeviceTypeFilter::AranetRadon => device_type == Some(DeviceType::AranetRadon),
            DeviceTypeFilter::AranetRadiation => device_type == Some(DeviceType::AranetRadiation),
            DeviceTypeFilter::Aranet2 => device_type == Some(DeviceType::Aranet2),
        }
    }
}

/// Filter for device list by connection status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionFilter {
    #[default]
    All,
    Connected,
    Disconnected,
}

impl ConnectionFilter {
    /// Get display label for the filter.
    pub fn label(&self) -> &'static str {
        match self {
            ConnectionFilter::All => "All",
            ConnectionFilter::Connected => "Connected",
            ConnectionFilter::Disconnected => "Offline",
        }
    }
}

impl HistoryFilter {
    /// Get display label.
    pub fn label(&self) -> &'static str {
        match self {
            HistoryFilter::All => "All",
            HistoryFilter::Last24Hours => "24h",
            HistoryFilter::Last7Days => "7 days",
            HistoryFilter::Last30Days => "30 days",
            HistoryFilter::Custom => "Custom",
        }
    }
}

/// Trend direction for a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trend {
    #[default]
    Stable,
    Rising,
    Falling,
}

impl Trend {
    /// Calculate trend from two values (current vs previous).
    pub fn from_delta(current: f64, previous: f64, threshold: f64) -> Self {
        let delta = current - previous;
        if delta.abs() < threshold {
            Self::Stable
        } else if delta > 0.0 {
            Self::Rising
        } else {
            Self::Falling
        }
    }

    /// Get the trend indicator text.
    pub fn indicator(&self) -> &'static str {
        match self {
            Trend::Stable => "-",
            Trend::Rising => "^",
            Trend::Falling => "v",
        }
    }
}

/// State for a single device in the UI.
#[derive(Debug, Clone)]
pub struct DeviceState {
    pub id: String,
    pub name: Option<String>,
    pub device_type: Option<DeviceType>,
    pub rssi: Option<i16>,
    pub connection: ConnectionState,
    pub reading: Option<CurrentReading>,
    pub previous_reading: Option<CurrentReading>,
    pub history: Vec<HistoryRecord>,
    pub syncing_history: bool,
    pub settings: Option<DeviceSettings>,
    /// Whether the current reading was loaded from cache (not live from device).
    pub reading_from_cache: bool,
    /// When history was last synced from the device.
    pub last_sync: Option<time::OffsetDateTime>,
}

impl DeviceState {
    /// Create from a discovered device.
    pub fn from_discovered(device: &DiscoveredDevice) -> Self {
        Self {
            id: device.identifier.clone(),
            name: device.name.clone(),
            device_type: device.device_type,
            rssi: device.rssi,
            connection: ConnectionState::Disconnected,
            reading: None,
            previous_reading: None,
            history: Vec::new(),
            syncing_history: false,
            settings: None,
            reading_from_cache: false,
            last_sync: None,
        }
    }

    /// Create from a cached device (loaded from store).
    pub fn from_cached(cached: &CachedDevice) -> Self {
        Self {
            id: cached.id.clone(),
            name: cached.name.clone(),
            device_type: cached.device_type,
            rssi: None,
            connection: ConnectionState::Disconnected,
            reading: cached.reading,
            previous_reading: None,
            history: Vec::new(),
            syncing_history: false,
            settings: None,
            reading_from_cache: cached.reading.is_some(), // Mark as cached if reading exists
            last_sync: cached.last_sync,
        }
    }

    /// Get display name (name or ID).
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// Update reading and store previous for trend calculation.
    ///
    /// This marks the reading as live (not from cache) since it came from the device.
    pub fn update_reading(&mut self, reading: CurrentReading) {
        self.previous_reading = self.reading;
        self.reading = Some(reading);
        self.reading_from_cache = false; // Live reading from device
    }

    /// Check if showing cached/offline data (disconnected but has reading).
    pub fn is_showing_cached_data(&self) -> bool {
        self.reading.is_some()
            && !matches!(self.connection, ConnectionState::Connected)
            && self.reading_from_cache
    }

    /// Get CO2 trend if both current and previous readings are available.
    pub fn co2_trend(&self) -> Option<Trend> {
        let current = self.reading.as_ref()?.co2;
        let previous = self.previous_reading.as_ref()?.co2;
        if current == 0 || previous == 0 {
            return None;
        }
        Some(Trend::from_delta(current as f64, previous as f64, 20.0))
    }

    /// Get temperature trend.
    pub fn temperature_trend(&self) -> Option<Trend> {
        let current = self.reading.as_ref()?.temperature;
        let previous = self.previous_reading.as_ref()?.temperature;
        Some(Trend::from_delta(current as f64, previous as f64, 0.3))
    }

    /// Get humidity trend.
    pub fn humidity_trend(&self) -> Option<Trend> {
        let current = self.reading.as_ref()?.humidity;
        let previous = self.previous_reading.as_ref()?.humidity;
        Some(Trend::from_delta(current as f64, previous as f64, 2.0))
    }
}

/// CO2 level for color coding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Co2Level {
    Good,     // < 800 ppm (green)
    Moderate, // 800-1000 ppm (yellow)
    Poor,     // 1000-1500 ppm (orange)
    Bad,      // > 1500 ppm (red)
}

impl Co2Level {
    /// Evaluate CO2 level from ppm.
    pub fn from_ppm(co2: u16) -> Self {
        if co2 < 800 {
            Self::Good
        } else if co2 < 1000 {
            Self::Moderate
        } else if co2 < 1500 {
            Self::Poor
        } else {
            Self::Bad
        }
    }
}

/// Radon level for color coding.
/// Based on WHO and EPA guidelines:
/// - < 100 Bq/m³: Low risk (green)
/// - 100-300 Bq/m³: Moderate risk, consider action (yellow)
/// - > 300 Bq/m³: High risk, action recommended (red)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadonLevel {
    Low,      // < 100 Bq/m³ (green)
    Moderate, // 100-300 Bq/m³ (yellow)
    High,     // > 300 Bq/m³ (red)
}

impl RadonLevel {
    /// Evaluate radon level from Bq/m³.
    pub fn from_bq(bq: u32) -> Self {
        if bq < 100 {
            Self::Low
        } else if bq < 300 {
            Self::Moderate
        } else {
            Self::High
        }
    }

    /// Get status text for this level.
    pub fn status_text(&self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Moderate => "Moderate",
            Self::High => "High",
        }
    }
}

/// Radiation level for color coding.
/// Based on typical background radiation levels:
/// - < 0.3 µSv/h: Normal background (green)
/// - 0.3-1.0 µSv/h: Elevated (yellow)
/// - > 1.0 µSv/h: High, investigate (red)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadiationLevel {
    Normal,   // < 0.3 µSv/h (green)
    Elevated, // 0.3-1.0 µSv/h (yellow)
    High,     // > 1.0 µSv/h (red)
}

/// Alert severity level for categorizing alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    /// Informational alert (e.g., level returning to normal)
    Info,
    /// Warning alert (e.g., CO2 becoming elevated)
    Warning,
    /// Critical alert (e.g., CO2 at dangerous level)
    Critical,
}

impl AlertSeverity {
    /// Get display label for the severity.
    pub fn label(&self) -> &'static str {
        match self {
            AlertSeverity::Info => "Info",
            AlertSeverity::Warning => "Warning",
            AlertSeverity::Critical => "Critical",
        }
    }

    /// Get icon for the severity.
    pub fn icon(&self) -> &'static str {
        match self {
            AlertSeverity::Info => "[i]",
            AlertSeverity::Warning => "[!]",
            AlertSeverity::Critical => "[!!]",
        }
    }
}

/// Type of measurement that triggered an alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertType {
    /// CO2 level alert
    Co2,
    /// Radon level alert
    Radon,
    /// Radiation level alert
    Radiation,
    /// Battery low alert
    BatteryLow,
}

impl AlertType {
    /// Get display label for the alert type.
    pub fn label(&self) -> &'static str {
        match self {
            AlertType::Co2 => "CO2",
            AlertType::Radon => "Radon",
            AlertType::Radiation => "Radiation",
            AlertType::BatteryLow => "Battery",
        }
    }
}

/// An entry in the alert history log.
#[derive(Debug, Clone)]
pub struct AlertEntry {
    /// When the alert was triggered.
    pub timestamp: std::time::Instant,
    /// Human-readable timestamp for display.
    pub time_str: String,
    /// Device name that triggered the alert.
    pub device_name: String,
    /// Type of alert (CO2, Radon, etc.).
    pub alert_type: AlertType,
    /// Severity of the alert.
    pub severity: AlertSeverity,
    /// The measurement value that triggered the alert.
    pub value: String,
    /// Alert message/description.
    pub message: String,
}

impl AlertEntry {
    /// Create a new CO2 alert entry.
    pub fn co2(device_name: &str, co2_ppm: u16, level: Co2Level) -> Self {
        let (severity, message) = match level {
            Co2Level::Good => (
                AlertSeverity::Info,
                format!("CO2 level returned to normal ({} ppm)", co2_ppm),
            ),
            Co2Level::Moderate => (
                AlertSeverity::Info,
                format!(
                    "CO2 level moderate ({} ppm) - consider ventilating",
                    co2_ppm
                ),
            ),
            Co2Level::Poor => (
                AlertSeverity::Warning,
                format!("CO2 level poor ({} ppm) - ventilation recommended", co2_ppm),
            ),
            Co2Level::Bad => (
                AlertSeverity::Critical,
                format!(
                    "CO2 level dangerous ({} ppm) - ventilate immediately",
                    co2_ppm
                ),
            ),
        };

        Self {
            timestamp: std::time::Instant::now(),
            time_str: format_current_time(),
            device_name: device_name.to_string(),
            alert_type: AlertType::Co2,
            severity,
            value: format!("{} ppm", co2_ppm),
            message,
        }
    }

    /// Create a new radon alert entry.
    pub fn radon(device_name: &str, bq: u32, level: RadonLevel) -> Self {
        let (severity, message) = match level {
            RadonLevel::Low => (
                AlertSeverity::Info,
                format!("Radon level returned to low ({} Bq/m³)", bq),
            ),
            RadonLevel::Moderate => (
                AlertSeverity::Warning,
                format!("Radon level moderate ({} Bq/m³) - consider mitigation", bq),
            ),
            RadonLevel::High => (
                AlertSeverity::Critical,
                format!("Radon level high ({} Bq/m³) - action recommended", bq),
            ),
        };

        Self {
            timestamp: std::time::Instant::now(),
            time_str: format_current_time(),
            device_name: device_name.to_string(),
            alert_type: AlertType::Radon,
            severity,
            value: format!("{} Bq/m³", bq),
            message,
        }
    }

    /// Create a battery low alert entry.
    pub fn battery_low(device_name: &str, battery_pct: u8) -> Self {
        Self {
            timestamp: std::time::Instant::now(),
            time_str: format_current_time(),
            device_name: device_name.to_string(),
            alert_type: AlertType::BatteryLow,
            severity: AlertSeverity::Warning,
            value: format!("{}%", battery_pct),
            message: format!("Battery low ({}%) - consider charging", battery_pct),
        }
    }

    /// Get the age of this alert as a human-readable string.
    pub fn age_str(&self) -> String {
        let elapsed = self.timestamp.elapsed();
        let secs = elapsed.as_secs();
        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            format!("{} min ago", secs / 60)
        } else if secs < 86400 {
            format!("{} hr ago", secs / 3600)
        } else {
            format!("{} days ago", secs / 86400)
        }
    }
}

/// Format the current local time as HH:MM:SS.
fn format_current_time() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    format!("{:02}:{:02}:{:02}", now.hour(), now.minute(), now.second())
}

impl RadiationLevel {
    /// Evaluate radiation level from µSv/h.
    pub fn from_usv(usv: f32) -> Self {
        if usv < 0.3 {
            Self::Normal
        } else if usv < 1.0 {
            Self::Elevated
        } else {
            Self::High
        }
    }

    /// Get status text for this level.
    pub fn status_text(&self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Elevated => "Elevated",
            Self::High => "High",
        }
    }
}
