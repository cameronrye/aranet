//! Output formatting utilities for text, JSON, and CSV output.

use anyhow::Result;
use aranet_core::DiscoveredDevice;
use aranet_types::{CurrentReading, DeviceInfo, HistoryRecord, Status};
use owo_colors::OwoColorize;
use serde::Serialize;

/// Formatting options for output.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormatOptions {
    /// Disable colored output.
    pub no_color: bool,
    /// Use Fahrenheit for temperatures.
    pub fahrenheit: bool,
    /// Omit header row in CSV output.
    pub no_header: bool,
    /// Use compact JSON output (no pretty-printing).
    pub compact: bool,
    /// Use Bq/m³ for radon (SI units) instead of pCi/L.
    pub bq: bool,
}

impl FormatOptions {
    pub fn new(no_color: bool, fahrenheit: bool) -> Self {
        Self {
            no_color,
            fahrenheit,
            no_header: false,
            compact: false,
            bq: false,
        }
    }

    /// Create with no_header option for CSV output.
    pub fn with_no_header(mut self, no_header: bool) -> Self {
        self.no_header = no_header;
        self
    }

    /// Create with compact JSON option.
    pub fn with_compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    /// Create with Bq/m³ radon unit option.
    pub fn with_bq(mut self, bq: bool) -> Self {
        self.bq = bq;
        self
    }

    /// Serialize value to JSON string, respecting compact option.
    pub fn as_json<T: serde::Serialize>(&self, value: &T) -> Result<String> {
        let json = if self.compact {
            serde_json::to_string(value)?
        } else {
            serde_json::to_string_pretty(value)?
        };
        Ok(json + "\n")
    }

    /// Format temperature with appropriate unit.
    #[must_use]
    pub fn format_temp(&self, celsius: f32) -> String {
        if self.fahrenheit {
            let fahrenheit = celsius * 9.0 / 5.0 + 32.0;
            format!("{:.1}°F", fahrenheit)
        } else {
            format!("{:.1}°C", celsius)
        }
    }

    /// Convert temperature value (for CSV/JSON output).
    #[must_use]
    pub fn convert_temp(&self, celsius: f32) -> f32 {
        if self.fahrenheit {
            celsius * 9.0 / 5.0 + 32.0
        } else {
            celsius
        }
    }

    /// Format radon value with appropriate unit.
    #[must_use]
    pub fn format_radon(&self, bq: u32) -> String {
        if self.bq {
            format!("{} Bq/m³", bq)
        } else {
            format!("{:.2} pCi/L", bq_to_pci(bq))
        }
    }

    /// Get radon CSV header name.
    #[must_use]
    pub fn radon_csv_header(&self) -> &'static str {
        if self.bq { "radon_bq" } else { "radon_pci" }
    }

    /// Convert radon value for CSV/JSON output.
    #[must_use]
    pub fn convert_radon(&self, bq: u32) -> f32 {
        if self.bq { bq as f32 } else { bq_to_pci(bq) }
    }
}

/// Convert Bq/m³ to pCi/L (1 Bq/m³ = 0.027 pCi/L)
#[must_use]
pub fn bq_to_pci(bq: u32) -> f32 {
    bq as f32 * 0.027
}

/// Escape a string for CSV output.
/// Wraps the value in quotes if it contains commas, quotes, or newlines.
/// Double quotes are escaped by doubling them.
#[must_use]
pub fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Format CO2 status with color
#[must_use]
pub fn format_status(status: Status, no_color: bool) -> String {
    let label = match status {
        Status::Green => "GREEN",
        Status::Yellow => "AMBER",
        Status::Red => "RED",
        Status::Error => "ERROR",
        _ => "UNKNOWN",
    };

    if no_color {
        format!("[{}]", label)
    } else {
        match status {
            Status::Green => format!("[{}]", label.green()),
            Status::Yellow => format!("[{}]", label.yellow()),
            Status::Red => format!("[{}]", label.red()),
            Status::Error => format!("[{}]", label.dimmed()),
            _ => format!("[{}]", label.dimmed()),
        }
    }
}

/// Format age in human-readable format
#[must_use]
pub fn format_age(seconds: u16) -> String {
    if seconds < 60 {
        format!("{}s ago", seconds)
    } else {
        format!("{}m {}s ago", seconds / 60, seconds % 60)
    }
}

// ============================================================================
// Scan formatting
// ============================================================================

pub fn format_scan_json(devices: &[DiscoveredDevice], opts: &FormatOptions) -> Result<String> {
    #[derive(Serialize)]
    struct ScanResult<'a> {
        count: usize,
        devices: Vec<DeviceJson<'a>>,
    }

    #[derive(Serialize)]
    struct DeviceJson<'a> {
        name: Option<&'a str>,
        address: &'a str,
        identifier: &'a str,
        rssi: Option<i16>,
        device_type: Option<String>,
    }

    let result = ScanResult {
        count: devices.len(),
        devices: devices
            .iter()
            .map(|d| DeviceJson {
                name: d.name.as_deref(),
                address: &d.address,
                identifier: &d.identifier,
                rssi: d.rssi,
                device_type: d.device_type.map(|t| format!("{:?}", t)),
            })
            .collect(),
    };

    opts.as_json(&result)
}

#[must_use]
pub fn format_scan_text(devices: &[DiscoveredDevice], opts: &FormatOptions) -> String {
    if devices.is_empty() {
        return "No Aranet devices found.\n".to_string();
    }

    let mut output = format!("Found {} device(s):\n\n", devices.len());
    for device in devices {
        let name = device.name.as_deref().unwrap_or("Unknown");
        let rssi = device
            .rssi
            .map(|r| format!("{} dBm", r))
            .unwrap_or_else(|| "N/A".to_string());
        let dtype = device
            .device_type
            .map(|t| format!("{:?}", t))
            .unwrap_or_else(|| "Unknown".to_string());

        // Color the device name if colors are enabled
        let name_display = if opts.no_color {
            name.to_string()
        } else {
            format!("{}", name.cyan())
        };

        output.push_str(&format!(
            "  {:<20} {:<20} RSSI: {:<10} Type: {}\n",
            name_display, device.identifier, rssi, dtype
        ));
    }
    output
}

#[must_use]
pub fn format_scan_csv(devices: &[DiscoveredDevice], opts: &FormatOptions) -> String {
    let mut output = if opts.no_header {
        String::new()
    } else {
        "name,address,identifier,rssi,device_type\n".to_string()
    };
    for device in devices {
        output.push_str(&format!(
            "{},{},{},{},{}\n",
            csv_escape(device.name.as_deref().unwrap_or("")),
            csv_escape(&device.address),
            csv_escape(&device.identifier),
            device.rssi.map(|r| r.to_string()).unwrap_or_default(),
            device
                .device_type
                .map(|t| format!("{:?}", t))
                .unwrap_or_default()
        ));
    }
    output
}

// ============================================================================
// Reading formatting
// ============================================================================

#[must_use]
pub fn format_reading_text(reading: &CurrentReading, opts: &FormatOptions) -> String {
    let mut output = String::new();

    // CO2 (Aranet4)
    if reading.co2 > 0 {
        output.push_str(&format!(
            "CO₂:         {:>5} ppm   {}\n",
            reading.co2,
            format_status(reading.status, opts.no_color)
        ));
    }

    // Radon (AranetRn+)
    if let Some(radon) = reading.radon {
        output.push_str(&format!(
            "Radon:       {:>10}  {}\n",
            opts.format_radon(radon),
            format_status(reading.status, opts.no_color)
        ));
    }

    // Radon averages (AranetRn+)
    if let Some(avg_24h) = reading.radon_avg_24h {
        output.push_str(&format!(
            "  24h Avg:   {:>10}\n",
            opts.format_radon(avg_24h)
        ));
    }
    if let Some(avg_7d) = reading.radon_avg_7d {
        output.push_str(&format!("  7d Avg:    {:>10}\n", opts.format_radon(avg_7d)));
    }
    if let Some(avg_30d) = reading.radon_avg_30d {
        output.push_str(&format!(
            "  30d Avg:   {:>10}\n",
            opts.format_radon(avg_30d)
        ));
    }

    // Radiation (Aranet Radiation)
    if let Some(rate) = reading.radiation_rate {
        output.push_str(&format!("Radiation:   {:>5.3} µSv/h\n", rate));
    }
    if let Some(total) = reading.radiation_total {
        output.push_str(&format!("Total Dose:  {:>5.3} mSv\n", total));
    }

    // Common fields
    if reading.temperature != 0.0 {
        output.push_str(&format!(
            "Temperature: {:>8}\n",
            opts.format_temp(reading.temperature)
        ));
    }
    if reading.humidity > 0 {
        output.push_str(&format!("Humidity:    {:>5}%\n", reading.humidity));
    }
    if reading.pressure != 0.0 {
        output.push_str(&format!("Pressure:    {:>5.1} hPa\n", reading.pressure));
    }
    output.push_str(&format!("Battery:     {:>5}%\n", reading.battery));
    output.push_str(&format!("Last Update: {}\n", format_age(reading.age)));
    output.push_str(&format!("Interval:    {} minutes\n", reading.interval / 60));

    output
}

#[must_use]
pub fn format_reading_csv(reading: &CurrentReading, opts: &FormatOptions) -> String {
    let temp_header = if opts.fahrenheit {
        "temperature_f"
    } else {
        "temperature_c"
    };
    let radon_value = reading
        .radon
        .map(|r| format!("{:.2}", opts.convert_radon(r)))
        .unwrap_or_default();
    let radiation_rate = reading
        .radiation_rate
        .map(|r| format!("{:.3}", r))
        .unwrap_or_default();
    let radiation_total = reading
        .radiation_total
        .map(|r| format!("{:.3}", r))
        .unwrap_or_default();

    if opts.no_header {
        format!(
            "{},{:.1},{},{:.1},{},{:?},{},{},{},{},{}\n",
            reading.co2,
            opts.convert_temp(reading.temperature),
            reading.humidity,
            reading.pressure,
            reading.battery,
            reading.status,
            reading.age,
            reading.interval,
            radon_value,
            radiation_rate,
            radiation_total
        )
    } else {
        format!(
            "co2,{},humidity,pressure,battery,status,age,interval,{},radiation_usvh,radiation_msv\n\
             {},{:.1},{},{:.1},{},{:?},{},{},{},{},{}\n",
            temp_header,
            opts.radon_csv_header(),
            reading.co2,
            opts.convert_temp(reading.temperature),
            reading.humidity,
            reading.pressure,
            reading.battery,
            reading.status,
            reading.age,
            reading.interval,
            radon_value,
            radiation_rate,
            radiation_total
        )
    }
}

/// Format reading as JSON with temperature unit conversion applied.
pub fn format_reading_json(reading: &CurrentReading, opts: &FormatOptions) -> Result<String> {
    #[derive(Serialize)]
    struct ReadingJson {
        co2: u16,
        temperature: f32,
        temperature_unit: &'static str,
        humidity: u8,
        pressure: f32,
        battery: u8,
        status: String,
        age: u16,
        interval: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_bq: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_pci: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_avg_24h_bq: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_avg_7d_bq: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_avg_30d_bq: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radiation_rate: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radiation_total: Option<f64>,
    }

    let json = ReadingJson {
        co2: reading.co2,
        temperature: opts.convert_temp(reading.temperature),
        temperature_unit: if opts.fahrenheit { "F" } else { "C" },
        humidity: reading.humidity,
        pressure: reading.pressure,
        battery: reading.battery,
        status: format!("{:?}", reading.status),
        age: reading.age,
        interval: reading.interval,
        radon_bq: reading.radon,
        radon_pci: reading.radon.map(bq_to_pci),
        radon_avg_24h_bq: reading.radon_avg_24h,
        radon_avg_7d_bq: reading.radon_avg_7d,
        radon_avg_30d_bq: reading.radon_avg_30d,
        radiation_rate: reading.radiation_rate,
        radiation_total: reading.radiation_total,
    };

    opts.as_json(&json)
}

// ============================================================================
// Info formatting
// ============================================================================

#[must_use]
pub fn format_info_text(info: &DeviceInfo, opts: &FormatOptions) -> String {
    if opts.no_color {
        format!(
            "Device Information\n\
             ────────────────────────────────────\n\
             Name:         {}\n\
             Model:        {}\n\
             Serial:       {}\n\
             Firmware:     {}\n\
             Hardware:     {}\n\
             Software:     {}\n\
             Manufacturer: {}\n",
            info.name,
            info.model,
            info.serial,
            info.firmware,
            info.hardware,
            info.software,
            info.manufacturer
        )
    } else {
        format!(
            "{}\n\
             ────────────────────────────────────\n\
             {:<14}{}\n\
             {:<14}{}\n\
             {:<14}{}\n\
             {:<14}{}\n\
             {:<14}{}\n\
             {:<14}{}\n\
             {:<14}{}\n",
            "Device Information".bold(),
            "Name:".dimmed(),
            info.name.cyan(),
            "Model:".dimmed(),
            info.model,
            "Serial:".dimmed(),
            info.serial,
            "Firmware:".dimmed(),
            info.firmware,
            "Hardware:".dimmed(),
            info.hardware,
            "Software:".dimmed(),
            info.software,
            "Manufacturer:".dimmed(),
            info.manufacturer
        )
    }
}

#[must_use]
pub fn format_info_csv(info: &DeviceInfo, opts: &FormatOptions) -> String {
    if opts.no_header {
        format!(
            "{},{},{},{},{},{},{}\n",
            csv_escape(&info.name),
            csv_escape(&info.model),
            csv_escape(&info.serial),
            csv_escape(&info.firmware),
            csv_escape(&info.hardware),
            csv_escape(&info.software),
            csv_escape(&info.manufacturer)
        )
    } else {
        format!(
            "name,model,serial,firmware,hardware,software,manufacturer\n\
             {},{},{},{},{},{},{}\n",
            csv_escape(&info.name),
            csv_escape(&info.model),
            csv_escape(&info.serial),
            csv_escape(&info.firmware),
            csv_escape(&info.hardware),
            csv_escape(&info.software),
            csv_escape(&info.manufacturer)
        )
    }
}

// ============================================================================
// History formatting
// ============================================================================

#[must_use]
pub fn format_history_text(history: &[HistoryRecord], opts: &FormatOptions) -> String {
    if history.is_empty() {
        return "No history records found.\n".to_string();
    }

    // Detect device type from first record
    let is_radon = history.first().is_some_and(|r| r.radon.is_some());

    let temp_header = if opts.fahrenheit {
        "Temp (°F)"
    } else {
        "Temp (°C)"
    };
    let mut output = format!("History ({} records):\n\n", history.len());

    if is_radon {
        output.push_str(&format!(
            "Timestamp                    Radon     {:>9}  Humidity  Pressure\n",
            temp_header
        ));
        output
            .push_str("───────────────────────────────────────────────────────────────────────\n");
    } else {
        output.push_str(&format!(
            "Timestamp                    CO2    {:>9}  Humidity  Pressure\n",
            temp_header
        ));
        output.push_str("─────────────────────────────────────────────────────────────────\n");
    }

    for record in history.iter().take(20) {
        let ts = record
            .timestamp
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "Unknown".to_string());

        if let Some(radon) = record.radon {
            output.push_str(&format!(
                "{}  {:>10}  {:>9}  {:>5}%    {:>6.1} hPa\n",
                ts,
                opts.format_radon(radon),
                opts.format_temp(record.temperature),
                record.humidity,
                record.pressure
            ));
        } else {
            output.push_str(&format!(
                "{}  {:>5} ppm  {:>9}  {:>5}%    {:>6.1} hPa\n",
                ts,
                record.co2,
                opts.format_temp(record.temperature),
                record.humidity,
                record.pressure
            ));
        }
    }

    if history.len() > 20 {
        output.push_str(&format!("... and {} more records\n", history.len() - 20));
        output.push_str("(Use --format csv or --format json for full data)\n");
    }

    output
}

#[must_use]
pub fn format_history_csv(history: &[HistoryRecord], opts: &FormatOptions) -> String {
    let temp_header = if opts.fahrenheit {
        "temperature_f"
    } else {
        "temperature_c"
    };
    let mut output = if opts.no_header {
        String::new()
    } else {
        format!(
            "timestamp,co2,{},humidity,pressure,{}\n",
            temp_header,
            opts.radon_csv_header()
        )
    };
    for record in history {
        let ts = record
            .timestamp
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| String::new());

        let radon_value = record
            .radon
            .map(|r| format!("{:.2}", opts.convert_radon(r)))
            .unwrap_or_default();
        output.push_str(&format!(
            "{},{},{:.1},{},{:.1},{}\n",
            ts,
            record.co2,
            opts.convert_temp(record.temperature),
            record.humidity,
            record.pressure,
            radon_value
        ));
    }
    output
}

/// Format history as JSON with temperature unit conversion applied.
pub fn format_history_json(history: &[HistoryRecord], opts: &FormatOptions) -> Result<String> {
    #[derive(Serialize)]
    struct HistoryRecordJson {
        timestamp: String,
        co2: u16,
        temperature: f32,
        temperature_unit: &'static str,
        humidity: u8,
        pressure: f32,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_bq: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_pci: Option<f32>,
    }

    let records: Vec<HistoryRecordJson> = history
        .iter()
        .map(|r| {
            let ts = r
                .timestamp
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| String::new());
            HistoryRecordJson {
                timestamp: ts,
                co2: r.co2,
                temperature: opts.convert_temp(r.temperature),
                temperature_unit: if opts.fahrenheit { "F" } else { "C" },
                humidity: r.humidity,
                pressure: r.pressure,
                radon_bq: r.radon,
                radon_pci: r.radon.map(bq_to_pci),
            }
        })
        .collect();

    opts.as_json(&records)
}

// ============================================================================
// Watch formatting
// ============================================================================

#[must_use]
pub fn format_watch_line(reading: &CurrentReading, opts: &FormatOptions) -> String {
    let now = time::OffsetDateTime::now_utc();
    let ts = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "???".to_string());

    let status = format_status(reading.status, opts.no_color);

    // Format the primary reading based on device type
    let primary = if let Some(radon) = reading.radon {
        // Radon device
        opts.format_radon(radon)
    } else if let Some(rate) = reading.radiation_rate {
        // Radiation device
        format!("{:.3} µSv/h", rate)
    } else {
        // CO2 device (Aranet4)
        format!("{} ppm", reading.co2)
    };

    // Build the output line with available sensor data
    let mut parts = vec![ts, status, primary];

    // Temperature (if available - radiation devices don't have it)
    if reading.temperature != 0.0 {
        parts.push(opts.format_temp(reading.temperature));
    }

    // Humidity (if available)
    if reading.humidity > 0 {
        parts.push(format!("{}%", reading.humidity));
    }

    // Pressure (if available)
    if reading.pressure > 0.0 {
        parts.push(format!("{:.1} hPa", reading.pressure));
    }

    // Battery is always shown
    parts.push(format!("{}%🔋", reading.battery));

    parts.join("  ") + "\n"
}

/// Get the CSV header for watch output.
#[must_use]
pub fn format_watch_csv_header(opts: &FormatOptions) -> String {
    let temp_header = if opts.fahrenheit {
        "temperature_f"
    } else {
        "temperature_c"
    };
    format!(
        "timestamp,co2,{},humidity,pressure,battery,status,{},radiation_usvh\n",
        temp_header,
        opts.radon_csv_header()
    )
}

/// Format a reading as a CSV line for watch output (no header).
#[must_use]
pub fn format_watch_csv_line(reading: &CurrentReading, opts: &FormatOptions) -> String {
    let now = time::OffsetDateTime::now_utc();
    let ts = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "???".to_string());

    let radon_value = reading
        .radon
        .map(|r| format!("{:.2}", opts.convert_radon(r)))
        .unwrap_or_default();
    let radiation_rate = reading
        .radiation_rate
        .map(|r| format!("{:.3}", r))
        .unwrap_or_default();

    format!(
        "{},{},{:.1},{},{:.1},{},{:?},{},{}\n",
        ts,
        reading.co2,
        opts.convert_temp(reading.temperature),
        reading.humidity,
        reading.pressure,
        reading.battery,
        reading.status,
        radon_value,
        radiation_rate
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use aranet_types::DeviceType;

    // ========================================================================
    // format_status tests
    // ========================================================================

    #[test]
    fn test_format_status_green_no_color() {
        let result = format_status(Status::Green, true);
        assert_eq!(result, "[GREEN]");
    }

    #[test]
    fn test_format_status_yellow_no_color() {
        let result = format_status(Status::Yellow, true);
        assert_eq!(result, "[AMBER]");
    }

    #[test]
    fn test_format_status_red_no_color() {
        let result = format_status(Status::Red, true);
        assert_eq!(result, "[RED]");
    }

    #[test]
    fn test_format_status_error_no_color() {
        let result = format_status(Status::Error, true);
        assert_eq!(result, "[ERROR]");
    }

    #[test]
    fn test_format_status_with_color_contains_label() {
        let result = format_status(Status::Green, false);
        assert!(result.contains("GREEN"));

        let result = format_status(Status::Yellow, false);
        assert!(result.contains("AMBER"));

        let result = format_status(Status::Red, false);
        assert!(result.contains("RED"));
    }

    // ========================================================================
    // format_age tests
    // ========================================================================

    #[test]
    fn test_format_age_seconds_only() {
        assert_eq!(format_age(0), "0s ago");
        assert_eq!(format_age(30), "30s ago");
        assert_eq!(format_age(59), "59s ago");
    }

    #[test]
    fn test_format_age_minutes_and_seconds() {
        assert_eq!(format_age(60), "1m 0s ago");
        assert_eq!(format_age(90), "1m 30s ago");
        assert_eq!(format_age(125), "2m 5s ago");
        assert_eq!(format_age(3600), "60m 0s ago");
    }

    // ========================================================================
    // format_scan_* tests
    // ========================================================================

    // Note: DiscoveredDevice tests only run on macOS because:
    // - Linux: bluez-async's DeviceId/AdapterId constructors are private
    // - Windows: btleplug's PeripheralId constructor is not publicly accessible
    // The formatting logic is still tested on macOS where PeripheralId can be created from UUID.

    /// Create a test PeripheralId for macOS (uses UUID)
    #[cfg(target_os = "macos")]
    fn make_test_peripheral_id() -> btleplug::platform::PeripheralId {
        btleplug::platform::PeripheralId::from(uuid::Uuid::nil())
    }

    #[cfg(target_os = "macos")]
    fn make_test_device(
        name: Option<&str>,
        address: &str,
        rssi: Option<i16>,
        device_type: Option<DeviceType>,
    ) -> DiscoveredDevice {
        DiscoveredDevice {
            name: name.map(|s| s.to_string()),
            id: make_test_peripheral_id(),
            address: address.to_string(),
            identifier: address.to_string(),
            rssi,
            device_type,
            is_aranet: true,
            manufacturer_data: None,
        }
    }

    #[test]
    fn test_format_scan_text_empty() {
        let devices: Vec<DiscoveredDevice> = vec![];
        let opts = FormatOptions::new(true, false);
        let result = format_scan_text(&devices, &opts);
        assert_eq!(result, "No Aranet devices found.\n");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_format_scan_text_single_device() {
        let devices = vec![make_test_device(
            Some("Aranet4 12345"),
            "AA:BB:CC:DD:EE:FF",
            Some(-50),
            Some(DeviceType::Aranet4),
        )];
        let opts = FormatOptions::new(true, false);
        let result = format_scan_text(&devices, &opts);
        assert!(result.contains("Found 1 device(s)"));
        assert!(result.contains("Aranet4 12345"));
        assert!(result.contains("-50 dBm"));
        assert!(result.contains("Aranet4"));
    }

    #[test]
    fn test_format_scan_csv_header() {
        let devices: Vec<DiscoveredDevice> = vec![];
        let opts = FormatOptions::new(true, false);
        let result = format_scan_csv(&devices, &opts);
        assert!(result.starts_with("name,address,identifier,rssi,device_type\n"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_format_scan_csv_no_header() {
        let devices = vec![make_test_device(
            Some("Aranet4 12345"),
            "AA:BB:CC:DD:EE:FF",
            Some(-50),
            Some(DeviceType::Aranet4),
        )];
        let opts = FormatOptions::new(true, false).with_no_header(true);
        let result = format_scan_csv(&devices, &opts);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("Aranet4 12345"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_format_scan_json_structure() {
        let devices = vec![make_test_device(
            Some("Aranet4 12345"),
            "AA:BB:CC:DD:EE:FF",
            Some(-50),
            Some(DeviceType::Aranet4),
        )];
        let opts = FormatOptions::default();
        let result = format_scan_json(&devices, &opts).unwrap();
        assert!(result.contains("\"count\": 1"));
        assert!(result.contains("\"devices\""));
        assert!(result.contains("\"name\": \"Aranet4 12345\""));
    }

    // ========================================================================
    // format_reading_* tests
    // ========================================================================

    fn make_aranet4_reading() -> CurrentReading {
        CurrentReading {
            co2: 800,
            temperature: 22.5,
            humidity: 45,
            pressure: 1013.2,
            battery: 85,
            status: Status::Green,
            interval: 300,
            age: 120,
            captured_at: None,
            radon: None,
            radiation_rate: None,
            radiation_total: None,
            radon_avg_24h: None,
            radon_avg_7d: None,
            radon_avg_30d: None,
        }
    }

    #[test]
    fn test_format_reading_text_aranet4() {
        let reading = make_aranet4_reading();
        let opts = FormatOptions::new(true, false);
        let result = format_reading_text(&reading, &opts);
        assert!(result.contains("CO₂:"));
        assert!(result.contains("800 ppm"));
        assert!(result.contains("[GREEN]"));
        assert!(result.contains("Temperature:"));
        assert!(result.contains("22.5°C"));
    }

    #[test]
    fn test_format_reading_csv_header() {
        let reading = make_aranet4_reading();
        let opts = FormatOptions::new(true, false);
        let result = format_reading_csv(&reading, &opts);
        assert!(result.starts_with("co2,temperature_c,humidity,pressure,battery,status,age,interval,radon_pci,radiation_usvh,radiation_msv\n"));
    }

    #[test]
    fn test_format_reading_json() {
        let reading = make_aranet4_reading();
        let opts = FormatOptions::new(true, false);
        let result = format_reading_json(&reading, &opts).unwrap();
        assert!(result.contains("\"co2\": 800"));
        assert!(result.contains("\"temperature\": 22.5"));
        assert!(result.contains("\"temperature_unit\": \"C\""));
    }

    // ========================================================================
    // format_info_* tests
    // ========================================================================

    fn make_test_device_info() -> DeviceInfo {
        DeviceInfo {
            name: "Aranet4 12345".to_string(),
            model: "Aranet4".to_string(),
            serial: "SN12345678".to_string(),
            firmware: "v1.2.3".to_string(),
            hardware: "v2.0".to_string(),
            software: "v1.0".to_string(),
            manufacturer: "SAF Tehnika".to_string(),
        }
    }

    #[test]
    fn test_format_info_text_contains_all_fields() {
        let info = make_test_device_info();
        let opts = FormatOptions::new(true, false);
        let result = format_info_text(&info, &opts);
        assert!(result.contains("Device Information"));
        assert!(result.contains("Aranet4 12345"));
        assert!(result.contains("SN12345678"));
        assert!(result.contains("SAF Tehnika"));
    }

    #[test]
    fn test_format_info_csv_header() {
        let info = make_test_device_info();
        let opts = FormatOptions::new(true, false);
        let result = format_info_csv(&info, &opts);
        assert!(result.starts_with("name,model,serial,firmware,hardware,software,manufacturer\n"));
    }

    // ========================================================================
    // FormatOptions tests
    // ========================================================================

    #[test]
    fn test_format_temp_celsius() {
        let opts = FormatOptions::new(true, false);
        assert_eq!(opts.format_temp(22.5), "22.5°C");
    }

    #[test]
    fn test_format_temp_fahrenheit() {
        let opts = FormatOptions::new(true, true);
        assert_eq!(opts.format_temp(0.0), "32.0°F");
        assert_eq!(opts.format_temp(100.0), "212.0°F");
    }

    #[test]
    fn test_convert_temp_celsius() {
        let opts = FormatOptions::new(true, false);
        assert_eq!(opts.convert_temp(22.5), 22.5);
    }

    #[test]
    fn test_convert_temp_fahrenheit() {
        let opts = FormatOptions::new(true, true);
        assert!((opts.convert_temp(0.0) - 32.0).abs() < 0.01);
        assert!((opts.convert_temp(100.0) - 212.0).abs() < 0.01);
    }
}
