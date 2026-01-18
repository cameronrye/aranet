//! Watch command implementation.
//!
//! Uses a persistent BLE connection to reduce overhead. The connection is only
//! re-established when a read fails, indicating the device has disconnected.
//! Implements exponential backoff for reconnection attempts to reduce resource usage.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use aranet_core::Device;
use aranet_core::advertisement::parse_advertisement_with_name;
use aranet_core::scan::{ScanOptions, scan_with_options};
use aranet_types::CurrentReading;
use owo_colors::OwoColorize;

use crate::cli::OutputFormat;
use crate::format::{
    FormatOptions, bq_to_pci, format_reading_json, format_reading_json_with_device,
    format_watch_csv_header, format_watch_csv_header_with_device, format_watch_csv_line,
    format_watch_csv_line_with_device, format_watch_line_with_device,
};
use crate::style;
use crate::util::{require_device_interactive, write_output};

/// Minimum backoff delay for reconnection attempts
const MIN_BACKOFF_SECS: u64 = 2;
/// Maximum backoff delay for reconnection attempts
const MAX_BACKOFF_SECS: u64 = 300; // 5 minutes

/// Arguments for the watch command.
pub struct WatchArgs<'a> {
    pub device: Option<String>,
    pub interval: u64,
    pub count: u32,
    pub timeout: Duration,
    pub format: OutputFormat,
    pub output: Option<&'a PathBuf>,
    pub passive: bool,
    pub opts: &'a FormatOptions,
}

pub async fn cmd_watch(args: WatchArgs<'_>) -> Result<()> {
    let WatchArgs {
        device,
        interval,
        count,
        timeout,
        format,
        output,
        passive,
        opts,
    } = args;

    if passive {
        return cmd_watch_passive(device, interval, count, timeout, format, output, opts).await;
    }

    let identifier = require_device_interactive(device).await?;

    let mut header_written = opts.no_header;
    let mut current_device: Option<Device> = None;
    let mut readings_taken: u32 = 0;
    let mut backoff_secs = MIN_BACKOFF_SECS;
    let mut previous_reading: Option<CurrentReading> = None;
    let mut header_printed = false;

    loop {
        // Check if we've reached the count limit
        if count > 0 && readings_taken >= count {
            eprintln!("Completed {} readings.", readings_taken);
            if let Some(d) = current_device.take() {
                d.disconnect().await.ok();
            }
            return Ok(());
        }

        // Connect if we don't have a connection
        let is_connected = match &current_device {
            Some(d) => d.is_connected().await,
            None => false,
        };

        if !is_connected {
            // Need to connect (or reconnect)
            if current_device.is_some() {
                eprintln!("Connection lost. Reconnecting...");
            }
            match Device::connect_with_timeout(&identifier, timeout).await {
                Ok(d) => {
                    // Reset backoff on successful connection
                    backoff_secs = MIN_BACKOFF_SECS;
                    current_device = Some(d);
                }
                Err(e) => {
                    eprintln!("Connection failed: {}. Retrying in {}s...", e, backoff_secs);
                    current_device = None;

                    // Wait with graceful shutdown support using exponential backoff
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {
                            eprintln!("\nShutting down...");
                            return Ok(());
                        }
                        _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                    }

                    // Increase backoff for next attempt (exponential with cap)
                    backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                    continue;
                }
            }
        } else {
            // Reset backoff on successful connection check
            backoff_secs = MIN_BACKOFF_SECS;
        }

        // At this point we're guaranteed to have a device
        let device = current_device.as_ref().expect("device should be connected");

        // Print header with device info (after first successful connection)
        if !header_printed {
            let device_name = device.name().unwrap_or("Unknown");
            let header = if opts.no_color {
                format!("Watching: {} ({})", device_name, identifier)
            } else {
                format!("Watching: {} ({})", device_name.green(), identifier.cyan())
            };
            eprintln!("{}", header);
            if count > 0 {
                eprintln!(
                    "Interval: {}s | Count: {} | Press Ctrl+C to stop",
                    interval, count
                );
            } else {
                eprintln!("Interval: {}s | Press Ctrl+C to stop", interval);
            }
            eprintln!("{}", "-".repeat(50));
            header_printed = true;
        }

        // Read current values
        match device.read_current().await {
            Ok(reading) => {
                readings_taken += 1;
                let content = match format {
                    OutputFormat::Json => format_reading_json(&reading, opts)?,
                    OutputFormat::Csv => {
                        let mut out = String::new();
                        if !header_written {
                            out.push_str(&format_watch_csv_header(opts));
                            header_written = true;
                        }
                        out.push_str(&format_watch_csv_line(&reading, opts));
                        out
                    }
                    OutputFormat::Text => {
                        format_watch_line_with_trend(&reading, previous_reading.as_ref(), opts)
                    }
                };
                write_output(output, &content)?;
                previous_reading = Some(reading);
            }
            Err(e) => {
                eprintln!("Read failed: {}. Will reconnect on next poll.", e);
                // Mark connection as lost so we reconnect on next iteration
                if let Some(d) = current_device.take() {
                    d.disconnect().await.ok();
                }
            }
        }

        // Check if we've reached the count limit after this reading
        if count > 0 && readings_taken >= count {
            continue; // Loop will exit at the top
        }

        // Wait for next interval with graceful shutdown support
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nShutting down...");
                // Clean up connection before exit
                if let Some(d) = current_device.take() {
                    d.disconnect().await.ok();
                }
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
        }
    }
}

/// Watch sensor data from BLE advertisements without connecting.
async fn cmd_watch_passive(
    device: Option<String>,
    interval: u64,
    count: u32,
    timeout: Duration,
    format: OutputFormat,
    output: Option<&PathBuf>,
    opts: &FormatOptions,
) -> Result<()> {
    let target = device.as_deref();
    let mode_desc = if let Some(t) = target {
        if opts.no_color {
            format!("{} (passive)", t)
        } else {
            format!("{} (passive)", t.cyan())
        }
    } else {
        "all devices (passive)".to_string()
    };

    eprintln!("Watching: {}", mode_desc);
    if count > 0 {
        eprintln!(
            "Interval: {}s | Count: {} | Press Ctrl+C to stop",
            interval, count
        );
    } else {
        eprintln!("Interval: {}s | Press Ctrl+C to stop", interval);
    }
    eprintln!("{}", "-".repeat(60));

    let mut header_written = opts.no_header;
    let mut readings_taken: u32 = 0;

    loop {
        // Check if we've reached the count limit
        if count > 0 && readings_taken >= count {
            eprintln!("Completed {} readings.", readings_taken);
            return Ok(());
        }

        // Scan for advertisements
        let options = ScanOptions {
            duration: timeout,
            filter_aranet_only: true,
        };

        match scan_with_options(options).await {
            Ok(devices) => {
                // Filter devices based on target or get all with advertisement data
                let matching_devices: Vec<_> = devices
                    .iter()
                    .filter(|d| {
                        if let Some(t) = target {
                            d.name.as_deref() == Some(t) || d.address == t || d.identifier == t
                        } else {
                            d.manufacturer_data.is_some()
                        }
                    })
                    .collect();

                if matching_devices.is_empty() {
                    if let Some(t) = target {
                        eprintln!("Device '{}' not found. Retrying...", t);
                    } else {
                        eprintln!("No Aranet devices with advertisement data found. Retrying...");
                    }
                } else {
                    // Process ALL matching devices
                    for discovered in matching_devices {
                        if let Some(mfr_data) = &discovered.manufacturer_data {
                            let device_name = discovered.name.as_deref();
                            match parse_advertisement_with_name(mfr_data, device_name) {
                                Ok(adv) => {
                                    // Convert to CurrentReading
                                    let mut builder = CurrentReading::builder()
                                        .co2(adv.co2.unwrap_or(0))
                                        .temperature(adv.temperature.unwrap_or(0.0))
                                        .pressure(adv.pressure.unwrap_or(0.0))
                                        .humidity(adv.humidity.unwrap_or(0))
                                        .battery(adv.battery)
                                        .status(adv.status)
                                        .interval(adv.interval)
                                        .age(adv.age);

                                    if let Some(radon) = adv.radon {
                                        builder = builder.radon(radon);
                                    }
                                    if let Some(rate) = adv.radiation_dose_rate {
                                        builder = builder.radiation_rate(rate);
                                    }

                                    let reading = builder.build();
                                    readings_taken += 1;

                                    // Get a short device name for display
                                    let display_name = device_name.unwrap_or(&discovered.address);

                                    let content = match format {
                                        OutputFormat::Json => format_reading_json_with_device(
                                            &reading,
                                            display_name,
                                            opts,
                                        )?,
                                        OutputFormat::Csv => {
                                            let mut out = String::new();
                                            if !header_written {
                                                out.push_str(&format_watch_csv_header_with_device(
                                                    opts,
                                                ));
                                                header_written = true;
                                            }
                                            out.push_str(&format_watch_csv_line_with_device(
                                                &reading,
                                                display_name,
                                                opts,
                                            ));
                                            out
                                        }
                                        OutputFormat::Text => format_watch_line_with_device(
                                            &reading,
                                            display_name,
                                            opts,
                                        ),
                                    };
                                    write_output(output, &content)?;
                                }
                                Err(e) => {
                                    eprintln!(
                                        "Failed to parse advertisement from {}: {}",
                                        discovered.name.as_deref().unwrap_or(&discovered.address),
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Scan failed: {}. Retrying...", e);
            }
        }

        // Check if we've reached the count limit after this reading
        if count > 0 && readings_taken >= count {
            continue; // Loop will exit at the top
        }

        // Wait for next interval with graceful shutdown support
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nShutting down...");
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
        }
    }
}

/// Format a watch line with trend indicators comparing to previous reading.
fn format_watch_line_with_trend(
    reading: &CurrentReading,
    previous: Option<&CurrentReading>,
    opts: &FormatOptions,
) -> String {
    use chrono::Local;

    let timestamp = Local::now().format("%H:%M:%S").to_string();

    // Get trend indicators if we have a previous reading
    // Use "~" for first reading to indicate "no change data yet" rather than "-" which could be confused with "decreasing"
    let co2_trend = previous
        .map(|p| style::trend_indicator_int(reading.co2 as i32, p.co2 as i32, opts.no_color))
        .unwrap_or("~");
    let temp_trend = previous
        .map(|p| style::trend_indicator(reading.temperature, p.temperature, opts.no_color))
        .unwrap_or("~");

    // Format values with colors
    let co2_display = if reading.co2 > 0 {
        format!(
            "{} ppm {}",
            style::format_co2_colored(reading.co2, opts.no_color),
            co2_trend
        )
    } else {
        String::new()
    };

    let temp_display = format!(
        "{} {} {}",
        style::format_temp_colored(opts.convert_temp(reading.temperature), opts.no_color),
        if opts.fahrenheit { "°F" } else { "°C" },
        temp_trend
    );

    let humidity_display = style::format_humidity_colored(reading.humidity, opts.no_color);
    let battery_display = style::format_battery_colored(reading.battery, opts.no_color);

    // Build output line
    if reading.co2 > 0 {
        // Aranet4
        format!(
            "[{}] {} | {} | {} | {}\n",
            timestamp, co2_display, temp_display, humidity_display, battery_display
        )
    } else if let Some(radon) = reading.radon {
        // AranetRn+ - format radon with proper unit conversion and coloring
        let radon_display = if opts.bq {
            style::format_radon_colored(radon, opts.no_color)
        } else {
            style::format_radon_pci_colored(radon, bq_to_pci(radon), opts.no_color)
        };
        format!(
            "[{}] {} {} | {} | {} | {}\n",
            timestamp, radon_display, opts.radon_display_unit(), temp_display, humidity_display, battery_display
        )
    } else if let Some(rate) = reading.radiation_rate {
        // Aranet Radiation
        format!("[{}] {:.3} uSv/h | {}\n", timestamp, rate, battery_display)
    } else {
        // Aranet2
        format!(
            "[{}] {} | {} | {}\n",
            timestamp, temp_display, humidity_display, battery_display
        )
    }
}
