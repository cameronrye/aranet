//! Report command - generate data summaries.

use anyhow::{Context, Result, bail};
use aranet_store::{HistoryQuery, Store};
use time::{Duration, OffsetDateTime};

use crate::cli::{ReportFormat, ReportOutputArgs, ReportPeriod};
use crate::config::Config;

/// Execute the report command.
pub fn cmd_report(
    device: Option<String>,
    all: bool,
    period: ReportPeriod,
    format: Option<ReportFormat>,
    output: ReportOutputArgs,
    config: &Config,
) -> Result<()> {
    let store = Store::open_default().context("Failed to open database")?;

    let now = OffsetDateTime::now_utc();
    let (since, period_label) = match period {
        ReportPeriod::Daily => (now - Duration::hours(24), "Daily"),
        ReportPeriod::Weekly => (now - Duration::days(7), "Weekly"),
        ReportPeriod::Monthly => (now - Duration::days(30), "Monthly"),
    };

    let use_json = matches!(format, Some(ReportFormat::Json));
    let devices = resolve_report_devices(&store, device, all)?;

    if devices.is_empty() {
        if use_json {
            println!("[]");
        } else {
            println!("No devices found. Run 'aranet sync' first.");
        }
        return Ok(());
    }

    if use_json {
        let mut reports = Vec::new();
        for device_id in &devices {
            if let Some(report) = generate_device_report(&store, device_id, since)? {
                reports.push(report);
            }
        }
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }

    println!("{} Report", period_label);
    println!("{}", "=".repeat(60));
    println!(
        "Period: {} to {}",
        since
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        now.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    );
    println!();

    let fahrenheit = output.resolve_fahrenheit(config.fahrenheit);
    let inhg = output.resolve_inhg(config.inhg);
    let bq = output.resolve_bq(config.bq);

    for device_id in &devices {
        if let Some(report) = generate_device_report(&store, device_id, since)? {
            print_device_report(&report, fahrenheit, inhg, bq);
        }
    }

    Ok(())
}

fn resolve_report_devices(store: &Store, device: Option<String>, all: bool) -> Result<Vec<String>> {
    if all {
        return Ok(store.list_devices()?.into_iter().map(|d| d.id).collect());
    }

    if let Some(device) = device {
        return Ok(vec![device]);
    }

    let devices = store.list_devices()?;
    match devices.as_slice() {
        [] => Ok(Vec::new()),
        [device] => Ok(vec![device.id.clone()]),
        _ => bail!("Multiple cached devices found. Use --device <ADDRESS|ALIAS> or --all."),
    }
}

#[derive(serde::Serialize)]
struct DeviceReport {
    device_id: String,
    record_count: usize,
    co2: Option<MetricSummary>,
    temperature: Option<MetricSummary>,
    humidity: Option<MetricSummary>,
    pressure: Option<MetricSummary>,
    radon: Option<MetricSummary>,
}

#[derive(serde::Serialize)]
struct MetricSummary {
    min: f64,
    max: f64,
    avg: f64,
    time_above_threshold: Option<f64>,
}

fn generate_device_report(
    store: &Store,
    device_id: &str,
    since: OffsetDateTime,
) -> Result<Option<DeviceReport>> {
    let query = HistoryQuery::new().device(device_id).since(since);
    let records = store.query_history(&query)?;

    if records.is_empty() {
        return Ok(None);
    }

    let count = records.len();

    // CO2
    let co2_vals: Vec<f64> = records
        .iter()
        .map(|r| r.co2 as f64)
        .filter(|&v| v > 0.0)
        .collect();
    let co2 = if !co2_vals.is_empty() {
        let above_1000 = co2_vals.iter().filter(|&&v| v >= 1000.0).count();
        Some(MetricSummary {
            min: co2_vals.iter().copied().reduce(f64::min).unwrap_or(0.0),
            max: co2_vals.iter().copied().reduce(f64::max).unwrap_or(0.0),
            avg: co2_vals.iter().sum::<f64>() / co2_vals.len() as f64,
            time_above_threshold: Some((above_1000 as f64 / count as f64) * 100.0),
        })
    } else {
        None
    };

    // Temperature
    let temp_vals: Vec<f64> = records.iter().map(|r| r.temperature as f64).collect();
    let temperature = if !temp_vals.is_empty() {
        Some(MetricSummary {
            min: temp_vals.iter().copied().reduce(f64::min).unwrap_or(0.0),
            max: temp_vals.iter().copied().reduce(f64::max).unwrap_or(0.0),
            avg: temp_vals.iter().sum::<f64>() / temp_vals.len() as f64,
            time_above_threshold: None,
        })
    } else {
        None
    };

    // Humidity
    let hum_vals: Vec<f64> = records.iter().map(|r| r.humidity as f64).collect();
    let humidity = if !hum_vals.is_empty() {
        Some(MetricSummary {
            min: hum_vals.iter().copied().reduce(f64::min).unwrap_or(0.0),
            max: hum_vals.iter().copied().reduce(f64::max).unwrap_or(0.0),
            avg: hum_vals.iter().sum::<f64>() / hum_vals.len() as f64,
            time_above_threshold: None,
        })
    } else {
        None
    };

    // Pressure
    let press_vals: Vec<f64> = records
        .iter()
        .map(|r| r.pressure as f64)
        .filter(|&v| v > 0.0)
        .collect();
    let pressure = if !press_vals.is_empty() {
        Some(MetricSummary {
            min: press_vals.iter().copied().reduce(f64::min).unwrap_or(0.0),
            max: press_vals.iter().copied().reduce(f64::max).unwrap_or(0.0),
            avg: press_vals.iter().sum::<f64>() / press_vals.len() as f64,
            time_above_threshold: None,
        })
    } else {
        None
    };

    // Radon
    let radon_vals: Vec<f64> = records
        .iter()
        .filter_map(|r| r.radon.map(|v| v as f64))
        .collect();
    let radon = if !radon_vals.is_empty() {
        let above_300 = radon_vals.iter().filter(|&&v| v >= 300.0).count();
        Some(MetricSummary {
            min: radon_vals.iter().copied().reduce(f64::min).unwrap_or(0.0),
            max: radon_vals.iter().copied().reduce(f64::max).unwrap_or(0.0),
            avg: radon_vals.iter().sum::<f64>() / radon_vals.len() as f64,
            time_above_threshold: Some((above_300 as f64 / count as f64) * 100.0),
        })
    } else {
        None
    };

    Ok(Some(DeviceReport {
        device_id: device_id.to_string(),
        record_count: count,
        co2,
        temperature,
        humidity,
        pressure,
        radon,
    }))
}

fn print_device_report(report: &DeviceReport, fahrenheit: bool, inhg: bool, bq: bool) {
    println!("Device: {}", report.device_id);
    println!("  Records: {}", report.record_count);

    if let Some(ref co2) = report.co2 {
        println!("  CO\u{2082}:");
        println!(
            "    Min: {:.0} ppm  Max: {:.0} ppm  Avg: {:.0} ppm",
            co2.min, co2.max, co2.avg
        );
        if let Some(pct) = co2.time_above_threshold {
            println!("    Time above 1000 ppm: {:.1}%", pct);
        }
    }

    if let Some(ref temp) = report.temperature {
        if fahrenheit {
            println!("  Temperature:");
            println!(
                "    Min: {:.1}\u{00b0}F  Max: {:.1}\u{00b0}F  Avg: {:.1}\u{00b0}F",
                temp.min * 9.0 / 5.0 + 32.0,
                temp.max * 9.0 / 5.0 + 32.0,
                temp.avg * 9.0 / 5.0 + 32.0
            );
        } else {
            println!("  Temperature:");
            println!(
                "    Min: {:.1}\u{00b0}C  Max: {:.1}\u{00b0}C  Avg: {:.1}\u{00b0}C",
                temp.min, temp.max, temp.avg
            );
        }
    }

    if let Some(ref hum) = report.humidity {
        println!("  Humidity:");
        println!(
            "    Min: {:.0}%  Max: {:.0}%  Avg: {:.0}%",
            hum.min, hum.max, hum.avg
        );
    }

    if let Some(ref press) = report.pressure {
        if inhg {
            let to_inhg = |hpa: f64| hpa * 0.02953;
            println!("  Pressure:");
            println!(
                "    Min: {:.2} inHg  Max: {:.2} inHg  Avg: {:.2} inHg",
                to_inhg(press.min),
                to_inhg(press.max),
                to_inhg(press.avg)
            );
        } else {
            println!("  Pressure:");
            println!(
                "    Min: {:.1} hPa  Max: {:.1} hPa  Avg: {:.1} hPa",
                press.min, press.max, press.avg
            );
        }
    }

    if let Some(ref radon) = report.radon {
        if bq {
            println!("  Radon:");
            println!(
                "    Min: {:.0} Bq/m\u{00b3}  Max: {:.0} Bq/m\u{00b3}  Avg: {:.0} Bq/m\u{00b3}",
                radon.min, radon.max, radon.avg
            );
            if let Some(pct) = radon.time_above_threshold {
                println!("    Time above 300 Bq/m\u{00b3}: {:.1}%", pct);
            }
        } else {
            let to_pci = |bq_val: f64| bq_val * 0.027;
            println!("  Radon:");
            println!(
                "    Min: {:.2} pCi/L  Max: {:.2} pCi/L  Avg: {:.2} pCi/L",
                to_pci(radon.min),
                to_pci(radon.max),
                to_pci(radon.avg)
            );
            if let Some(pct) = radon.time_above_threshold {
                println!("    Time above 4.0 pCi/L: {:.1}%", pct);
            }
        }
    }

    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_store(device_ids: &[&str]) -> Store {
        let store = Store::open_in_memory().unwrap();
        for device_id in device_ids {
            store.upsert_device(device_id, None).unwrap();
        }
        store
    }

    #[test]
    fn test_resolve_report_devices_prefers_explicit_device() {
        let store = seed_store(&["device-1", "device-2"]);
        let devices = resolve_report_devices(&store, Some("device-2".to_string()), false).unwrap();
        assert_eq!(devices, vec!["device-2"]);
    }

    #[test]
    fn test_resolve_report_devices_supports_all_flag() {
        let store = seed_store(&["device-1", "device-2"]);
        let devices = resolve_report_devices(&store, None, true).unwrap();
        assert_eq!(devices, vec!["device-1", "device-2"]);
    }

    #[test]
    fn test_resolve_report_devices_errors_on_ambiguous_none() {
        let store = seed_store(&["device-1", "device-2"]);
        let err = resolve_report_devices(&store, None, false).unwrap_err();
        assert!(err.to_string().contains("--all"));
    }
}
