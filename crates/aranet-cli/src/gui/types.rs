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
    Error(String),
}

/// Active view/tab in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Dashboard,
    History,
    Settings,
}

/// Time filter for history display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryFilter {
    #[default]
    All,
    Last24Hours,
    Last7Days,
    Last30Days,
}

impl HistoryFilter {
    /// Get display label.
    pub fn label(&self) -> &'static str {
        match self {
            HistoryFilter::All => "All",
            HistoryFilter::Last24Hours => "24h",
            HistoryFilter::Last7Days => "7 days",
            HistoryFilter::Last30Days => "30 days",
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
