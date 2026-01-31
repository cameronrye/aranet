//! History data export functionality for the Aranet GUI.
//!
//! This module provides CSV and JSON export functions for sensor history data.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing::{debug, info};

/// Export result containing the filename on success.
pub type ExportResult = io::Result<String>;

/// Generate export path from configuration and device info.
///
/// Uses the configured export directory if set, otherwise falls back to
/// downloads or documents directory.
pub fn generate_export_path(
    export_directory: &str,
    device_name: &str,
    format: &str,
) -> (PathBuf, String) {
    // Generate filename with timestamp
    let timestamp = time::OffsetDateTime::now_utc()
        .format(
            &time::format_description::parse("[year][month][day]_[hour][minute][second]").unwrap(),
        )
        .unwrap_or_else(|_| "export".to_string());
    let safe_device_name = device_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>();
    let filename = format!("aranet_{}_{}.{}", safe_device_name, timestamp, format);

    // Use configured export directory if set, otherwise fallback to downloads/documents
    let export_dir = if !export_directory.is_empty() {
        PathBuf::from(export_directory)
    } else {
        dirs::download_dir()
            .or_else(dirs::document_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    let export_path = export_dir.join(&filename);

    (export_path, filename)
}

/// Export history records to a file (CSV or JSON).
///
/// Returns the filename on success for display in notifications.
pub fn export_history(
    records: &[&aranet_types::HistoryRecord],
    export_directory: &str,
    device_name: &str,
    format: &str,
) -> ExportResult {
    let (export_path, filename) = generate_export_path(export_directory, device_name, format);

    let result = match format {
        "csv" => export_to_csv(records, &export_path),
        "json" => export_to_json(records, &export_path),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unknown format",
        )),
    };

    match &result {
        Ok(_) => {
            info!("History exported to {:?}", export_path);
        }
        Err(e) => {
            debug!("Export failed: {}", e);
        }
    }

    result.map(|_| filename)
}

/// Export records to CSV format.
pub fn export_to_csv(records: &[&aranet_types::HistoryRecord], path: &Path) -> io::Result<()> {
    let mut file = File::create(path)?;

    // Write header
    writeln!(
        file,
        "timestamp,co2_ppm,temperature_c,humidity_pct,pressure_hpa,radon_bq,radiation_usv"
    )?;

    // Write records
    for record in records {
        let ts = record
            .timestamp
            .format(&time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_default();
        let co2 = if record.co2 > 0 {
            record.co2.to_string()
        } else {
            String::new()
        };
        let temp = format!("{:.1}", record.temperature);
        let humidity = record.humidity.to_string();
        let pressure = if record.pressure > 0.0 {
            format!("{:.1}", record.pressure)
        } else {
            String::new()
        };
        let radon = record.radon.map(|r| r.to_string()).unwrap_or_default();
        let radiation = record
            .radiation_rate
            .map(|r| format!("{:.3}", r))
            .unwrap_or_default();

        writeln!(
            file,
            "{},{},{},{},{},{},{}",
            ts, co2, temp, humidity, pressure, radon, radiation
        )?;
    }

    Ok(())
}

/// Export records to JSON format.
pub fn export_to_json(records: &[&aranet_types::HistoryRecord], path: &Path) -> io::Result<()> {
    let mut file = File::create(path)?;

    // Build JSON array
    let json_records: Vec<serde_json::Value> = records
        .iter()
        .map(|r| {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "timestamp".to_string(),
                serde_json::Value::String(
                    r.timestamp
                        .format(&time::format_description::well_known::Iso8601::DEFAULT)
                        .unwrap_or_default(),
                ),
            );
            if r.co2 > 0 {
                obj.insert("co2_ppm".to_string(), serde_json::json!(r.co2));
            }
            obj.insert(
                "temperature_c".to_string(),
                serde_json::json!(
                    format!("{:.1}", r.temperature)
                        .parse::<f32>()
                        .unwrap_or(r.temperature)
                ),
            );
            obj.insert("humidity_pct".to_string(), serde_json::json!(r.humidity));
            if r.pressure > 0.0 {
                obj.insert(
                    "pressure_hpa".to_string(),
                    serde_json::json!(
                        format!("{:.1}", r.pressure)
                            .parse::<f32>()
                            .unwrap_or(r.pressure)
                    ),
                );
            }
            if let Some(radon) = r.radon {
                obj.insert("radon_bq".to_string(), serde_json::json!(radon));
            }
            if let Some(radiation) = r.radiation_rate {
                obj.insert("radiation_usv".to_string(), serde_json::json!(radiation));
            }
            serde_json::Value::Object(obj)
        })
        .collect();

    let json = serde_json::json!({
        "exported_at": time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Iso8601::DEFAULT)
            .unwrap_or_default(),
        "record_count": records.len(),
        "records": json_records
    });

    let json_str = serde_json::to_string_pretty(&json).map_err(io::Error::other)?;
    file.write_all(json_str.as_bytes())?;

    Ok(())
}
