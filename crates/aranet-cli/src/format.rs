//! Output formatting utilities for text, JSON, and CSV output.

use anyhow::Result;
use aranet_core::DiscoveredDevice;
use aranet_types::{CurrentReading, DeviceInfo, HistoryRecord, Status};
use owo_colors::OwoColorize;
use serde::Serialize;

use crate::cli::StyleMode;
use crate::style;

/// Formatting options for output.
#[derive(Debug, Clone, Copy)]
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
    /// Use inHg for pressure instead of hPa.
    pub inhg: bool,
    /// Visual styling mode.
    pub style: StyleMode,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            no_color: false,
            fahrenheit: false,
            no_header: false,
            compact: false,
            bq: false,
            inhg: false,
            style: StyleMode::Rich,
        }
    }
}

impl FormatOptions {
    pub fn new(no_color: bool, fahrenheit: bool, style: StyleMode) -> Self {
        // Plain mode automatically disables colors for pipe-friendliness
        let effective_no_color = no_color || style == StyleMode::Plain;
        Self {
            no_color: effective_no_color,
            fahrenheit,
            no_header: false,
            compact: false,
            bq: false,
            inhg: false,
            style,
        }
    }

    /// Check if rich styling is enabled.
    pub fn is_rich(&self) -> bool {
        self.style == StyleMode::Rich
    }

    /// Check if plain styling is enabled (no decorations).
    #[allow(dead_code)]
    pub fn is_plain(&self) -> bool {
        self.style == StyleMode::Plain
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

    /// Create with inHg pressure unit option.
    pub fn with_inhg(mut self, inhg: bool) -> Self {
        self.inhg = inhg;
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
    /// Uses ASCII-only output for Plain mode (pipe-friendly).
    #[must_use]
    pub fn format_temp(&self, celsius: f32) -> String {
        let value = if self.fahrenheit {
            celsius * 9.0 / 5.0 + 32.0
        } else {
            celsius
        };
        let unit = if self.fahrenheit { "F" } else { "C" };

        // Plain mode: ASCII-only (no degree symbol)
        if self.is_plain() {
            format!("{:.1}{}", value, unit)
        } else {
            format!("{:.1}°{}", value, unit)
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
    /// Uses ASCII-only output for Plain mode (pipe-friendly).
    #[must_use]
    pub fn format_radon(&self, bq: u32) -> String {
        if self.bq {
            // Plain mode: ASCII-only (no superscript)
            if self.is_plain() {
                format!("{} Bq/m3", bq)
            } else {
                format!("{} Bq/m³", bq)
            }
        } else {
            format!("{:.2} pCi/L", bq_to_pci(bq))
        }
    }

    /// Get radon CSV header name.
    #[must_use]
    pub fn radon_csv_header(&self) -> &'static str {
        if self.bq { "radon_bq" } else { "radon_pci" }
    }

    /// Get radon display unit string.
    #[must_use]
    pub fn radon_unit(&self) -> &'static str {
        if self.bq { "Bq/m3" } else { "pCi/L" }
    }

    /// Convert radon value for CSV/JSON output.
    #[must_use]
    pub fn convert_radon(&self, bq: u32) -> f32 {
        if self.bq { bq as f32 } else { bq_to_pci(bq) }
    }

    /// Format pressure with appropriate unit.
    #[must_use]
    pub fn format_pressure(&self, hpa: f32) -> String {
        if self.inhg {
            format!("{:.2} inHg", hpa_to_inhg(hpa))
        } else {
            format!("{:.1} hPa", hpa)
        }
    }

    /// Get pressure CSV header name.
    #[must_use]
    pub fn pressure_csv_header(&self) -> &'static str {
        if self.inhg {
            "pressure_inhg"
        } else {
            "pressure_hpa"
        }
    }

    /// Convert pressure value for CSV/JSON output.
    #[must_use]
    pub fn convert_pressure(&self, hpa: f32) -> f32 {
        if self.inhg { hpa_to_inhg(hpa) } else { hpa }
    }
}

/// Convert Bq/m³ to pCi/L (1 Bq/m³ = 0.027 pCi/L)
#[must_use]
pub fn bq_to_pci(bq: u32) -> f32 {
    bq as f32 * 0.027
}

/// Convert hPa to inHg (1 hPa = 0.02953 inHg)
#[must_use]
pub fn hpa_to_inhg(hpa: f32) -> f32 {
    hpa * 0.02953
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

/// Format scan results with optional alias lookup.
/// If `aliases` is provided, shows alias column for known devices.
#[must_use]
pub fn format_scan_text_with_aliases(
    devices: &[DiscoveredDevice],
    opts: &FormatOptions,
    aliases: Option<&std::collections::HashMap<String, String>>,
    show_tips: bool,
) -> String {
    use tabled::{Table, Tabled};

    if devices.is_empty() {
        return "No Aranet devices found.\n".to_string();
    }

    // Use plain signal format for Plain mode
    let use_plain_signal = opts.is_plain();

    // Check if any device has an alias
    let has_aliases = aliases.map_or(false, |a| {
        devices.iter().any(|d| {
            let id_lower = d.identifier.to_lowercase();
            a.values().any(|v| v.to_lowercase() == id_lower)
        })
    });

    // Rich mode header
    let header = if opts.is_rich() {
        let count_display = if opts.no_color {
            format!("{}", devices.len())
        } else {
            format!("{}", devices.len().to_string().green().bold())
        };
        format!("Found {} Aranet device(s)\n\n", count_display)
    } else {
        format!("Found {} Aranet device(s)\n\n", devices.len())
    };

    // Use different row structs based on whether we have aliases
    if has_aliases {
        #[derive(Tabled)]
        struct DeviceRowWithAlias {
            #[tabled(rename = "Name")]
            name: String,
            #[tabled(rename = "Alias")]
            alias: String,
            #[tabled(rename = "Type")]
            device_type: String,
            #[tabled(rename = "Signal")]
            signal: String,
            #[tabled(rename = "Identifier")]
            identifier: String,
        }

        let rows: Vec<DeviceRowWithAlias> = devices
            .iter()
            .map(|d| {
                let name = d.name.as_deref().unwrap_or("Unknown");
                let id_lower = d.identifier.to_lowercase();
                let alias = aliases
                    .and_then(|a| {
                        a.iter()
                            .find(|(_, v)| v.to_lowercase() == id_lower)
                            .map(|(k, _)| k.clone())
                    })
                    .unwrap_or_else(|| "-".to_string());
                DeviceRowWithAlias {
                    name: if opts.no_color {
                        name.to_string()
                    } else {
                        format!("{}", name.cyan())
                    },
                    alias,
                    device_type: d
                        .device_type
                        .map(|t| format!("{:?}", t))
                        .unwrap_or_else(|| "Unknown".to_string()),
                    signal: if use_plain_signal {
                        d.rssi.map(|r| r.to_string()).unwrap_or_else(|| "N/A".to_string())
                    } else {
                        style::format_signal_bar(d.rssi, opts.no_color)
                    },
                    identifier: d.identifier.clone(),
                }
            })
            .collect();

        let mut table = Table::new(rows);
        style::apply_table_style(&mut table, opts.style);

        let mut output = format!("{}{}\n", header, table);
        if show_tips && !opts.is_plain() {
            output.push_str(&format_scan_tips(opts.no_color));
        }
        output
    } else {
        #[derive(Tabled)]
        struct DeviceRow {
            #[tabled(rename = "Name")]
            name: String,
            #[tabled(rename = "Type")]
            device_type: String,
            #[tabled(rename = "Signal")]
            signal: String,
            #[tabled(rename = "Identifier")]
            identifier: String,
        }

        let rows: Vec<DeviceRow> = devices
            .iter()
            .map(|d| {
                let name = d.name.as_deref().unwrap_or("Unknown");
                DeviceRow {
                    name: if opts.no_color {
                        name.to_string()
                    } else {
                        format!("{}", name.cyan())
                    },
                    device_type: d
                        .device_type
                        .map(|t| format!("{:?}", t))
                        .unwrap_or_else(|| "Unknown".to_string()),
                    signal: if use_plain_signal {
                        d.rssi.map(|r| r.to_string()).unwrap_or_else(|| "N/A".to_string())
                    } else {
                        style::format_signal_bar(d.rssi, opts.no_color)
                    },
                    identifier: d.identifier.clone(),
                }
            })
            .collect();

        let mut table = Table::new(rows);
        style::apply_table_style(&mut table, opts.style);

        let mut output = format!("{}{}\n", header, table);
        if show_tips && !opts.is_plain() {
            output.push_str(&format_scan_tips(opts.no_color));
        }
        output
    }
}

/// Format helpful tips shown after scan results.
#[must_use]
pub fn format_scan_tips(no_color: bool) -> String {
    let tip_label = if no_color {
        "Tip:".to_string()
    } else {
        format!("{}", "Tip:".yellow().bold())
    };
    format!(
        "\n{} Use 'aranet alias set <name> <identifier>' to save a device alias\n     Use 'aranet config set device <identifier>' to set as default\n",
        tip_label
    )
}

/// Format scan results as text (simple version without aliases).
/// Used primarily in tests and as a simpler API.
#[must_use]
#[allow(dead_code)]
pub fn format_scan_text(devices: &[DiscoveredDevice], opts: &FormatOptions) -> String {
    format_scan_text_with_aliases(devices, opts, None, false)
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
    format_reading_text_with_name(reading, opts, None)
}

/// Format reading with optional device name header.
#[must_use]
pub fn format_reading_text_with_name(
    reading: &CurrentReading,
    opts: &FormatOptions,
    device_name: Option<&str>,
) -> String {
    // Use Rich mode panel formatting when appropriate
    if opts.is_rich() {
        return format_reading_rich(reading, opts, device_name);
    }

    // Minimal/Plain mode - simple text output
    let mut output = String::new();

    // CO2 (Aranet4) - with colored value
    if reading.co2 > 0 {
        let co2_display = style::format_co2_colored(reading.co2, opts.no_color);
        output.push_str(&format!(
            "CO2:         {:>5} ppm   {}\n",
            co2_display,
            format_status(reading.status, opts.no_color)
        ));
    }

    // Radon (AranetRn+) - with colored value
    if let Some(radon) = reading.radon {
        let radon_display = if opts.bq {
            style::format_radon_colored(radon, opts.no_color)
        } else {
            // Convert to pCi/L for display
            let pci = radon as f32 / 37.0;
            if opts.no_color {
                format!("{:.2}", pci)
            } else if radon < style::radon::GOOD {
                format!("{}", format!("{:.2}", pci).green())
            } else if radon < style::radon::MODERATE {
                format!("{}", format!("{:.2}", pci).yellow())
            } else {
                format!("{}", format!("{:.2}", pci).red())
            }
        };
        let unit = if opts.bq { "Bq/m3" } else { "pCi/L" };
        output.push_str(&format!(
            "Radon:       {:>6} {}  {}\n",
            radon_display,
            unit,
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
        output.push_str(&format!("Radiation:   {:>5.3} uSv/h\n", rate));
    }
    if let Some(total) = reading.radiation_total {
        output.push_str(&format!("Total Dose:  {:>5.3} mSv\n", total));
    }

    // Common fields - with colored values
    if reading.temperature != 0.0 {
        let unit = if opts.fahrenheit { "F" } else { "C" };
        let temp_value = opts.convert_temp(reading.temperature);
        let temp_display = if opts.no_color {
            format!("{:.1}", temp_value)
        } else {
            style::format_temp_colored(temp_value, opts.no_color)
        };
        output.push_str(&format!("Temperature: {:>6} {}\n", temp_display, unit));
    }
    if reading.humidity > 0 {
        let humidity_display = style::format_humidity_colored(reading.humidity, opts.no_color);
        output.push_str(&format!("Humidity:    {:>6}\n", humidity_display));
    }
    if reading.pressure != 0.0 {
        output.push_str(&format!(
            "Pressure:    {:>10}\n",
            opts.format_pressure(reading.pressure)
        ));
    }

    // Battery with colored value
    let battery_display = style::format_battery_colored(reading.battery, opts.no_color);
    output.push_str(&format!("Battery:     {:>6}\n", battery_display));
    output.push_str(&format!("Last Update: {}\n", format_age(reading.age)));
    output.push_str(&format!("Interval:    {} minutes\n", reading.interval / 60));

    output
}

/// Format reading with Rich mode styling.
/// Uses a clean format with header and organized sections.
#[must_use]
fn format_reading_rich(
    reading: &CurrentReading,
    opts: &FormatOptions,
    device_name: Option<&str>,
) -> String {
    use owo_colors::OwoColorize;

    let mut output = String::new();

    // Header with device name
    let title = device_name.unwrap_or("Sensor Reading");
    if opts.no_color {
        output.push_str(&format!("  {}\n", title));
        output.push_str(&format!("  {}\n\n", "─".repeat(title.len())));
    } else {
        output.push_str(&format!("  {}\n", title.cyan().bold()));
        output.push_str(&format!("  {}\n\n", "─".repeat(title.len()).dimmed()));
    }

    // Air quality summary for CO2 devices (prominent display)
    if reading.co2 > 0 {
        let quality = style::air_quality_summary_colored(reading.co2, opts.no_color);
        let bar = style::format_air_quality_bar(reading.co2, opts.no_color);
        output.push_str(&format!("  Air Quality: {} {}\n\n", quality, bar));
    }

    // Helper for formatted key-value lines
    let kv = |key: &str, value: &str| -> String {
        if opts.no_color {
            format!("  {:>11}:  {}\n", key, value)
        } else {
            format!("  {:>11}:  {}\n", key.dimmed(), value)
        }
    };

    // CO2 (Aranet4)
    if reading.co2 > 0 {
        let co2_display = style::format_co2_colored(reading.co2, opts.no_color);
        let status = format_status(reading.status, opts.no_color);
        output.push_str(&kv("CO2", &format!("{} ppm {}", co2_display, status)));
    }

    // Radon (AranetRn+)
    if let Some(radon) = reading.radon {
        let radon_display = if opts.bq {
            style::format_radon_colored(radon, opts.no_color)
        } else {
            let pci = radon as f32 / 37.0;
            if opts.no_color {
                format!("{:.2}", pci)
            } else if radon < style::radon::GOOD {
                format!("{}", format!("{:.2}", pci).green())
            } else if radon < style::radon::MODERATE {
                format!("{}", format!("{:.2}", pci).yellow())
            } else {
                format!("{}", format!("{:.2}", pci).red())
            }
        };
        let unit = if opts.bq { "Bq/m3" } else { "pCi/L" };
        let status = format_status(reading.status, opts.no_color);
        output.push_str(&kv("Radon", &format!("{} {} {}", radon_display, unit, status)));

        // Radon averages
        if let Some(avg_24h) = reading.radon_avg_24h {
            output.push_str(&kv("24h Avg", &opts.format_radon(avg_24h)));
        }
        if let Some(avg_7d) = reading.radon_avg_7d {
            output.push_str(&kv("7d Avg", &opts.format_radon(avg_7d)));
        }
        if let Some(avg_30d) = reading.radon_avg_30d {
            output.push_str(&kv("30d Avg", &opts.format_radon(avg_30d)));
        }
    }

    // Radiation (Aranet Radiation)
    if let Some(rate) = reading.radiation_rate {
        output.push_str(&kv("Radiation", &format!("{:.3} uSv/h", rate)));
    }
    if let Some(total) = reading.radiation_total {
        output.push_str(&kv("Total Dose", &format!("{:.3} mSv", total)));
    }

    // Common fields
    if reading.temperature != 0.0 {
        let unit = if opts.fahrenheit { "F" } else { "C" };
        let temp_value = opts.convert_temp(reading.temperature);
        let temp_display = style::format_temp_colored(temp_value, opts.no_color);
        output.push_str(&kv("Temperature", &format!("{} {}", temp_display, unit)));
    }

    if reading.humidity > 0 {
        let humidity_display = style::format_humidity_colored(reading.humidity, opts.no_color);
        output.push_str(&kv("Humidity", &humidity_display));
    }

    if reading.pressure != 0.0 {
        output.push_str(&kv("Pressure", &opts.format_pressure(reading.pressure)));
    }

    output.push('\n');

    // Battery and metadata section
    let battery_display = style::format_battery_colored(reading.battery, opts.no_color);
    output.push_str(&kv("Battery", &battery_display));
    output.push_str(&kv("Updated", &format_age(reading.age)));
    output.push_str(&kv("Interval", &format!("{} min", reading.interval / 60)));

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
            "{},{:.1},{},{:.2},{},{:?},{},{},{},{},{}\n",
            reading.co2,
            opts.convert_temp(reading.temperature),
            reading.humidity,
            opts.convert_pressure(reading.pressure),
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
            "co2,{},humidity,{},battery,status,age,interval,{},radiation_usvh,radiation_msv\n\
             {},{:.1},{},{:.2},{},{:?},{},{},{},{},{}\n",
            temp_header,
            opts.pressure_csv_header(),
            opts.radon_csv_header(),
            reading.co2,
            opts.convert_temp(reading.temperature),
            reading.humidity,
            opts.convert_pressure(reading.pressure),
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

/// Format reading as JSON with temperature and pressure unit conversion applied.
pub fn format_reading_json(reading: &CurrentReading, opts: &FormatOptions) -> Result<String> {
    #[derive(Serialize)]
    struct ReadingJson {
        co2: u16,
        temperature: f32,
        temperature_unit: &'static str,
        humidity: u8,
        pressure: f32,
        pressure_unit: &'static str,
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
        pressure: opts.convert_pressure(reading.pressure),
        pressure_unit: if opts.inhg { "inHg" } else { "hPa" },
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
// Multi-device reading formatting
// ============================================================================

use crate::commands::DeviceReading;

/// Format multiple device readings as text
#[must_use]
pub fn format_multi_reading_text(readings: &[DeviceReading], opts: &FormatOptions) -> String {
    let mut output = String::new();

    for (i, dr) in readings.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }

        // Device header - use ASCII dashes for Plain mode
        let dash = if opts.is_plain() { "--" } else { "──" };
        if opts.no_color {
            output.push_str(&format!("{} {} {}\n", dash, dr.identifier, dash));
        } else {
            output.push_str(&format!("{} {} {}\n", dash, dr.identifier.cyan(), dash));
        }

        output.push_str(&format_reading_text(&dr.reading, opts));
    }

    output
}

/// Format multiple device readings as JSON
pub fn format_multi_reading_json(
    readings: &[DeviceReading],
    opts: &FormatOptions,
) -> Result<String> {
    #[derive(Serialize)]
    struct MultiReadingJson {
        count: usize,
        readings: Vec<DeviceReadingJson>,
    }

    #[derive(Serialize)]
    struct DeviceReadingJson {
        device: String,
        co2: u16,
        temperature: f32,
        temperature_unit: &'static str,
        humidity: u8,
        pressure: f32,
        pressure_unit: &'static str,
        battery: u8,
        status: String,
        age: u16,
        interval: u16,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_bq: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_pci: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radiation_rate: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radiation_total: Option<f64>,
    }

    let json = MultiReadingJson {
        count: readings.len(),
        readings: readings
            .iter()
            .map(|dr| DeviceReadingJson {
                device: dr.identifier.clone(),
                co2: dr.reading.co2,
                temperature: opts.convert_temp(dr.reading.temperature),
                temperature_unit: if opts.fahrenheit { "F" } else { "C" },
                humidity: dr.reading.humidity,
                pressure: opts.convert_pressure(dr.reading.pressure),
                pressure_unit: if opts.inhg { "inHg" } else { "hPa" },
                battery: dr.reading.battery,
                status: format!("{:?}", dr.reading.status),
                age: dr.reading.age,
                interval: dr.reading.interval,
                radon_bq: dr.reading.radon,
                radon_pci: dr.reading.radon.map(bq_to_pci),
                radiation_rate: dr.reading.radiation_rate,
                radiation_total: dr.reading.radiation_total,
            })
            .collect(),
    };

    opts.as_json(&json)
}

/// Format multiple device readings as CSV
#[must_use]
pub fn format_multi_reading_csv(readings: &[DeviceReading], opts: &FormatOptions) -> String {
    let temp_header = if opts.fahrenheit {
        "temperature_f"
    } else {
        "temperature_c"
    };

    let mut output = if opts.no_header {
        String::new()
    } else {
        format!(
            "device,co2,{},humidity,{},battery,status,age,interval,{},radiation_usvh,radiation_msv\n",
            temp_header,
            opts.pressure_csv_header(),
            opts.radon_csv_header()
        )
    };

    for dr in readings {
        let radon_value = dr
            .reading
            .radon
            .map(|r| format!("{:.2}", opts.convert_radon(r)))
            .unwrap_or_default();
        let radiation_rate = dr
            .reading
            .radiation_rate
            .map(|r| format!("{:.3}", r))
            .unwrap_or_default();
        let radiation_total = dr
            .reading
            .radiation_total
            .map(|r| format!("{:.3}", r))
            .unwrap_or_default();

        output.push_str(&format!(
            "{},{},{:.1},{},{:.2},{},{:?},{},{},{},{},{}\n",
            csv_escape(&dr.identifier),
            dr.reading.co2,
            opts.convert_temp(dr.reading.temperature),
            dr.reading.humidity,
            opts.convert_pressure(dr.reading.pressure),
            dr.reading.battery,
            dr.reading.status,
            dr.reading.age,
            dr.reading.interval,
            radon_value,
            radiation_rate,
            radiation_total
        ));
    }

    output
}

// ============================================================================
// Info formatting
// ============================================================================

#[must_use]
pub fn format_info_text(info: &DeviceInfo, opts: &FormatOptions) -> String {
    use tabled::builder::Builder;

    let mut builder = Builder::default();
    builder.push_record(["Property", "Value"]);
    builder.push_record(["Name", &info.name]);
    builder.push_record(["Model", &info.model]);
    builder.push_record(["Serial", &info.serial]);
    builder.push_record(["Firmware", &info.firmware]);
    builder.push_record(["Hardware", &info.hardware]);
    builder.push_record(["Software", &info.software]);
    builder.push_record(["Manufacturer", &info.manufacturer]);

    let mut table = builder.build();
    style::apply_table_style(&mut table, opts.style);

    let title = if opts.no_color {
        "Device Information".to_string()
    } else {
        format!("{}", "Device Information".bold())
    };

    format!("{}\n{}\n", title, table)
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
    use tabled::builder::Builder;

    if history.is_empty() {
        return "No history records found.\n".to_string();
    }

    // Detect device type from first record
    let is_radon = history.first().is_some_and(|r| r.radon.is_some());

    let temp_header = if opts.fahrenheit {
        "Temp (F)"
    } else {
        "Temp (C)"
    };

    // Determine how many records to show based on terminal width
    // Narrow terminals get fewer records to avoid wrapping issues
    let term_width = style::terminal_width();
    let max_records = if term_width < 80 { 10 } else { 20 };

    let mut output = format!("History ({} records):\n\n", history.len());

    // Build table with dynamic headers
    let mut builder = Builder::default();

    // Use compact timestamp format for narrow terminals
    let use_compact_ts = term_width < 100;

    // Add header row
    if is_radon {
        builder.push_record(["Timestamp", "Radon", temp_header, "Humidity", "Pressure"]);
    } else {
        builder.push_record(["Timestamp", "CO2", temp_header, "Humidity", "Pressure"]);
    }

    // Add data rows
    for record in history.iter().take(max_records) {
        let ts = if use_compact_ts {
            // Compact format: YYYY-MM-DD HH:MM
            record
                .timestamp
                .format(
                    &time::format_description::parse("[year]-[month]-[day] [hour]:[minute]")
                        .expect("valid format"),
                )
                .unwrap_or_else(|_| "Unknown".to_string())
        } else {
            record
                .timestamp
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "Unknown".to_string())
        };

        let value = if let Some(radon) = record.radon {
            opts.format_radon(radon)
        } else {
            format!("{} ppm", record.co2)
        };

        builder.push_record([
            ts,
            value,
            opts.format_temp(record.temperature),
            format!("{}%", record.humidity),
            opts.format_pressure(record.pressure),
        ]);
    }

    let mut table = builder.build();
    style::apply_table_style(&mut table, opts.style);
    output.push_str(&table.to_string());
    output.push('\n');

    if history.len() > max_records {
        output.push_str(&format!(
            "... and {} more records\n",
            history.len() - max_records
        ));
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
            "timestamp,co2,{},humidity,{},{}\n",
            temp_header,
            opts.pressure_csv_header(),
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
            "{},{},{:.1},{},{:.2},{}\n",
            ts,
            record.co2,
            opts.convert_temp(record.temperature),
            record.humidity,
            opts.convert_pressure(record.pressure),
            radon_value
        ));
    }
    output
}

/// Format history as JSON with temperature and pressure unit conversion applied.
pub fn format_history_json(history: &[HistoryRecord], opts: &FormatOptions) -> Result<String> {
    #[derive(Serialize)]
    struct HistoryRecordJson {
        timestamp: String,
        co2: u16,
        temperature: f32,
        temperature_unit: &'static str,
        humidity: u8,
        pressure: f32,
        pressure_unit: &'static str,
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
                pressure: opts.convert_pressure(r.pressure),
                pressure_unit: if opts.inhg { "inHg" } else { "hPa" },
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

/// Format a watch line for single-device passive mode (without device name).
/// Used when watching a single device or for backward compatibility.
#[must_use]
#[allow(dead_code)] // Available for future use, passive mode now uses format_watch_line_with_device
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
        parts.push(opts.format_pressure(reading.pressure));
    }

    // Battery is always shown (using text "BAT" instead of emoji per project guidelines)
    parts.push(format!("BAT {}%", reading.battery));

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
        "timestamp,co2,{},humidity,{},battery,status,{},radiation_usvh\n",
        temp_header,
        opts.pressure_csv_header(),
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
        "{},{},{:.1},{},{:.2},{},{:?},{},{}\n",
        ts,
        reading.co2,
        opts.convert_temp(reading.temperature),
        reading.humidity,
        opts.convert_pressure(reading.pressure),
        reading.battery,
        reading.status,
        radon_value,
        radiation_rate
    )
}

// ============================================================================
// Watch formatting with device name (for multi-device passive mode)
// ============================================================================

/// Format a watch line with device name for multi-device output.
#[must_use]
pub fn format_watch_line_with_device(
    reading: &CurrentReading,
    device_name: &str,
    opts: &FormatOptions,
) -> String {
    use chrono::Local;

    let timestamp = Local::now().format("%H:%M:%S").to_string();
    let status = format_status(reading.status, opts.no_color);

    // Format device name with color
    let device_display = if opts.no_color {
        format!("[{}]", device_name)
    } else {
        format!("[{}]", device_name.cyan())
    };

    // Format the primary reading based on device type
    let primary = if let Some(radon) = reading.radon {
        let radon_display = style::format_radon_colored(radon, opts.no_color);
        format!("{} {}", radon_display, opts.radon_unit())
    } else if let Some(rate) = reading.radiation_rate {
        format!("{:.3} uSv/h", rate)
    } else if reading.co2 > 0 {
        let co2_display = style::format_co2_colored(reading.co2, opts.no_color);
        format!("{} ppm", co2_display)
    } else {
        String::new()
    };

    // Build the output line with available sensor data
    let mut parts = vec![format!("[{}]", timestamp), device_display, status];

    if !primary.is_empty() {
        parts.push(primary);
    }

    // Temperature (if available - radiation devices don't have it)
    if reading.temperature != 0.0 {
        let temp_display = style::format_temp_colored(
            opts.convert_temp(reading.temperature),
            opts.no_color,
        );
        let unit = if opts.fahrenheit { "F" } else { "C" };
        parts.push(format!("{}{}", temp_display, unit));
    }

    // Humidity (if available)
    if reading.humidity > 0 {
        let humidity_display = style::format_humidity_colored(reading.humidity, opts.no_color);
        parts.push(humidity_display);
    }

    // Pressure (if available)
    if reading.pressure > 0.0 {
        parts.push(opts.format_pressure(reading.pressure));
    }

    // Battery
    let battery_display = style::format_battery_colored(reading.battery, opts.no_color);
    parts.push(format!("BAT {}", battery_display));

    parts.join("  ") + "\n"
}

/// Get the CSV header for watch output with device column.
#[must_use]
pub fn format_watch_csv_header_with_device(opts: &FormatOptions) -> String {
    let temp_header = if opts.fahrenheit {
        "temperature_f"
    } else {
        "temperature_c"
    };
    format!(
        "timestamp,device,co2,{},humidity,{},battery,status,{},radiation_usvh\n",
        temp_header,
        opts.pressure_csv_header(),
        opts.radon_csv_header()
    )
}

/// Format a reading as a CSV line for watch output with device column.
#[must_use]
pub fn format_watch_csv_line_with_device(
    reading: &CurrentReading,
    device_name: &str,
    opts: &FormatOptions,
) -> String {
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
        "{},{},{},{:.1},{},{:.2},{},{:?},{},{}\n",
        ts,
        device_name,
        reading.co2,
        opts.convert_temp(reading.temperature),
        reading.humidity,
        opts.convert_pressure(reading.pressure),
        reading.battery,
        reading.status,
        radon_value,
        radiation_rate
    )
}

/// Format a reading as JSON with device name.
pub fn format_reading_json_with_device(
    reading: &CurrentReading,
    device_name: &str,
    opts: &FormatOptions,
) -> Result<String> {
    #[derive(Serialize)]
    struct WatchReadingJson {
        timestamp: String,
        device: String,
        co2: u16,
        temperature: f32,
        temperature_unit: &'static str,
        humidity: u8,
        pressure: f32,
        pressure_unit: &'static str,
        battery: u8,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_unit: Option<&'static str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radiation_rate: Option<f32>,
    }

    let now = time::OffsetDateTime::now_utc();
    let ts = now
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "???".to_string());

    let json = WatchReadingJson {
        timestamp: ts,
        device: device_name.to_string(),
        co2: reading.co2,
        temperature: opts.convert_temp(reading.temperature),
        temperature_unit: if opts.fahrenheit { "F" } else { "C" },
        humidity: reading.humidity,
        pressure: opts.convert_pressure(reading.pressure),
        pressure_unit: if opts.inhg { "inHg" } else { "hPa" },
        battery: reading.battery,
        status: format!("{:?}", reading.status),
        radon: reading.radon.map(|r| opts.convert_radon(r)),
        radon_unit: reading.radon.map(|_| opts.radon_unit()),
        radiation_rate: reading.radiation_rate,
    };

    let output = if opts.compact {
        serde_json::to_string(&json)?
    } else {
        serde_json::to_string_pretty(&json)?
    };

    Ok(output + "\n")
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

    /// Create test FormatOptions with no_color enabled (for consistent test output)
    fn test_opts() -> FormatOptions {
        FormatOptions {
            no_color: true,
            ..FormatOptions::default()
        }
    }

    #[test]
    fn test_format_scan_text_empty() {
        let devices: Vec<DiscoveredDevice> = vec![];
        let opts = test_opts();
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
        let opts = test_opts();
        let result = format_scan_text(&devices, &opts);
        assert!(result.contains("Found 1 Aranet device(s)"));
        assert!(result.contains("Aranet4 12345"));
        // Signal bar now shows visual bar with RSSI value
        assert!(result.contains("-50"));
        assert!(result.contains("Aranet4"));
    }

    #[test]
    fn test_format_scan_csv_header() {
        let devices: Vec<DiscoveredDevice> = vec![];
        let opts = test_opts();
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
        let opts = FormatOptions {
            no_header: true,
            ..test_opts()
        };
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
        let opts = test_opts();
        let result = format_reading_text(&reading, &opts);
        assert!(result.contains("CO2:"));
        assert!(result.contains("800"));
        assert!(result.contains("ppm"));
        assert!(result.contains("[GREEN]"));
        assert!(result.contains("Temperature:"));
        assert!(result.contains("22.5"));
    }

    #[test]
    fn test_format_reading_csv_header() {
        let reading = make_aranet4_reading();
        let opts = test_opts();
        let result = format_reading_csv(&reading, &opts);
        assert!(result.starts_with("co2,temperature_c,humidity,pressure_hpa,battery,status,age,interval,radon_pci,radiation_usvh,radiation_msv\n"));
    }

    #[test]
    fn test_format_reading_json() {
        let reading = make_aranet4_reading();
        let opts = test_opts();
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
        let opts = test_opts();
        let result = format_info_text(&info, &opts);
        assert!(result.contains("Device Information"));
        assert!(result.contains("Aranet4 12345"));
        assert!(result.contains("SN12345678"));
        assert!(result.contains("SAF Tehnika"));
    }

    #[test]
    fn test_format_info_csv_header() {
        let info = make_test_device_info();
        let opts = test_opts();
        let result = format_info_csv(&info, &opts);
        assert!(result.starts_with("name,model,serial,firmware,hardware,software,manufacturer\n"));
    }

    // ========================================================================
    // FormatOptions tests
    // ========================================================================

    #[test]
    fn test_format_temp_celsius() {
        let opts = test_opts();
        assert_eq!(opts.format_temp(22.5), "22.5°C");
    }

    #[test]
    fn test_format_temp_fahrenheit() {
        let opts = FormatOptions {
            fahrenheit: true,
            ..test_opts()
        };
        assert_eq!(opts.format_temp(0.0), "32.0°F");
        assert_eq!(opts.format_temp(100.0), "212.0°F");
    }

    #[test]
    fn test_convert_temp_celsius() {
        let opts = test_opts();
        assert_eq!(opts.convert_temp(22.5), 22.5);
    }

    #[test]
    fn test_convert_temp_fahrenheit() {
        let opts = FormatOptions {
            fahrenheit: true,
            ..test_opts()
        };
        assert!((opts.convert_temp(0.0) - 32.0).abs() < 0.01);
        assert!((opts.convert_temp(100.0) - 212.0).abs() < 0.01);
    }
}
