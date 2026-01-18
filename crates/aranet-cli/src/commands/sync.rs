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
}

/// Execute the sync command.
pub async fn cmd_sync(args: SyncArgs, config: &Config) -> Result<()> {
    // Resolve device address from args, env, or config
    let device_input = args.device.device.clone().or_else(|| config.device.clone());
    let device_address = require_device_interactive(device_input).await?;
    let timeout = Duration::from_secs(args.device.timeout);

    // Open the store
    let store = Store::open_default().context("Failed to open database")?;

    // Connect to device
    let device =
        crate::util::connect_device_with_progress(&device_address, timeout, true).await?;

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

