//! Status command implementation.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::format::{FormatOptions, bq_to_pci, csv_escape, format_status};
use crate::style;
use crate::util::{connect_device_with_progress, require_device_interactive, write_output};

pub async fn cmd_status(
    device: Option<String>,
    timeout: Duration,
    format: OutputFormat,
    output: Option<&PathBuf>,
    opts: &FormatOptions,
    brief: bool,
) -> Result<()> {
    let identifier = require_device_interactive(device).await?;

    // Use connect_device_with_progress which has its own spinner
    let device = connect_device_with_progress(&identifier, timeout, true).await?;

    let name = device.name().map(|s| s.to_string());
    let reading = device
        .read_current()
        .await
        .context("Failed to read current values")?;

    device.disconnect().await.ok();

    let device_name = name.clone().unwrap_or_else(|| identifier.clone());

    let content = match format {
        OutputFormat::Json => format_status_json(&device_name, &reading, opts)?,
        OutputFormat::Csv => format_status_csv(&device_name, &reading, opts),
        OutputFormat::Text => {
            if brief {
                format_status_brief(&reading, opts)
            } else {
                format_status_text(&device_name, &reading, opts)
            }
        }
    };

    write_output(output, &content)?;
    Ok(())
}

/// Format status as one-line text output with colored values
fn format_status_text(
    device_name: &str,
    reading: &aranet_types::CurrentReading,
    opts: &FormatOptions,
) -> String {
    let status_str = format_status(reading.status, opts.no_color);
    let temp = opts.format_temp(reading.temperature);

    // Color the device name
    let name_display = if opts.no_color {
        device_name.to_string()
    } else {
        format!("{}", device_name.cyan())
    };

    if reading.co2 > 0 {
        // Aranet4 - with colored CO2
        let co2_display = style::format_co2_colored(reading.co2, opts.no_color);
        let humidity_display = style::format_humidity_colored(reading.humidity, opts.no_color);
        format!(
            "{}: {} ppm {} {} {} {:.1}hPa\n",
            name_display, co2_display, status_str, temp, humidity_display, reading.pressure
        )
    } else if let Some(radon) = reading.radon {
        // AranetRn+ - with colored radon
        let radon_display = style::format_radon_colored(radon, opts.no_color);
        let humidity_display = style::format_humidity_colored(reading.humidity, opts.no_color);
        format!(
            "{}: {} Bq/m3 {} {} {} {:.1}hPa\n",
            name_display, radon_display, status_str, temp, humidity_display, reading.pressure
        )
    } else if let Some(rate) = reading.radiation_rate {
        // Aranet Radiation
        format!("{}: {:.3} uSv/h\n", name_display, rate)
    } else {
        // Aranet2 or unknown
        let humidity_display = style::format_humidity_colored(reading.humidity, opts.no_color);
        format!("{}: {} {}\n", name_display, temp, humidity_display)
    }
}

/// Format status as super-compact brief output (just the key value)
fn format_status_brief(reading: &aranet_types::CurrentReading, opts: &FormatOptions) -> String {
    if reading.co2 > 0 {
        // Aranet4: just CO2 value
        format!("{}\n", reading.co2)
    } else if let Some(radon) = reading.radon {
        // AranetRn+: just radon value
        if opts.bq {
            format!("{}\n", radon)
        } else {
            format!("{:.2}\n", radon as f32 / 37.0)
        }
    } else if let Some(rate) = reading.radiation_rate {
        // Aranet Radiation: just rate
        format!("{:.3}\n", rate)
    } else {
        // Aranet2: temp and humidity
        format!("{:.1},{}\n", reading.temperature, reading.humidity)
    }
}

/// Format status as JSON output
fn format_status_json(
    device_name: &str,
    reading: &aranet_types::CurrentReading,
    opts: &FormatOptions,
) -> Result<String> {
    #[derive(Serialize)]
    struct StatusJson<'a> {
        device: &'a str,
        co2: u16,
        temperature: f32,
        temperature_unit: &'static str,
        humidity: u8,
        pressure: f32,
        battery: u8,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_bq: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radon_pci: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        radiation_rate: Option<f32>,
    }

    let json = StatusJson {
        device: device_name,
        co2: reading.co2,
        temperature: opts.convert_temp(reading.temperature),
        temperature_unit: if opts.fahrenheit { "F" } else { "C" },
        humidity: reading.humidity,
        pressure: reading.pressure,
        battery: reading.battery,
        status: format!("{:?}", reading.status),
        radon_bq: reading.radon,
        radon_pci: reading.radon.map(bq_to_pci),
        radiation_rate: reading.radiation_rate,
    };

    opts.as_json(&json)
}

/// Format status as CSV output
fn format_status_csv(
    device_name: &str,
    reading: &aranet_types::CurrentReading,
    opts: &FormatOptions,
) -> String {
    let temp_header = if opts.fahrenheit {
        "temperature_f"
    } else {
        "temperature_c"
    };
    let radon_value = reading
        .radon
        .map(|r| format!("{:.2}", opts.convert_radon(r)))
        .unwrap_or_default();
    if opts.no_header {
        format!(
            "{},{},{:.1},{},{:.1},{},{:?},{},{}\n",
            csv_escape(device_name),
            reading.co2,
            opts.convert_temp(reading.temperature),
            reading.humidity,
            reading.pressure,
            reading.battery,
            reading.status,
            radon_value,
            reading
                .radiation_rate
                .map(|r| format!("{:.3}", r))
                .unwrap_or_default()
        )
    } else {
        format!(
            "device,co2,{},humidity,pressure,battery,status,{},radiation_usvh\n\
             {},{},{:.1},{},{:.1},{},{:?},{},{}\n",
            temp_header,
            opts.radon_csv_header(),
            csv_escape(device_name),
            reading.co2,
            opts.convert_temp(reading.temperature),
            reading.humidity,
            reading.pressure,
            reading.battery,
            reading.status,
            radon_value,
            reading
                .radiation_rate
                .map(|r| format!("{:.3}", r))
                .unwrap_or_default()
        )
    }
}
