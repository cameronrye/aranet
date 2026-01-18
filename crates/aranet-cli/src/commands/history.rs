//! History command implementation.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aranet_core::HistoryOptions;
use time::OffsetDateTime;

use crate::cli::OutputFormat;
use crate::format::{FormatOptions, format_history_csv, format_history_json, format_history_text};
use crate::style;
use crate::util::{require_device_interactive, write_output};

/// Parse a date/time string in RFC3339 or YYYY-MM-DD format.
fn parse_datetime(s: &str) -> Result<OffsetDateTime> {
    // Try RFC3339 first (e.g., "2024-01-15T10:30:00Z")
    if let Ok(dt) = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return Ok(dt);
    }

    // Try YYYY-MM-DD format (treat as start of day in UTC)
    let format =
        time::format_description::parse("[year]-[month]-[day]").expect("valid format description");
    if let Ok(date) = time::Date::parse(s, &format) {
        return Ok(date.with_hms(0, 0, 0).expect("valid time").assume_utc());
    }

    bail!(
        "Invalid date format '{}'. Use RFC3339 (e.g., 2024-01-15T10:30:00Z) or YYYY-MM-DD",
        s
    )
}

/// Arguments for the history command.
pub struct HistoryArgs<'a> {
    pub device: Option<String>,
    pub count: u32,
    pub since: Option<String>,
    pub until: Option<String>,
    pub timeout: Duration,
    pub format: OutputFormat,
    pub output: Option<&'a PathBuf>,
    pub quiet: bool,
    pub opts: &'a FormatOptions,
}

pub async fn cmd_history(args: HistoryArgs<'_>) -> Result<()> {
    let HistoryArgs {
        device,
        count,
        since,
        until,
        timeout,
        format,
        output,
        quiet,
        opts,
    } = args;
    let identifier = require_device_interactive(device).await?;

    // Parse date filters upfront to fail fast
    let since_dt = since.as_ref().map(|s| parse_datetime(s)).transpose()?;
    let until_dt = until.as_ref().map(|s| parse_datetime(s)).transpose()?;

    // Set up progress bar for text output
    let show_progress = !quiet && matches!(format, OutputFormat::Text);

    // Connect to device (with its own spinner if show_progress is true)
    let device =
        crate::util::connect_device_with_progress(&identifier, timeout, show_progress).await?;

    // Create progress bar for download phase
    let pb = if show_progress {
        let pb = style::download_progress_bar();
        pb.set_message("Downloading history...");
        Some(pb)
    } else {
        None
    };

    // Clone progress bar for callback (ProgressBar uses Arc internally)
    let pb_for_callback = pb.clone();

    let history_options = if let Some(pb_callback) = pb_for_callback {
        HistoryOptions::default().with_progress(move |progress| {
            let percent = (progress.overall_progress * 100.0) as u64;
            pb_callback.set_position(percent);
            pb_callback.set_message(format!(
                "Downloading {:?} ({}/{})",
                progress.current_param, progress.param_index, progress.total_params
            ));
        })
    } else {
        HistoryOptions::default()
    };

    let history = device
        .download_history_with_options(history_options)
        .await
        .context("Failed to download history")?;

    if let Some(pb) = pb {
        pb.finish_with_message("Download complete");
    }

    device.disconnect().await.ok();

    // Apply date filters
    let history: Vec<_> = history
        .into_iter()
        .filter(|r| {
            if let Some(since) = since_dt
                && r.timestamp < since
            {
                return false;
            }
            if let Some(until) = until_dt
                && r.timestamp > until
            {
                return false;
            }
            true
        })
        .collect();

    // Apply count limit if specified (0 means all)
    let history: Vec<_> = if count > 0 {
        history.into_iter().take(count as usize).collect()
    } else {
        history
    };

    if !quiet && matches!(format, OutputFormat::Text) {
        eprintln!("Downloaded {} records.", history.len());
    }

    let content = match format {
        OutputFormat::Json => format_history_json(&history, opts)?,
        OutputFormat::Text => format_history_text(&history, opts),
        OutputFormat::Csv => format_history_csv(&history, opts),
    };

    write_output(output, &content)?;
    Ok(())
}
