//! Read command implementation.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aranet_core::advertisement::parse_advertisement_with_name;
use aranet_core::scan::{ScanOptions, scan_with_options};
use aranet_types::CurrentReading;
use futures::future::join_all;

use crate::cli::OutputFormat;
use crate::format::{
    FormatOptions, format_multi_reading_csv, format_multi_reading_json, format_multi_reading_text,
    format_reading_csv, format_reading_json, format_reading_text, format_reading_text_with_name,
};
use crate::util::{require_device_interactive, write_output};

/// Result of reading from a device
pub struct DeviceReading {
    pub identifier: String,
    pub reading: CurrentReading,
}

pub async fn cmd_read(
    devices: Vec<String>,
    timeout: Duration,
    format: OutputFormat,
    output: Option<&PathBuf>,
    quiet: bool,
    passive: bool,
    opts: &FormatOptions,
) -> Result<()> {
    if passive {
        let device = devices.first().cloned();
        return cmd_read_passive(device, timeout, format, output, quiet, opts).await;
    }

    // If no devices specified, use interactive picker
    let devices = if devices.is_empty() {
        vec![require_device_interactive(None).await?]
    } else {
        devices
    };

    // Single device: use simple output
    if devices.len() == 1 {
        return cmd_read_single(&devices[0], timeout, format, output, quiet, opts).await;
    }

    // Multiple devices: read in parallel
    cmd_read_multi(devices, timeout, format, output, quiet, opts).await
}

/// Read from a single device
async fn cmd_read_single(
    identifier: &str,
    timeout: Duration,
    format: OutputFormat,
    output: Option<&PathBuf>,
    quiet: bool,
    opts: &FormatOptions,
) -> Result<()> {
    // Use connect_device_with_progress which has its own spinner
    // Don't create a separate spinner here to avoid duplication
    let show_progress = !quiet && matches!(format, OutputFormat::Text);
    let device =
        crate::util::connect_device_with_progress(identifier, timeout, show_progress).await?;
    let device_name = device.name().map(|s| s.to_string());
    let reading = device
        .read_current()
        .await
        .context("Failed to read current values")?;

    device.disconnect().await.ok();

    let content = match format {
        OutputFormat::Json => format_reading_json(&reading, opts)?,
        OutputFormat::Text => format_reading_text_with_name(&reading, opts, device_name.as_deref()),
        OutputFormat::Csv => format_reading_csv(&reading, opts),
    };

    write_output(output, &content)?;
    Ok(())
}

/// Read from multiple devices in parallel
async fn cmd_read_multi(
    devices: Vec<String>,
    timeout: Duration,
    format: OutputFormat,
    output: Option<&PathBuf>,
    quiet: bool,
    opts: &FormatOptions,
) -> Result<()> {
    let total_devices = devices.len();
    let show_progress = !quiet && matches!(format, OutputFormat::Text);

    if show_progress {
        eprintln!("Reading from {} devices...", total_devices);
    }

    // Track progress with atomic counter
    let completed = Arc::new(AtomicUsize::new(0));

    // Read from all devices in parallel with progress updates
    let futures = devices.iter().map(|id| {
        let completed = Arc::clone(&completed);
        let id = id.clone();
        async move {
            let result = read_device(id.clone(), timeout).await;
            let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
            if show_progress {
                match &result {
                    Ok(reading) => {
                        eprintln!("  [{}/{}] {} - OK", done, total_devices, reading.identifier);
                    }
                    Err((id, _)) => {
                        eprintln!("  [{}/{}] {} - FAILED", done, total_devices, id);
                    }
                }
            }
            result
        }
    });
    let results: Vec<Result<DeviceReading, (String, anyhow::Error)>> = join_all(futures).await;

    // Collect successful readings and errors
    let mut readings = Vec::new();
    let mut errors = Vec::new();

    for result in results {
        match result {
            Ok(reading) => readings.push(reading),
            Err((id, err)) => errors.push((id, err)),
        }
    }

    // Report detailed errors
    if !quiet && !errors.is_empty() {
        eprintln!();
        for (id, err) in &errors {
            eprintln!("Error reading {}: {}", id, err);
        }
    }

    if readings.is_empty() {
        bail!("Failed to read from any device");
    }

    let content = match format {
        OutputFormat::Json => format_multi_reading_json(&readings, opts)?,
        OutputFormat::Text => format_multi_reading_text(&readings, opts),
        OutputFormat::Csv => format_multi_reading_csv(&readings, opts),
    };

    write_output(output, &content)?;
    Ok(())
}

/// Read from a single device, returning the identifier with the result
async fn read_device(
    identifier: String,
    timeout: Duration,
) -> Result<DeviceReading, (String, anyhow::Error)> {
    // Don't show progress for individual devices in multi-read mode
    // to avoid multiple spinners running in parallel
    let device = crate::util::connect_device_with_progress(&identifier, timeout, false)
        .await
        .map_err(|e| (identifier.clone(), e))?;

    let reading = device
        .read_current()
        .await
        .context("Failed to read current values")
        .map_err(|e| (identifier.clone(), e))?;

    device.disconnect().await.ok();

    Ok(DeviceReading {
        identifier,
        reading,
    })
}

/// Read sensor data from BLE advertisements without connecting.
async fn cmd_read_passive(
    device: Option<String>,
    timeout: Duration,
    format: OutputFormat,
    output: Option<&PathBuf>,
    quiet: bool,
    opts: &FormatOptions,
) -> Result<()> {
    if !quiet && matches!(format, OutputFormat::Text) {
        eprintln!("Scanning for advertisements (passive mode)...");
    }

    let options = ScanOptions {
        duration: timeout,
        filter_aranet_only: true,
    };

    let devices = scan_with_options(options)
        .await
        .context("Failed to scan for devices")?;

    // Find the target device (if specified) or use the first one with advertisement data
    let target = device.as_deref();
    let found = devices.iter().find(|d| {
        if let Some(target) = target {
            // Match by name or address
            d.name.as_deref() == Some(target) || d.address == target || d.identifier == target
        } else {
            // No target specified, find first with manufacturer data
            d.manufacturer_data.is_some()
        }
    });

    let discovered = match found {
        Some(d) => d,
        None => {
            if let Some(target) = target {
                bail!("Device '{}' not found in advertisements", target);
            } else {
                bail!(
                    "No Aranet devices found with advertisement data. \
                       Make sure Smart Home integration is enabled on the device."
                );
            }
        }
    };

    let mfr_data = discovered.manufacturer_data.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "Device '{}' has no advertisement data. \
             Enable Smart Home integration on the device to use passive mode.",
            discovered.name.as_deref().unwrap_or(&discovered.identifier)
        )
    })?;

    let device_name = discovered.name.as_deref();
    let adv = parse_advertisement_with_name(mfr_data, device_name)
        .context("Failed to parse advertisement data")?;

    // Convert AdvertisementData to CurrentReading using builder
    let mut builder = CurrentReading::builder()
        .co2(adv.co2.unwrap_or(0))
        .temperature(adv.temperature.unwrap_or(0.0))
        .pressure(adv.pressure.unwrap_or(0.0))
        .humidity(adv.humidity.unwrap_or(0))
        .battery(adv.battery)
        .status(adv.status)
        .interval(adv.interval)
        .age(adv.age);

    // Add device-specific fields
    if let Some(radon) = adv.radon {
        builder = builder.radon(radon);
    }
    if let Some(rate) = adv.radiation_dose_rate {
        builder = builder.radiation_rate(rate);
    }

    let reading = builder.build();

    if !quiet && matches!(format, OutputFormat::Text) {
        let name = discovered.name.as_deref().unwrap_or(&discovered.identifier);
        eprintln!("Read from {} (passive)", name);
    }

    let content = match format {
        OutputFormat::Json => format_reading_json(&reading, opts)?,
        OutputFormat::Text => format_reading_text(&reading, opts),
        OutputFormat::Csv => format_reading_csv(&reading, opts),
    };

    write_output(output, &content)?;
    Ok(())
}
