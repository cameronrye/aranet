//! Cache command - query local database.

use std::io::{Read, Write};

use anyhow::{Context, Result};
use aranet_store::{HistoryQuery, Store};
use time::OffsetDateTime;

use crate::cli::{CacheAction, ExportFormat, OutputArgs, OutputFormat};
use crate::config::Config;
use crate::format::{FormatOptions, format_history_csv, format_history_json, format_history_text};

/// Execute the cache command.
pub fn cmd_cache(action: CacheAction, config: &Config) -> Result<()> {
    let store = Store::open_default().context("Failed to open database")?;

    match action {
        CacheAction::Devices => list_devices(&store),
        CacheAction::Stats { device } => show_stats(&store, device.as_deref()),
        CacheAction::History {
            device,
            count,
            since,
            until,
            output,
        } => query_history(&store, &device, count, since, until, output, config),
        CacheAction::Aggregate {
            device,
            since,
            until,
            format,
        } => show_aggregate_stats(&store, &device, since, until, format),
        CacheAction::Export {
            device,
            format,
            output,
            since,
            until,
        } => export_history(&store, &device, format, output, since, until),
        CacheAction::Info => show_info(),
        CacheAction::Import { format, input } => import_history(&store, format, input),
    }
}

fn list_devices(store: &Store) -> Result<()> {
    let devices = store.list_devices()?;

    if devices.is_empty() {
        println!("No devices in cache. Run 'aranet sync' to cache device data.");
        return Ok(());
    }

    println!("Cached devices:\n");
    for device in devices {
        let name = device.name.as_deref().unwrap_or("(unnamed)");
        let device_type = device
            .device_type
            .map(|dt| format!("{:?}", dt))
            .unwrap_or_else(|| "Unknown".to_string());
        println!("  {} - {} ({})", device.id, name, device_type);
        println!(
            "    First seen: {}",
            device
                .first_seen
                .format(&time::format_description::well_known::Rfc3339)?
        );
        println!(
            "    Last seen:  {}",
            device
                .last_seen
                .format(&time::format_description::well_known::Rfc3339)?
        );
        println!();
    }

    Ok(())
}

fn show_stats(store: &Store, device_id: Option<&str>) -> Result<()> {
    let total_readings = store.count_readings(device_id)?;
    let total_history = store.count_history(device_id)?;

    match device_id {
        Some(id) => {
            println!("Cache statistics for {}:", id);
            if let Some(state) = store.get_sync_state(id)? {
                if let Some(last_sync) = state.last_sync_at {
                    println!(
                        "  Last sync: {}",
                        last_sync.format(&time::format_description::well_known::Rfc3339)?
                    );
                }
                if let Some(idx) = state.last_history_index {
                    println!("  Last history index: {}", idx);
                }
            }
        }
        None => {
            println!("Cache statistics (all devices):");
        }
    }

    println!("  Readings: {}", total_readings);
    println!("  History records: {}", total_history);

    Ok(())
}

fn query_history(
    store: &Store,
    device_id: &str,
    count: u32,
    since: Option<String>,
    until: Option<String>,
    output: OutputArgs,
    config: &Config,
) -> Result<()> {
    let mut query = HistoryQuery::new().device(device_id);

    if count > 0 {
        query = query.limit(count);
    }

    if let Some(since_str) = since {
        let ts = parse_datetime(&since_str)?;
        query = query.since(ts);
    }

    if let Some(until_str) = until {
        let ts = parse_datetime(&until_str)?;
        query = query.until(ts);
    }

    let records = store.query_history(&query)?;

    if records.is_empty() {
        println!("No history records found for {}", device_id);
        return Ok(());
    }

    // Convert to HistoryRecord for formatting
    let history: Vec<_> = records.iter().map(|r| r.to_history()).collect();

    let fahrenheit = output.resolve_fahrenheit(config.fahrenheit);
    let bq = output.resolve_bq(config.bq);
    let inhg = output.resolve_inhg(config.inhg);

    let opts = FormatOptions::new(false, fahrenheit, crate::cli::StyleMode::Rich)
        .with_no_header(output.no_header)
        .with_bq(bq)
        .with_inhg(inhg);

    let formatted = match output.format {
        crate::cli::OutputFormat::Json => format_history_json(&history, &opts)?,
        crate::cli::OutputFormat::Csv => format_history_csv(&history, &opts),
        crate::cli::OutputFormat::Text => format_history_text(&history, &opts),
    };

    print!("{}", formatted);

    Ok(())
}

fn show_info() -> Result<()> {
    let db_path = aranet_store::default_db_path();
    println!("Database path: {}", db_path.display());

    if db_path.exists() {
        let metadata = std::fs::metadata(&db_path)?;
        let size_kb = metadata.len() / 1024;
        println!("Database size: {} KB", size_kb);
    } else {
        println!("Database does not exist yet. Run 'aranet sync' to create it.");
    }

    Ok(())
}

fn parse_datetime(s: &str) -> Result<OffsetDateTime> {
    // Try RFC3339 first
    if let Ok(dt) = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return Ok(dt);
    }

    // Try date only (YYYY-MM-DD)
    let format = time::format_description::parse("[year]-[month]-[day]")?;
    if let Ok(date) = time::Date::parse(s, &format) {
        return Ok(date.with_hms(0, 0, 0)?.assume_utc());
    }

    anyhow::bail!("Invalid date/time format: {}. Use RFC3339 or YYYY-MM-DD", s)
}

fn show_aggregate_stats(
    store: &Store,
    device_id: &str,
    since: Option<String>,
    until: Option<String>,
    format: OutputFormat,
) -> Result<()> {
    let mut query = HistoryQuery::new().device(device_id);

    if let Some(since_str) = since {
        let ts = parse_datetime(&since_str)?;
        query = query.since(ts);
    }

    if let Some(until_str) = until {
        let ts = parse_datetime(&until_str)?;
        query = query.until(ts);
    }

    let stats = store.history_stats(&query)?;

    if stats.count == 0 {
        println!("No history records found for {}", device_id);
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&stats)?;
            println!("{}", json);
        }
        _ => {
            println!("Aggregate statistics for {}:", device_id);
            println!("  Records: {}", stats.count);

            if let Some((start, end)) = stats.time_range {
                let rfc3339 = time::format_description::well_known::Rfc3339;
                println!(
                    "  Time range: {} to {}",
                    start.format(&rfc3339)?,
                    end.format(&rfc3339)?
                );
            }

            println!();
            println!("  {:12} {:>10} {:>10} {:>10}", "", "Min", "Max", "Avg");
            println!("  {}", "-".repeat(46));

            if let (Some(min), Some(max), Some(avg)) =
                (stats.min.co2, stats.max.co2, stats.avg.co2)
            {
                println!(
                    "  {:12} {:>10.0} {:>10.0} {:>10.1} ppm",
                    "CO2", min, max, avg
                );
            }

            if let (Some(min), Some(max), Some(avg)) =
                (stats.min.temperature, stats.max.temperature, stats.avg.temperature)
            {
                println!(
                    "  {:12} {:>10.1} {:>10.1} {:>10.1} C",
                    "Temperature", min, max, avg
                );
            }

            if let (Some(min), Some(max), Some(avg)) =
                (stats.min.pressure, stats.max.pressure, stats.avg.pressure)
            {
                println!(
                    "  {:12} {:>10.1} {:>10.1} {:>10.1} hPa",
                    "Pressure", min, max, avg
                );
            }

            if let (Some(min), Some(max), Some(avg)) =
                (stats.min.humidity, stats.max.humidity, stats.avg.humidity)
            {
                println!(
                    "  {:12} {:>10.0} {:>10.0} {:>10.1} %",
                    "Humidity", min, max, avg
                );
            }

            if let (Some(min), Some(max), Some(avg)) =
                (stats.min.radon, stats.max.radon, stats.avg.radon)
            {
                println!(
                    "  {:12} {:>10.0} {:>10.0} {:>10.1} Bq/m3",
                    "Radon", min, max, avg
                );
            }
        }
    }

    Ok(())
}

fn export_history(
    store: &Store,
    device_id: &str,
    format: ExportFormat,
    output: Option<std::path::PathBuf>,
    since: Option<String>,
    until: Option<String>,
) -> Result<()> {
    let mut query = HistoryQuery::new().device(device_id);

    if let Some(since_str) = since {
        let ts = parse_datetime(&since_str)?;
        query = query.since(ts);
    }

    if let Some(until_str) = until {
        let ts = parse_datetime(&until_str)?;
        query = query.until(ts);
    }

    let content = match format {
        ExportFormat::Csv => store.export_history_csv(&query)?,
        ExportFormat::Json => store.export_history_json(&query)?,
    };

    match output {
        Some(path) => {
            let mut file = std::fs::File::create(&path)
                .with_context(|| format!("Failed to create file: {}", path.display()))?;
            file.write_all(content.as_bytes())?;
            println!("Exported to {}", path.display());
        }
        None => {
            print!("{}", content);
        }
    }

    Ok(())
}

fn import_history(
    store: &Store,
    format: ExportFormat,
    input: Option<std::path::PathBuf>,
) -> Result<()> {
    // Read input data
    let data = match input {
        Some(path) => {
            std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read file: {}", path.display()))?
        }
        None => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .context("Failed to read from stdin")?;
            buffer
        }
    };

    // Import based on format
    let result = match format {
        ExportFormat::Csv => store.import_history_csv(&data)?,
        ExportFormat::Json => store.import_history_json(&data)?,
    };

    // Report results
    println!("Import complete:");
    println!("  Total records: {}", result.total);
    println!("  Imported: {}", result.imported);
    println!("  Skipped (duplicates): {}", result.skipped);

    if !result.errors.is_empty() {
        println!("\nErrors ({}):", result.errors.len());
        for (i, err) in result.errors.iter().enumerate().take(10) {
            println!("  {}", err);
            if i == 9 && result.errors.len() > 10 {
                println!("  ... and {} more errors", result.errors.len() - 10);
            }
        }
    }

    Ok(())
}
