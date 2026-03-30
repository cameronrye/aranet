//! Utility functions for CLI operations.

use std::fs::OpenOptions;
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

/// Disconnect from a device, logging any errors at debug level.
pub async fn disconnect_device(device: &aranet_core::Device) {
    if let Err(e) = device.disconnect().await {
        tracing::debug!("Failed to disconnect device: {e}");
    }
}

/// Build a user-friendly device error with suggestions and timestamp.
fn device_error(operation: &str, identifier: &str, cause: impl std::fmt::Display) -> anyhow::Error {
    let timestamp = aranet_cli::local_now_fmt("[year]-[month]-[day] [hour]:[minute]:[second]");
    let base_msg = format!("Failed to {} device: {}", operation, identifier);
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
    anyhow::anyhow!("{}\n\nCause: {}{}", base_msg, cause, suggestion)
}

/// Open the store database, printing a warning to stderr on failure.
fn open_store() -> Option<aranet_store::Store> {
    let store_path = aranet_store::default_db_path();
    match aranet_store::Store::open(&store_path) {
        Ok(store) => Some(store),
        Err(e) => {
            tracing::warn!("Failed to open store: {}", e);
            eprintln!(
                "Warning: could not open local database at {}. Readings will not be cached.",
                store_path.display()
            );
            None
        }
    }
}

/// Get device identifier, scanning and prompting interactively if none specified.
pub async fn require_device_interactive(device: Option<String>) -> Result<String> {
    if let Some(dev) = device {
        return Ok(dev);
    }

    // Check if we're in an interactive terminal
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!(
            "No device specified (non-interactive mode).\n\n\
             How to fix:\n  \
             1. Run 'aranet scan' to discover nearby devices\n  \
             2. Use --device <ADDRESS> with the device address\n  \
             3. Set ARANET_DEVICE environment variable\n  \
             4. Set a default with 'aranet config set device <ADDRESS>'\n\n\
             Example: aranet read --device AA:BB:CC:DD:EE:FF"
        );
    }

    eprintln!("No device specified. Scanning for nearby devices...");

    let options = ScanOptions::default()
        .duration_secs(5)
        .filter_aranet_only(true);

    let devices = scan::scan_with_options(options)
        .await
        .context("Failed to scan for devices")?;

    if devices.is_empty() {
        bail!(
            "No Aranet devices found nearby.\n\n\
             Troubleshooting:\n  \
             - Ensure your device is powered on\n  \
             - Move closer to the device (within 10m)\n  \
             - Check that Bluetooth is enabled on your computer\n  \
             - Run 'aranet doctor' to diagnose Bluetooth issues\n\n\
             If your device has Smart Home mode disabled, it may not be visible.\n\
             Enable it in the Aranet mobile app: Settings > Smart Home Integration"
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

    let options = ScanOptions::default()
        .duration(timeout)
        .filter_aranet_only(false);

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
    let (adapter, peripheral) = result.map_err(|e| device_error("find", identifier, e))?;

    let device = Device::from_peripheral(adapter, peripheral)
        .await
        .map_err(|e| device_error("connect to", identifier, e))?;

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
fn save_device_to_store(device_id: &str, name: Option<&str>) {
    if let Some(store) = open_store()
        && let Err(e) = store.upsert_device(device_id, name)
    {
        tracing::warn!("Failed to save device to store: {}", e);
        eprintln!("Warning: could not save device to local database: {e}");
    }
}

/// Save a reading to the store database.
pub fn save_reading_to_store(device_id: &str, reading: &aranet_types::CurrentReading) {
    if let Some(store) = open_store()
        && let Err(e) = store.insert_reading(device_id, reading)
    {
        tracing::warn!("Failed to save reading to store: {}", e);
        eprintln!("Warning: could not save reading to local database: {e}");
    }
}

/// Save history records to the store database. Returns the number of records inserted.
pub fn save_history_to_store(device_id: &str, records: &[aranet_types::HistoryRecord]) -> usize {
    let Some(store) = open_store() else {
        return 0;
    };
    match store.insert_history(device_id, records) {
        Ok(count) => count,
        Err(e) => {
            tracing::warn!("Failed to save history to store: {}", e);
            eprintln!("Warning: could not save history to local database: {e}");
            0
        }
    }
}

fn write_output_inner(output: Option<&PathBuf>, content: &str, append: bool) -> Result<()> {
    match output {
        Some(path) => {
            if append {
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .with_context(|| format!("Failed to open {} for append", path.display()))?;
                file.write_all(content.as_bytes())
                    .with_context(|| format!("Failed to write to {}", path.display()))?;
            } else {
                std::fs::write(path, content)
                    .with_context(|| format!("Failed to write to {}", path.display()))?;
            }
        }
        None => {
            print!("{}", content);
            io::stdout().flush()?;
        }
    }
    Ok(())
}

/// Write output to file or stdout, replacing any existing file contents.
pub fn write_output(output: Option<&PathBuf>, content: &str) -> Result<()> {
    write_output_inner(output, content, false)
}

/// Append output to a file or stdout.
pub fn append_output(output: Option<&PathBuf>, content: &str) -> Result<()> {
    write_output_inner(output, content, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_output_replaces_existing_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("output.txt");

        write_output(Some(&path), "first").unwrap();
        write_output(Some(&path), "second").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "second");
    }

    #[test]
    fn test_append_output_preserves_existing_file_contents() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("output.txt");

        write_output(Some(&path), "header\n").unwrap();
        append_output(Some(&path), "row1\n").unwrap();
        append_output(Some(&path), "row2\n").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "header\nrow1\nrow2\n");
    }
}
