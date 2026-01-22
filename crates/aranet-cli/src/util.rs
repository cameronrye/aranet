//! Utility functions for CLI operations.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aranet_core::{Device, FindProgress, ScanOptions, find_device_with_progress, scan};
use dialoguer::{Select, theme::ColorfulTheme};
use indicatif::ProgressBar;

use crate::config::update_last_device;
use crate::style;

/// Get device identifier, with helpful error message.
/// Used for non-interactive contexts (e.g., scripts, piped input).
#[allow(dead_code)]
pub fn require_device(device: Option<String>) -> Result<String> {
    device.ok_or_else(|| {
        anyhow::anyhow!(
            "No device specified. Use --device <ADDRESS> or set ARANET_DEVICE environment variable.\n\
             Run 'aranet scan' to find nearby devices, or omit --device for interactive selection."
        )
    })
}

/// Get device identifier, scanning and prompting interactively if none specified.
pub async fn require_device_interactive(device: Option<String>) -> Result<String> {
    if let Some(dev) = device {
        return Ok(dev);
    }

    // Check if we're in an interactive terminal
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!(
            "No device specified. Use --device <ADDRESS> or set ARANET_DEVICE environment variable.\n\
             Run 'aranet scan' to find nearby devices."
        );
    }

    eprintln!("No device specified. Scanning for nearby devices...");

    let options = ScanOptions {
        duration: Duration::from_secs(5),
        filter_aranet_only: true,
    };

    let devices = scan::scan_with_options(options)
        .await
        .context("Failed to scan for devices")?;

    if devices.is_empty() {
        bail!(
            "No Aranet devices found nearby.\n\
             Make sure your device is powered on and in range."
        );
    }

    if devices.len() == 1 {
        let dev = &devices[0];
        let name = dev.name.as_deref().unwrap_or("Unknown");
        eprintln!("Found 1 device: {} ({})", name, dev.identifier);
        return Ok(dev.identifier.clone());
    }

    // Build selection items
    let items: Vec<String> = devices
        .iter()
        .map(|d| {
            let name = d.name.as_deref().unwrap_or("Unknown");
            format!("{} ({})", name, d.identifier)
        })
        .collect();

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select a device")
        .items(&items)
        .default(0)
        .interact()
        .context("Failed to get user selection")?;

    Ok(devices[selection].identifier.clone())
}

/// Connect to a device with timeout and improved error messages.
/// Shows progress feedback for connection attempts.
#[allow(dead_code)]
pub async fn connect_device(identifier: &str, timeout: Duration) -> Result<Device> {
    connect_device_with_progress(identifier, timeout, true).await
}

/// Connect to a device with optional progress display.
pub async fn connect_device_with_progress(
    identifier: &str,
    timeout: Duration,
    show_progress: bool,
) -> Result<Device> {
    // Create spinner for visual feedback
    let spinner: Option<Arc<ProgressBar>> = if show_progress && io::stderr().is_terminal() {
        Some(Arc::new(style::connecting_spinner(identifier)))
    } else {
        None
    };

    // Create progress callback
    let spinner_clone = spinner.clone();
    let progress_callback: Option<aranet_core::ProgressCallback> = if show_progress {
        Some(Box::new(move |progress: FindProgress| {
            if let Some(ref sp) = spinner_clone {
                match progress {
                    FindProgress::CacheHit => {
                        sp.set_message("Found device (cached)".to_string());
                    }
                    FindProgress::ScanAttempt {
                        attempt,
                        total,
                        duration_secs,
                    } => {
                        sp.set_message(format!(
                            "Scanning... (attempt {}/{}, {}s)",
                            attempt, total, duration_secs
                        ));
                    }
                    FindProgress::Found { attempt } => {
                        if attempt > 1 {
                            sp.set_message(format!("Found on attempt {}", attempt));
                        } else {
                            sp.set_message("Found device".to_string());
                        }
                    }
                    FindProgress::RetryNeeded { attempt } => {
                        sp.set_message(format!("Not found, retrying... (attempt {})", attempt + 1));
                    }
                }
            }
        }))
    } else {
        None
    };

    let options = ScanOptions {
        duration: timeout,
        filter_aranet_only: false,
    };

    // Find the device with progress
    let result = find_device_with_progress(identifier, options, progress_callback).await;

    // Update spinner based on result
    if let Some(ref sp) = spinner {
        match &result {
            Ok(_) => {
                sp.set_message("Connecting...".to_string());
            }
            Err(_) => {
                sp.finish_and_clear();
            }
        }
    }

    // Now create Device from peripheral
    let (adapter, peripheral) = result.map_err(|e| {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let base_msg = format!("Failed to find device: {}", identifier);
        let suggestion = format!(
            "\n\nPossible causes:\n  \
            - Bluetooth may be disabled -- check system settings\n  \
            - Device may be out of range -- try moving closer\n  \
            - Device may be connected to another host\n  \
            - Device address may be incorrect -- run 'aranet scan' to verify\n\n\
            Tip: Run 'aranet doctor' to diagnose Bluetooth issues\n\
            Time: {}",
            timestamp
        );
        anyhow::anyhow!("{}\n\nCause: {}{}", base_msg, e, suggestion)
    })?;

    let device = Device::from_peripheral(adapter, peripheral)
        .await
        .map_err(|e| {
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let base_msg = format!("Failed to connect to device: {}", identifier);
            let suggestion = format!(
                "\n\nPossible causes:\n  \
                - Device may have gone out of range\n  \
                - Device may be connected to another host\n  \
                - Bluetooth connection was interrupted\n\n\
                Tip: Run 'aranet doctor' to diagnose Bluetooth issues\n\
                Time: {}",
                timestamp
            );
            anyhow::anyhow!("{}\n\nCause: {}{}", base_msg, e, suggestion)
        })?;

    // Finish spinner
    if let Some(sp) = spinner {
        sp.finish_and_clear();
    }

    // Save last connected device (ignore errors - this is a convenience feature)
    let device_name = device.name().map(|s| s.to_string());
    let device_address = device.address().to_string();
    let _ = update_last_device(&device_address, device_name.as_deref());

    // Save device to store database (unified data architecture)
    save_device_to_store(&device_address, device_name.as_deref());

    Ok(device)
}

/// Save a device connection to the store database.
///
/// This is part of the unified data architecture - all tools share the same database.
fn save_device_to_store(device_id: &str, name: Option<&str>) {
    let store_path = aranet_store::default_db_path();
    if let Ok(store) = aranet_store::Store::open(&store_path) {
        // Insert or update the device in the store
        if let Err(e) = store.upsert_device(device_id, name) {
            tracing::warn!("Failed to save device to store: {}", e);
        }
    }
}

/// Save a reading to the store database.
///
/// This is part of the unified data architecture - all tools share the same database.
pub fn save_reading_to_store(device_id: &str, reading: &aranet_types::CurrentReading) {
    let store_path = aranet_store::default_db_path();
    if let Ok(store) = aranet_store::Store::open(&store_path)
        && let Err(e) = store.insert_reading(device_id, reading)
    {
        tracing::warn!("Failed to save reading to store: {}", e);
    }
}

/// Save history records to the store database.
///
/// This is part of the unified data architecture - all tools share the same database.
/// Returns the number of records inserted.
pub fn save_history_to_store(device_id: &str, records: &[aranet_types::HistoryRecord]) -> usize {
    let store_path = aranet_store::default_db_path();
    if let Ok(store) = aranet_store::Store::open(&store_path) {
        match store.insert_history(device_id, records) {
            Ok(count) => count,
            Err(e) => {
                tracing::warn!("Failed to save history to store: {}", e);
                0
            }
        }
    } else {
        0
    }
}

/// Write output to file or stdout
pub fn write_output(output: Option<&PathBuf>, content: &str) -> Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, content)
                .with_context(|| format!("Failed to write to {}", path.display()))?;
        }
        None => {
            print!("{}", content);
            io::stdout().flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_require_device_with_some() {
        let result = require_device(Some("AA:BB:CC:DD:EE:FF".to_string()));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn test_require_device_with_none() {
        let result = require_device(None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No device specified"));
        assert!(err.contains("ARANET_DEVICE"));
    }
}
