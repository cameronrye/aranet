//! Sync command - download and cache device history.

use std::time::Duration;

use anyhow::{Context, Result};
use aranet_core::HistoryOptions;
use aranet_store::Store;
use indicatif::ProgressBar;
use serde::Serialize;
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

#[derive(Debug, Serialize)]
struct SingleDeviceSyncSummary {
    device: String,
    name: String,
    status: &'static str,
    downloaded: usize,
    inserted: usize,
    total_cached: u64,
    total_on_device: u16,
}

impl SingleDeviceSyncSummary {
    fn synced(
        device: String,
        name: String,
        downloaded: usize,
        inserted: usize,
        total_cached: u64,
        total_on_device: u16,
    ) -> Self {
        Self {
            device,
            name,
            status: "synced",
            downloaded,
            inserted,
            total_cached,
            total_on_device,
        }
    }

    fn up_to_date(device: String, name: String, total_cached: u64, total_on_device: u16) -> Self {
        Self {
            device,
            name,
            status: "up_to_date",
            downloaded: 0,
            inserted: 0,
            total_cached,
            total_on_device,
        }
    }
}

fn build_history_options(start_index: u16, progress: Option<ProgressBar>) -> HistoryOptions {
    let options = HistoryOptions::default().start_index(start_index);

    if let Some(pb_for_callback) = progress {
        options.with_progress(move |progress| {
            let percent = (progress.overall_progress * 100.0) as u64;
            pb_for_callback.set_position(percent);
            pb_for_callback.set_message(format!(
                "Downloading {:?} ({}/{})",
                progress.current_param, progress.param_index, progress.total_params
            ));
        })
    } else {
        options
    }
}

fn render_single_device_sync_json(summary: &SingleDeviceSyncSummary) -> Result<String> {
    Ok(serde_json::to_string_pretty(summary)?)
}

fn render_sync_all_json(
    total_devices: usize,
    successful: usize,
    failed: usize,
    total_downloaded: usize,
    total_inserted: usize,
    devices: Vec<serde_json::Value>,
) -> Result<String> {
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "total_devices": total_devices,
        "successful": successful,
        "failed": failed,
        "total_downloaded": total_downloaded,
        "total_inserted": total_inserted,
        "devices": devices,
    }))?)
}

fn print_single_device_sync_summary(
    format: OutputFormat,
    summary: &SingleDeviceSyncSummary,
) -> Result<()> {
    match format {
        OutputFormat::Json => {
            println!("{}", render_single_device_sync_json(summary)?);
        }
        _ if summary.status == "up_to_date" => {
            println!("Already up to date - no new readings to sync");
            println!("Total cached: {}", summary.total_cached);
            println!("Total on device: {}", summary.total_on_device);
        }
        _ => {
            println!("Downloaded: {} records", summary.downloaded);
            println!("New records: {}", summary.inserted);
            println!("Total cached: {}", summary.total_cached);
            println!("Total on device: {}", summary.total_on_device);
        }
    }

    Ok(())
}

/// Execute the sync command.
pub async fn cmd_sync(args: SyncArgs, config: &Config) -> Result<()> {
    // Open the store
    let store = Store::open_default().context("Failed to open database")?;
    let timeout_secs = crate::config::resolve_timeout(args.device.timeout, config, 30);

    // If --all flag is set, sync all known devices
    if args.all {
        return sync_all_devices(&store, args.format, args.full, timeout_secs).await;
    }

    // Resolve device address from args, env, or config
    let device_input = args.device.device.clone().or_else(|| config.device.clone());
    let device_address = require_device_interactive(device_input).await?;
    let timeout = Duration::from_secs(timeout_secs);

    // Connect to device
    let device = crate::util::connect_device_with_progress(&device_address, timeout, true).await?;
    let sync_result: Result<SingleDeviceSyncSummary> = async {
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
                let total_cached = store.count_history(Some(&device_address))?;
                return Ok(SingleDeviceSyncSummary::up_to_date(
                    device_address.clone(),
                    device_name,
                    total_cached,
                    total_on_device,
                ));
            }
            info!(
                "Incremental sync: downloading records {} to {}",
                start, total_on_device
            );
            start
        };

        let records_to_download = total_on_device.saturating_sub(start_index) + 1;
        eprintln!(
            "Syncing {} ({} records)...",
            device_name, records_to_download
        );

        let pb = if matches!(args.format, OutputFormat::Json) {
            None
        } else {
            let pb = style::download_progress_bar();
            pb.set_message("Downloading history...");
            Some(pb)
        };

        let history_opts = build_history_options(start_index, pb.clone());
        let history_result = device
            .download_history_with_options(history_opts)
            .await
            .context("Failed to download history");

        if let Some(pb) = pb {
            if history_result.is_ok() {
                pb.finish_with_message("Download complete");
            } else {
                pb.finish_and_clear();
            }
        }

        let history = history_result?;

        // Store history records
        let inserted = store.insert_history(&device_address, &history)?;

        // Update sync state
        store.update_sync_state(&device_address, total_on_device, total_on_device)?;

        let total_cached = store.count_history(Some(&device_address))?;
        Ok(SingleDeviceSyncSummary::synced(
            device_address.clone(),
            device_name,
            history.len(),
            inserted,
            total_cached,
            total_on_device,
        ))
    }
    .await;
    crate::util::disconnect_device(&device).await;
    let summary = sync_result?;
    print_single_device_sync_summary(args.format, &summary)
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
        return match format {
            OutputFormat::Json => {
                println!("{}", render_sync_all_json(0, 0, 0, 0, 0, Vec::new())?);
                Ok(())
            }
            _ => {
                println!(
                    "No devices found in database. Run 'aranet scan' first to discover devices."
                );
                Ok(())
            }
        };
    }

    let timeout = Duration::from_secs(timeout_secs);
    let total_devices = devices.len();
    let mut successful = 0;
    let mut failed = 0;
    let mut total_downloaded = 0usize;
    let mut total_inserted = 0usize;
    let mut results = Vec::new();

    eprintln!("Syncing {} device(s)...\n", total_devices);

    for (idx, stored_device) in devices.iter().enumerate() {
        let device_name = stored_device
            .name
            .as_ref()
            .unwrap_or(&stored_device.id)
            .clone();
        eprintln!("[{}/{}] Syncing {}...", idx + 1, total_devices, device_name);

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
                eprintln!(
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
                eprintln!("  [FAIL] {}\n", e);
            }
        }
    }

    // Summary
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                render_sync_all_json(
                    total_devices,
                    successful,
                    failed,
                    total_downloaded,
                    total_inserted,
                    results,
                )?
            );
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
    let sync_result: Result<(usize, usize)> = async {
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
                return Ok((0, 0)); // Already up to date
            }
            info!(
                "{}: Incremental sync: downloading records {} to {}",
                device_name, start, total_on_device
            );
            start
        };

        // Download history (without progress bar to keep output clean for multiple devices)
        let history_opts = build_history_options(start_index, None);
        let history = device
            .download_history_with_options(history_opts)
            .await
            .context("Failed to download history")?;

        // Store history records
        let inserted = store.insert_history(device_address, &history)?;

        // Update sync state
        store.update_sync_state(device_address, total_on_device, total_on_device)?;

        Ok((history.len(), inserted))
    }
    .await;
    crate::util::disconnect_device(&device).await;

    sync_result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_history_options_sets_start_index_without_progress() {
        let options = build_history_options(42, None);
        assert_eq!(options.start_index, Some(42));
        assert!(options.progress_callback.is_none());
    }

    #[test]
    fn test_build_history_options_sets_start_index_with_progress() {
        let options = build_history_options(7, Some(ProgressBar::hidden()));
        assert_eq!(options.start_index, Some(7));
        assert!(options.progress_callback.is_some());
    }

    #[test]
    fn test_render_single_device_sync_json_is_valid() {
        let json = render_single_device_sync_json(&SingleDeviceSyncSummary::up_to_date(
            "device-1".to_string(),
            "Office".to_string(),
            12,
            12,
        ))
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["device"], "device-1");
        assert_eq!(parsed["status"], "up_to_date");
        assert_eq!(parsed["downloaded"], 0);
    }

    #[test]
    fn test_render_sync_all_json_is_valid() {
        let json = render_sync_all_json(2, 1, 1, 10, 4, Vec::new()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["total_devices"], 2);
        assert_eq!(parsed["successful"], 1);
        assert_eq!(parsed["failed"], 1);
    }
}
