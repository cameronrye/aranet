//! History command implementation.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use aranet_core::HistoryOptions;
use aranet_store::{HistoryQuery, Store};
use time::OffsetDateTime;

use crate::cli::OutputFormat;
use crate::format::{FormatOptions, format_history_csv, format_history_json, format_history_text};
use crate::style;
use crate::util::{require_device_interactive, write_output};

/// Options for querying history from the cache.
struct CacheQueryOptions<'a> {
    device: Option<String>,
    count: u32,
    since_dt: Option<OffsetDateTime>,
    until_dt: Option<OffsetDateTime>,
    format: OutputFormat,
    output: Option<&'a PathBuf>,
    quiet: bool,
    opts: &'a FormatOptions,
}

/// Parse a date/time string in various formats:
/// - RFC3339: "2024-01-15T10:30:00Z"
/// - YYYY-MM-DD: "2024-01-15"
/// - Relative: "today", "yesterday", "7d", "24h", "1w"
fn parse_datetime(s: &str) -> Result<OffsetDateTime> {
    let s_lower = s.to_lowercase();
    let now = OffsetDateTime::now_utc();

    // Handle relative date keywords
    match s_lower.as_str() {
        "now" => return Ok(now),
        "today" => {
            let today = now.date();
            return Ok(today.with_hms(0, 0, 0).expect("valid time").assume_utc());
        }
        "yesterday" => {
            let yesterday = now.date() - time::Duration::days(1);
            return Ok(yesterday
                .with_hms(0, 0, 0)
                .expect("valid time")
                .assume_utc());
        }
        _ => {}
    }

    // Handle relative duration patterns: "7d", "24h", "1w", "30m"
    if let Some(duration) = parse_relative_duration(&s_lower) {
        return Ok(now - duration);
    }

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
        "Invalid date format '{}'. Use RFC3339 (2024-01-15T10:30:00Z), YYYY-MM-DD, \
         or relative (today, yesterday, 7d, 24h, 1w)",
        s
    )
}

/// Parse relative duration strings like "7d", "24h", "1w", "30m"
fn parse_relative_duration(s: &str) -> Option<time::Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Find where the number ends and the unit begins
    let (num_str, unit) = s.split_at(s.chars().take_while(|c| c.is_ascii_digit()).count());

    let num: i64 = num_str.parse().ok()?;
    if num <= 0 {
        return None;
    }

    match unit.trim() {
        "m" | "min" | "mins" | "minute" | "minutes" => Some(time::Duration::minutes(num)),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(time::Duration::hours(num)),
        "d" | "day" | "days" => Some(time::Duration::days(num)),
        "w" | "wk" | "wks" | "week" | "weeks" => Some(time::Duration::weeks(num)),
        _ => None,
    }
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
    pub cache: bool,
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
        cache,
    } = args;

    // Parse date filters upfront to fail fast
    let since_dt = since.as_ref().map(|s| parse_datetime(s)).transpose()?;
    let until_dt = until.as_ref().map(|s| parse_datetime(s)).transpose()?;

    // If --cache flag is set, read from local database instead of device
    if cache {
        return cmd_history_from_cache(CacheQueryOptions {
            device,
            count,
            since_dt,
            until_dt,
            format,
            output,
            quiet,
            opts,
        });
    }

    let identifier = require_device_interactive(device).await?;

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

    let device_id = device.address().to_string();
    let history = device
        .download_history_with_options(history_options)
        .await
        .context("Failed to download history")?;

    if let Some(pb) = pb {
        pb.finish_with_message("Download complete");
    }

    device.disconnect().await.ok();

    // Save history to store (unified data architecture)
    crate::util::save_history_to_store(&device_id, &history);

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

    // Reverse to show newest first (device sends oldest first)
    let mut history = history;
    history.reverse();

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

/// Read history from local cache instead of connecting to the device.
fn cmd_history_from_cache(options: CacheQueryOptions<'_>) -> Result<()> {
    let CacheQueryOptions {
        device,
        count,
        since_dt,
        until_dt,
        format,
        output,
        quiet,
        opts,
    } = options;

    let store = Store::open_default().context("Failed to open database")?;

    // For cache mode, we need a device identifier
    let device_id = match device {
        Some(id) => id,
        None => {
            // Try to find a default device or list available devices
            let devices = store.list_devices()?;
            if devices.is_empty() {
                bail!("No devices in cache. Run 'aranet sync' first to cache device data.");
            }
            if devices.len() == 1 {
                devices[0].id.clone()
            } else {
                eprintln!("Multiple devices in cache. Please specify one with --device:");
                for d in &devices {
                    let name = d.name.as_deref().unwrap_or("(unnamed)");
                    eprintln!("  {} - {}", d.id, name);
                }
                bail!("Device required when multiple devices are cached");
            }
        }
    };

    // Build query
    let mut query = HistoryQuery::new().device(&device_id);

    if count > 0 {
        query = query.limit(count);
    }

    if let Some(since) = since_dt {
        query = query.since(since);
    }

    if let Some(until) = until_dt {
        query = query.until(until);
    }

    let records = store.query_history(&query)?;

    if records.is_empty() {
        if !quiet {
            eprintln!(
                "No history records found for {}. Run 'aranet sync' to cache device history.",
                device_id
            );
        }
        return Ok(());
    }

    // Convert to HistoryRecord for formatting
    let history: Vec<_> = records.iter().map(|r| r.to_history()).collect();

    if !quiet && matches!(format, OutputFormat::Text) {
        eprintln!("Retrieved {} records from cache.", history.len());
    }

    let content = match format {
        OutputFormat::Json => format_history_json(&history, opts)?,
        OutputFormat::Text => format_history_text(&history, opts),
        OutputFormat::Csv => format_history_csv(&history, opts),
    };

    write_output(output, &content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // parse_relative_duration tests
    // ========================================================================

    #[test]
    fn test_parse_relative_duration_empty() {
        assert!(parse_relative_duration("").is_none());
        assert!(parse_relative_duration("   ").is_none());
    }

    #[test]
    fn test_parse_relative_duration_minutes() {
        assert_eq!(
            parse_relative_duration("30m"),
            Some(time::Duration::minutes(30))
        );
        assert_eq!(
            parse_relative_duration("30min"),
            Some(time::Duration::minutes(30))
        );
        assert_eq!(
            parse_relative_duration("30mins"),
            Some(time::Duration::minutes(30))
        );
        assert_eq!(
            parse_relative_duration("1minute"),
            Some(time::Duration::minutes(1))
        );
        assert_eq!(
            parse_relative_duration("5minutes"),
            Some(time::Duration::minutes(5))
        );
    }

    #[test]
    fn test_parse_relative_duration_hours() {
        assert_eq!(
            parse_relative_duration("24h"),
            Some(time::Duration::hours(24))
        );
        assert_eq!(
            parse_relative_duration("1hr"),
            Some(time::Duration::hours(1))
        );
        assert_eq!(
            parse_relative_duration("2hrs"),
            Some(time::Duration::hours(2))
        );
        assert_eq!(
            parse_relative_duration("1hour"),
            Some(time::Duration::hours(1))
        );
        assert_eq!(
            parse_relative_duration("12hours"),
            Some(time::Duration::hours(12))
        );
    }

    #[test]
    fn test_parse_relative_duration_days() {
        assert_eq!(parse_relative_duration("7d"), Some(time::Duration::days(7)));
        assert_eq!(
            parse_relative_duration("1day"),
            Some(time::Duration::days(1))
        );
        assert_eq!(
            parse_relative_duration("30days"),
            Some(time::Duration::days(30))
        );
    }

    #[test]
    fn test_parse_relative_duration_weeks() {
        assert_eq!(
            parse_relative_duration("1w"),
            Some(time::Duration::weeks(1))
        );
        assert_eq!(
            parse_relative_duration("2wk"),
            Some(time::Duration::weeks(2))
        );
        assert_eq!(
            parse_relative_duration("4wks"),
            Some(time::Duration::weeks(4))
        );
        assert_eq!(
            parse_relative_duration("1week"),
            Some(time::Duration::weeks(1))
        );
        assert_eq!(
            parse_relative_duration("2weeks"),
            Some(time::Duration::weeks(2))
        );
    }

    #[test]
    fn test_parse_relative_duration_invalid() {
        // Invalid unit
        assert!(parse_relative_duration("7x").is_none());
        assert!(parse_relative_duration("7y").is_none());
        assert!(parse_relative_duration("7s").is_none()); // seconds not supported

        // No number
        assert!(parse_relative_duration("d").is_none());
        assert!(parse_relative_duration("days").is_none());

        // Zero or negative not allowed
        assert!(parse_relative_duration("0d").is_none());
        assert!(parse_relative_duration("-1d").is_none());
    }

    // ========================================================================
    // parse_datetime tests
    // ========================================================================

    #[test]
    fn test_parse_datetime_now() {
        let before = OffsetDateTime::now_utc();
        let result = parse_datetime("now").unwrap();
        let after = OffsetDateTime::now_utc();

        assert!(result >= before);
        assert!(result <= after);
    }

    #[test]
    fn test_parse_datetime_now_case_insensitive() {
        // Should work with any case
        assert!(parse_datetime("NOW").is_ok());
        assert!(parse_datetime("Now").is_ok());
    }

    #[test]
    fn test_parse_datetime_today() {
        let result = parse_datetime("today").unwrap();
        let now = OffsetDateTime::now_utc();

        assert_eq!(result.date(), now.date());
        assert_eq!(result.hour(), 0);
        assert_eq!(result.minute(), 0);
        assert_eq!(result.second(), 0);
    }

    #[test]
    fn test_parse_datetime_yesterday() {
        let result = parse_datetime("yesterday").unwrap();
        let now = OffsetDateTime::now_utc();
        let expected_date = now.date() - time::Duration::days(1);

        assert_eq!(result.date(), expected_date);
        assert_eq!(result.hour(), 0);
        assert_eq!(result.minute(), 0);
        assert_eq!(result.second(), 0);
    }

    #[test]
    fn test_parse_datetime_rfc3339() {
        let result = parse_datetime("2024-01-15T10:30:00Z").unwrap();

        assert_eq!(result.year(), 2024);
        assert_eq!(result.month(), time::Month::January);
        assert_eq!(result.day(), 15);
        assert_eq!(result.hour(), 10);
        assert_eq!(result.minute(), 30);
        assert_eq!(result.second(), 0);
    }

    #[test]
    fn test_parse_datetime_date_only() {
        let result = parse_datetime("2024-01-15").unwrap();

        assert_eq!(result.year(), 2024);
        assert_eq!(result.month(), time::Month::January);
        assert_eq!(result.day(), 15);
        // Date-only should be start of day
        assert_eq!(result.hour(), 0);
        assert_eq!(result.minute(), 0);
        assert_eq!(result.second(), 0);
    }

    #[test]
    fn test_parse_datetime_relative_days() {
        let before = OffsetDateTime::now_utc();
        let result = parse_datetime("7d").unwrap();
        let after = OffsetDateTime::now_utc();

        // Result should be approximately 7 days ago
        let expected_min = before - time::Duration::days(7);
        let expected_max = after - time::Duration::days(7);

        assert!(result >= expected_min);
        assert!(result <= expected_max);
    }

    #[test]
    fn test_parse_datetime_relative_hours() {
        let before = OffsetDateTime::now_utc();
        let result = parse_datetime("24h").unwrap();
        let after = OffsetDateTime::now_utc();

        let expected_min = before - time::Duration::hours(24);
        let expected_max = after - time::Duration::hours(24);

        assert!(result >= expected_min);
        assert!(result <= expected_max);
    }

    #[test]
    fn test_parse_datetime_invalid() {
        assert!(parse_datetime("invalid").is_err());
        assert!(parse_datetime("2024/01/15").is_err()); // Wrong format
        assert!(parse_datetime("").is_err());
        assert!(parse_datetime("not-a-date").is_err());
    }

    #[test]
    fn test_parse_datetime_error_message() {
        let result = parse_datetime("invalid");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Invalid date format"));
        assert!(err.to_string().contains("invalid"));
    }
}
