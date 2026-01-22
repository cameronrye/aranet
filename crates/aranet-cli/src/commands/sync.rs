//! Sync command - download and cache device history.

use std::time::Duration;

use anyhow::{Context, Result};
use aranet_core::HistoryOptions;
use aranet_store::Store;
use tracing::info;

use crate::cli::{DeviceArgs, OutputFormat};
use crate::config::Config;
use crate::style;
use crate::util::require_device_interactive;

/// Arguments for the sync command.
pub struct SyncArgs {
    pub device: DeviceArgs,
    pub format: OutputFormat,
    pub full: bool,
    pub all: bool,
}

/// Execute the sync command.
pub async fn cmd_sync(args: SyncArgs, config: &Config) -> Result<()> {
    // Open the store
    let store = Store::open_default().context("Failed to open database")?;

    // If --all flag is set, sync all known devices
    if args.all {
        return sync_all_devices(&store, args.format, args.full, args.device.timeout).await;
    }

    // Resolve device address from args, env, or config
    let device_input = args.device.device.clone().or_else(|| config.device.clone());
    let device_address = require_device_interactive(device_input).await?;
    let timeout = Duration::from_secs(args.device.timeout);

    // Connect to device
    let device = crate::util::connect_device_with_progress(&device_address, timeout, true).await?;

    // Get device info for display
    let device_info = device.read_device_info().await?;
    let device_name = if device_info.name.is_empty() {
        device_address.clone()
    } else {
        device_info.name.clone()
    };

    // Store device info
    store.upsert_device(&device_address, Some(&device_name))?;
    store.update_device_info(&device_address, &device_info)?;

    // Get history info to know total count
    let history_info = device.get_history_info().await?;
    let total_on_device = history_info.total_readings;

    // Calculate sync start based on incremental sync
    let start_index = if args.full {
        info!(
            "Full sync requested, downloading all {} records",
            total_on_device
        );
        1u16
    } else {
        let start = store.calculate_sync_start(&device_address, total_on_device)?;
        if start > total_on_device {
            println!("Already up to date - no new readings to sync");
            device.disconnect().await.ok();
            return Ok(());
        }
        info!(
            "Incremental sync: downloading records {} to {}",
            start, total_on_device
        );
        start
    };

    // Download history
    let records_to_download = total_on_device.saturating_sub(start_index) + 1;
    println!(
        "Syncing {} ({} records)...",
        device_name, records_to_download
    );

    // Create progress bar
    let pb = style::download_progress_bar();
    pb.set_message("Downloading history...");
    let pb_for_callback = pb.clone();

    let history_opts = HistoryOptions::default().with_progress(move |progress| {
        let percent = (progress.overall_progress * 100.0) as u64;
        pb_for_callback.set_position(percent);
        pb_for_callback.set_message(format!(
            "Downloading {:?} ({}/{})",
            progress.current_param, progress.param_index, progress.total_params
        ));
    });

    let history = device
        .download_history_with_options(history_opts)
        .await
        .context("Failed to download history")?;

    pb.finish_with_message("Download complete");
    device.disconnect().await.ok();

    // Store history records
    let inserted = store.insert_history(&device_address, &history)?;

    // Update sync state
    store.update_sync_state(&device_address, total_on_device, total_on_device)?;

    // Report results
    let total_cached = store.count_history(Some(&device_address))?;

    match args.format {
        OutputFormat::Json => {
            let result = serde_json::json!({
                "device": device_address,
                "name": device_name,
                "downloaded": history.len(),
                "inserted": inserted,
                "total_cached": total_cached,
                "total_on_device": total_on_device,
            });
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        _ => {
            println!("Downloaded: {} records", history.len());
            println!("New records: {}", inserted);
            println!("Total cached: {}", total_cached);
            println!("Total on device: {}", total_on_device);
        }
    }

    Ok(())
}

/// Sync all known devices from the database.
async fn sync_all_devices(
    store: &Store,
    format: OutputFormat,
    full: bool,
    timeout_secs: u64,
) -> Result<()> {
    let devices = store.list_devices().context("Failed to list devices")?;

    if devices.is_empty() {
        println!("No devices found in database. Run 'aranet scan' first to discover devices.");
        return Ok(());
    }

    let timeout = Duration::from_secs(timeout_secs);
    let total_devices = devices.len();
    let mut successful = 0;
    let mut failed = 0;
    let mut total_downloaded = 0usize;
    let mut total_inserted = 0usize;
    let mut results = Vec::new();

    println!("Syncing {} device(s)...\n", total_devices);

    for (idx, stored_device) in devices.iter().enumerate() {
        let device_name = stored_device
            .name
            .as_ref()
            .unwrap_or(&stored_device.id)
            .clone();
        println!("[{}/{}] Syncing {}...", idx + 1, total_devices, device_name);

        match sync_single_device(store, &stored_device.id, &device_name, full, timeout).await {
            Ok((downloaded, inserted)) => {
                successful += 1;
                total_downloaded += downloaded;
                total_inserted += inserted;
                results.push(serde_json::json!({
                    "device": stored_device.id,
                    "name": device_name,
                    "status": "success",
                    "downloaded": downloaded,
                    "inserted": inserted,
                }));
                println!(
                    "  [PASS] Downloaded {} records, {} new\n",
                    downloaded, inserted
                );
            }
            Err(e) => {
                failed += 1;
                results.push(serde_json::json!({
                    "device": stored_device.id,
                    "name": device_name,
                    "status": "failed",
                    "error": e.to_string(),
                }));
                println!("  [FAIL] {}\n", e);
            }
        }
    }

    // Summary
    match format {
        OutputFormat::Json => {
            let summary = serde_json::json!({
                "total_devices": total_devices,
                "successful": successful,
                "failed": failed,
                "total_downloaded": total_downloaded,
                "total_inserted": total_inserted,
                "devices": results,
            });
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
        _ => {
            println!("---");
            println!("Summary:");
            println!("  Devices synced: {}/{}", successful, total_devices);
            if failed > 0 {
                println!("  Failed: {}", failed);
            }
            println!("  Total downloaded: {} records", total_downloaded);
            println!("  New records: {}", total_inserted);
        }
    }

    Ok(())
}

/// Sync a single device and return (downloaded, inserted) counts.
async fn sync_single_device(
    store: &Store,
    device_address: &str,
    device_name: &str,
    full: bool,
    timeout: Duration,
) -> Result<(usize, usize)> {
    // Connect to device
    let device = crate::util::connect_device_with_progress(device_address, timeout, false).await?;

    // Get device info and update store
    let device_info = device.read_device_info().await?;
    store.update_device_info(device_address, &device_info)?;

    // Get history info to know total count
    let history_info = device.get_history_info().await?;
    let total_on_device = history_info.total_readings;

    // Calculate sync start based on incremental sync
    let start_index = if full {
        info!(
            "{}: Full sync requested, downloading all {} records",
            device_name, total_on_device
        );
        1u16
    } else {
        let start = store.calculate_sync_start(device_address, total_on_device)?;
        if start > total_on_device {
            device.disconnect().await.ok();
            return Ok((0, 0)); // Already up to date
        }
        info!(
            "{}: Incremental sync: downloading records {} to {}",
            device_name, start, total_on_device
        );
        start
    };

    // Download history (without progress bar to keep output clean for multiple devices)
    let history_opts = HistoryOptions::default().start_index(start_index);
    let history = device
        .download_history_with_options(history_opts)
        .await
        .context("Failed to download history")?;

    device.disconnect().await.ok();

    // Store history records
    let inserted = store.insert_history(device_address, &history)?;

    // Update sync state
    store.update_sync_state(device_address, total_on_device, total_on_device)?;

    Ok((history.len(), inserted))
}
